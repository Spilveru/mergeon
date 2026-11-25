//! Acceso ordenado a la configuración persistente.
//!
//! Las escrituras de ajustes y de sesión tienen que llegar al disco en el
//! orden en que se pidieron (dos cambios de tema seguidos deben dejar guardado
//! el último). En la app Vue eso lo daba el orden de los mensajes IPC; aquí lo
//! da una cola con un único consumidor en un hilo de fondo.

use std::sync::Arc;

use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use gpui_kit::{App, AppContext as _, Task};
use mergeon_backend::{Backend, Store};

use crate::AppModel;

type Job = Box<dyn FnOnce() + Send>;

pub(crate) struct StorageQueue {
    sender: mpsc::UnboundedSender<Job>,
    _worker: Task<()>,
}

impl StorageQueue {
    pub fn new(cx: &App) -> Self {
        let (sender, mut receiver) = mpsc::unbounded::<Job>();
        let worker = cx.background_spawn(async move {
            while let Some(job) = receiver.next().await {
                job();
            }
        });

        Self {
            sender,
            _worker: worker,
        }
    }

    /// Encola `work`. Se ejecuta aunque se suelte el `Task` devuelto.
    pub fn run<R>(
        &self,
        cx: &App,
        backend: Arc<Backend>,
        work: impl FnOnce(&Store) -> R + Send + 'static,
    ) -> Task<R>
    where
        R: Send + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        let job: Job = Box::new(move || {
            let _ = sender.send(work(backend.storage()));
        });
        if self.sender.unbounded_send(job).is_err() {
            log::error!("the settings queue is closed; a storage request was dropped");
        }

        cx.background_spawn(async move {
            match receiver.await {
                Ok(output) => output,
                Err(_) => std::future::pending().await,
            }
        })
    }
}

impl AppModel {
    /// Ejecuta `work` contra la configuración persistente, en orden con el
    /// resto de peticiones y fuera del hilo de UI.
    pub(crate) fn storage<R>(
        &self,
        cx: &App,
        work: impl FnOnce(&Store) -> R + Send + 'static,
    ) -> Task<R>
    where
        R: Send + 'static,
    {
        self.storage_queue.run(cx, self.backend.clone(), work)
    }
}
