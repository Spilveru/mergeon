//! Historial listo para pintar: la parte de datos de `CommitView.vue`
//! (`performLoadCommits`, `performLoadMoreCommits`) y de la búsqueda de la
//! barra lateral.
//!
//! El backend devuelve commits crudos; aquí salen como filas
//! (`mergeon_model::commit_view_utils::CommitRow`) ya normalizadas, con su
//! grafo y su pertenencia a la rama, calculadas en el hilo de fondo.
//!
//! Queda en la vista lo que es suyo: qué commit está seleccionado
//! (`reconcileSelectedCommit`), cuándo recargar (`loadCommits` y su
//! coalescing) y el scroll. `reconcileCommitRows` no se porta: reutilizaba el
//! objeto de la fila anterior para no invalidar la fila ya montada, y en GPUI
//! la identidad de una fila es su hash, no el objeto.

use std::collections::HashSet;
use std::sync::Arc;

use gpui_kit::{Context, Task};
use mergeon_backend::{CommitLogOptions, CommitSearchOptions, QueryResult};
use mergeon_model::commit_view_utils::{
    CommitRow, build_commit_graph, mark_branch_membership, normalize_commit_view_branch,
    normalize_raw_commit,
};

use crate::AppModel;
use crate::convert::raw_commit;

/// `HISTORY_PAGE_SIZE` de `CommitView.vue`.
pub const HISTORY_PAGE_SIZE: u32 = 200;

/// Ventana a la que se amplía el historial para alcanzar un commit pedido
/// desde la barra lateral o la búsqueda.
const ENSURE_VISIBLE_PAGE_SIZE: u32 = 1000;

/// Qué historial cargar.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommitHistoryRequest {
    /// Rama actual del repositorio; da color a los commits sin rama propia.
    pub current_branch: Option<String>,
    /// Filas ya cargadas: se recarga esa misma ventana, no solo la primera
    /// página (con 600 commits traídos por scroll, refrescar a 200 encogería la
    /// lista bajo los pies del usuario en cada fetch).
    pub loaded_count: usize,
    /// Commit (hash completo) que tiene que quedar dentro de la ventana; si no
    /// está, se reintenta una vez con una ventana mayor.
    pub ensure_commit_visible: Option<String>,
}

/// Una ventana del historial.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommitHistory {
    /// Filas con grafo y pertenencia de rama.
    pub commits: Vec<CommitRow>,
    /// Puede haber commits más viejos por cargar.
    pub has_more: bool,
    /// Se pidió un commit concreto y no entró ni en la ventana ampliada.
    pub missing_requested_commit: bool,
}

fn rows(raw: &[mergeon_backend::Commit], current_branch: &str) -> Vec<CommitRow> {
    raw.iter()
        .map(|commit| normalize_raw_commit(&raw_commit(commit), current_branch))
        .collect()
}

/// Los commits de `page` que no están ya en `loaded` (por `full_hash`), en
/// su orden: un conjunto de hashes en vez de recorrer `loaded` por cada uno.
fn fresh_commits(
    page: Vec<mergeon_backend::Commit>,
    loaded: &[CommitRow],
) -> Vec<mergeon_backend::Commit> {
    let known: HashSet<&str> = loaded.iter().map(|row| row.full_hash.as_str()).collect();
    page.into_iter()
        .filter(|commit| !known.contains(commit.full_hash.as_str()))
        .collect()
}

