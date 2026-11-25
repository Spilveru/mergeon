//! Carga del historial, selección del commit y sus detalles: las funciones
//! `loadCommits`, `performLoadCommits`, `performLoadMoreCommits`,
//! `reconcileSelectedCommit`, `selectCommit`, `loadCommitDetails`,
//! `reconcileSelectedFile` y `loadDiff` de `CommitView.vue`.

use std::sync::Arc;

use futures::FutureExt as _;
use futures::future::Shared;
use gpui_kit::{AsyncApp, Context, Task, WeakEntity};
use mergeon_app::history::{CommitHistory, CommitHistoryRequest};
use mergeon_backend::{DiffOptions, FileChange, QueryResult};
use mergeon_model::commit_view_utils::{
    CommitRow, RefType, is_head_commit, normalize_commit_view_branch,
};

use super::{CommitLoadOptions, CommitsView, CommitsViewEvent, MultiStats};
use crate::history_list::row_key;

/// Lo que una pasada de `performLoadCommits` recuerda entre sus esperas.
struct HistoryLoad {
    repo_id: String,
    /// Solo la carga inicial de un repo puede vaciar la lista y mostrar el
    /// spinner.
    is_initial_load: bool,
    read: Task<QueryResult<CommitHistory>>,
}

/// Un mensaje de git que delata un hash obsoleto (`STALE_OBJECT_ERROR`).
fn is_stale_object_error(message: &str) -> bool {
    let message = message.to_lowercase();
    ["bad object", "unknown revision", "ambiguous argument"]
        .iter()
        .any(|needle| message.contains(needle))
}

impl CommitsView {
    // ------------------------------------------------------------------
    // Historial
    // ------------------------------------------------------------------

    /// `loadCommits(options)`: carga o refresca el historial. Mientras haya una
    /// carga en curso, las nuevas peticiones no arrancan otra: sus opciones se
    /// funden y se atienden en una sola pasada al terminar la actual. El
    /// futuro devuelto se resuelve cuando no queda nada pendiente.
    pub fn load_commits(
        &mut self,
        options: CommitLoadOptions,
        cx: &mut Context<Self>,
    ) -> Shared<Task<()>> {
        if self.commit_load_running
            && let Some(running) = self.commit_load.clone()
        {
            self.queued_commit_load_options = Some(
                self.queued_commit_load_options
                    .take()
                    .unwrap_or_default()
                    .merged_with(options),
            );
            return running;
        }

        self.commit_load_running = true;
        self.queued_commit_load_options = None;
        let load = Self::flow(cx, async move |this, cx| {
            let mut next = Some(options);
            while let Some(options) = next {
                Self::perform_load_commits(&this, options, cx).await;
                next = this
                    .update(cx, |this, _| this.queued_commit_load_options.take())
                    .ok()
                    .flatten();
            }
            let _ = this.update(cx, |this, cx| {
                this.commit_load_running = false;
                this.queued_commit_load_options = None;
                cx.notify();
            });
        })
        .shared();
        self.commit_load = Some(load.clone());
        load
    }

    /// Como [`Self::load_commits`], para quien no necesita esperar a que la
    /// carga termine.
    pub fn reload_history(&mut self, options: CommitLoadOptions, cx: &mut Context<Self>) {
        // La carga la conduce un flujo propio: soltar el futuro no la cancela.
        drop(self.load_commits(options, cx));
    }

    /// `performLoadCommits`.
    async fn perform_load_commits(
        this: &WeakEntity<Self>,
        options: CommitLoadOptions,
        cx: &mut AsyncApp,
    ) {
        let Ok(Some(load)) = this.update(cx, |this, cx| this.begin_history_load(&options, cx))
        else {
            return;
        };
        let HistoryLoad {
            repo_id,
            is_initial_load,
            read,
        } = load;

        let history = read.await;
        let reconcile = this
            .update(cx, |this, cx| {
                this.finish_history_load(&repo_id, is_initial_load, history, options, cx)
            })
            .ok()
            .flatten();
        if let Some(reconcile) = reconcile {
            reconcile.await;
        }

        if is_initial_load {
            let _ = this.update(cx, |this, cx| {
                this.loading_initial_history = false;
                this.publish(cx);
            });
        }
    }

