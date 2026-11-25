//! Pestañas de repositorio más allá de abrir y activar: cerrarlas (con
//! confirmación si se perdería algo de vista), reabrir la última cerrada,
//! recorrerlas con el teclado, nombrarlas sin ambigüedad y saber si un
//! repositorio cambió mientras su pestaña estaba oculta.
//!
//! No viene del original: `App.vue` cerraba sin preguntar, no tenía atajos
//! para las pestañas de repositorio y mostraba el mismo nombre para dos
//! repositorios homónimos.

use std::path::Path;

use gpui_kit::{AppContext as _, Context, Task};
use mergeon_model::dialogs::ConfirmOptions;
use mergeon_model::repositories::distinguishing_parent_suffixes;
use mergeon_model::workspace_tabs::is_fixed_tab;

use crate::AppModel;
use crate::tasks::flow;

/// Cuántos repositorios cerrados recuerda "Reopen closed repository".
const CLOSED_REPOSITORIES_LIMIT: usize = 20;

/// Nombre de una pestaña de repositorio: el del repositorio y, si otro
/// abierto se llama igual, el tramo de carpetas que los distingue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryTabLabel {
    pub name: String,
    pub suffix: Option<String>,
}

impl RepositoryTabLabel {
    /// En una sola línea, para avisos y menús: `demo (fx2)`.
    pub fn text(&self) -> String {
        match &self.suffix {
            Some(suffix) => format!("{} ({suffix})", self.name),
            None => self.name.clone(),
        }
    }
}

impl AppModel {
    // ------------------------------------------------------------------
    // Nombres
    // ------------------------------------------------------------------

    /// Nombres de las pestañas de repositorio visibles, en su orden.
    pub fn repository_tab_labels(&self) -> Vec<RepositoryTabLabel> {
        let repositories = self.visible_repositories();
        let suffixes = distinguishing_parent_suffixes(
            repositories
                .iter()
                .map(|repository| (repository.name.as_str(), repository.path.as_str())),
        );
        repositories
            .iter()
            .zip(suffixes)
            .map(|(repository, suffix)| RepositoryTabLabel {
                name: repository.name.clone(),
                suffix,
            })
            .collect()
    }

    /// El nombre de la pestaña de un repositorio abierto, por ruta. Prioriza
    /// los visibles en el workspace activo y cae al conjunto total de abiertos
    /// si el repositorio está oculto en otro workspace.
    pub fn repository_label(&self, repo_path: &str) -> Option<RepositoryTabLabel> {
        let visible = self.visible_repositories();
        if let Some(index) = visible
            .iter()
            .position(|repository| repository.path == repo_path)
        {
            return self.repository_tab_labels().into_iter().nth(index);
        }
        let all = self.open_repositories();
        let index = all
            .iter()
            .position(|repository| repository.path == repo_path)?;
        let suffixes = distinguishing_parent_suffixes(
            all.iter()
                .map(|repository| (repository.name.as_str(), repository.path.as_str())),
        );
        Some(RepositoryTabLabel {
            name: all[index].name.clone(),
            suffix: suffixes.into_iter().nth(index).flatten(),
        })
    }

    /// Un aviso sobre un repositorio dice de cuál es cuando hay más de uno
    /// abierto: `demo (fx2): Push completed successfully.`
    pub fn message_for_repository(&self, repo_path: &str, message: &str) -> String {
        if self.open_repositories().len() < 2 {
            return message.to_string();
        }
        match self.repository_label(repo_path) {
            Some(label) => format!("{}: {message}", label.text()),
            None => message.to_string(),
        }
    }

    // ------------------------------------------------------------------
    // Teclado
    // ------------------------------------------------------------------

    /// `Ctrl+Tab` / `Ctrl+Shift+Tab`: la pestaña de repositorio visible
    /// siguiente o anterior, dando la vuelta. Desde Home o Settings, la
    /// primera (o la última hacia atrás).
    pub fn cycle_repository_tab(&mut self, forward: bool, cx: &mut Context<Self>) {
        let visible_ids: Vec<String> = self
            .visible_repositories()
            .into_iter()
            .map(|repository| repository.id.clone())
            .collect();
        let count = visible_ids.len();
        if count == 0 {
            return;
        }
        let current = (!self.show_preferences()).then(|| {
            self.active_repository()
                .and_then(|active| visible_ids.iter().position(|id| id == &active.id))
        });
        let current = current.flatten();
        let next = match (current, forward) {
            (Some(index), true) => (index + 1) % count,
            (Some(index), false) => (index + count - 1) % count,
            (None, true) => 0,
            (None, false) => count - 1,
        };
        self.select_visible_repository_tab(next, cx);
    }