impl AppModel {
    /// `performLoadCommits`: carga (o refresca) la ventana del historial de un
    /// repositorio. Un fallo se avisa (`Error loading commits`) y se devuelve
    /// para que la vista decida si conserva lo que tenía.
    pub fn load_commit_history(
        &mut self,
        repo_path: &str,
        request: CommitHistoryRequest,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<CommitHistory>> {
        let repo_path = repo_path.to_string();
        let read = self.read(Some("Error loading commits"), cx, move |backend| {
            let current_branch =
                normalize_commit_view_branch(request.current_branch.as_deref()).to_string();
            let mut page_size =
                HISTORY_PAGE_SIZE.max(u32::try_from(request.loaded_count).unwrap_or(u32::MAX));
            let load = |page_size: u32| {
                backend.git().get_commits(
                    &repo_path,
                    &CommitLogOptions {
                        max_count: Some(page_size),
                        skip: None,
                    },
                )
            };
            let mut raw = load(page_size)?;

            /*
             * Un salto pedido desde el sidebar o la búsqueda puede apuntar a un commit
             * más viejo que la ventana normal: se reintenta una vez con una ventana
             * mayor antes de rendirse, y si aun así no aparece se avisa en vez de
             * caer en silencio a otro commit.
             */
            let mut missing_requested_commit = false;
            if let Some(wanted) = &request.ensure_commit_visible
                && !raw.iter().any(|commit| commit.full_hash == *wanted)
            {
                page_size = ENSURE_VISIBLE_PAGE_SIZE.max(page_size);
                raw = load(page_size)?;
                missing_requested_commit = !raw.iter().any(|commit| commit.full_hash == *wanted);
            }

            Ok(CommitHistory {
                has_more: raw.len() == page_size as usize,
                commits: mark_branch_membership(build_commit_graph(rows(&raw, &current_branch))),
                missing_requested_commit,
            })
        });

        cx.spawn(async move |this, cx| {
            let history = read.await;
            static FIRST_HISTORY: std::sync::Once = std::sync::Once::new();
            FIRST_HISTORY.call_once(|| crate::startup::trace("first history page"));
            if let Ok(history) = &history
                && history.missing_requested_commit
            {
                this.update(cx, |this, cx| {
                    this.notify_warning("That commit is outside the loaded history", cx);
                })
                .ok();
            }
            history
        })
    }

    /// `performLoadMoreCommits`: la página siguiente del historial. El grafo y
    /// la pertenencia de rama se recalculan sobre la lista completa: ambos son
    /// deterministas de arriba hacia abajo, así que las filas ya visibles no
    /// cambian de carril ni de color.
    ///
    /// `loaded` son las filas ya cargadas; el resultado las incluye. Llegan
    /// compartidas (`Arc`): la vista que las pinta no copia nada en el hilo de
    /// UI, y la copia que hace falta para fundirlas con la página nueva se
    /// hace en el de fondo, y solo si llega algo nuevo.
    pub fn load_more_commit_history(
        &mut self,
        repo_path: &str,
        current_branch: Option<&str>,
        loaded: impl Into<Arc<Vec<CommitRow>>>,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<CommitHistory>> {
        let repo_path = repo_path.to_string();
        let current_branch = normalize_commit_view_branch(current_branch).to_string();
        let loaded: Arc<Vec<CommitRow>> = loaded.into();
        self.read(Some("Error loading commits"), cx, move |backend| {
            let raw = backend.git().get_commits(
                &repo_path,
                &CommitLogOptions {
                    max_count: Some(HISTORY_PAGE_SIZE),
                    skip: Some(u32::try_from(loaded.len()).unwrap_or(u32::MAX)),
                },
            )?;
            let has_more = raw.len() == HISTORY_PAGE_SIZE as usize;

            // Si el historial mutó entre páginas el offset puede desalinearse:
            // el dedupe por fullHash absorbe los solapes.
            let fresh = fresh_commits(raw, &loaded);
            if fresh.is_empty() {
                return Ok(CommitHistory {
                    commits: Arc::unwrap_or_clone(loaded),
                    has_more,
                    missing_requested_commit: false,
                });
            }

            let mut merged = Arc::unwrap_or_clone(loaded);
            merged.extend(rows(&fresh, &current_branch));
            Ok(CommitHistory {
                commits: mark_branch_membership(build_commit_graph(merged)),
                has_more,
                missing_requested_commit: false,
            })
        })
    }

    /// Búsqueda en el historial completo, como filas. Quien llama decide cómo
    /// presentar el fallo.
    pub fn search_commit_history(
        &mut self,
        repo_path: &str,
        options: CommitSearchOptions,
        current_branch: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<Vec<CommitRow>>> {
        let repo_path = repo_path.to_string();
        let current_branch = normalize_commit_view_branch(current_branch).to_string();
        self.blocking(cx, move |backend| {
            let raw = backend.git().search_commits(&repo_path, &options)?;
            Ok(rows(&raw, &current_branch))
        })
    }
}

#[cfg(test)]
mod fresh_commits_tests {
    use super::*;

    fn commit(full_hash: &str) -> mergeon_backend::Commit {
        mergeon_backend::Commit {
            hash: full_hash.to_string(),
            full_hash: full_hash.to_string(),
            message: String::new(),
            author: String::new(),
            email: String::new(),
            date: String::new(),
            body: String::new(),
            refs: String::new(),
            parents: String::new(),
        }
    }

    fn row(full_hash: &str) -> CommitRow {
        CommitRow {
            full_hash: full_hash.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn keeps_only_unseen_commits_in_page_order() {
        let loaded = [row("a"), row("b")];
        let page = vec![commit("b"), commit("c"), commit("a"), commit("d")];

        let fresh = fresh_commits(page, &loaded);

        let hashes: Vec<_> = fresh
            .iter()
            .map(|commit| commit.full_hash.as_str())
            .collect();
        assert_eq!(hashes, ["c", "d"]);
        assert!(fresh_commits(vec![commit("a")], &loaded).is_empty());
        assert_eq!(fresh_commits(vec![commit("x")], &[]).len(), 1);
    }
}