    fn begin_history_load(
        &mut self,
        options: &CommitLoadOptions,
        cx: &mut Context<Self>,
    ) -> Option<HistoryLoad> {
        let Some(repo) = self.active_repo(cx) else {
            self.clear_history(cx);
            return None;
        };

        /*
         * Solo la carga inicial de un repo puede vaciar la lista y mostrar el spinner.
         * Un refresco (fetch, pull, commit…) mantiene las filas en pantalla: si se
         * desmontan, el contenedor pierde el scroll y la lista parece rehacerse.
         */
        let is_repo_switch = self.history_repo_id.as_deref() != Some(repo.id.as_str());
        let is_initial_load = is_repo_switch || self.commits.is_empty();

        if is_repo_switch && !self.commits.is_empty() {
            self.set_commit_rows(Vec::new());
        }
        if is_initial_load {
            self.loading_initial_history = true;
        }
        if let Some(branch) = &repo.current_branch {
            self.current_branch = normalize_commit_view_branch(Some(branch)).to_string();
        }

        /*
         * Se recarga la ventana ya cargada, no solo la primera página: con 600
         * commits traídos por scroll infinito, refrescar a 200 encogería la lista
         * bajo los pies del usuario en cada fetch.
         */
        let request = CommitHistoryRequest {
            current_branch: (!self.current_branch.is_empty()).then(|| self.current_branch.clone()),
            loaded_count: self.commits.len(),
            ensure_commit_visible: options
                .ensure_commit_visible
                .then(|| options.preferred_commit_hash.clone())
                .flatten(),
        };
        self.history_load_count += 1;
        let read = self.model.update(cx, |model, cx| {
            model.load_commit_history(&repo.path, request, cx)
        });
        self.publish(cx);

        Some(HistoryLoad {
            repo_id: repo.id,
            is_initial_load,
            read,
        })
    }

    fn finish_history_load(
        &mut self,
        repo_id: &str,
        is_initial_load: bool,
        history: QueryResult<CommitHistory>,
        options: CommitLoadOptions,
        cx: &mut Context<Self>,
    ) -> Option<Task<()>> {
        let history = match history {
            Ok(history) => history,
            Err(_) => {
                // Un refresco fallido no debe llevarse por delante lo ya cargado:
                // solo la carga inicial, que no tiene nada que conservar, limpia.
                if is_initial_load {
                    self.clear_history(cx);
                }
                return None;
            }
        };

        // Si el repo activo cambió durante la carga se descarta esta pasada: la
        // carga encolada del repo nuevo corre a continuación.
        if !self.active_repo(cx).is_some_and(|repo| repo.id == repo_id) {
            return None;
        }

        self.has_more_commits = history.has_more;
        self.set_commit_rows(history.commits);
        self.history_repo_id = Some(repo_id.to_string());
        self.multi_commits_changed(cx);

        let preferred_branch = options
            .preferred_branch
            .unwrap_or_else(|| self.current_branch.clone());
        Some(self.reconcile_selected_commit(
            options.preferred_commit_hash,
            preferred_branch,
            options.force_details_reload,
            options.preferred_file_path,
            cx,
        ))
    }

    /// `clearHistory`.
    pub(crate) fn clear_history(&mut self, cx: &mut Context<Self>) {
        self.set_commit_rows(Vec::new());
        self.history_repo_id = None;
        self.has_more_commits = false;
        self.loading_more_commits = false;
        self.selected_commit = None;
        self.current_branch.clear();
        self.clear_commit_details(cx);
        self.collapse_multi_selection(cx);
    }

    /// `handleLoadMoreCommits` / `performLoadMoreCommits`: la página siguiente
    /// del historial. Un fallo conserva lo ya cargado y no toca la selección.
    pub fn handle_load_more_commits(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo(cx) else {
            return;
        };
        if self.loading_more_commits || !self.has_more_commits {
            return;
        }

        self.loading_more_commits = true;
        // Compartidas, no copiadas: la copia (si hace falta) es del hilo de fondo.
        let loaded = Arc::clone(&self.commits);
        let current_branch = self.current_branch.clone();
        let read = self.model.update(cx, |model, cx| {
            model.load_more_commit_history(
                &repo.path,
                (!current_branch.is_empty()).then_some(current_branch.as_str()),
                loaded,
                cx,
            )
        });
        self.publish(cx);

        Self::flow(cx, async move |this, cx| {
            let history = read.await;
            let _ = this.update(cx, |this, cx| {
                match history {
                    Ok(history) => {
                        if this
                            .active_repo(cx)
                            .is_some_and(|active| active.id == repo.id)
                        {
                            this.has_more_commits = history.has_more;
                            if history.commits.len() != this.commits.len() {
                                this.set_commit_rows(history.commits);
                                this.multi_commits_changed(cx);
                            }
                        }
                    }
                    Err(error) => {
                        let message = if error.message().is_empty() {
                            "Failed to load more commits"
                        } else {
                            error.message()
                        }
                        .to_string();
                        this.model.update(cx, |model, cx| {
                            model.notify_error_with(&message, &error, cx);
                        });
                    }
                }
                this.loading_more_commits = false;
                this.publish(cx);
            });
        })
        .detach();
    }