    /// `mod+N`: la pestaña de repositorio visible N (desde 1); 9 es la última.
    pub fn select_repository_tab_number(&mut self, number: u8, cx: &mut Context<Self>) {
        let count = self.visible_repositories().len();
        if count == 0 || number == 0 {
            return;
        }
        let index = if number >= 9 {
            count - 1
        } else {
            usize::from(number) - 1
        };
        if index < count {
            self.select_visible_repository_tab(index, cx);
        }
    }

    /// `mod+w`: cierra el documento que se está viendo. En Commits o Local
    /// Changes no hace nada (cerrar el repositorio es `mod+shift+w`: antes un
    /// `mod+w` de más se llevaba el repositorio entero). Con Settings a la
    /// vista, su pestaña. En Home, nada.
    pub fn close_current_tab(&mut self, cx: &mut Context<Self>) {
        if self.show_preferences() {
            if self.settings_tab_open() {
                self.close_settings_tab(cx);
            }
            return;
        }
        let Some(path) = self
            .active_repository()
            .map(|repository| repository.path.clone())
        else {
            return;
        };
        let active_tab = self.repository_workspace(&path).active_tab;
        if !is_fixed_tab(&active_tab) {
            self.close_workspace_tab(&path, &active_tab, cx);
        }
    }

    /// `mod+shift+w`: cierra el repositorio activo.
    pub fn close_active_repository(&mut self, cx: &mut Context<Self>) {
        if self.show_preferences() {
            return;
        }
        if let Some(id) = self
            .active_repository()
            .map(|repository| repository.id.clone())
        {
            self.request_close_repositories(vec![id], cx).detach();
        }
    }

    // ------------------------------------------------------------------
    // Cerrar y reabrir
    // ------------------------------------------------------------------

    /// Por qué cerrar un repositorio necesita confirmación: un borrador de
    /// commit o una operación en curso o en cola. Vacío si nada.
    pub fn repository_close_warnings(&self, repo_id: &str) -> Vec<String> {
        let Some(repository) = self.repository(repo_id) else {
            return Vec::new();
        };
        let mut warnings = Vec::new();
        let queue = &repository.mutation_queue;
        let running: Vec<&str> = queue
            .active
            .iter()
            .chain(queue.queued.iter())
            .map(|operation| operation.operation.as_str())
            .collect();
        if !running.is_empty() {
            warnings.push(format!(
                "An operation is still running ({}). It will finish in the background, but you won't see its progress.",
                running.join(", ")
            ));
        }
        if !self.commit_draft(&repository.path).trim().is_empty() {
            warnings.push(
                "It has an unfinished commit message. It is kept and comes back when you reopen the repository."
                    .to_string(),
            );
        }
        warnings
    }

