//! Piezas de concurrencia de la capa de aplicación.
//!
//! En la app Vue todo eran promesas: una vez lanzada, una promesa termina
//! aunque nadie la espere, y dos interesados pueden esperar la misma. Un
//! `Task` de GPUI, en cambio, se cancela al soltarlo. Aquí están las dos
//! piezas que devuelven la semántica original:
//!
//! - [`flow`]: lanza un flujo que corre hasta el final aunque quien lo pidió
//!   suelte el `Task` (un checkout con stash no puede quedarse a medias porque
//!   una vista se cerró).
//! - [`InFlightMap`] + [`AppModel::run_shared`]: lo que en el original eran
//!   los mapas de promesas en vuelo (`sharedInFlightOperations`,
//!   `inFlightSectionReads`, `inFlightPromises` de `useSingleFlight`,
//!   `accountRefreshRequests`, `summaryRequests`).

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::FutureExt as _;
use futures::channel::oneshot;
use futures::future::Shared;
use gpui_kit::{App, AppContext as _, AsyncApp, Context, Entity, Task};
use mergeon_backend::Backend;
use mergeon_gitcmd::{OperationContext, READ_TIMEOUT, run_with_operation_context};

use crate::AppModel;

/// Futuro compartido con el resultado de un trabajo en vuelo. `Err` solo si
/// el trabajo se abandonó (la app se está cerrando).
pub(crate) type SharedReceiver<T> = Shared<oneshot::Receiver<T>>;

struct InFlightEntry {
    token: u64,
    future: Box<dyn Any>,
}

/// Trabajos en vuelo por clave. Cada clave guarda un único futuro compartido;
/// el tipo de su resultado lo fija quien usa la clave.
#[derive(Default)]
pub(crate) struct InFlightMap {
    entries: HashMap<String, InFlightEntry>,
    next_token: u64,
}

impl InFlightMap {
    pub fn get<T: Clone + 'static>(&self, key: &str) -> Option<SharedReceiver<T>> {
        self.entries
            .get(key)
            .and_then(|entry| entry.future.downcast_ref::<SharedReceiver<T>>())
            .cloned()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Registra el futuro y devuelve su ficha, con la que luego se retira.
    pub fn insert<T: Clone + 'static>(&mut self, key: String, future: SharedReceiver<T>) -> u64 {
        self.next_token += 1;
        let token = self.next_token;
        self.entries.insert(
            key,
            InFlightEntry {
                token,
                future: Box::new(future),
            },
        );
        token
    }

    /// `if (map.get(key) === promise) map.delete(key)`: solo retira la entrada
    /// si sigue siendo la que se registró con esa ficha.
    pub fn remove_if(&mut self, key: &str, token: u64) {
        if self
            .entries
            .get(key)
            .is_some_and(|entry| entry.token == token)
        {
            self.entries.remove(key);
        }
    }

    pub fn remove_prefix(&mut self, prefix: &str) {
        self.entries.retain(|key, _| !key.starts_with(prefix));
    }
}

impl std::fmt::Debug for InFlightMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.entries.keys()).finish()
    }
}

/// Lanza `f` como un flujo que no depende de quien lo pide: corre hasta el
/// final aunque se suelte el `Task` devuelto, que solo sirve para esperar su
/// resultado.
pub(crate) fn flow<R, F>(cx: &mut Context<AppModel>, f: F) -> Task<R>
where
    R: 'static,
    F: AsyncFnOnce(Entity<AppModel>, &mut AsyncApp) -> R + 'static,
{
    let this = cx.entity();
    let (sender, receiver) = oneshot::channel();
    cx.spawn(async move |_, cx| {
        let output = f(this, cx).await;
        let _ = sender.send(output);
    })
    .detach();

    cx.foreground_executor().spawn(async move {
        match receiver.await {
            Ok(output) => output,
            // El flujo se abandonó sin terminar: la app se está cerrando.
            Err(_) => std::future::pending().await,
        }
    })
}

/// Espera un trabajo compartido. `None` si se abandonó sin resultado.
pub(crate) async fn settled<T: Clone>(shared: SharedReceiver<T>) -> Option<T> {
    shared.await.ok()
}

/// Cancela el [`OperationContext`] del trabajo bloqueante si el `Task` se
/// suelta antes de que termine: mata el proceso `git` obsoleto y libera el hilo.
struct CancelOnDrop {
    context: OperationContext,
    completed: Arc<AtomicBool>,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if !self.completed.load(Ordering::SeqCst) {
            self.context.cancel();
        }
    }
}