    // ------------------------------------------------------------------
    // Selección
    // ------------------------------------------------------------------

    /// `reconcileSelectedCommit`: qué commit queda seleccionado tras cargar el
    /// historial. Por orden: el pedido, el que ya estaba, la punta de la rama
    /// preferida, si no el de HEAD y, si no, el primero.
    fn reconcile_selected_commit(
        &mut self,
        preferred_commit_hash: Option<String>,
        preferred_branch: String,
        force_details_reload: bool,
        preferred_file_path: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        if self.commits.is_empty() {
            self.clear_history(cx);
            self.publish(cx);
            return Task::ready(());
        }

        let by_hash = |hash: &str| {
            self.commits
                .iter()
                .find(|commit| commit.full_hash == hash)
                .cloned()
        };
        let next_commit = preferred_commit_hash
            .as_deref()
            .and_then(by_hash)
            .or_else(|| {
                self.selected_commit
                    .as_ref()
                    .and_then(|selected| by_hash(&selected.full_hash))
            })
            .or_else(|| {
                self.commits
                    .iter()
                    .find(|commit| {
                        commit.refs.iter().any(|commit_ref| {
                            commit_ref.kind == RefType::Local && commit_ref.name == preferred_branch
                        })
                    })
                    .cloned()
            })
            .or_else(|| {
                self.commits
                    .iter()
                    .find(|commit| is_head_commit(*commit))
                    .cloned()
            })
            .or_else(|| self.commits.first().cloned());

        self.select_commit(next_commit, force_details_reload, preferred_file_path, cx)
    }

    /// `selectCommit(commit, options)`.
    pub(crate) fn select_commit(
        &mut self,
        commit: Option<CommitRow>,
        force_details_reload: bool,
        preferred_file_path: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let Some(commit) = commit else {
            self.selected_commit = None;
            self.summary_expanded = false;
            self.clear_commit_details(cx);
            self.publish(cx);
            return Task::ready(());
        };

        let changed_commit = self
            .selected_commit
            .as_ref()
            .is_none_or(|selected| selected.full_hash != commit.full_hash);
        let full_hash = commit.full_hash.clone();
        self.selected_commit = Some(commit);
        // El mensaje desplegado es el de ese commit: el siguiente empieza
        // recortado.
        if changed_commit {
            self.summary_expanded = false;
        }

        // `@select-commit="state.selectedCommit = $event"` del espacio de trabajo.
        if self.workspace_mode
            && let Some(repo) = self.active_repo(cx)
        {
            self.model.update(cx, |model, cx| {
                model.set_workspace_selected_commit(&repo.path, Some(&full_hash), cx);
            });
        }
        cx.emit(CommitsViewEvent::CommitSelected { hash: full_hash });

        self.reload_commit_details_if_needed(
            force_details_reload || changed_commit,
            preferred_file_path,
            cx,
        )
    }

    /// `handleHistorySelect`: clic directo en una fila del historial. Además de
    /// seleccionar, avisa de que la selección ya no corresponde al salto que
    /// pudo pedir el sidebar.
    pub fn handle_history_select(&mut self, commit: CommitRow, cx: &mut Context<Self>) {
        // Un clic (o una flecha) sin modificadores vuelve a la selección
        // simple, con el ancla de Shift en ese commit.
        let was_multi = self.multi.is_multi();
        self.multi.collapse(Some(row_key(&commit)));
        if was_multi {
            self.multi_stats = MultiStats::default();
            self.multi_stats_range = None;
            self.multi_stats_task = None;
        }
        if let Some(repo) = self.active_repo(cx) {
            cx.emit(CommitsViewEvent::CommitPicked {
                repo_id: repo.id,
                hash: commit.full_hash.clone(),
            });
        }
        self.select_commit(Some(commit), false, None, cx).detach();
    }

