//! El store de repositorios (`src/stores/repositories.js`) sobre
//! `mergeon_model::repositories::RepositoriesStore`: aquí está lo que el
//! modelo puro dejaba fuera, es decir, persistir (recientes y sesión) y avisar
//! a GPUI de cada cambio.
//!
//! Las acciones que en el store recibían `repoIdOrIndex` aceptan
//! `impl Into<RepoKey>`: el `id` del repositorio (`&str` / `&String`) o su
//! posición (`usize`).

use anyhow::anyhow;
use gpui_kit::{Context, Task};
use mergeon_backend::{StashEntry, StructuredStatus, Submodule};
use mergeon_model::operation_types::QueueOperation;
use mergeon_model::repositories::{
    AddRepositoryOptions, MutationQueueSnapshot, RecentRepository, RepoKey, SidebarSection,
    WorkspaceSession,
};
use mergeon_model::repository_read_model::{BranchItem, RemoteItem};

use crate::convert::{StructuredOutcome, recent_from_store, recent_to_store, session_to_store};
use crate::repository_sync::SyncRequest;
use crate::tasks::flow;
use crate::{AppEvent, AppModel, OpenRepository, Repositories};

/// Lo que acompaña a la ruta al abrir un repositorio (los parámetros
/// opcionales de `addRepository`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddRepository {
    pub name: Option<String>,
    pub is_github: bool,
    pub remote_url: Option<String>,
    /// Ruta canónica (symlinks resueltos); manda sobre la que se pidió.
    pub canonical_path: Option<String>,
    /// No tocar la lista de recientes (al restaurar la sesión).
    pub skip_recent_repository_update: bool,
}

impl AppModel {
    // ------------------------------------------------------------------
    // Lectura
    // ------------------------------------------------------------------

    /// El store completo: pestañas, activa, recientes y sesión.
    pub fn repositories(&self) -> &Repositories {
        &self.repositories
    }

    /// Repositorios abiertos, en el orden de sus pestañas.
    pub fn open_repositories(&self) -> &[OpenRepository] {
        &self.repositories.repositories
    }

    /// `activeRepository`.
    pub fn active_repository(&self) -> Option<&OpenRepository> {
        self.repositories.active_repository()
    }

    /// `activeRepositoryIndex` (`None` es el -1 del original).
    pub fn active_repository_index(&self) -> Option<usize> {
        self.repositories.active_repository_index
    }