impl AppModel {
    /// Ejecuta `work` (bloqueante) en un hilo de fondo: git y disco nunca
    /// corren en el hilo de UI. Cada llamada corre bajo un [`OperationContext`]
    /// con timeout por inactividad ([`READ_TIMEOUT`]) y se cancela si el `Task`
    /// se suelta antes de terminar.
    pub(crate) fn blocking<R>(
        &self,
        cx: &App,
        work: impl FnOnce(&Backend) -> R + Send + 'static,
    ) -> Task<R>
    where
        R: Send + 'static,
    {
        let backend: Arc<Backend> = self.backend.clone();
        let context = OperationContext::new(None, READ_TIMEOUT);
        let worker_context = context.clone();
        let completed = Arc::new(AtomicBool::new(false));
        let worker_completed = completed.clone();
        let (sender, receiver) = oneshot::channel();

        cx.background_spawn(async move {
            let result = run_with_operation_context(worker_context, || work(&backend));
            worker_completed.store(true, Ordering::SeqCst);
            let _ = sender.send(result);
        })
        .detach();

        cx.foreground_executor().spawn(async move {
            let _cancel = CancelOnDrop { context, completed };
            match receiver.await {
                Ok(output) => output,
                Err(_) => std::future::pending().await,
            }
        })
    }

    /// Un trabajo por clave: si ya hay uno en vuelo con esa clave se devuelve
    /// ese; si no, se arranca con `start` y se registra hasta que termine.
    ///
    /// `observable`: alguna vista pinta si la clave está en vuelo (el
    /// spinner de un botón), así que registrarla y retirarla avisa a las
    /// vistas. Si no, no: cada aviso del modelo repinta la ventana entera, y
    /// lo que el trabajo cambie ya avisa por su cuenta.
    pub(crate) fn run_shared<T>(
        &mut self,
        registry: fn(&mut AppModel) -> &mut InFlightMap,
        key: String,
        observable: bool,
        cx: &mut Context<Self>,
        start: impl FnOnce(&mut Self, &mut Context<Self>) -> Task<T>,
    ) -> SharedReceiver<T>
    where
        T: Clone + 'static,
    {
        if let Some(existing) = registry(self).get::<T>(&key) {
            return existing;
        }

        let work = start(self, cx);
        let (sender, receiver) = oneshot::channel();
        let shared = receiver.shared();
        let token = registry(self).insert(key.clone(), shared.clone());
        if observable {
            cx.notify();
        }

        cx.spawn(async move |this, cx| {
            let output = work.await;
            this.update(cx, |this, cx| {
                registry(this).remove_if(&key, token);
                if observable {
                    cx.notify();
                }
            })
            .ok();
            let _ = sender.send(output);
        })
        .detach();

        shared
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_registered_future_removes_its_entry() {
        let mut map = InFlightMap::default();
        let (_first_sender, first) = oneshot::channel::<u8>();
        let (_second_sender, second) = oneshot::channel::<u8>();

        let first_token = map.insert("fetch:/repo".to_string(), first.shared());
        let second_token = map.insert("fetch:/repo".to_string(), second.shared());

        // La primera ya fue reemplazada: su fin no debe llevarse a la segunda.
        map.remove_if("fetch:/repo", first_token);
        assert!(map.contains("fetch:/repo"));

        map.remove_if("fetch:/repo", second_token);
        assert!(!map.contains("fetch:/repo"));
    }

    #[test]
    fn a_key_only_answers_for_the_type_it_was_registered_with() {
        let mut map = InFlightMap::default();
        let (_sender, receiver) = oneshot::channel::<u8>();
        map.insert("repo-1:tags".to_string(), receiver.shared());

        assert!(map.get::<u8>("repo-1:tags").is_some());
        assert!(map.get::<String>("repo-1:tags").is_none());
        assert!(map.get::<u8>("repo-1:branches").is_none());
    }

    #[test]
    fn drops_every_entry_of_a_repository_by_prefix() {
        let mut map = InFlightMap::default();
        for key in ["repo-1:status", "repo-1:branches", "repo-2:status"] {
            let (_sender, receiver) = oneshot::channel::<u8>();
            map.insert(key.to_string(), receiver.shared());
        }

        map.remove_prefix("repo-1:");

        assert_eq!(map.len(), 1);
        assert!(map.contains("repo-2:status"));
    }
}