    /// `selectParentCommit`: el historial se carga acotado, así que el padre
    /// puede no estar cargado: avisar es mejor que un botón que no hace nada.
    pub fn select_parent_commit(&mut self, parent_hash: &str, cx: &mut Context<Self>) {
        let parent = self
            .commits
            .iter()
            .find(|commit| commit.full_hash == parent_hash)
            .cloned();

        match parent {
            Some(parent) => self.select_commit(Some(parent), false, None, cx).detach(),
            None => {
                self.model.update(cx, |model, cx| {
                    model.notify_warning("That parent is outside the loaded history", cx);
                });
            }
        }
    }

    // ------------------------------------------------------------------
    // Detalles del commit
    // ------------------------------------------------------------------

    /// `clearCommitDetails`.
    pub(crate) fn clear_commit_details(&mut self, _: &mut Context<Self>) {
        self.details_request += 1;
        self.loading_details = false;
        self.commit_files.clear();
        self.selected_file = None;
        self.file_diff.clear();
        self.details_commit_hash = None;
        self.details_repo_path = None;
    }

    /// `recoverFromStaleCommit`: ante un `bad object` se recarga la lista una
    /// vez por hash para re-seleccionar un commit vivo, en vez de dejar el
    /// detalle vacío.
    fn recover_from_stale_commit(
        &mut self,
        commit_hash: &str,
        message: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        if commit_hash.is_empty() || !is_stale_object_error(message) {
            return false;
        }
        if !self
            .stale_hash_reloads_attempted
            .insert(commit_hash.to_string())
        {
            return false;
        }

        // Sin esperarla: si ya hay una carga en vuelo esto solo encola opciones.
        self.reload_history(CommitLoadOptions::new().force_details_reload(true), cx);
        true
    }

    /// `reloadCommitDetailsIfNeeded`.
    fn reload_commit_details_if_needed(
        &mut self,
        force: bool,
        preferred_file_path: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let Some(selected) = self.selected_commit.clone() else {
            self.clear_commit_details(cx);
            self.publish(cx);
            return Task::ready(());
        };

        if force || self.details_commit_hash.as_deref() != Some(selected.full_hash.as_str()) {
            return self.load_commit_details(selected, preferred_file_path, cx);
        }

        self.reconcile_selected_file(preferred_file_path, cx)
    }

    /// `loadCommitDetails`.
    fn load_commit_details(
        &mut self,
        commit: CommitRow,
        preferred_file_path: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        // El repo se captura al entrar: si cambia durante las esperas se aborta
        // sin pisar el estado del repo nuevo.
        let Some(repo) = self.active_repo(cx) else {
            self.clear_commit_details(cx);
            self.publish(cx);
            return Task::ready(());
        };

        self.details_request += 1;
        let request = self.details_request;
        self.loading_details = true;
        self.commit_files.clear();
        self.selected_file = None;
        self.file_diff.clear();
        let read = self.model.update(cx, |model, cx| {
            model.get_commit_files(&repo.path, &commit.full_hash, cx)
        });
        self.publish(cx);

        Self::flow(cx, async move |this, cx| {
            let files = read.await;
            let reconcile = this
                .update(cx, |this, cx| match files {
                    Ok(files) => {
                        let still_wanted = this
                            .active_repo(cx)
                            .is_some_and(|active| active.id == repo.id)
                            && request == this.details_request
                            && this
                                .selected_commit
                                .as_ref()
                                .is_some_and(|selected| selected.full_hash == commit.full_hash);
                        if !still_wanted {
                            return None;
                        }

                        this.commit_files = files;
                        this.details_commit_hash = Some(commit.full_hash.clone());
                        this.details_repo_path = Some(repo.path.clone());
                        Some(this.reconcile_selected_file(preferred_file_path, cx))
                    }
                    Err(error) => {
                        if request == this.details_request {
                            let recovered = this.recover_from_stale_commit(
                                &commit.full_hash,
                                error.message(),
                                cx,
                            );
                            if !recovered {
                                this.clear_commit_details(cx);
                            }
                        }
                        None
                    }
                })
                .ok()
                .flatten();
            if let Some(reconcile) = reconcile {
                reconcile.await;
            }

            let _ = this.update(cx, |this, cx| {
                if request == this.details_request {
                    this.loading_details = false;
                }
                this.publish(cx);
            });
        })
    }

