//! Estado de las pestañas de cada repositorio (el `<script setup>` de
//! `RepositoryWorkspace.vue`): qué archivos están abiertos, cuál es la pestaña
//! activa, la sección fija visible y el commit seleccionado, por ruta de
//! repositorio y guardado en la sesión.
//!
//! En el original cada `RepositoryWorkspace` tenía una copia reactiva del
//! estado y la guardaba en el store al cambiar. Aquí el estado vive solo en la
//! sesión del modelo: cada método cambia el estado del repositorio y lo
//! persiste. Qué paneles están montados y en qué ranura (el `visited` y los
//! `slots` del original) es cosa de la vista, que se entera de un cambio de
//! identidad de pestaña por [`AppEvent::WorkspaceTabReplaced`].

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::{Context, Task};
use mergeon_model::workspace_tabs::{
    CHANGES_TAB, FileOpener, FileTabDescriptor, HOME_TAB, RepositoryWorkspace, Scroll,
    TabReplacement, WorkspaceSection, adjacent_file_tab, close_file_tab, is_fixed_tab,
    move_file_tab, open_preview_file_tab, pin_file_tab,
};

use crate::{AppEvent, AppModel};

/// Espera tras el último cambio "de vista" (scroll, commit seleccionado,
/// borrador del commit) antes de guardar la sesión.
pub const WORKSPACE_PERSIST_DELAY: Duration = Duration::from_millis(750);

#[derive(Debug, Default)]
pub(crate) struct WorkspaceState {
    /// `createFileOpener` de cada repositorio: recuerda la pestaña sustituida
    /// por el primer clic de un doble clic.
    openers: HashMap<String, FileOpener>,
    /// Guardado diferido de la sesión tras cambios de vista. Soltarlo lo
    /// cancela; cualquier guardado inmediato lo sustituye.
    pub(crate) pending_persist: Option<Task<()>>,
}

/// Tecla de navegación entre pestañas (`navigateTabs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TabNavigation {
    /// `ArrowLeft`.
    Previous,
    /// `ArrowRight`.
    Next,
    /// `Home`.
    First,
    /// `End`.
    Last,
}

impl AppModel {
    /// Estado de las pestañas de un repositorio; el de por defecto si todavía
    /// no tiene ninguno guardado.
    pub fn repository_workspace(&self, path: &str) -> RepositoryWorkspace {
        self.repositories
            .workspace_session
            .repository_workspaces
            .get(path)
            .cloned()
            .unwrap_or_default()
    }

    /// `getRepositoryWorkspace`: como [`Self::repository_workspace`], creando
    /// la entrada si no existía.
    pub fn get_repository_workspace(&mut self, path: &str) -> &RepositoryWorkspace {
        self.repositories.get_repository_workspace(path)
    }

    /// `saveRepositoryWorkspace`: reemplaza el estado de un repositorio y
    /// guarda la sesión.
    pub fn save_repository_workspace(
        &mut self,
        path: &str,
        state: RepositoryWorkspace,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        self.repositories
            .workspace_session
            .repository_workspaces
            .insert(path.to_string(), state);
        let persisted = self.persist_workspace_session(cx);
        cx.background_executor().spawn(async move {
            persisted.await;
        })
    }

    /// Cambia el estado de las pestañas de un repositorio y lo guarda (el
    /// `watch(state, flush, { deep: true })` del original).
    pub fn update_repository_workspace<R>(
        &mut self,
        path: &str,
        cx: &mut Context<Self>,
        update: impl FnOnce(&mut RepositoryWorkspace) -> R,
    ) -> R {
        let workspace = self.repositories.get_repository_workspace(path);
        let before = workspace.clone();
        let output = update(workspace);
        let changed = *workspace != before;
        let active_tab_changed = workspace.active_tab != before.active_tab;

        if active_tab_changed {
            // Cambiar de pestaña por otro camino cierra la ventana del doble clic.
            let snapshot = workspace.clone();
            if let Some(opener) = self.workspaces.openers.get_mut(path) {
                opener.sync(&snapshot);
            }
        }
        if changed {
            self.persist_workspace_session(cx).detach();
            cx.notify();
        }
        output
    }