    /// Cierra esas pestañas de repositorio. Con un workspace activo las quita
    /// del workspace y solo cierra de `open_repositories` las que no estén en
    /// ningún otro workspace (pidiendo confirmación únicamente por las que se
    /// cierran de verdad y tengan aviso). Se resuelve a `true` si se aplicó.
    pub fn request_close_repositories(
        &mut self,
        repo_ids: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let repo_ids: Vec<String> = repo_ids
            .into_iter()
            .filter(|id| self.repository(id.as_str()).is_some())
            .collect();
        if repo_ids.is_empty() {
            return Task::ready(false);
        }

        let active_workspace_id = self.active_workspace_id().map(str::to_string);
        let closing_from_open: Vec<String> = repo_ids
            .iter()
            .filter(|id| match active_workspace_id.as_deref() {
                Some(ws_id) => self.repository(id.as_str()).is_some_and(|repo| {
                    !self
                        .repositories
                        .is_repository_in_other_workspace(&repo.path, ws_id)
                }),
                None => true,
            })
            .cloned()
            .collect();

        let warnings: Vec<(String, Vec<String>)> = closing_from_open
            .iter()
            .filter_map(|id| {
                let warnings = self.repository_close_warnings(id);
                let path = self.repository(id.as_str())?.path.clone();
                let label = self.repository_label(&path)?.text();
                (!warnings.is_empty()).then_some((label, warnings))
            })
            .collect();
        if warnings.is_empty() {
            self.close_repositories(&repo_ids, active_workspace_id.as_deref(), cx);
            return Task::ready(true);
        }

        let title = if closing_from_open.len() == 1 {
            format!("Close \u{201c}{}\u{201d}?", warnings[0].0)
        } else {
            format!("Close {} repositories?", closing_from_open.len())
        };
        let description = warnings
            .iter()
            .map(|(label, warnings)| {
                if closing_from_open.len() == 1 {
                    warnings.join(" ")
                } else {
                    format!("{label}: {}", warnings.join(" "))
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let answer = self.confirm(
            ConfirmOptions {
                title: Some(title),
                description: Some(description),
                confirm_text: Some("Close".to_string()),
                cancel_text: Some("Cancel".to_string()),
                ..Default::default()
            },
            cx,
        );
        flow(cx, async move |this, cx| {
            let confirmed = answer.await;
            if confirmed {
                this.update(cx, |this, cx| {
                    this.close_repositories(&repo_ids, active_workspace_id.as_deref(), cx);
                });
            }
            confirmed
        })
    }

    /// Cierra sin preguntar y recuerda las rutas para poder reabrirlas.
    fn close_repositories(
        &mut self,
        repo_ids: &[String],
        active_workspace_id: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if let Some(ws_id) = active_workspace_id
            && self.active_workspace_id() == Some(ws_id)
        {
            let previous_id = self.active_repository_id();
            let visible_ids: Vec<String> = self
                .visible_repositories()
                .into_iter()
                .map(|repository| repository.id.clone())
                .collect();
            let next_active_id = if previous_id
                .as_ref()
                .is_some_and(|active_id| repo_ids.contains(active_id))
            {
                let active_pos = previous_id
                    .as_ref()
                    .and_then(|active_id| visible_ids.iter().position(|id| id == active_id));
                let survivors: Vec<(usize, &String)> = visible_ids
                    .iter()
                    .enumerate()
                    .filter(|(_, id)| !repo_ids.contains(*id))
                    .collect();
                active_pos
                    .and_then(|active_pos| {
                        survivors
                            .iter()
                            .find(|(index, _)| *index > active_pos)
                            .or_else(|| survivors.last())
                            .map(|(_, id)| (*id).clone())
                    })
                    .or_else(|| survivors.first().map(|(_, id)| (*id).clone()))
            } else {
                previous_id.clone()
            };

            for id in repo_ids {
                let Some(path) = self.repository(id.as_str()).map(|repo| repo.path.clone()) else {
                    continue;
                };
                let close_from_open = !self
                    .repositories
                    .is_repository_in_other_workspace(&path, ws_id);
                self.repositories
                    .remove_repository_from_workspace(&path, ws_id);
                if close_from_open {
                    self.shell
                        .closed_repositories
                        .retain(|closed| *closed != path);
                    self.shell.closed_repositories.push(path.clone());
                    self.shell.refs_fingerprints.remove(&path);
                    self.shell.visited_repository_ids.remove(id);
                    if self.repositories.remove_repository(id.as_str()).is_some() {
                        self.release_http_credentials(&path, cx);
                        if !self
                            .repositories
                            .recent_repositories
                            .iter()
                            .any(|recent| recent.path == path)
                        {
                            self.summaries.remove(&path);
                        }
                    }
                }
            }

            let excess = self
                .shell
                .closed_repositories
                .len()
                .saturating_sub(CLOSED_REPOSITORIES_LIMIT);
            self.shell.closed_repositories.drain(..excess);

            let next_index = next_active_id
                .as_deref()
                .and_then(|id| self.repositories.find_repository_index(id));
            self.repositories.set_active_repository(next_index);
            self.persist_workspace_session(cx).detach();
            self.after_active_repository_change(previous_id, cx);
            cx.notify();
            return;
        }

        // En "All": cierra directamente de `open_repositories`.
        // Si se cierran varias y una es la activa, se salta de una vez a la
        // que quedará (la primera superviviente a su derecha, o la última a
        // su izquierda) en vez de pasar por cada vecina y leerla.
        if repo_ids.len() > 1
            && let Some(active) = self.active_repository_index()
            && self
                .active_repository()
                .is_some_and(|repository| repo_ids.contains(&repository.id))
        {
            let survivors: Vec<usize> = self
                .open_repositories()
                .iter()
                .enumerate()
                .filter(|(_, repository)| !repo_ids.contains(&repository.id))
                .map(|(index, _)| index)
                .collect();
            let next = survivors
                .iter()
                .find(|index| **index > active)
                .or_else(|| survivors.last())
                .copied();
            if let Some(next) = next {
                self.select_repository_tab(next, cx);
            }
        }
        for id in repo_ids {
            let Some(path) = self.repository(id.as_str()).map(|repo| repo.path.clone()) else {
                continue;
            };
            self.shell
                .closed_repositories
                .retain(|closed| *closed != path);
            self.shell.closed_repositories.push(path.clone());
            self.shell.refs_fingerprints.remove(&path);
            self.shell.visited_repository_ids.remove(id);
            self.remove_repository(id.as_str(), cx);
        }
        let excess = self
            .shell
            .closed_repositories
            .len()
            .saturating_sub(CLOSED_REPOSITORIES_LIMIT);
        self.shell.closed_repositories.drain(..excess);
    }

    /// Las demás pestañas de repositorio visibles ("Close others").
    pub fn close_other_repositories(
        &mut self,
        repo_id: &str,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let others = self
            .visible_repositories()
            .into_iter()
            .filter(|repository| repository.id != repo_id)
            .map(|repository| repository.id.clone())
            .collect();
        self.request_close_repositories(others, cx)
    }

    /// Las pestañas visibles a la derecha de esta ("Close to the right").
    pub fn close_repositories_to_the_right(
        &mut self,
        repo_id: &str,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let visible: Vec<String> = self
            .visible_repositories()
            .into_iter()
            .map(|repository| repository.id.clone())
            .collect();
        let Some(index) = visible.iter().position(|id| id == repo_id) else {
            return Task::ready(false);
        };
        let right = visible[index + 1..].to_vec();
        self.request_close_repositories(right, cx)
    }

    /// Repositorios cerrados en esta sesión, el último al final.
    pub fn closed_repositories(&self) -> &[String] {
        &self.shell.closed_repositories
    }

    /// `mod+shift+t`: vuelve a abrir el último repositorio cerrado que no
    /// esté abierto ya, con sus pestañas (siguen en la sesión).
    pub fn reopen_closed_repository(&mut self, cx: &mut Context<Self>) -> Task<bool> {
        while let Some(path) = self.shell.closed_repositories.pop() {
            if self
                .open_repositories()
                .iter()
                .any(|repository| repository.path == path)
            {
                continue;
            }
            let open = self.open_repository_path(
                &path,
                "The repository could not be reopened. It may have been moved or deleted.",
                cx,
            );
            return cx
                .background_executor()
                .spawn(async move { open.await.unwrap_or(false) });
        }
        Task::ready(false)
    }

    /// Mueve una pestaña de repositorio visible (arrastrar y soltar). En "All"
    /// reordena `open_repositories`; con un workspace activo, reordena dentro
    /// de ese workspace.
    pub fn move_repository_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if self
            .repositories
            .move_visible_repository(from, to)
            .is_some()
        {
            self.persist_workspace_session(cx).detach();
            cx.notify();
        }
    }

    // ------------------------------------------------------------------
    // Frescura de un repositorio oculto
    // ------------------------------------------------------------------

    /// Al ocultar un repositorio (deja de ser el activo y se deja de vigilar)
    /// se guarda la huella de sus refs.
    pub(crate) fn remember_hidden_repository_refs(
        &mut self,
        repo_path: String,
        cx: &mut Context<Self>,
    ) {
        let read = cx.background_spawn({
            let repo_path = repo_path.clone();
            async move { mergeon_runtime::get_refs_fingerprint(Path::new(&repo_path)) }
        });
        cx.spawn(async move |this, cx| {
            let Some(fingerprint) = read.await else {
                return;
            };
            this.update(cx, |this, _| {
                // Si mientras tanto volvió a ser el activo, la huella ya no sirve.
                let still_hidden = this
                    .active_repository()
                    .is_none_or(|repository| repository.path != repo_path);
                let still_open = this
                    .open_repositories()
                    .iter()
                    .any(|repository| repository.path == repo_path);
                if still_hidden && still_open {
                    this.shell.refs_fingerprints.insert(repo_path, fingerprint);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Al volver a un repositorio oculto: ¿cambiaron sus refs (commit,
    /// checkout, fetch, tag... hechos fuera) desde que se ocultó? El watcher
    /// solo vigila el activo, así que nadie más se habría enterado. `false`
    /// si no hay huella guardada (nunca se ocultó estando hidratado).
    pub(crate) fn hidden_repository_refs_changed(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let Some(previous) = self.shell.refs_fingerprints.remove(repo_path) else {
            return Task::ready(false);
        };
        let repo_path = repo_path.to_string();
        cx.background_spawn(async move {
            mergeon_runtime::get_refs_fingerprint(Path::new(&repo_path))
                .is_some_and(|current| current != previous)
        })
    }
}