    /// `reconcileSelectedFile`: el archivo pedido, el que ya estaba elegido o
    /// el primero. En el espacio de trabajo no se elige el primero: ahí la
    /// lista no tiene un diff al lado (un clic abre la pestaña del archivo) y
    /// marcar uno por defecto no significaba nada.
    fn reconcile_selected_file(
        &mut self,
        preferred_path: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let by_path = |path: &str| self.commit_files.iter().find(|file| file.path == path);
        let next_file = preferred_path
            .as_deref()
            .and_then(by_path)
            .or_else(|| {
                self.selected_file
                    .as_ref()
                    .and_then(|selected| by_path(&selected.path))
            })
            .or_else(|| {
                if self.workspace_mode {
                    None
                } else {
                    self.commit_files.first()
                }
            })
            .cloned();

        let Some(next_file) = next_file else {
            self.selected_file = None;
            self.file_diff.clear();
            self.publish(cx);
            return Task::ready(());
        };

        self.selected_file = Some(next_file.clone());
        self.load_diff(Some(next_file), cx)
    }

    /// `loadDiff`: el diff del archivo dentro del commit seleccionado. En el
    /// espacio de trabajo no se pide: el archivo se ve en su pestaña.
    pub(crate) fn load_diff(
        &mut self,
        file: Option<FileChange>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        if self.workspace_mode {
            self.publish(cx);
            return Task::ready(());
        }

        // La ruta viene del contexto en que se cargaron los detalles, para que
        // hash y repo sean siempre consistentes.
        let repo_path = self.details_repo_path.clone();
        let commit_hash = self
            .selected_commit
            .as_ref()
            .map(|commit| commit.full_hash.clone())
            .filter(|hash| !hash.is_empty());
        let repo_id = self.active_repo(cx).map(|repo| repo.id);

        let (Some(file), Some(commit_hash), Some(repo_path)) = (file, commit_hash, repo_path)
        else {
            self.file_diff.clear();
            self.diff_too_large = false;
            self.diff_whole_file_fell_back = false;
            self.diff_size = 0;
            self.publish(cx);
            return Task::ready(());
        };

        self.loading_diff = true;
        self.diff_request += 1;
        let request = self.diff_request;
        let preferences = *self.model.read(cx).diff_preferences();
        let options = DiffOptions {
            ignore_whitespace: preferences.ignore_whitespace,
            whole_file: preferences.whole_file,
        };
        if mergeon_view_diff::is_image_path(&file.path) {
            let viewer = self.changes.read(cx).image_viewer().clone();
            let pair = mergeon_backend::ImagePairRequest::Commit {
                hash: commit_hash.clone(),
                path: file.path.clone(),
                previous_path: None,
            };
            let task = crate::images::load_image_pair(
                &self.model,
                &viewer,
                &repo_path,
                &file.path,
                pair,
                cx,
            );
            cx.spawn(async move |this, cx| {
                let images = task.await;
                let _ = this.update(cx, |this, cx| {
                    if request == this.diff_request {
                        crate::images::show(&viewer, images, cx);
                    }
                });
            })
            .detach();
        }
        let read = self.model.update(cx, |model, cx| {
            model.get_commit_diff(&repo_path, &commit_hash, &file.path, options, cx)
        });
        self.publish(cx);

        Self::flow(cx, async move |this, cx| {
            let payload = read.await;
            let _ = this.update(cx, |this, cx| {
                // Solo cuenta la respuesta de la última petición: dos archivos
                // elegidos seguidos no se pisan.
                if request != this.diff_request {
                    return;
                }
                match payload {
                    Ok(payload) => {
                        if this.active_repo(cx).map(|repo| repo.id) == repo_id {
                            this.file_diff = payload.diff;
                            this.diff_too_large = payload.too_large;
                            this.diff_whole_file_fell_back = payload.whole_file_fell_back;
                            this.diff_size = payload.size;
                        }
                    }
                    Err(error) => {
                        this.file_diff.clear();
                        this.diff_too_large = false;
                        this.diff_whole_file_fell_back = false;
                        this.diff_size = 0;
                        this.recover_from_stale_commit(&commit_hash, error.message(), cx);
                    }
                }
                this.loading_diff = false;
                this.publish(cx);
            });
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_the_errors_of_a_stale_hash() {
        assert!(is_stale_object_error(
            "Failed to get commit files: fatal: bad object 1111111111111111"
        ));
        assert!(is_stale_object_error("fatal: Unknown Revision or path"));
        assert!(is_stale_object_error("ambiguous argument 'abc'"));
        assert!(!is_stale_object_error("Permission denied"));
        assert!(!is_stale_object_error(""));
    }
}