    /// Como [`Self::update_repository_workspace`], para el estado "de vista"
    /// que cambia muy a menudo y que nadie pinta desde el modelo (scroll de
    /// una pestaña, commit seleccionado, borrador del commit): no avisa a los
    /// observadores de `AppModel` y agrupa el guardado en uno, 750 ms después
    /// del último cambio (o antes, si otro cambio guarda la sesión, al
    /// cambiar de pestaña o al salir). Antes cada paso de la rueda reescribía
    /// `settings.json` con `fsync` y repintaba todas las vistas.
    pub fn update_repository_workspace_quietly<R>(
        &mut self,
        path: &str,
        cx: &mut Context<Self>,
        update: impl FnOnce(&mut RepositoryWorkspace) -> R,
    ) -> R {
        let workspace = self.repositories.get_repository_workspace(path);
        let before = workspace.clone();
        let output = update(workspace);
        if *workspace != before {
            self.schedule_workspace_persist(cx);
        }
        output
    }

    /// Programa (o reprograma) el guardado diferido de la sesión.
    fn schedule_workspace_persist(&mut self, cx: &mut Context<Self>) {
        let timer = cx.background_executor().timer(WORKSPACE_PERSIST_DELAY);
        self.workspaces.pending_persist = Some(cx.spawn(async move |this, cx| {
            timer.await;
            this.update(cx, |this, cx| {
                // Es esta misma tarea la que corre: se suelta sin cancelarla.
                if let Some(task) = this.workspaces.pending_persist.take() {
                    task.detach();
                }
                this.persist_workspace_session(cx).detach();
            })
            .ok();
        }));
    }

    /// Hay cambios de vista esperando a guardarse.
    pub fn has_pending_workspace_persist(&self) -> bool {
        self.workspaces.pending_persist.is_some()
    }

    /// `flush` de todos los `RepositoryWorkspace`: deja guardado el estado de
    /// las pestañas. Se resuelve cuando la escritura terminó.
    pub fn flush_workspaces(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let persisted = self.persist_workspace_session(cx);
        cx.background_executor().spawn(async move {
            persisted.await;
        })
    }

    /// `activate(id)`: muestra una pestaña. Si es una de las fijas, pasa a ser
    /// la sección del repositorio y, cuando es el repositorio activo, la vista
    /// activa de la aplicación.
    pub fn activate_workspace_tab(&mut self, path: &str, id: &str, cx: &mut Context<Self>) {
        let section = match id {
            HOME_TAB => Some(WorkspaceSection::Commits),
            CHANGES_TAB => Some(WorkspaceSection::Changes),
            _ => None,
        };
        self.update_repository_workspace(path, cx, |workspace| {
            workspace.active_tab = id.to_string();
            if let Some(section) = section {
                workspace.section = section;
            }
        });

        if let Some(section) = section
            && self
                .active_repository()
                .is_some_and(|repository| repository.path == path)
        {
            // `@change-view="activeView = $event"`
            self.shell.active_view = section;
            cx.notify();
        }
    }

    /// `openFile(descriptor)`: abre un archivo en una pestaña. Con `replace`
    /// (un clic dentro de una pestaña de archivo) reutiliza la pestaña actual;
    /// sin él abre otra. Devuelve qué pestaña tomó el lugar de cuál, si alguna
    /// lo hizo.
    pub fn open_workspace_file(
        &mut self,
        path: &str,
        descriptor: &FileTabDescriptor,
        replace: bool,
        cx: &mut Context<Self>,
    ) -> Option<TabReplacement> {
        let mut opener = self.workspaces.openers.remove(path).unwrap_or_default();
        let moved = self.update_repository_workspace(path, cx, |workspace| {
            opener.open(workspace, descriptor, replace)
        });
        self.workspaces.openers.insert(path.to_string(), opener);

        if let Some(moved) = &moved {
            cx.emit(AppEvent::WorkspaceTabReplaced {
                repo_path: path.to_string(),
                from: moved.from.clone(),
                to: moved.to.clone(),
            });
        }
        moved
    }

    /// Abre `descriptor` en la pestaña de vista previa del repositorio (la
    /// paleta "Go to file…"): reutiliza la que haya en vez de sumar otra.
    pub fn open_workspace_file_preview(
        &mut self,
        path: &str,
        descriptor: &FileTabDescriptor,
        cx: &mut Context<Self>,
    ) {
        self.update_repository_workspace(path, cx, |workspace| {
            open_preview_file_tab(workspace, descriptor);
        });
    }