    /// Un repositorio abierto, por `id` o por posición.
    pub fn repository<'a>(&self, key: impl Into<RepoKey<'a>>) -> Option<&OpenRepository> {
        self.repositories.repository(key)
    }

    /// `findRepositoryIndex`.
    pub fn find_repository_index<'a>(&self, key: impl Into<RepoKey<'a>>) -> Option<usize> {
        self.repositories.find_repository_index(key)
    }

    /// `recentRepositories`.
    pub fn recent_repositories(&self) -> &[RecentRepository] {
        &self.repositories.recent_repositories
    }

    /// `workspaceSession`.
    pub fn workspace_session(&self) -> &WorkspaceSession {
        &self.repositories.workspace_session
    }

    fn active_repository_id(&self) -> Option<String> {
        self.repositories
            .active_repository()
            .map(|repository| repository.id.clone())
    }

    // ------------------------------------------------------------------
    // Persistencia
    // ------------------------------------------------------------------

    /// `loadFromStorage` del store de repositorios: recientes y sesión. Solo
    /// lee la primera vez.
    pub fn load_repositories_from_storage(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if self.repositories.loaded {
            return Task::ready(());
        }

        let read = self.storage(cx, |store| {
            (
                store.get_recent_repositories(),
                store.get_workspace_session(),
            )
        });
        flow(cx, async move |this, cx| {
            let (recent, session) = read.await;
            this.update(cx, |this, cx| {
                let recent = recent.iter().map(recent_from_store).collect();
                let session = serde_json::to_value(&session).ok();
                this.repositories
                    .load_from_storage(Some(recent), session.as_ref());
                cx.notify();
            });
        })
    }

    /// `persistWorkspaceSession`: recalcula la sesión a partir de lo abierto y
    /// la guarda. Un fallo al guardar solo se anota en el log, como en el
    /// original.
    pub fn persist_workspace_session(&mut self, cx: &mut Context<Self>) -> Task<WorkspaceSession> {
        // Este guardado incluye lo que esperaba al diferido.
        self.workspaces.pending_persist = None;
        let session = self.repositories.persist_workspace_session();
        let stored = session_to_store(&session);
        let write = self.storage(cx, move |store| {
            store
                .set_workspace_session(&stored)
                .map(|_| ())
                .map_err(|error| error.to_string())
        });
        cx.notify();

        cx.background_executor().spawn(async move {
            if let Err(error) = write.await {
                log::error!("Failed to persist workspace session: {error}");
            }
            session
        })
    }

    // ------------------------------------------------------------------
    // Pestañas
    // ------------------------------------------------------------------

    /// `addRepository`: abre un repositorio (o activa su pestaña si ya estaba
    /// abierto), lo anota en los recientes y guarda la sesión. Falla solo si
    /// no se pudieron guardar los recientes, como el original.
    pub fn add_repository(
        &mut self,
        repo_path: &str,
        options: AddRepository,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        let previous_id = self.active_repository_id();
        let Some(outcome) = self.repositories.add_repository(
            repo_path,
            AddRepositoryOptions {
                name: options.name.as_deref(),
                is_github: options.is_github,
                remote_url: options.remote_url.as_deref(),
                canonical_path: options.canonical_path.as_deref(),
                skip_recent_repository_update: options.skip_recent_repository_update,
            },
        ) else {
            return Task::ready(Ok(()));
        };
        cx.notify();
        self.after_active_repository_change(previous_id, cx);

        let recent = outcome.recent_repository.as_ref().map(recent_to_store);
        flow(cx, async move |this, cx| {
            // Agregar a repositorios recientes
            if let Some(recent) = recent {
                let write = this.update(cx, |this, cx| {
                    this.storage(cx, move |store| {
                        store
                            .add_recent_repository(&recent)
                            .map_err(|error| error.to_string())
                    })
                });
                let recent_repositories = write.await.map_err(|error| anyhow!(error))?;
                this.update(cx, |this, cx| {
                    this.repositories.recent_repositories =
                        recent_repositories.iter().map(recent_from_store).collect();
                    cx.notify();
                });
            }

            this.update(cx, |this, cx| this.persist_workspace_session(cx))
                .await;
            Ok(())
        })
    }

    /// `removeRepository`: cierra una pestaña.
    pub fn remove_repository<'a>(&mut self, key: impl Into<RepoKey<'a>>, cx: &mut Context<Self>) {
        let previous_id = self.active_repository_id();
        let key = key.into();
        let path = self
            .repositories
            .repository(key)
            .map(|repository| repository.path.clone());
        if self.repositories.remove_repository(key).is_none() {
            return;
        }
        // Las credenciales HTTP que solo usaba esta pestaña se borran.
        if let Some(path) = &path {
            self.release_http_credentials(path, cx);
        }
        // Su resumen solo sirve al inicio, que lista los recientes y los
        // abiertos: si no está entre los recientes ya no se verá.
        if let Some(path) = path
            && !self
                .repositories
                .recent_repositories
                .iter()
                .any(|recent| recent.path == path)
        {
            self.summaries.remove(&path);
        }

        self.persist_workspace_session(cx).detach();
        self.after_active_repository_change(previous_id, cx);
        cx.notify();
    }

    /// `setActiveRepository`: cambia la pestaña activa (`None` deja ninguna).
    pub fn set_active_repository(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        let previous_id = self.active_repository_id();
        if self.repositories.set_active_repository(index).is_none() {
            return;
        }

        self.persist_workspace_session(cx).detach();
        self.after_active_repository_change(previous_id, cx);
        cx.notify();
    }

    // ------------------------------------------------------------------
    // Estado de un repositorio
    // ------------------------------------------------------------------

    /// `updateRepositoryData`: aplica `update` al repositorio y avisa si
    /// algo cambió. El cierre dice si cambió (`bool`); uno que no lo dice
    /// (`()`) cuenta como cambio. Los setters de este módulo usan
    /// [`tracked`], que lo averigua sin clonar las listas del repositorio.
    pub fn update_repository<'a, C: RepositoryChange>(
        &mut self,
        key: impl Into<RepoKey<'a>>,
        cx: &mut Context<Self>,
        update: impl FnOnce(&mut OpenRepository) -> C,
    ) {
        if self.update_repository_quietly(key, update) {
            cx.notify();
        }
    }

    /// Como [`AppModel::update_repository`], pero el aviso se agrupa con los
    /// demás cambios de esta vuelta del executor ([`AppModel::notify_soon`]).
    /// Lo usan las relecturas, que aplican varias secciones seguidas.
    pub(crate) fn update_repository_soon<'a>(
        &mut self,
        key: impl Into<RepoKey<'a>>,
        cx: &mut Context<Self>,
        update: impl FnOnce(&mut OpenRepository) -> bool,
    ) {
        if self.update_repository_quietly(key, update) {
            self.notify_soon(cx);
        }
    }

    /// Aplica `update` sin avisar. Devuelve si el repositorio cambió (una
    /// relectura que trae lo mismo que había no repinta nada).
    fn update_repository_quietly<'a, C: RepositoryChange>(
        &mut self,
        key: impl Into<RepoKey<'a>>,
        update: impl FnOnce(&mut OpenRepository) -> C,
    ) -> bool {
        let Some(repository) = self.repositories.repository_mut(key) else {
            return false;
        };
        update(repository).changed()
    }

    /// `setRepositoryMutationQueue`.
    pub fn set_repository_mutation_queue<'a>(
        &mut self,
        key: impl Into<RepoKey<'a>>,
        queue: &MutationQueueSnapshot,
        cx: &mut Context<Self>,
    ) {
        self.update_repository(key, cx, |repository| {
            tracked::set_mutation_queue(repository, queue)
        });
    }

    /// `applyRepositoryOperationEvent`: la cola tal como la trae un evento.
    pub fn apply_repository_operation_event<'a>(
        &mut self,
        key: impl Into<RepoKey<'a>>,
        operations: &[QueueOperation],
        cx: &mut Context<Self>,
    ) {
        self.update_repository(key, cx, |repository| {
            tracked::apply_operation_event(repository, operations)
        });
    }

    /// `setRepositoryStatus`.
    pub fn set_repository_status<'a>(
        &mut self,
        key: impl Into<RepoKey<'a>>,
        status: Option<StructuredStatus>,
        cx: &mut Context<Self>,
    ) {
        self.update_repository(key, cx, |repository| {
            tracked::set_status(repository, status)
        });
    }

    /// `applyRepositoryOperationResult`: aplica al repositorio el estado que
    /// trae el resultado (o invalida el que había), anota la mutación y deja el
    /// resultado en el log de operaciones.
    ///
    /// Todos los resultados de mutaciones pasan por aquí: es el punto único
    /// donde alimentar el log de operaciones con el mensaje completo.
    pub fn apply_repository_operation_result<'a>(
        &mut self,
        key: impl Into<RepoKey<'a>>,
        result: &impl StructuredOutcome,
        cx: &mut Context<Self>,
    ) {
        let key = key.into();
        let result = result.to_operation_result();
        let repo_id = self
            .repositories
            .repository(key)
            .map(|repository| repository.id.clone());

        self.repositories
            .apply_repository_operation_result(key, &result, &mut self.operations_log);

        // Un fallo sin estado adjunto (git se negó: cambios locales, ref
        // inexistente…) deja el estado invalidado y los flujos no releen tras
        // un fallo: sin esta relectura la fila de cambios sin commitear y los
        // contadores desaparecían hasta reiniciar.
        if let Some(repo_id) = repo_id.as_deref()
            && failed_without_status(&result)
        {
            self.sync_repository_read_model(
                repo_id,
                SyncRequest::new().status(true).changes_count(true),
                cx,
            )
            .detach();
        }

        if let Some(repo_id) = repo_id {
            cx.emit(AppEvent::MutationRecorded {
                repo_id,
                operation: result.operation.clone().filter(|name| !name.is_empty()),
            });
        }
        cx.notify();
    }

    /// `setRepositorySidebarLoading`.
    pub fn set_repository_sidebar_loading<'a>(
        &mut self,
        key: impl Into<RepoKey<'a>>,
        section: SidebarSection,
        is_loading: bool,
        cx: &mut Context<Self>,
    ) {
        self.update_repository(key, cx, |repository| {
            tracked::set_sidebar_loading(repository, section, is_loading)
        });
    }

    /// `setRepositoryBranchesData`.
    pub fn set_repository_branches_data<'a>(
        &mut self,
        key: impl Into<RepoKey<'a>>,
        branches: Vec<BranchItem>,
        current_branch: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.update_repository(key, cx, |repository| {
            tracked::set_branches_data(repository, branches, current_branch)
        });
    }

    /// `setRepositoryStarredBranches`.
    pub fn set_repository_starred_branches<'a>(
        &mut self,
        key: impl Into<RepoKey<'a>>,
        starred_branches: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        self.update_repository(key, cx, |repository| {
            tracked::set_starred_branches(repository, starred_branches)
        });
    }

    /// `setRepositoryRemotesData`.
    pub fn set_repository_remotes_data<'a>(
        &mut self,
        key: impl Into<RepoKey<'a>>,
        remotes: Vec<RemoteItem>,
        cx: &mut Context<Self>,
    ) {
        self.update_repository(key, cx, |repository| {
            tracked::set_remotes_data(repository, remotes)
        });
    }

    /// `setRepositoryTagsData`.
    pub fn set_repository_tags_data<'a>(
        &mut self,
        key: impl Into<RepoKey<'a>>,
        tags: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        self.update_repository(key, cx, |repository| {
            tracked::set_tags_data(repository, tags)
        });
    }

    /// `setRepositoryStashesData`.
    pub fn set_repository_stashes_data<'a>(
        &mut self,
        key: impl Into<RepoKey<'a>>,
        stashes: Vec<StashEntry>,
        cx: &mut Context<Self>,
    ) {
        self.update_repository(key, cx, |repository| {
            tracked::set_stashes_data(repository, stashes)
        });
    }

    /// `setRepositorySubmodulesData`.
    pub fn set_repository_submodules_data<'a>(
        &mut self,
        key: impl Into<RepoKey<'a>>,
        submodules: Vec<Submodule>,
        cx: &mut Context<Self>,
    ) {
        self.update_repository(key, cx, |repository| {
            tracked::set_submodules_data(repository, submodules)
        });
    }

    /// `invalidateRepositorySidebarData`.
    pub fn invalidate_repository_sidebar_data<'a>(
        &mut self,
        key: impl Into<RepoKey<'a>>,
        cx: &mut Context<Self>,
    ) {
        self.update_repository(key, cx, |repository| {
            tracked::invalidate_sidebar_data(repository)
        });
    }

    /// `invalidateRepositorySidebarSections`.
    pub fn invalidate_repository_sidebar_sections<'a>(
        &mut self,
        key: impl Into<RepoKey<'a>>,
        sections: &[SidebarSection],
        cx: &mut Context<Self>,
    ) {
        self.update_repository(key, cx, |repository| {
            tracked::invalidate_sidebar_sections(repository, sections)
        });
    }

    /// `invalidateRepositoryState`.
    pub fn invalidate_repository_state<'a>(
        &mut self,
        key: impl Into<RepoKey<'a>>,
        cx: &mut Context<Self>,
    ) {
        self.update_repository(key, cx, tracked::invalidate_state);
    }

    /// `updateRepositoryBranch`.
    pub fn update_repository_branch(
        &mut self,
        repo_id: &str,
        branch_name: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.update_repository(repo_id, cx, |repository| {
            tracked::update_branch(repository, branch_name)
        });
    }

    // ------------------------------------------------------------------
    // Recientes
    // ------------------------------------------------------------------

    /// `removeRecentRepository`.
    pub fn remove_recent_repository(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        let path = repo_path.to_string();
        let write = self.storage(cx, {
            let path = path.clone();
            move |store| {
                store
                    .remove_recent_repository(&path)
                    .map_err(|error| error.to_string())
            }
        });
        flow(cx, async move |this, cx| {
            write.await.map_err(|error| anyhow!(error))?;
            // Actualizar la lista local directamente
            this.update(cx, |this, cx| {
                this.repositories.remove_recent_repository(&path);
                cx.notify();
            });
            Ok(())
        })
    }

    /// `clearRecentRepositories`.
    pub fn clear_recent_repositories(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        let write = self.storage(cx, |store| {
            store
                .clear_recent_repositories()
                .map_err(|error| error.to_string())
        });
        flow(cx, async move |this, cx| {
            write.await.map_err(|error| anyhow!(error))?;
            this.update(cx, |this, cx| {
                this.repositories.clear_recent_repositories();
                cx.notify();
            });
            Ok(())
        })
    }

    // ------------------------------------------------------------------
    // Ramas destacadas
    // ------------------------------------------------------------------

    /// `storage.addStarredBranch` + la relectura de la sección.
    pub fn add_starred_branch(
        &mut self,
        repo_path: &str,
        branch_name: &str,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<Vec<String>>> {
        let path = repo_path.to_string();
        let branch = branch_name.to_string();
        self.write_starred_branches(repo_path, cx, move |store| {
            store.add_starred_branch(&path, &branch)
        })
    }

    /// `storage.removeStarredBranch` + la relectura de la sección.
    pub fn remove_starred_branch(
        &mut self,
        repo_path: &str,
        branch_name: &str,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<Vec<String>>> {
        let path = repo_path.to_string();
        let branch = branch_name.to_string();
        self.write_starred_branches(repo_path, cx, move |store| {
            store.remove_starred_branch(&path, &branch)
        })
    }

    fn write_starred_branches(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
        write: impl FnOnce(&mergeon_backend::Store) -> Result<Vec<String>, mergeon_backend::StoreError>
        + Send
        + 'static,
    ) -> Task<anyhow::Result<Vec<String>>> {
        let repo_path = repo_path.to_string();
        let write = self.storage(cx, move |store| {
            write(store).map_err(|error| error.to_string())
        });
        flow(cx, async move |this, cx| {
            let starred = write.await.map_err(|error| anyhow!(error))?;
            this.update(cx, |this, cx| {
                let repo_id = this
                    .repositories
                    .repositories
                    .iter()
                    .find(|repository| repository.path == repo_path)
                    .map(|repository| repository.id.clone());
                if let Some(repo_id) = repo_id {
                    this.set_repository_starred_branches(repo_id.as_str(), starred.clone(), cx);
                }
            });
            Ok(starred)
        })
    }
}

/// Una operación que falló sin cambiar nada que traiga en el resultado: ni
/// `noop` ni `conflicted` (esos ya los releen los flujos).
fn failed_without_status(result: &mergeon_model::operation_types::OperationResult) -> bool {
    !result.success
        && result.status.is_none()
        && !matches!(result.outcome.as_deref(), Some("noop" | "conflicted"))
}

/// Lo que devuelve el cierre de [`AppModel::update_repository`]: si el
/// repositorio cambió. `()` (un cierre que no lo sabe) cuenta como cambio.
pub trait RepositoryChange {
    fn changed(self) -> bool;
}

impl RepositoryChange for bool {
    fn changed(self) -> bool {
        self
    }
}

impl RepositoryChange for () {
    fn changed(self) -> bool {
        true
    }
}

/// Los setters de [`OpenRepository`] que dicen si cambiaron algo, sin clonar
/// el repositorio entero.
///
/// Antes se clonaba el repositorio completo (status, listas de la barra
/// lateral y commits) antes de cada setter para compararlo después. Aquí lo
/// pesado no se clona nunca: lo que el setter reemplaza se aparta (se mueve,
/// no se copia) para compararlo con lo nuevo, las listas que solo puede
/// vaciar se comparan por si estaban vacías, y del resto (escalares, flags,
/// cola de mutaciones, conflicto) se compara una copia barata.
pub(crate) mod tracked {
    use std::mem;

    use mergeon_backend::{StashEntry, StructuredStatus, Submodule};
    use mergeon_model::commit_view_utils::CommitRow;
    use mergeon_model::operation_types::QueueOperation;
    use mergeon_model::repositories::{MutationQueueSnapshot, SidebarSection};
    use mergeon_model::repository_read_model::{BranchItem, RemoteItem};

    use crate::OpenRepository;

    /// Las partes caras de clonar de un repositorio.
    #[derive(Default)]
    struct Heavy {
        commits: Vec<CommitRow>,
        branches: Vec<BranchItem>,
        starred_branches: Vec<String>,
        remotes: Vec<RemoteItem>,
        tags: Vec<String>,
        stashes: Vec<StashEntry>,
        submodules: Vec<Submodule>,
        status: Option<StructuredStatus>,
    }

    impl Heavy {
        fn take(repository: &mut OpenRepository) -> Self {
            Self {
                commits: mem::take(&mut repository.commits),
                branches: mem::take(&mut repository.branches),
                starred_branches: mem::take(&mut repository.starred_branches),
                remotes: mem::take(&mut repository.remotes),
                tags: mem::take(&mut repository.tags),
                stashes: mem::take(&mut repository.stashes),
                submodules: mem::take(&mut repository.submodules),
                status: repository.status.take(),
            }
        }

        fn restore(self, repository: &mut OpenRepository) {
            repository.commits = self.commits;
            repository.branches = self.branches;
            repository.starred_branches = self.starred_branches;
            repository.remotes = self.remotes;
            repository.tags = self.tags;
            repository.stashes = self.stashes;
            repository.submodules = self.submodules;
            repository.status = self.status;
        }

        /// Qué partes están vacías (barato: sin recorrer nada).
        fn emptiness(repository: &OpenRepository) -> [bool; 8] {
            [
                repository.commits.is_empty(),
                repository.branches.is_empty(),
                repository.starred_branches.is_empty(),
                repository.remotes.is_empty(),
                repository.tags.is_empty(),
                repository.stashes.is_empty(),
                repository.submodules.is_empty(),
                repository.status.is_none(),
            ]
        }
    }

    /// La parte pesada que un setter reemplaza entera.
    #[derive(Clone, Copy)]
    enum Part {
        Status,
        Branches,
        StarredBranches,
        Remotes,
        Tags,
        Stashes,
        Submodules,
    }

    impl Part {
        const fn index(self) -> usize {
            match self {
                Part::Branches => 1,
                Part::StarredBranches => 2,
                Part::Remotes => 3,
                Part::Tags => 4,
                Part::Stashes => 5,
                Part::Submodules => 6,
                Part::Status => 7,
            }
        }

        /// Aparta la parte (sin copiarla) y la deja vacía en el repositorio.
        fn take(self, repository: &mut OpenRepository) -> Heavy {
            let mut heavy = Heavy::default();
            match self {
                Part::Status => heavy.status = repository.status.take(),
                Part::Branches => heavy.branches = mem::take(&mut repository.branches),
                Part::StarredBranches => {
                    heavy.starred_branches = mem::take(&mut repository.starred_branches)
                }
                Part::Remotes => heavy.remotes = mem::take(&mut repository.remotes),
                Part::Tags => heavy.tags = mem::take(&mut repository.tags),
                Part::Stashes => heavy.stashes = mem::take(&mut repository.stashes),
                Part::Submodules => heavy.submodules = mem::take(&mut repository.submodules),
            }
            heavy
        }

        fn unchanged(self, repository: &OpenRepository, before: &Heavy) -> bool {
            match self {
                Part::Status => repository.status == before.status,
                Part::Branches => repository.branches == before.branches,
                Part::StarredBranches => repository.starred_branches == before.starred_branches,
                Part::Remotes => repository.remotes == before.remotes,
                Part::Tags => repository.tags == before.tags,
                Part::Stashes => repository.stashes == before.stashes,
                Part::Submodules => repository.submodules == before.submodules,
            }
        }
    }

    /// Copia de todo lo barato del repositorio (lo pesado queda vacío).
    fn light_copy(repository: &mut OpenRepository) -> OpenRepository {
        let heavy = Heavy::take(repository);
        let copy = repository.clone();
        heavy.restore(repository);
        copy
    }

    fn light_eq(repository: &mut OpenRepository, light: &OpenRepository) -> bool {
        let heavy = Heavy::take(repository);
        let equal = *repository == *light;
        heavy.restore(repository);
        equal
    }

    /// Aplica `apply` y dice si cambió algo.
    ///
    /// `replaces` es la parte pesada que el setter reemplaza entera: se
    /// aparta antes (el setter no debe leerla) y se compara con la nueva. Las
    /// demás partes pesadas el setter solo puede vaciarlas, así que basta con
    /// ver si alguna pasó de tener algo a estar vacía.
    fn track(
        repository: &mut OpenRepository,
        replaces: Option<Part>,
        apply: impl FnOnce(&mut OpenRepository),
    ) -> bool {
        let emptiness = Heavy::emptiness(repository);
        let replaced = replaces.map(|part| part.take(repository));
        let light = light_copy(repository);

        apply(repository);

        let mut changed = !light_eq(repository, &light);
        if let (Some(part), Some(before)) = (replaces, &replaced) {
            changed |= !part.unchanged(repository, before);
        }
        if !changed {
            let after = Heavy::emptiness(repository);
            changed = after
                .iter()
                .zip(emptiness)
                .enumerate()
                .any(|(index, (after, before))| {
                    *after != before && replaces.is_none_or(|part| part.index() != index)
                });
        }
        changed
    }

    pub(crate) fn set_mutation_queue(
        repository: &mut OpenRepository,
        queue: &MutationQueueSnapshot,
    ) -> bool {
        track(repository, None, |repository| {
            repository.set_mutation_queue(queue)
        })
    }

    pub(crate) fn apply_operation_event(
        repository: &mut OpenRepository,
        operations: &[QueueOperation],
    ) -> bool {
        track(repository, None, |repository| {
            repository.apply_operation_event(operations)
        })
    }

    /// `set_status` no lee el status anterior: lo deriva todo de la lectura
    /// nueva y del resto del repositorio, así que apartarlo es seguro.
    pub(crate) fn set_status(
        repository: &mut OpenRepository,
        status: Option<StructuredStatus>,
    ) -> bool {
        track(repository, Some(Part::Status), |repository| {
            repository.set_status(status)
        })
    }

    pub(crate) fn set_sidebar_loading(
        repository: &mut OpenRepository,
        section: SidebarSection,
        is_loading: bool,
    ) -> bool {
        track(repository, None, |repository| {
            repository.set_sidebar_loading(section, is_loading)
        })
    }

    pub(crate) fn set_branches_data(
        repository: &mut OpenRepository,
        branches: Vec<BranchItem>,
        current_branch: Option<&str>,
    ) -> bool {
        track(repository, Some(Part::Branches), |repository| {
            repository.set_branches_data(branches, current_branch)
        })
    }

    pub(crate) fn set_starred_branches(
        repository: &mut OpenRepository,
        starred_branches: Vec<String>,
    ) -> bool {
        track(repository, Some(Part::StarredBranches), |repository| {
            repository.set_starred_branches(starred_branches)
        })
    }

    pub(crate) fn set_remotes_data(
        repository: &mut OpenRepository,
        remotes: Vec<RemoteItem>,
    ) -> bool {
        track(repository, Some(Part::Remotes), |repository| {
            repository.set_remotes_data(remotes)
        })
    }

    pub(crate) fn set_tags_data(repository: &mut OpenRepository, tags: Vec<String>) -> bool {
        track(repository, Some(Part::Tags), |repository| {
            repository.set_tags_data(tags)
        })
    }

    pub(crate) fn set_stashes_data(
        repository: &mut OpenRepository,
        stashes: Vec<StashEntry>,
    ) -> bool {
        track(repository, Some(Part::Stashes), |repository| {
            repository.set_stashes_data(stashes)
        })
    }

    pub(crate) fn set_submodules_data(
        repository: &mut OpenRepository,
        submodules: Vec<Submodule>,
    ) -> bool {
        track(repository, Some(Part::Submodules), |repository| {
            repository.set_submodules_data(submodules)
        })
    }

    pub(crate) fn invalidate_sidebar_data(repository: &mut OpenRepository) -> bool {
        track(repository, None, OpenRepository::invalidate_sidebar_data)
    }

    pub(crate) fn invalidate_sidebar_sections(
        repository: &mut OpenRepository,
        sections: &[SidebarSection],
    ) -> bool {
        track(repository, None, |repository| {
            repository.invalidate_sidebar_sections(sections)
        })
    }

    pub(crate) fn invalidate_state(repository: &mut OpenRepository) -> bool {
        track(repository, None, OpenRepository::invalidate_state)
    }

    pub(crate) fn update_branch(
        repository: &mut OpenRepository,
        branch_name: Option<&str>,
    ) -> bool {
        track(repository, None, |repository| {
            repository.update_branch(branch_name)
        })
    }
}

#[cfg(test)]
mod tracked_tests {
    use mergeon_backend::{FileStatus, StructuredStatus};
    use mergeon_model::repositories::{MutationQueueSnapshot, SidebarSection};
    use mergeon_model::repository_read_model::BranchItem;

    use super::{RepositoryChange, tracked};
    use crate::OpenRepository;

    fn repository() -> OpenRepository {
        OpenRepository::new("/tmp/tracked-demo", None).unwrap()
    }

    fn status(branch: &str, files: &[&str]) -> StructuredStatus {
        StructuredStatus {
            current: Some(branch.to_string()),
            head_short_hash: Some("abc1234".to_string()),
            files: files
                .iter()
                .map(|path| FileStatus {
                    path: (*path).to_string(),
                    index: " ".to_string(),
                    working_dir: "M".to_string(),
                    from: None,
                })
                .collect(),
            ..StructuredStatus::default()
        }
    }

    fn branch(name: &str) -> BranchItem {
        BranchItem {
            name: name.to_string(),
            ..BranchItem::default()
        }
    }

    #[test]
    fn the_same_status_twice_reports_no_change() {
        let mut repo = repository();
        assert!(tracked::set_status(
            &mut repo,
            Some(status("main", &["a.txt"]))
        ));
        assert_eq!(repo.changes_count, 1);
        assert!(!tracked::set_status(
            &mut repo,
            Some(status("main", &["a.txt"]))
        ));
        // El status apartado vuelve a su sitio intacto.
        assert_eq!(repo.status.as_ref().unwrap().files.len(), 1);
    }

    #[test]
    fn a_different_status_reports_a_change() {
        let mut repo = repository();
        tracked::set_status(&mut repo, Some(status("main", &["a.txt"])));
        // Solo cambia la lista de archivos (mismo número): se nota igual.
        assert!(tracked::set_status(
            &mut repo,
            Some(status("main", &["b.txt"]))
        ));
        assert!(tracked::set_status(
            &mut repo,
            Some(status("dev", &["b.txt"]))
        ));
        assert_eq!(repo.current_branch.as_deref(), Some("dev"));
        assert!(tracked::set_status(&mut repo, None));
        assert!(!tracked::set_status(&mut repo, None));
    }

    #[test]
    fn identical_sections_report_no_change_and_new_ones_do() {
        let mut repo = repository();
        tracked::set_status(&mut repo, Some(status("main", &[])));

        assert!(tracked::set_branches_data(
            &mut repo,
            vec![branch("main")],
            Some("main")
        ));
        assert!(!tracked::set_branches_data(
            &mut repo,
            vec![branch("main")],
            Some("main")
        ));
        assert!(tracked::set_branches_data(
            &mut repo,
            vec![branch("main"), branch("dev")],
            Some("main")
        ));
        assert_eq!(repo.branches.len(), 2);

        assert!(tracked::set_tags_data(&mut repo, vec!["v1".to_string()]));
        assert!(!tracked::set_tags_data(&mut repo, vec!["v1".to_string()]));
        assert!(tracked::set_tags_data(&mut repo, vec!["v2".to_string()]));

        assert!(tracked::set_remotes_data(&mut repo, Vec::new()));
        assert!(!tracked::set_remotes_data(&mut repo, Vec::new()));
        assert!(tracked::set_stashes_data(&mut repo, Vec::new()));
        assert!(!tracked::set_stashes_data(&mut repo, Vec::new()));
        assert!(tracked::set_submodules_data(&mut repo, Vec::new()));
        assert!(!tracked::set_submodules_data(&mut repo, Vec::new()));
        assert!(tracked::set_starred_branches(
            &mut repo,
            vec!["main".to_string()]
        ));
        assert!(!tracked::set_starred_branches(
            &mut repo,
            vec!["main".to_string()]
        ));

        // Las listas que no tocó ningún setter siguen en su sitio.
        assert_eq!(repo.tags, ["v2"]);
        assert_eq!(repo.branches.len(), 2);
        assert!(repo.status.is_some());
    }

    #[test]
    fn flags_and_scalars_report_only_real_changes() {
        let mut repo = repository();
        assert!(tracked::set_sidebar_loading(
            &mut repo,
            SidebarSection::Tags,
            true
        ));
        assert!(!tracked::set_sidebar_loading(
            &mut repo,
            SidebarSection::Tags,
            true
        ));
        assert!(tracked::set_sidebar_loading(
            &mut repo,
            SidebarSection::Tags,
            false
        ));

        assert!(tracked::update_branch(&mut repo, Some("main")));
        assert!(!tracked::update_branch(&mut repo, Some("main")));

        let queue = MutationQueueSnapshot::default();
        assert!(!tracked::set_mutation_queue(&mut repo, &queue));
        assert!(!tracked::apply_operation_event(&mut repo, &[]));
    }

    #[test]
    fn invalidations_report_a_change_only_when_something_was_there() {
        let mut repo = repository();
        assert!(!tracked::invalidate_sidebar_sections(
            &mut repo,
            &[SidebarSection::Tags]
        ));
        assert!(!tracked::invalidate_state(&mut repo));

        tracked::set_tags_data(&mut repo, vec!["v1".to_string()]);
        assert!(tracked::invalidate_sidebar_sections(
            &mut repo,
            &[SidebarSection::Tags]
        ));
        assert!(repo.tags.is_empty());
        assert!(!tracked::invalidate_sidebar_sections(
            &mut repo,
            &[SidebarSection::Tags]
        ));

        tracked::set_status(&mut repo, Some(status("main", &[])));
        assert!(tracked::invalidate_state(&mut repo));
        assert!(repo.status.is_none());
        assert!(!tracked::invalidate_state(&mut repo));

        tracked::set_branches_data(&mut repo, vec![branch("main")], Some("main"));
        assert!(tracked::invalidate_sidebar_data(&mut repo));
        assert!(repo.branches.is_empty());
        assert!(!tracked::invalidate_sidebar_data(&mut repo));
    }

    #[test]
    fn a_closure_without_a_verdict_counts_as_a_change() {
        assert!(().changed());
        assert!(true.changed());
        assert!(!false.changed());
    }
}