    /// "Keep open" / doble clic en la pestaña: deja de ser la de vista previa.
    pub fn pin_workspace_tab(&mut self, path: &str, id: &str, cx: &mut Context<Self>) {
        self.update_repository_workspace(path, cx, |workspace| {
            pin_file_tab(workspace, id);
        });
    }

    /// Arrastrar y soltar una pestaña de archivo (queda fijada).
    pub fn move_workspace_tab(
        &mut self,
        path: &str,
        from: usize,
        to: usize,
        cx: &mut Context<Self>,
    ) {
        self.update_repository_workspace(path, cx, |workspace| {
            move_file_tab(workspace, from, to);
        });
    }

    /// `close(id)`: cierra una pestaña de archivo; pasa a su vecina o, si era
    /// el último documento, a la vista fija que lo abrió.
    pub fn close_workspace_tab(&mut self, path: &str, id: &str, cx: &mut Context<Self>) {
        let next = self.update_repository_workspace(path, cx, |workspace| {
            close_file_tab(workspace, id);
            workspace.active_tab.clone()
        });
        self.activate_workspace_tab(path, &next, cx);
    }

    /// Cierra varias pestañas de archivo de una vez (menú contextual: "Close
    /// others", "Close to the right").
    pub fn close_workspace_tabs(&mut self, path: &str, ids: &[String], cx: &mut Context<Self>) {
        if ids.is_empty() {
            return;
        }
        let next = self.update_repository_workspace(path, cx, |workspace| {
            for id in ids {
                close_file_tab(workspace, id);
            }
            workspace.active_tab.clone()
        });
        self.activate_workspace_tab(path, &next, cx);
    }

    /// "Close others": todas las pestañas de archivo menos `keep`, que
    /// queda a la vista.
    pub fn close_other_workspace_tabs(&mut self, path: &str, keep: &str, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .repository_workspace(path)
            .tabs
            .iter()
            .filter(|tab| tab.id != keep)
            .map(|tab| tab.id.clone())
            .collect();
        self.close_workspace_tabs(path, &ids, cx);
        self.activate_workspace_tab(path, keep, cx);
    }

    /// "Close all": todos los documentos; vuelve a la última vista fija.
    pub fn close_all_workspace_tabs(&mut self, path: &str, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .repository_workspace(path)
            .tabs
            .iter()
            .map(|tab| tab.id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        self.close_workspace_tabs(path, &ids, cx);
        self.show_workspace_home(path, None, cx);
    }

    /// "Close to the right": las pestañas de archivo a la derecha de `id`.
    pub fn close_workspace_tabs_to_the_right(
        &mut self,
        path: &str,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        let tabs = self.repository_workspace(path).tabs;
        let Some(index) = tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        let ids: Vec<String> = tabs[index + 1..].iter().map(|tab| tab.id.clone()).collect();
        let active_closed = ids.contains(&self.repository_workspace(path).active_tab);
        self.close_workspace_tabs(path, &ids, cx);
        if active_closed {
            self.activate_workspace_tab(path, id, cx);
        }
    }

    /// El atajo `mod+w`: cierra la pestaña activa si es de archivo.
    pub fn close_active_workspace_tab(&mut self, path: &str, cx: &mut Context<Self>) {
        let active_tab = self.repository_workspace(path).active_tab;
        if !is_fixed_tab(&active_tab) {
            self.close_workspace_tab(path, &active_tab, cx);
        }
    }

    /// `showHome(section)`: vuelve a una de las vistas fijas; sin `section`, a
    /// la última que se vio.
    pub fn show_workspace_home(
        &mut self,
        path: &str,
        section: Option<WorkspaceSection>,
        cx: &mut Context<Self>,
    ) {
        let section = section.unwrap_or_else(|| self.repository_workspace(path).section);
        let id = match section {
            WorkspaceSection::Changes => CHANGES_TAB,
            WorkspaceSection::Commits => HOME_TAB,
        };
        self.activate_workspace_tab(path, id, cx);
    }

    /// Va al commit `hash` en el historial de un repositorio abierto: cierra
    /// Settings, activa el repositorio y su pestaña Commits, y pide a la vista
    /// de commits que lo elija y lo traiga a la vista
    /// ([`AppEvent::CommitSelectionRequested`]). `false` si el repositorio no
    /// está abierto o el hash viene vacío.
    pub fn show_commit_in_history(
        &mut self,
        repo_id: &str,
        hash: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let hash = hash.trim();
        let Some(index) = self.find_repository_index(repo_id) else {
            return false;
        };
        if hash.is_empty() {
            return false;
        }
        let repo_id = self.repositories.repositories[index].id.clone();
        if self.active_repository().map(|repo| repo.id.as_str()) != Some(repo_id.as_str()) {
            self.set_active_repository(Some(index), cx);
        }
        self.handle_change_view(WorkspaceSection::Commits, cx);
        cx.emit(AppEvent::CommitSelectionRequested {
            repo_id,
            hash: hash.to_string(),
        });
        true
    }

    /// `navigateTabs`: flechas, Inicio y Fin sobre la barra de pestañas.
    /// Devuelve el id de la pestaña que queda activa.
    pub fn navigate_workspace_tabs(
        &mut self,
        path: &str,
        key: TabNavigation,
        cx: &mut Context<Self>,
    ) -> String {
        let workspace = self.repository_workspace(path);
        let ids: Vec<String> = [HOME_TAB.to_string(), CHANGES_TAB.to_string()]
            .into_iter()
            .chain(workspace.tabs.iter().map(|tab| tab.id.clone()))
            .collect();
        let count = ids.len();
        let current = ids.iter().position(|id| *id == workspace.active_tab);

        let next = match key {
            TabNavigation::First => 0,
            TabNavigation::Last => count - 1,
            // Sin pestaña activa conocida el original partía de -1.
            TabNavigation::Next => current.map_or(0, |index| (index + 1) % count),
            TabNavigation::Previous => match current {
                Some(index) => (index + count - 1) % count,
                None => count - 2,
            },
        };

        let id = ids[next].clone();
        self.activate_workspace_tab(path, &id, cx);
        id
    }

    /// `Alt+→` / `Alt+←`: el documento siguiente o anterior del repositorio
    /// activo, dando la vuelta (desde Commits o Local Changes, el primero o
    /// el último). Devuelve el id que queda activo, si había documentos.
    pub fn cycle_document_tab(&mut self, forward: bool, cx: &mut Context<Self>) -> Option<String> {
        if self.show_preferences() {
            return None;
        }
        let path = self.active_repository()?.path.clone();
        let workspace = self.repository_workspace(&path);
        let id = adjacent_file_tab(&workspace, forward)?.id.clone();
        self.activate_workspace_tab(&path, &id, cx);
        Some(id)
    }

    /// `rememberScroll(tab, position)`.
    pub fn remember_workspace_scroll(
        &mut self,
        path: &str,
        tab_id: &str,
        position: Scroll,
        cx: &mut Context<Self>,
    ) {
        self.update_repository_workspace_quietly(path, cx, |workspace| {
            if let Some(tab) = workspace.tabs.iter_mut().find(|tab| tab.id == tab_id)
                && (tab.scroll.top != position.top || tab.scroll.left != position.left)
            {
                tab.scroll = position;
            }
        });
    }

    /// Borrador del mensaje de commit de un repositorio ("" si no hay).
    pub fn commit_draft(&self, path: &str) -> String {
        self.repositories
            .workspace_session
            .repository_workspaces
            .get(path)
            .map(|workspace| workspace.commit_draft.clone())
            .unwrap_or_default()
    }

    /// Guarda el borrador del mensaje de commit (diferido, sin repintar).
    pub fn set_commit_draft(&mut self, path: &str, draft: &str, cx: &mut Context<Self>) {
        self.update_repository_workspace_quietly(path, cx, |workspace| {
            if workspace.commit_draft != draft {
                workspace.commit_draft = draft.to_string();
            }
        });
    }

    /// Si los commits de un repositorio se firman ("Sign" de la caja de commit).
    pub fn commit_sign(&self, path: &str) -> bool {
        self.repositories
            .workspace_session
            .repository_workspaces
            .get(path)
            .is_some_and(|workspace| workspace.commit_sign)
    }

    /// Recuerda "Sign" para un repositorio (diferido, sin repintar).
    pub fn set_commit_sign(&mut self, path: &str, sign: bool, cx: &mut Context<Self>) {
        self.update_repository_workspace_quietly(path, cx, |workspace| {
            workspace.commit_sign = sign;
        });
    }

    /// `@select-commit="state.selectedCommit = $event"`.
    pub fn set_workspace_selected_commit(
        &mut self,
        path: &str,
        commit_hash: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.update_repository_workspace_quietly(path, cx, |workspace| {
            workspace.selected_commit = commit_hash.map(str::to_string);
        });
    }
}
