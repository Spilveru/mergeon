//! Repositorios abiertos (`src/stores/repositories.js`): el objeto de cada
//! repositorio, las reglas que derivan su estado y la lista de pestañas.
//!
//! Aquí está solo lo que transforma datos. Lo que en el store de Pinia llamaba
//! a IPC o persistía (repos recientes, sesión) queda para quien use esto: los
//! métodos que cambian algo persistible devuelven lo que hay que guardar.
//!
//! [`Repository`] es genérico en los tipos de stash y submódulo, que define el
//! crate de lecturas; por defecto son JSON sin tipar.

use mergeon_gitcmd::{ConflictState, OperationState, StructuredStatus};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::commit_view_utils::CommitRow;
use crate::format::now_iso_string;
use crate::jsstr::{js_trim, utf16_len};
use crate::operation_types::{OperationPhase, OperationResult, QueueOperation, truthy};
use crate::operations_log::{OperationsLog, ResultContext};
use crate::repository_read_model::{BranchItem, RemoteItem};
use crate::workspace_tabs::{RepositoryWorkspace, normalize_repository_workspace};

// ---------------------------------------------------------------------------
// Piezas del estado
// ---------------------------------------------------------------------------

pub fn create_empty_conflict_state() -> ConflictState {
    ConflictState::default()
}

/// Cola de mutaciones de un repositorio, como la pinta el banner.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MutationQueue {
    /// La operación en curso (`running` o `cancelling`).
    pub active: Option<QueueOperation>,
    /// Las que esperan, con `position` desde 1.
    pub queued: Vec<QueueOperation>,
}

pub fn create_empty_mutation_queue() -> MutationQueue {
    MutationQueue::default()
}

/// Lo que llega del gate: la lista completa (`operations`) o, en su forma
/// antigua, la activa más las encoladas.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MutationQueueSnapshot {
    pub operations: Option<Vec<QueueOperation>>,
    pub active: Option<QueueOperation>,
    pub queued: Option<Vec<QueueOperation>>,
}

pub fn normalize_mutation_queue(queue: &MutationQueueSnapshot) -> MutationQueue {
    match &queue.operations {
        Some(operations) => normalize_mutation_operations(operations),
        None => {
            let operations: Vec<QueueOperation> = queue
                .active
                .iter()
                .chain(queue.queued.iter().flatten())
                .cloned()
                .collect();
            normalize_mutation_operations(&operations)
        }
    }
}

/// [`normalize_mutation_queue`] a partir de la lista de operaciones de un evento.
pub fn normalize_mutation_operations(operations: &[QueueOperation]) -> MutationQueue {
    MutationQueue {
        active: operations
            .iter()
            .find(|operation| {
                matches!(
                    operation.phase,
                    OperationPhase::Running | OperationPhase::Cancelling
                )
            })
            .cloned(),
        queued: operations
            .iter()
            .filter(|operation| operation.phase == OperationPhase::Queued)
            .enumerate()
            .map(|(index, operation)| QueueOperation {
                position: index + 1,
                ..operation.clone()
            })
            .collect(),
    }
}

/// Secciones de la barra lateral que se cargan (y se invalidan) por separado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SidebarSection {
    Branches,
    StarredBranches,
    Remotes,
    Tags,
    Stashes,
    Submodules,
    ChangesCount,
}

impl SidebarSection {
    pub const ALL: [SidebarSection; 7] = [
        SidebarSection::Branches,
        SidebarSection::StarredBranches,
        SidebarSection::Remotes,
        SidebarSection::Tags,
        SidebarSection::Stashes,
        SidebarSection::Submodules,
        SidebarSection::ChangesCount,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SidebarSection::Branches => "branches",
            SidebarSection::StarredBranches => "starredBranches",
            SidebarSection::Remotes => "remotes",
            SidebarSection::Tags => "tags",
            SidebarSection::Stashes => "stashes",
            SidebarSection::Submodules => "submodules",
            SidebarSection::ChangesCount => "changesCount",
        }
    }
}

/// Un booleano por sección: `sidebarLoading` y `sidebarLoaded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SidebarFlags {
    pub branches: bool,
    pub starred_branches: bool,
    pub remotes: bool,
    pub tags: bool,
    pub stashes: bool,
    pub submodules: bool,
    pub changes_count: bool,
}

impl SidebarFlags {
    pub fn get(&self, section: SidebarSection) -> bool {
        match section {
            SidebarSection::Branches => self.branches,
            SidebarSection::StarredBranches => self.starred_branches,
            SidebarSection::Remotes => self.remotes,
            SidebarSection::Tags => self.tags,
            SidebarSection::Stashes => self.stashes,
            SidebarSection::Submodules => self.submodules,
            SidebarSection::ChangesCount => self.changes_count,
        }
    }

    pub fn set(&mut self, section: SidebarSection, value: bool) {
        let flag = match section {
            SidebarSection::Branches => &mut self.branches,
            SidebarSection::StarredBranches => &mut self.starred_branches,
            SidebarSection::Remotes => &mut self.remotes,
            SidebarSection::Tags => &mut self.tags,
            SidebarSection::Stashes => &mut self.stashes,
            SidebarSection::Submodules => &mut self.submodules,
            SidebarSection::ChangesCount => &mut self.changes_count,
        };
        *flag = value;
    }
}

pub fn create_empty_sidebar_loading_state() -> SidebarFlags {
    SidebarFlags::default()
}

pub fn create_empty_sidebar_loaded_state() -> SidebarFlags {
    SidebarFlags::default()
}

/// Un grupo nombrado de repositorios (workspace), para filtrar las pestañas
/// de la barra de título. Distinto de [`RepositoryWorkspace`], que guarda las
/// pestañas de archivos dentro de un único repositorio.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoWorkspace {
    pub id: String,
    pub name: String,
    /// Rutas normalizadas de los repositorios de este grupo, en su orden.
    #[serde(default)]
    pub repositories: Vec<String>,
    /// Último repositorio activo dentro de este grupo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_repository_path: Option<String>,
}

/// Lo que se guarda para reabrir la app como estaba.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSession {
    pub open_repositories: Vec<String>,
    pub active_repository_path: Option<String>,
    /// Pestañas de cada repositorio, por ruta.
    pub repository_workspaces: BTreeMap<String, RepositoryWorkspace>,
    /// Grupos nombrados de repositorios. Se omiten si está vacío para
    /// conservar el JSON idéntico a sesiones anteriores.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<RepoWorkspace>,
    /// Grupo activo (`None` = "All", muestra todos los repositorios abiertos).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_workspace_id: Option<String>,
}

impl<'de> Deserialize<'de> for WorkspaceSession {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(normalize_workspace_session(&value))
    }
}

pub fn create_empty_workspace_session() -> WorkspaceSession {
    WorkspaceSession::default()
}

fn non_blank(value: &Value) -> Option<&str> {
    value.as_str().filter(|path| !js_trim(path).is_empty())
}

fn normalize_repo_workspace(value: &Value) -> Option<RepoWorkspace> {
    let object = value.as_object()?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .map(js_trim)
        .filter(|id| !id.is_empty())?
        .to_string();
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .map(js_trim)
        .filter(|name| !name.is_empty())?
        .to_string();

    let mut repositories: Vec<String> = Vec::new();
    for item in object
        .get("repositories")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(raw) = item.as_str() else {
            continue;
        };
        let normalized = normalize_repository_path(raw);
        if !normalized.is_empty() && !repositories.contains(&normalized) {
            repositories.push(normalized);
        }
    }

    let active_repository_path = object
        .get("activeRepositoryPath")
        .and_then(Value::as_str)
        .map(normalize_repository_path)
        .filter(|path| !path.is_empty() && repositories.contains(path));

    Some(RepoWorkspace {
        id,
        name,
        repositories,
        active_repository_path,
    })
}

/// Deja una sesión guardada (posiblemente vieja o corrupta) en una forma válida.
pub fn normalize_workspace_session(session: &Value) -> WorkspaceSession {
    let mut workspaces: Vec<RepoWorkspace> = Vec::new();
    for item in session
        .get("workspaces")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(workspace) = normalize_repo_workspace(item)
            && !workspaces
                .iter()
                .any(|existing| existing.id == workspace.id)
        {
            workspaces.push(workspace);
        }
    }

    let active_workspace_id = session
        .get("activeWorkspaceId")
        .and_then(Value::as_str)
        .map(js_trim)
        .filter(|id| !id.is_empty() && workspaces.iter().any(|ws| ws.id == *id))
        .map(str::to_string);

    WorkspaceSession {
        open_repositories: session
            .get("openRepositories")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(non_blank)
            .map(str::to_string)
            .collect(),
        active_repository_path: session
            .get("activeRepositoryPath")
            .and_then(non_blank)
            .map(str::to_string),
        repository_workspaces: session
            .get("repositoryWorkspaces")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(path, state)| (path.clone(), normalize_repository_workspace(state)))
            .collect(),
        workspaces,
        active_workspace_id,
    }
}

/// Para cada repositorio `(nombre, ruta)`, el tramo de carpetas padre más
/// corto que lo distingue de los demás con el mismo nombre (`None` si su
/// nombre es único), como hacen VS Code y Zed con archivos homónimos:
/// `fx1/demo` y `fx2/demo` dan `fx1` y `fx2`; `a/x/demo` y `b/x/demo`, `a/x` y
/// `b/x`. Si hace falta la ruta padre entera se devuelve tal cual (con su `/`
/// inicial).
pub fn distinguishing_parent_suffixes<'a>(
    entries: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Vec<Option<String>> {
    let entries: Vec<(&str, Vec<&str>, bool)> = entries
        .into_iter()
        .map(|(name, path)| {
            let normalized = path.trim_end_matches(['/', '\\']);
            let mut segments: Vec<&str> = normalized
                .split(['/', '\\'])
                .filter(|segment| !segment.is_empty())
                .collect();
            segments.pop();
            (name, segments, normalized.starts_with('/'))
        })
        .collect();

    // Los `count` últimos padres (o todos, si no hay tantos).
    let key =
        |parents: &[&str], count: usize| parents[parents.len().saturating_sub(count)..].join("/");
    // Lo que se muestra: la clave, o la ruta padre entera cuando ya la abarca.
    let display = |(_, parents, absolute): &(&str, Vec<&str>, bool), count: usize| {
        let joined = key(parents, count);
        if count >= parents.len() && *absolute {
            format!("/{joined}")
        } else {
            joined
        }
    };

    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let others: Vec<&(&str, Vec<&str>, bool)> = entries
                .iter()
                .enumerate()
                .filter(|(other, candidate)| *other != index && candidate.0 == entry.0)
                .map(|(_, candidate)| candidate)
                .collect();
            if others.is_empty() {
                return None;
            }
            let mut count = 1;
            loop {
                let mine = key(&entry.1, count);
                let unique = others.iter().all(|other| key(&other.1, count) != mine);
                if unique || count >= entry.1.len() {
                    return Some(display(entry, count)).filter(|suffix| !suffix.is_empty());
                }
                count += 1;
            }
        })
        .collect()
}

/// Ruta con `/` y sin barra final, para comparar repositorios abiertos.
pub fn normalize_repository_path(repo_path: &str) -> String {
    let normalized = js_trim(repo_path).replace('\\', "/");
    // "/" y "C:/" son raíces: quitarles la barra las cambiaría de significado.
    let is_drive_root =
        matches!(normalized.as_bytes(), [drive, b':', b'/'] if drive.is_ascii_alphabetic());
    if utf16_len(&normalized) <= 1 || is_drive_root {
        return normalized;
    }

    normalized.trim_end_matches('/').to_string()
}

/// Rama actual según el estado; `HEAD` cuando el estado no nombra ninguna
/// (HEAD desanclado). Sin estado se conserva la anterior.
pub fn normalize_current_branch_from_status(
    status: Option<&StructuredStatus>,
    previous_branch: Option<&str>,
) -> String {
    let branch = match status {
        None => previous_branch,
        Some(status) => status.current.as_deref(),
    };

    branch
        .filter(|branch| !branch.is_empty())
        .unwrap_or("HEAD")
        .to_string()
}

/// Estado de conflicto que se muestra, conservando el progreso: el backend solo
/// sabe cuántos conflictos quedan, y cuántos había al empezar lo recuerda el
/// estado anterior mientras siga la misma operación.
pub fn normalize_conflict_state(
    previous_conflict_state: Option<&ConflictState>,
    status: Option<&StructuredStatus>,
) -> ConflictState {
    let Some(status) = status.filter(|status| status.operation_state.in_progress) else {
        return create_empty_conflict_state();
    };
    let base_conflict_state = &status.conflict_state;

    let pending_count = base_conflict_state.pending_count;
    let initial_count = match previous_conflict_state {
        Some(previous)
            if previous.in_conflict
                && previous.operation_type == base_conflict_state.operation_type
                && pending_count <= previous.initial_count =>
        {
            previous.initial_count
        }
        _ => pending_count,
    };

    ConflictState {
        initial_count,
        resolved_count: initial_count.saturating_sub(pending_count),
        ..base_conflict_state.clone()
    }
}

/// En qué punto está el repositorio respecto de una operación de varios pasos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoLifecycleState {
    #[default]
    Idle,
    /// Operación en curso sin conflictos a la vista.
    Busy,
    Conflicted,
    /// Conflictos resueltos: falta continuar la operación.
    ReadyToContinue,
}

impl RepoLifecycleState {
    pub fn as_str(self) -> &'static str {
        match self {
            RepoLifecycleState::Idle => "idle",
            RepoLifecycleState::Busy => "busy",
            RepoLifecycleState::Conflicted => "conflicted",
            RepoLifecycleState::ReadyToContinue => "ready_to_continue",
        }
    }
}

pub fn derive_repo_lifecycle_state(
    status: Option<&StructuredStatus>,
    conflict_state: Option<&ConflictState>,
) -> RepoLifecycleState {
    if conflict_state.is_some_and(|state| state.ready_to_continue) {
        return RepoLifecycleState::ReadyToContinue;
    }

    if conflict_state.is_some_and(|state| state.in_conflict) {
        return RepoLifecycleState::Conflicted;
    }

    if status.is_some_and(|status| status.operation_state.in_progress) {
        return RepoLifecycleState::Busy;
    }

    RepoLifecycleState::Idle
}

/// Resumen de seguimiento de la rama actual.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TrackingStatus {
    pub current: String,
    pub tracking: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

// ---------------------------------------------------------------------------
// Repository
// ---------------------------------------------------------------------------

/// Un repositorio abierto: el objeto `newRepo` de `addRepository`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repository<Stash = Value, Submodule = Value> {
    pub id: String,
    pub name: String,
    pub path: String,
    pub canonical_path: String,
    // La rama real llega con la primera lectura de status; hasta entonces
    // no inventamos un nombre (el repo puede usar master u otra).
    pub current_branch: Option<String>,
    pub detached_head_ref: Option<String>,
    pub head_short_hash: Option<String>,
    pub is_dirty: bool,
    pub operation_state: Option<OperationState>,
    pub conflict_state: ConflictState,
    pub mutation_queue: MutationQueue,
    pub repo_lifecycle_state: RepoLifecycleState,
    pub tracking_status: Option<TrackingStatus>,
    /// Timestamp ISO de la última mutación aplicada.
    pub last_mutation_at: Option<String>,
    pub last_mutation_operation: Option<String>,
    pub commits: Vec<CommitRow>,
    pub branches: Vec<BranchItem>,
    pub starred_branches: Vec<String>,
    pub remotes: Vec<RemoteItem>,
    pub tags: Vec<String>,
    pub stashes: Vec<Stash>,
    pub submodules: Vec<Submodule>,
    pub changes_count: usize,
    pub sidebar_loading: SidebarFlags,
    pub sidebar_loaded: SidebarFlags,
    pub status: Option<StructuredStatus>,
}

impl<Stash, Submodule> Repository<Stash, Submodule> {
    /// Repositorio recién abierto. La ruta se normaliza y es también su `id`;
    /// sin nombre se usa el último tramo de la ruta. `None` si la ruta queda vacía.
    pub fn new(repo_path: &str, name: Option<&str>) -> Option<Self> {
        let normalized_path = normalize_repository_path(repo_path);
        if normalized_path.is_empty() {
            return None;
        }

        // Extraer nombre del path si no se proporciona
        let repo_name = name
            .filter(|name| !name.is_empty())
            .or_else(|| {
                normalized_path
                    .rsplit('/')
                    .next()
                    .filter(|segment| !segment.is_empty())
            })
            .unwrap_or(&normalized_path)
            .to_string();

        Some(Self {
            id: normalized_path.clone(),
            name: repo_name,
            path: normalized_path.clone(),
            canonical_path: normalized_path,
            current_branch: None,
            detached_head_ref: None,
            head_short_hash: None,
            is_dirty: false,
            operation_state: None,
            conflict_state: create_empty_conflict_state(),
            mutation_queue: create_empty_mutation_queue(),
            repo_lifecycle_state: RepoLifecycleState::Idle,
            tracking_status: None,
            last_mutation_at: None,
            last_mutation_operation: None,
            commits: Vec::new(),
            branches: Vec::new(),
            starred_branches: Vec::new(),
            remotes: Vec::new(),
            tags: Vec::new(),
            stashes: Vec::new(),
            submodules: Vec::new(),
            changes_count: 0,
            sidebar_loading: create_empty_sidebar_loading_state(),
            sidebar_loaded: create_empty_sidebar_loaded_state(),
            status: None,
        })
    }

    fn should_preserve_detached_head(&self) -> bool {
        match &self.status {
            Some(status) => {
                normalize_current_branch_from_status(Some(status), self.current_branch.as_deref())
                    == "HEAD"
            }
            None => self.current_branch.as_deref() == Some("HEAD"),
        }
    }

    /// `setRepositoryMutationQueue`.
    pub fn set_mutation_queue(&mut self, queue: &MutationQueueSnapshot) {
        self.mutation_queue = normalize_mutation_queue(queue);
    }

    /// `applyRepositoryOperationEvent`: la cola tal como la trae un evento del gate.
    pub fn apply_operation_event(&mut self, operations: &[QueueOperation]) {
        self.mutation_queue = normalize_mutation_operations(operations);
    }

    /// `setRepositoryStatus`: aplica una lectura de estado (o su ausencia) y
    /// deriva de ella rama, conflicto, ciclo de vida y contadores.
    pub fn set_status(&mut self, status: Option<StructuredStatus>) {
        let conflict_state = normalize_conflict_state(Some(&self.conflict_state), status.as_ref());
        let normalized_status = status.map(|status| StructuredStatus {
            conflict_state: conflict_state.clone(),
            ..status
        });
        let current_branch = normalize_current_branch_from_status(
            normalized_status.as_ref(),
            self.current_branch.as_deref(),
        );
        let head_short_hash = normalized_status
            .as_ref()
            .and_then(|status| truthy(&status.head_short_hash))
            .map(str::to_string);

        self.operation_state = normalized_status
            .as_ref()
            .map(|status| status.operation_state.clone());
        self.repo_lifecycle_state =
            derive_repo_lifecycle_state(normalized_status.as_ref(), Some(&conflict_state));
        self.conflict_state = conflict_state;
        self.detached_head_ref = if current_branch == "HEAD" {
            head_short_hash
                .clone()
                .or_else(|| truthy(&self.detached_head_ref).map(str::to_string))
        } else {
            None
        };
        // Commit sobre el que está el working tree, en rama o desanclado. Se
        // conserva el anterior si el status no lo trae, para no dejar la barra
        // superior en blanco entre refrescos.
        self.head_short_hash =
            head_short_hash.or_else(|| truthy(&self.head_short_hash).map(str::to_string));
        let files = normalized_status
            .as_ref()
            .map_or(0, |status| status.files.len());
        self.is_dirty = files > 0;
        self.changes_count = files;
        self.sidebar_loading.changes_count = false;
        self.sidebar_loaded.changes_count = true;
        self.tracking_status = normalized_status.as_ref().map(|status| TrackingStatus {
            current: current_branch.clone(),
            tracking: truthy(&status.tracking).map(str::to_string),
            ahead: status.ahead,
            behind: status.behind,
        });
        self.current_branch = Some(current_branch);
        self.status = normalized_status;
    }

    /// La parte de `applyRepositoryOperationResult` que toca al repositorio:
    /// aplica el estado que trae el resultado (o invalida el que había) y anota
    /// la mutación. El registro en el log de operaciones lo hace
    /// [`RepositoriesStore::apply_repository_operation_result`].
    pub fn apply_operation_result(&mut self, result: &OperationResult) {
        self.apply_operation_result_at(result, now_iso_string());
    }

    /// [`Self::apply_operation_result`] con el timestamp (ISO) explícito.
    pub fn apply_operation_result_at(&mut self, result: &OperationResult, timestamp: String) {
        match &result.status {
            Some(status) => self.set_status(Some(status.clone())),
            None => self.invalidate_state(),
        }

        self.last_mutation_at = Some(timestamp);
        self.last_mutation_operation = truthy(&result.operation).map(str::to_string);
    }

    /// `setRepositorySidebarLoading`.
    pub fn set_sidebar_loading(&mut self, section: SidebarSection, is_loading: bool) {
        self.sidebar_loading.set(section, is_loading);
    }

    fn mark_loaded(&mut self, section: SidebarSection) {
        self.sidebar_loading.set(section, false);
        self.sidebar_loaded.set(section, true);
    }

    /// `setRepositoryBranchesData`. `current_branch` es la rama que dice la
    /// lectura de ramas; `None` conserva la que hubiera.
    pub fn set_branches_data(&mut self, branches: Vec<BranchItem>, current_branch: Option<&str>) {
        let should_keep_detached_head = self.should_preserve_detached_head();

        self.branches = branches;
        self.current_branch = Some(if should_keep_detached_head {
            "HEAD".to_string()
        } else {
            current_branch
                .or(self.current_branch.as_deref())
                .filter(|branch| !branch.is_empty())
                .unwrap_or("HEAD")
                .to_string()
        });
        self.detached_head_ref = if should_keep_detached_head {
            truthy(&self.detached_head_ref).map(str::to_string)
        } else {
            None
        };
        self.mark_loaded(SidebarSection::Branches);
    }

    /// `setRepositoryStarredBranches`.
    pub fn set_starred_branches(&mut self, starred_branches: Vec<String>) {
        self.starred_branches = starred_branches;
        self.mark_loaded(SidebarSection::StarredBranches);
    }

    /// `setRepositoryRemotesData`.
    pub fn set_remotes_data(&mut self, remotes: Vec<RemoteItem>) {
        self.remotes = remotes;
        self.mark_loaded(SidebarSection::Remotes);
    }

    /// `setRepositoryTagsData`.
    pub fn set_tags_data(&mut self, tags: Vec<String>) {
        self.tags = tags;
        self.mark_loaded(SidebarSection::Tags);
    }

    /// `setRepositoryStashesData`.
    pub fn set_stashes_data(&mut self, stashes: Vec<Stash>) {
        self.stashes = stashes;
        self.mark_loaded(SidebarSection::Stashes);
    }

    /// `setRepositorySubmodulesData`.
    pub fn set_submodules_data(&mut self, submodules: Vec<Submodule>) {
        self.submodules = submodules;
        self.mark_loaded(SidebarSection::Submodules);
    }

    fn clear_section(&mut self, section: SidebarSection) {
        match section {
            SidebarSection::Branches => self.branches.clear(),
            SidebarSection::StarredBranches => self.starred_branches.clear(),
            SidebarSection::Remotes => self.remotes.clear(),
            SidebarSection::Tags => self.tags.clear(),
            SidebarSection::Stashes => self.stashes.clear(),
            SidebarSection::Submodules => self.submodules.clear(),
            SidebarSection::ChangesCount => self.changes_count = 0,
        }
    }

    /// `invalidateRepositorySidebarData`: vacía todo lo de la barra lateral sin
    /// tocar el estado estructurado del repositorio.
    pub fn invalidate_sidebar_data(&mut self) {
        for section in SidebarSection::ALL {
            self.clear_section(section);
        }
        self.sidebar_loading = create_empty_sidebar_loading_state();
        self.sidebar_loaded = create_empty_sidebar_loaded_state();
    }

    /// `invalidateRepositorySidebarSections`.
    pub fn invalidate_sidebar_sections(&mut self, sections: &[SidebarSection]) {
        for section in sections {
            self.sidebar_loading.set(*section, false);
            self.sidebar_loaded.set(*section, false);
            self.clear_section(*section);
        }
    }

    /// `invalidateRepositoryState`: olvida el estado leído, a la espera de otro.
    pub fn invalidate_state(&mut self) {
        self.status = None;
        self.operation_state = None;
        self.conflict_state = create_empty_conflict_state();
        self.repo_lifecycle_state = RepoLifecycleState::Idle;
        self.detached_head_ref = None;
        self.is_dirty = false;
        self.changes_count = 0;
        self.sidebar_loading.changes_count = false;
        self.sidebar_loaded.changes_count = false;
    }

    /// `updateRepositoryBranch`.
    pub fn update_branch(&mut self, branch_name: Option<&str>) {
        let current_branch = if self.should_preserve_detached_head() {
            "HEAD"
        } else {
            branch_name
                .filter(|branch| !branch.is_empty())
                .unwrap_or("HEAD")
        };
        if current_branch != "HEAD" {
            self.detached_head_ref = None;
        }
        self.current_branch = Some(current_branch.to_string());
    }

    pub fn can_commit(&self) -> bool {
        self.repo_lifecycle_state == RepoLifecycleState::Idle
    }

    pub fn can_mutate_files(&self) -> bool {
        matches!(
            self.repo_lifecycle_state,
            RepoLifecycleState::Idle | RepoLifecycleState::Conflicted
        )
    }

    pub fn should_show_operation_banner(&self) -> bool {
        self.operation_state
            .as_ref()
            .is_some_and(|state| state.in_progress)
    }

    pub fn should_auto_select_conflict(&self) -> bool {
        self.repo_lifecycle_state == RepoLifecycleState::Conflicted
            && self.conflict_state.pending_count > 0
    }
}

pub fn can_commit_repository<S, M>(repository: Option<&Repository<S, M>>) -> bool {
    repository.is_some_and(Repository::can_commit)
}

pub fn can_mutate_files<S, M>(repository: Option<&Repository<S, M>>) -> bool {
    repository.is_some_and(Repository::can_mutate_files)
}

pub fn can_stage_repository<S, M>(repository: Option<&Repository<S, M>>) -> bool {
    can_mutate_files(repository)
}

pub fn can_unstage_repository<S, M>(repository: Option<&Repository<S, M>>) -> bool {
    can_mutate_files(repository)
}

pub fn can_discard_repository<S, M>(repository: Option<&Repository<S, M>>) -> bool {
    can_mutate_files(repository)
}

pub fn should_show_operation_banner<S, M>(repository: Option<&Repository<S, M>>) -> bool {
    repository.is_some_and(Repository::should_show_operation_banner)
}

pub fn should_auto_select_conflict<S, M>(repository: Option<&Repository<S, M>>) -> bool {
    repository.is_some_and(Repository::should_auto_select_conflict)
}

// ---------------------------------------------------------------------------
// Lista de repositorios abiertos
// ---------------------------------------------------------------------------

/// Un repositorio reciente. Al abrir uno se arma sin `last_opened`, que lo pone
/// el almacenamiento al guardarlo.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RecentRepository {
    pub id: String,
    pub name: String,
    pub path: String,
    pub current_branch: String,
    #[serde(rename = "isGitHub")]
    pub is_github: bool,
    pub remote_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_opened: Option<String>,
}

/// Cómo se refiere una acción a un repositorio: por `id` o por posición.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoKey<'a> {
    Id(&'a str),
    Index(usize),
}

impl<'a> From<&'a str> for RepoKey<'a> {
    fn from(id: &'a str) -> Self {
        RepoKey::Id(id)
    }
}

impl<'a> From<&'a String> for RepoKey<'a> {
    fn from(id: &'a String) -> Self {
        RepoKey::Id(id)
    }
}

impl From<usize> for RepoKey<'_> {
    fn from(index: usize) -> Self {
        RepoKey::Index(index)
    }
}

/// Lo que acompaña a la ruta al abrir un repositorio.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AddRepositoryOptions<'a> {
    pub name: Option<&'a str>,
    pub is_github: bool,
    pub remote_url: Option<&'a str>,
    /// Ruta canónica (symlinks resueltos); manda sobre la que se pidió.
    pub canonical_path: Option<&'a str>,
    pub skip_recent_repository_update: bool,
    /// No añadir el repositorio al workspace activo (al restaurar la sesión o
    /// al abrir los repositorios propios de un workspace al cambiar a él).
    pub skip_active_workspace_assignment: bool,
}

/// Resultado de abrir un repositorio, con lo que queda por persistir.
#[derive(Debug, Clone, PartialEq)]
pub struct AddRepositoryOutcome {
    pub index: usize,
    /// `false`: ya estaba abierto y solo se activó su pestaña.
    pub created: bool,
    /// Entrada para `addRecentRepository` (`None` si se pidió no tocar los recientes).
    pub recent_repository: Option<RecentRepository>,
    /// Sesión a guardar.
    pub session: WorkspaceSession,
}

/// Estado del store de repositorios: pestañas abiertas, activa, recientes y sesión.
#[derive(Debug, Clone, PartialEq)]
pub struct RepositoriesStore<Stash = Value, Submodule = Value> {
    // Lista de repositorios abiertos (pestañas)
    pub repositories: Vec<Repository<Stash, Submodule>>,
    // Índice del repositorio activo (`None` es el -1 del original)
    pub active_repository_index: Option<usize>,
    // Repositorios recientes
    pub recent_repositories: Vec<RecentRepository>,
    pub workspace_session: WorkspaceSession,
    // Flag para saber si ya se cargó
    pub loaded: bool,
}

impl<Stash, Submodule> Default for RepositoriesStore<Stash, Submodule> {
    fn default() -> Self {
        Self {
            repositories: Vec::new(),
            active_repository_index: None,
            recent_repositories: Vec::new(),
            workspace_session: create_empty_workspace_session(),
            loaded: false,
        }
    }
}

impl<Stash, Submodule> RepositoriesStore<Stash, Submodule> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Posición de un repositorio abierto, o `None` si no hay ninguno así.
    pub fn find_repository_index<'a>(&self, key: impl Into<RepoKey<'a>>) -> Option<usize> {
        match key.into() {
            RepoKey::Index(index) => (index < self.repositories.len()).then_some(index),
            RepoKey::Id(id) => self
                .repositories
                .iter()
                .position(|repository| repository.id == id),
        }
    }

    pub fn repository<'a>(
        &self,
        key: impl Into<RepoKey<'a>>,
    ) -> Option<&Repository<Stash, Submodule>> {
        self.find_repository_index(key)
            .map(|index| &self.repositories[index])
    }

    /// Acceso para aplicar cualquiera de las acciones de [`Repository`]
    /// (`store.setRepositoryStatus(id, s)` es `store.repository_mut(id)?.set_status(s)`).
    pub fn repository_mut<'a>(
        &mut self,
        key: impl Into<RepoKey<'a>>,
    ) -> Option<&mut Repository<Stash, Submodule>> {
        self.find_repository_index(key)
            .map(|index| &mut self.repositories[index])
    }

    pub fn active_repository(&self) -> Option<&Repository<Stash, Submodule>> {
        self.active_repository_index
            .and_then(|index| self.repositories.get(index))
    }

    pub fn active_repository_mut(&mut self) -> Option<&mut Repository<Stash, Submodule>> {
        self.active_repository_index
            .and_then(|index| self.repositories.get_mut(index))
    }

    pub fn has_repositories(&self) -> bool {
        !self.repositories.is_empty()
    }

    pub fn active_repository_lifecycle_state(&self) -> RepoLifecycleState {
        self.active_repository()
            .map_or(RepoLifecycleState::Idle, |repository| {
                repository.repo_lifecycle_state
            })
    }

    pub fn active_repository_can_commit(&self) -> bool {
        can_commit_repository(self.active_repository())
    }

    pub fn active_repository_can_mutate_files(&self) -> bool {
        can_mutate_files(self.active_repository())
    }

    pub fn active_repository_can_stage(&self) -> bool {
        can_stage_repository(self.active_repository())
    }

    pub fn active_repository_can_unstage(&self) -> bool {
        can_unstage_repository(self.active_repository())
    }

    pub fn active_repository_can_discard(&self) -> bool {
        can_discard_repository(self.active_repository())
    }

    pub fn active_repository_branches(&self) -> &[BranchItem] {
        self.active_repository()
            .map_or(&[], |repository| &repository.branches)
    }

    pub fn active_repository_starred_branches(&self) -> &[String] {
        self.active_repository()
            .map_or(&[], |repository| &repository.starred_branches)
    }

    pub fn active_repository_remotes(&self) -> &[RemoteItem] {
        self.active_repository()
            .map_or(&[], |repository| &repository.remotes)
    }

    pub fn active_repository_tags(&self) -> &[String] {
        self.active_repository()
            .map_or(&[], |repository| &repository.tags)
    }

    pub fn active_repository_stashes(&self) -> &[Stash] {
        self.active_repository()
            .map_or(&[], |repository| &repository.stashes)
    }

    pub fn active_repository_submodules(&self) -> &[Submodule] {
        self.active_repository()
            .map_or(&[], |repository| &repository.submodules)
    }

    pub fn active_repository_changes_count(&self) -> usize {
        self.active_repository()
            .map_or(0, |repository| repository.changes_count)
    }

    pub fn active_repository_should_show_operation_banner(&self) -> bool {
        should_show_operation_banner(self.active_repository())
    }

    pub fn active_repository_should_auto_select_conflict(&self) -> bool {
        should_auto_select_conflict(self.active_repository())
    }

    /// Pestañas de un repositorio; las crea vacías si no había.
    pub fn get_repository_workspace(&mut self, path: &str) -> &mut RepositoryWorkspace {
        self.workspace_session
            .repository_workspaces
            .entry(path.to_string())
            .or_default()
    }

    /// Guarda las pestañas de un repositorio y devuelve la sesión a persistir.
    pub fn save_repository_workspace(
        &mut self,
        path: &str,
        state: RepositoryWorkspace,
    ) -> WorkspaceSession {
        self.workspace_session
            .repository_workspaces
            .insert(path.to_string(), state);
        self.persist_workspace_session()
    }

    /// La parte de `loadFromStorage` que no lee nada: recibe lo leído (`None`
    /// si esa lectura falló) y lo adopta. Devuelve `false` si ya estaba cargado.
    pub fn load_from_storage(
        &mut self,
        recent_repositories: Option<Vec<RecentRepository>>,
        workspace_session: Option<&Value>,
    ) -> bool {
        if self.loaded {
            return false;
        }

        if let Some(recent_repositories) = recent_repositories {
            self.recent_repositories = recent_repositories;
        }
        self.workspace_session = match workspace_session {
            Some(session) => normalize_workspace_session(session),
            None => create_empty_workspace_session(),
        };
        self.loaded = true;
        true
    }

    /// Recalcula la sesión a partir de lo abierto y la devuelve para guardarla.
    pub fn persist_workspace_session(&mut self) -> WorkspaceSession {
        let active_repository_path = self
            .active_repository()
            .map(|repository| repository.path.clone())
            .filter(|path| !js_trim(path).is_empty());

        let mut workspaces = std::mem::take(&mut self.workspace_session.workspaces);
        let active_workspace_id = self
            .workspace_session
            .active_workspace_id
            .take()
            .filter(|id| workspaces.iter().any(|ws| ws.id == *id));

        // Si hay un workspace activo y el repositorio activo le pertenece, se
        // recuerda como el último activo de ese workspace.
        if let (Some(active_ws_id), Some(active_path)) = (
            active_workspace_id.as_deref(),
            active_repository_path.as_deref(),
        ) && let Some(workspace) = workspaces.iter_mut().find(|ws| ws.id == active_ws_id)
            && workspace
                .repositories
                .iter()
                .any(|path| path == active_path)
        {
            workspace.active_repository_path = Some(active_path.to_string());
        }

        let session = WorkspaceSession {
            repository_workspaces: std::mem::take(
                &mut self.workspace_session.repository_workspaces,
            ),
            open_repositories: self
                .repositories
                .iter()
                .map(|repository| repository.path.clone())
                .filter(|path| !js_trim(path).is_empty())
                .collect(),
            active_repository_path,
            workspaces,
            active_workspace_id,
        };

        self.workspace_session = session.clone();
        session
    }

    // ------------------------------------------------------------------
    // Workspaces (grupos de repositorios)
    // ------------------------------------------------------------------

    pub fn workspaces(&self) -> &[RepoWorkspace] {
        &self.workspace_session.workspaces
    }

    pub fn active_workspace_id(&self) -> Option<&str> {
        self.workspace_session.active_workspace_id.as_deref()
    }

    pub fn active_workspace(&self) -> Option<&RepoWorkspace> {
        let id = self.active_workspace_id()?;
        self.workspace(id)
    }

    pub fn workspace(&self, id: &str) -> Option<&RepoWorkspace> {
        self.workspace_session
            .workspaces
            .iter()
            .find(|workspace| workspace.id == id)
    }

    /// Repositorios abiertos visibles en la barra de pestañas: los del
    /// workspace activo en su orden, o todos los abiertos cuando está activo
    /// "All" (`None`).
    pub fn visible_repositories(&self) -> Vec<&Repository<Stash, Submodule>> {
        let Some(workspace) = self.active_workspace() else {
            return self.repositories.iter().collect();
        };
        workspace
            .repositories
            .iter()
            .filter_map(|path| {
                self.repositories
                    .iter()
                    .find(|repository| normalize_repository_path(&repository.path) == *path)
            })
            .collect()
    }

    pub fn is_repository_in_workspace(&self, repo_path: &str, workspace_id: &str) -> bool {
        let normalized = normalize_repository_path(repo_path);
        self.workspace(workspace_id)
            .is_some_and(|workspace| workspace.repositories.contains(&normalized))
    }

    pub fn is_repository_in_other_workspace(
        &self,
        repo_path: &str,
        exclude_workspace_id: &str,
    ) -> bool {
        let normalized = normalize_repository_path(repo_path);
        self.workspace_session.workspaces.iter().any(|workspace| {
            workspace.id != exclude_workspace_id && workspace.repositories.contains(&normalized)
        })
    }

    fn next_workspace_id(&self, name: &str) -> String {
        let mut slug = String::new();
        let mut prev_dash = false;
        for ch in name.chars() {
            if ch.is_ascii_alphanumeric() {
                slug.push(ch.to_ascii_lowercase());
                prev_dash = false;
            } else if !slug.is_empty() && !prev_dash {
                slug.push('-');
                prev_dash = true;
            }
        }
        let slug = slug.trim_end_matches('-');
        let base = if slug.is_empty() {
            "ws".to_string()
        } else {
            format!("ws-{slug}")
        };
        if !self
            .workspace_session
            .workspaces
            .iter()
            .any(|ws| ws.id == base)
        {
            return base;
        }
        let mut suffix = 2usize;
        loop {
            let candidate = format!("{base}-{suffix}");
            if !self
                .workspace_session
                .workspaces
                .iter()
                .any(|ws| ws.id == candidate)
            {
                return candidate;
            }
            suffix += 1;
        }
    }

    /// Crea un grupo nuevo. Devuelve `None` si el nombre queda vacío.
    pub fn create_workspace(
        &mut self,
        name: &str,
        initial_repos: &[String],
    ) -> Option<(RepoWorkspace, WorkspaceSession)> {
        let name = js_trim(name);
        if name.is_empty() {
            return None;
        }
        let id = self.next_workspace_id(name);
        let mut repositories = Vec::new();
        for path in initial_repos {
            let normalized = normalize_repository_path(path);
            if !normalized.is_empty() && !repositories.contains(&normalized) {
                repositories.push(normalized);
            }
        }
        let active_repository_path = self
            .active_repository()
            .map(|repo| normalize_repository_path(&repo.path))
            .filter(|path| repositories.contains(path))
            .or_else(|| repositories.first().cloned());

        let workspace = RepoWorkspace {
            id,
            name: name.to_string(),
            repositories,
            active_repository_path,
        };
        self.workspace_session.workspaces.push(workspace.clone());
        let session = self.persist_workspace_session();
        Some((workspace, session))
    }

    /// Renombra un grupo existente. Devuelve `None` si el nombre queda vacío,
    /// el grupo no existe o el nombre no cambia.
    pub fn rename_workspace(&mut self, id: &str, name: &str) -> Option<WorkspaceSession> {
        let name = js_trim(name);
        if name.is_empty() {
            return None;
        }
        let workspace = self
            .workspace_session
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == id)?;
        if workspace.name == name {
            return None;
        }
        workspace.name = name.to_string();
        Some(self.persist_workspace_session())
    }

    /// Borra un grupo sin cerrar sus repositorios. Si era el activo, pasa a
    /// "All" (`None`).
    pub fn delete_workspace(&mut self, id: &str) -> Option<WorkspaceSession> {
        let index = self
            .workspace_session
            .workspaces
            .iter()
            .position(|workspace| workspace.id == id)?;
        self.workspace_session.workspaces.remove(index);
        if self.workspace_session.active_workspace_id.as_deref() == Some(id) {
            self.workspace_session.active_workspace_id = None;
        }
        Some(self.persist_workspace_session())
    }

    /// Cambia el grupo activo (`None` = "All"). Devuelve `None` si el id no
    /// corresponde a ningún grupo.
    pub fn set_active_workspace(&mut self, id: Option<&str>) -> Option<WorkspaceSession> {
        if let Some(id) = id
            && !self
                .workspace_session
                .workspaces
                .iter()
                .any(|workspace| workspace.id == id)
        {
            return None;
        }
        self.workspace_session.active_workspace_id = id.map(str::to_string);
        Some(self.persist_workspace_session())
    }

    /// Añade un repositorio a un grupo. Devuelve `None` si la ruta está vacía,
    /// el grupo no existe o ya estaba incluido.
    pub fn add_repository_to_workspace(
        &mut self,
        repo_path: &str,
        workspace_id: &str,
    ) -> Option<WorkspaceSession> {
        let normalized = normalize_repository_path(repo_path);
        if normalized.is_empty() {
            return None;
        }
        let workspace = self
            .workspace_session
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == workspace_id)?;
        if workspace.repositories.contains(&normalized) {
            return None;
        }
        workspace.repositories.push(normalized.clone());
        if workspace.active_repository_path.is_none() {
            workspace.active_repository_path = Some(normalized);
        }
        Some(self.persist_workspace_session())
    }

    /// Quita un repositorio de un grupo sin cerrarlo de `repositories`.
    pub fn remove_repository_from_workspace(
        &mut self,
        repo_path: &str,
        workspace_id: &str,
    ) -> Option<WorkspaceSession> {
        let normalized = normalize_repository_path(repo_path);
        let workspace = self
            .workspace_session
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == workspace_id)?;
        let index = workspace
            .repositories
            .iter()
            .position(|path| *path == normalized)?;
        workspace.repositories.remove(index);
        if workspace.active_repository_path.as_deref() == Some(normalized.as_str()) {
            workspace.active_repository_path = workspace
                .repositories
                .get(index)
                .or_else(|| workspace.repositories.last())
                .cloned();
        }
        Some(self.persist_workspace_session())
    }

    /// Actualiza una ruta guardada en un grupo (p. ej. tras resolver un
    /// symlink al abrirlo) manteniendo su posición.
    pub fn replace_workspace_repository_path(
        &mut self,
        workspace_id: &str,
        old_path: &str,
        new_path: &str,
    ) -> bool {
        let old_normalized = normalize_repository_path(old_path);
        let new_normalized = normalize_repository_path(new_path);
        if old_normalized == new_normalized || new_normalized.is_empty() {
            return false;
        }
        let Some(workspace) = self
            .workspace_session
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == workspace_id)
        else {
            return false;
        };
        let Some(index) = workspace
            .repositories
            .iter()
            .position(|path| *path == old_normalized)
        else {
            return false;
        };
        if workspace.repositories.contains(&new_normalized) {
            workspace.repositories.remove(index);
        } else {
            workspace.repositories[index] = new_normalized.clone();
        }
        if workspace.active_repository_path.as_deref() == Some(old_normalized.as_str()) {
            workspace.active_repository_path = Some(new_normalized);
        }
        true
    }

    /// Mueve una pestaña en la lista visible: en "All" reordena
    /// `repositories`; con un workspace activo, reordena dentro de ese
    /// workspace.
    pub fn move_visible_repository(&mut self, from: usize, to: usize) -> Option<WorkspaceSession> {
        let Some(active_ws_id) = self.workspace_session.active_workspace_id.clone() else {
            return self.move_repository(from, to);
        };
        let visible_paths: Vec<String> = self
            .visible_repositories()
            .into_iter()
            .map(|repo| normalize_repository_path(&repo.path))
            .collect();
        if from >= visible_paths.len() || to >= visible_paths.len() || from == to {
            return None;
        }
        let from_path = &visible_paths[from];
        let to_path = &visible_paths[to];
        let workspace = self
            .workspace_session
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == active_ws_id)?;
        let ws_from = workspace
            .repositories
            .iter()
            .position(|path| path == from_path)?;
        let ws_to = workspace
            .repositories
            .iter()
            .position(|path| path == to_path)?;
        let moved = workspace.repositories.remove(ws_from);
        workspace.repositories.insert(ws_to, moved);
        Some(self.persist_workspace_session())
    }

    fn recent_entry(
        repository: &Repository<Stash, Submodule>,
        options: &AddRepositoryOptions<'_>,
    ) -> RecentRepository {
        RecentRepository {
            id: repository.id.clone(),
            name: repository.name.clone(),
            path: repository.path.clone(),
            current_branch: truthy(&repository.current_branch)
                .unwrap_or("HEAD")
                .to_string(),
            is_github: options.is_github,
            remote_url: options.remote_url.unwrap_or_default().to_string(),
            last_opened: None,
        }
    }

    /// Abre un repositorio (o activa su pestaña si ya estaba abierto). `None`
    /// si la ruta queda vacía.
    pub fn add_repository(
        &mut self,
        repo_path: &str,
        options: AddRepositoryOptions<'_>,
    ) -> Option<AddRepositoryOutcome> {
        let requested_path = options
            .canonical_path
            .filter(|path| !path.is_empty())
            .unwrap_or(repo_path);
        let normalized_path = normalize_repository_path(requested_path);
        if normalized_path.is_empty() {
            return None;
        }

        // Verificar si ya está abierto
        let existing_index = self
            .repositories
            .iter()
            .position(|repository| normalize_repository_path(&repository.path) == normalized_path);
        let (index, created) = match existing_index {
            Some(index) => (index, false),
            None => {
                self.repositories
                    .push(Repository::new(&normalized_path, options.name)?);
                (self.repositories.len() - 1, true)
            }
        };
        self.active_repository_index = Some(index);

        // Si hay un workspace activo y no se indicó saltarlo (restauración de
        // sesión o carga interna), añadir el repositorio automáticamente.
        if !options.skip_active_workspace_assignment
            && let Some(active_ws_id) = self.workspace_session.active_workspace_id.clone()
            && let Some(workspace) = self
                .workspace_session
                .workspaces
                .iter_mut()
                .find(|workspace| workspace.id == active_ws_id)
        {
            if !workspace.repositories.contains(&normalized_path) {
                workspace.repositories.push(normalized_path.clone());
            }
            workspace.active_repository_path = Some(normalized_path);
        }

        // Agregar a repositorios recientes
        let recent_repository = (!options.skip_recent_repository_update)
            .then(|| Self::recent_entry(&self.repositories[index], &options));

        Some(AddRepositoryOutcome {
            index,
            created,
            recent_repository,
            session: self.persist_workspace_session(),
        })
    }

    /// Cierra un repositorio. Devuelve la sesión a guardar, o `None` si no existía.
    ///
    /// El repositorio activo sigue siéndolo aunque cambie de posición. Si se
    /// cierra el activo, pasa a serlo el de su derecha o, si era el último, el
    /// de su izquierda (la convención de los editores). El original solo
    /// corregía el índice cuando quedaba fuera de rango, así que cerrar una
    /// pestaña a la izquierda de la activa activaba otro repositorio.
    pub fn remove_repository<'a>(
        &mut self,
        key: impl Into<RepoKey<'a>>,
    ) -> Option<WorkspaceSession> {
        let index = self.find_repository_index(key)?;
        self.repositories.remove(index);

        self.active_repository_index = match self.active_repository_index {
            Some(active) if active > index => Some(active - 1),
            Some(active) if active == index => {
                (!self.repositories.is_empty()).then(|| index.min(self.repositories.len() - 1))
            }
            other => other,
        };

        Some(self.persist_workspace_session())
    }

    /// Mueve la pestaña `from` a la posición `to`; el repositorio activo sigue
    /// siéndolo. `None` si alguno de los dos índices no existe o no cambia nada.
    pub fn move_repository(&mut self, from: usize, to: usize) -> Option<WorkspaceSession> {
        let len = self.repositories.len();
        if from >= len || to >= len || from == to {
            return None;
        }
        let active_id = self.active_repository().map(|repo| repo.id.clone());
        let repository = self.repositories.remove(from);
        self.repositories.insert(to, repository);
        if let Some(active_id) = active_id {
            self.active_repository_index = self
                .repositories
                .iter()
                .position(|repository| repository.id == active_id);
        }
        Some(self.persist_workspace_session())
    }

    /// Cambia la pestaña activa (`None` deja ninguna). Devuelve la sesión a
    /// guardar, o `None` si el índice no existe.
    pub fn set_active_repository(&mut self, index: Option<usize>) -> Option<WorkspaceSession> {
        if index.is_some_and(|index| index >= self.repositories.len()) {
            return None;
        }

        self.active_repository_index = index;
        Some(self.persist_workspace_session())
    }

    /// `applyRepositoryOperationResult`: aplica el resultado al repositorio y
    /// lo registra en el log de operaciones.
    ///
    /// Todos los resultados de mutaciones pasan por aquí: es el punto único
    /// donde alimentar el log de operaciones con el mensaje completo.
    pub fn apply_repository_operation_result<'a>(
        &mut self,
        key: impl Into<RepoKey<'a>>,
        result: &OperationResult,
        operations_log: &mut OperationsLog,
    ) {
        let repository = self.repository_mut(key);
        let context = match repository {
            Some(repository) => {
                repository.apply_operation_result(result);
                ResultContext {
                    repo_name: Some(repository.name.as_str()),
                    repo_path: Some(repository.path.as_str()),
                }
            }
            None => ResultContext::default(),
        };
        operations_log.record_result(result, context);
    }

    /// La parte local de `removeRecentRepository`.
    pub fn remove_recent_repository(&mut self, repo_path: &str) {
        self.recent_repositories
            .retain(|repository| repository.path != repo_path);
    }

    /// La parte local de `clearRecentRepositories`.
    pub fn clear_recent_repositories(&mut self) {
        self.recent_repositories.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace_tabs::{FileTabDescriptor, open_file_tab};
    use mergeon_gitcmd::{FileStatus, OperationType};
    use serde_json::json;

    type Repo = Repository;
    type Store = RepositoriesStore;

    fn file(path: &str, index: &str, working_dir: &str) -> FileStatus {
        FileStatus {
            path: path.to_string(),
            index: index.to_string(),
            working_dir: working_dir.to_string(),
            from: None,
        }
    }

    fn status(current: &str) -> StructuredStatus {
        StructuredStatus {
            current: Some(current.to_string()),
            is_clean: true,
            ..Default::default()
        }
    }

    fn merge_in_progress() -> OperationState {
        OperationState {
            in_progress: true,
            kind: Some(OperationType::Merge),
            label: "Merge in progress".to_string(),
            can_continue: true,
            can_abort: true,
            can_skip: false,
        }
    }

    fn merge_conflict(initial: usize, pending: &[&str], ready: bool) -> ConflictState {
        ConflictState {
            in_conflict: true,
            operation_type: Some(OperationType::Merge),
            initial_count: initial,
            pending_count: pending.len(),
            resolved_count: initial.saturating_sub(pending.len()),
            pending_files: pending.iter().map(|path| (*path).to_string()).collect(),
            ready_to_continue: ready,
        }
    }

    /// El repositorio de los tests del store: `{ id: 'repo-1', name: 'demo', path: '/tmp/demo' }`.
    fn demo_repo() -> Repo {
        Repo {
            id: "repo-1".to_string(),
            ..Repo::new("/tmp/demo", Some("demo")).unwrap()
        }
    }

    fn store_with(repository: Repo) -> Store {
        Store {
            repositories: vec![repository],
            ..Default::default()
        }
    }

    fn branch(name: &str, active: bool) -> BranchItem {
        BranchItem {
            name: name.to_string(),
            active,
            ..Default::default()
        }
    }

    fn origin() -> RemoteItem {
        RemoteItem {
            name: "origin".to_string(),
            branches: vec!["main".to_string()],
            ..Default::default()
        }
    }

    #[test]
    fn updates_repository_data_when_called_with_a_repository_id() {
        let mut store = store_with(demo_repo());

        store.repository_mut("repo-1").unwrap().status = Some(StructuredStatus {
            files: vec![file("README.md", " ", "M")],
            ..status("main")
        });

        assert_eq!(
            store.repositories[0].status.as_ref().unwrap().files[0].path,
            "README.md"
        );
        assert_eq!(store.find_repository_index("repo-1"), Some(0));
        assert_eq!(store.find_repository_index(0), Some(0));
        assert_eq!(store.find_repository_index("missing"), None);
        assert_eq!(store.find_repository_index(3), None);
        assert!(store.repository("missing").is_none());
    }

    #[test]
    fn refreshes_recent_repositories_even_when_storage_was_already_loaded() {
        let mut store = Store::new();
        store.loaded = true;

        let outcome = store
            .add_repository("C:\\repos\\demo", AddRepositoryOptions::default())
            .unwrap();

        let recent = outcome.recent_repository.expect("recent repository entry");
        assert_eq!(recent.name, "demo");
        assert_eq!(recent.path, "C:/repos/demo");
        assert_eq!(recent.current_branch, "HEAD");
        assert_eq!(
            serde_json::to_value(&recent).unwrap(),
            json!({
                "id": "C:/repos/demo",
                "name": "demo",
                "path": "C:/repos/demo",
                "currentBranch": "HEAD",
                "isGitHub": false,
                "remoteUrl": ""
            })
        );
    }

    #[test]
    fn deduplicates_equivalent_repository_paths_and_keeps_a_stable_repository_id() {
        let mut store = Store::new();

        let first = store
            .add_repository("/tmp/demo/", AddRepositoryOptions::default())
            .unwrap();
        let first_id = store.repositories[0].id.clone();
        let second = store
            .add_repository(
                "/tmp/demo",
                AddRepositoryOptions {
                    canonical_path: Some("/tmp/demo"),
                    ..Default::default()
                },
            )
            .unwrap();

        assert!(first.created);
        assert!(!second.created);
        assert_eq!(store.repositories.len(), 1);
        assert_eq!(store.repositories[0].id, first_id);
        assert_eq!(store.repositories[0].path, "/tmp/demo");
    }

    #[test]
    fn preserves_conflict_progress_while_the_same_operation_remains_in_progress() {
        let mut repository = Repo {
            conflict_state: merge_conflict(3, &["a.txt", "b.txt"], false),
            ..demo_repo()
        };

        repository.set_status(Some(StructuredStatus {
            tracking: Some("origin/main".to_string()),
            files: vec![file("a.txt", "U", "U")],
            conflicted: vec!["a.txt".to_string()],
            operation_state: merge_in_progress(),
            conflict_state: merge_conflict(1, &["a.txt"], false),
            ..status("main")
        }));

        assert_eq!(repository.conflict_state.initial_count, 3);
        assert_eq!(repository.conflict_state.resolved_count, 2);
        // El estado guardado lleva el mismo conflicto normalizado.
        assert_eq!(
            repository.status.as_ref().unwrap().conflict_state,
            repository.conflict_state
        );
        assert_eq!(
            repository.repo_lifecycle_state,
            RepoLifecycleState::Conflicted
        );
    }

    #[test]
    fn applies_structured_mutation_results_and_records_metadata() {
        let mut store = store_with(demo_repo());
        let mut log = OperationsLog::new();

        store.apply_repository_operation_result(
            "repo-1",
            &OperationResult {
                success: true,
                operation: Some("stage".to_string()),
                status: Some(StructuredStatus {
                    tracking: Some("origin/main".to_string()),
                    files: vec![file("file.txt", "M", " ")],
                    ..status("main")
                }),
                ..Default::default()
            },
            &mut log,
        );

        let repository = &store.repositories[0];
        assert_eq!(
            repository.status.as_ref().unwrap().files,
            [file("file.txt", "M", " ")]
        );
        assert_eq!(repository.repo_lifecycle_state, RepoLifecycleState::Idle);
        assert_eq!(repository.last_mutation_operation.as_deref(), Some("stage"));
        assert!(
            repository
                .last_mutation_at
                .as_deref()
                .is_some_and(|at| at.ends_with('Z'))
        );
        assert!(repository.is_dirty);
        assert_eq!(repository.changes_count, 1);

        // El mismo resultado alimenta el log de operaciones, con el repositorio.
        assert_eq!(log.entries.len(), 1);
        assert_eq!(log.entries[0].operation.as_deref(), Some("stage"));
        assert_eq!(log.entries[0].repo_name.as_deref(), Some("demo"));
        assert_eq!(log.entries[0].repo_path.as_deref(), Some("/tmp/demo"));
    }

    #[test]
    fn a_result_without_status_invalidates_what_was_read() {
        let mut repository = demo_repo();
        repository.set_status(Some(StructuredStatus {
            head_short_hash: Some("634a2d2".to_string()),
            files: vec![file("file.txt", " ", "M")],
            ..status("main")
        }));

        repository.apply_operation_result_at(
            &OperationResult {
                operation: Some("push".to_string()),
                ..Default::default()
            },
            "2026-03-06T00:00:00.000Z".to_string(),
        );

        assert_eq!(repository.status, None);
        assert!(!repository.is_dirty);
        assert_eq!(repository.changes_count, 0);
        assert!(!repository.sidebar_loaded.changes_count);
        assert_eq!(
            repository.last_mutation_at.as_deref(),
            Some("2026-03-06T00:00:00.000Z")
        );
        assert_eq!(repository.last_mutation_operation.as_deref(), Some("push"));
        // La rama y el commit de HEAD se conservan hasta la siguiente lectura.
        assert_eq!(repository.current_branch.as_deref(), Some("main"));
        assert_eq!(repository.head_short_hash.as_deref(), Some("634a2d2"));
    }

    fn queue_operation(
        operation_id: &str,
        operation: &str,
        phase: OperationPhase,
    ) -> QueueOperation {
        QueueOperation {
            operation_id: Some(operation_id.to_string()),
            operation: operation.to_string(),
            phase,
            ..Default::default()
        }
    }

    #[test]
    fn keeps_a_normalized_mutation_queue_snapshot_for_renderer_events() {
        let mut repository = demo_repo();

        repository.apply_operation_event(&[
            queue_operation("fetch-1", "fetch", OperationPhase::Running),
            QueueOperation {
                position: 1,
                ..queue_operation("pull-1", "pull", OperationPhase::Queued)
            },
        ]);

        let queue = &repository.mutation_queue;
        let active = queue.active.as_ref().expect("active operation");
        assert_eq!(active.operation_id.as_deref(), Some("fetch-1"));
        assert_eq!(active.phase, OperationPhase::Running);
        assert_eq!(queue.queued.len(), 1);
        assert_eq!(queue.queued[0].operation_id.as_deref(), Some("pull-1"));
        assert_eq!(queue.queued[0].position, 1);

        repository.apply_operation_event(&[]);
        assert_eq!(repository.mutation_queue, MutationQueue::default());
    }

    #[test]
    fn renumbers_the_queue_and_reads_the_older_snapshot_shape() {
        let operations = [
            queue_operation("old", "fetch", OperationPhase::Completed),
            queue_operation("stop", "pull", OperationPhase::Cancelling),
            QueueOperation {
                position: 7,
                ..queue_operation("a", "push", OperationPhase::Queued)
            },
            queue_operation("b", "merge", OperationPhase::Queued),
        ];

        let queue = normalize_mutation_operations(&operations);
        assert_eq!(
            queue
                .active
                .as_ref()
                .and_then(|active| active.operation_id.as_deref()),
            Some("stop")
        );
        assert_eq!(
            queue
                .queued
                .iter()
                .map(|operation| operation.position)
                .collect::<Vec<_>>(),
            [1, 2]
        );

        let legacy: MutationQueueSnapshot = serde_json::from_value(json!({
            "active": { "operationId": "fetch-1", "operation": "fetch", "phase": "running" },
            "queued": [{ "operationId": "pull-1", "operation": "pull", "phase": "queued" }]
        }))
        .unwrap();
        let mut repository = demo_repo();
        repository.set_mutation_queue(&legacy);
        assert_eq!(repository.mutation_queue.queued[0].position, 1);
        assert!(repository.mutation_queue.active.is_some());
        assert_eq!(
            normalize_mutation_queue(&MutationQueueSnapshot::default()),
            MutationQueue::default()
        );
    }

    #[test]
    fn represents_detached_head_explicitly_instead_of_keeping_the_previous_branch() {
        let mut repository = Repo {
            current_branch: Some("main".to_string()),
            ..demo_repo()
        };

        repository.set_status(Some(StructuredStatus {
            head_short_hash: Some("2eea100".to_string()),
            ..status("")
        }));

        assert_eq!(repository.current_branch.as_deref(), Some("HEAD"));
        assert_eq!(repository.detached_head_ref.as_deref(), Some("2eea100"));
        assert_eq!(repository.head_short_hash.as_deref(), Some("2eea100"));
        assert_eq!(repository.tracking_status.as_ref().unwrap().current, "HEAD");
    }

    #[test]
    fn keeps_the_head_commit_on_a_branch_and_preserves_it_when_a_refresh_omits_it() {
        let mut repository = Repo {
            current_branch: Some("main".to_string()),
            ..demo_repo()
        };

        repository.set_status(Some(StructuredStatus {
            head_short_hash: Some("634a2d2".to_string()),
            ..status("main")
        }));

        assert_eq!(repository.head_short_hash.as_deref(), Some("634a2d2"));
        assert_eq!(repository.detached_head_ref, None);

        repository.set_status(Some(status("main")));

        assert_eq!(repository.head_short_hash.as_deref(), Some("634a2d2"));
    }

    #[test]
    fn derives_ready_to_continue_from_structured_repository_status() {
        let mut repository = demo_repo();

        repository.set_status(Some(StructuredStatus {
            tracking: Some("origin/main".to_string()),
            operation_state: merge_in_progress(),
            conflict_state: merge_conflict(2, &[], true),
            ..status("main")
        }));

        assert_eq!(
            repository.repo_lifecycle_state,
            RepoLifecycleState::ReadyToContinue
        );
        assert_eq!(
            repository.repo_lifecycle_state.as_str(),
            "ready_to_continue"
        );
        assert!(!repository.can_commit());
        assert!(!repository.can_mutate_files());
    }

    #[test]
    fn exposes_lifecycle_derived_getters_for_the_active_repository() {
        let mut store = store_with(Repo {
            repo_lifecycle_state: RepoLifecycleState::Conflicted,
            operation_state: Some(merge_in_progress()),
            conflict_state: merge_conflict(2, &["a.txt"], false),
            ..demo_repo()
        });
        store.active_repository_index = Some(0);

        assert_eq!(
            store.active_repository_lifecycle_state(),
            RepoLifecycleState::Conflicted
        );
        assert!(!store.active_repository_can_commit());
        assert!(store.active_repository_can_mutate_files());
        assert!(store.active_repository_can_stage());
        assert!(store.active_repository_can_unstage());
        assert!(store.active_repository_can_discard());
        assert!(store.active_repository_should_show_operation_banner());
        assert!(store.active_repository_should_auto_select_conflict());
    }

    #[test]
    fn answers_for_no_active_repository_like_an_idle_one_that_cannot_be_touched() {
        let store = Store::new();

        assert_eq!(
            store.active_repository_lifecycle_state(),
            RepoLifecycleState::Idle
        );
        assert!(!store.active_repository_can_commit());
        assert!(!store.active_repository_can_stage());
        assert!(!store.active_repository_should_show_operation_banner());
        assert!(!store.active_repository_should_auto_select_conflict());
        assert!(store.active_repository_branches().is_empty());
        assert_eq!(store.active_repository_changes_count(), 0);
        assert!(!store.has_repositories());
        assert!(!can_commit_repository::<Value, Value>(None));
        assert!(can_commit_repository(Some(&demo_repo())));
        assert!(can_discard_repository(Some(&demo_repo())));
    }

    #[test]
    fn updates_sidebar_caches_independently_and_exposes_active_sidebar_getters() {
        let mut store = store_with(demo_repo());
        store.active_repository_index = Some(0);
        let stash = json!({ "hash": "stash@{0}", "message": "WIP" });

        let repository = store.repository_mut("repo-1").unwrap();
        repository.set_sidebar_loading(SidebarSection::Branches, true);
        assert!(repository.sidebar_loading.branches);
        repository.set_branches_data(vec![branch("main", true)], Some("main"));
        repository.set_starred_branches(vec!["main".to_string()]);
        repository.set_remotes_data(vec![origin()]);
        repository.set_tags_data(vec!["v1.0.0".to_string()]);
        repository.set_stashes_data(vec![stash.clone()]);
        repository.changes_count = 3;

        assert_eq!(store.active_repository_branches(), [branch("main", true)]);
        assert_eq!(store.active_repository_starred_branches(), ["main"]);
        assert_eq!(store.active_repository_remotes(), [origin()]);
        assert_eq!(store.active_repository_tags(), ["v1.0.0"]);
        assert_eq!(store.active_repository_stashes(), [stash]);
        assert!(store.active_repository_submodules().is_empty());
        assert_eq!(store.active_repository_changes_count(), 3);
        assert!(!store.repositories[0].sidebar_loading.branches);
        assert!(store.repositories[0].sidebar_loaded.branches);
        assert!(store.repositories[0].sidebar_loaded.tags);
        assert!(!store.repositories[0].sidebar_loaded.submodules);
        assert_eq!(
            store.repositories[0].current_branch.as_deref(),
            Some("main")
        );
    }

    fn loaded_sidebar() -> Repo {
        let mut repository = Repo {
            operation_state: Some(merge_in_progress()),
            conflict_state: merge_conflict(1, &["a.txt"], false),
            repo_lifecycle_state: RepoLifecycleState::Conflicted,
            status: Some(StructuredStatus {
                files: vec![file("a.txt", "U", "U")],
                ..status("main")
            }),
            ..demo_repo()
        };
        repository.set_branches_data(vec![branch("main", true)], None);
        repository.set_starred_branches(vec!["main".to_string()]);
        repository.set_remotes_data(vec![origin()]);
        repository.set_tags_data(vec!["v1.0.0".to_string()]);
        repository.set_stashes_data(vec![json!({ "hash": "stash@{0}", "message": "WIP" })]);
        repository.changes_count = 2;
        repository.sidebar_loaded.changes_count = true;
        repository
    }

    #[test]
    fn invalidates_sidebar_data_without_clearing_structured_repository_state() {
        let mut repository = loaded_sidebar();

        repository.invalidate_sidebar_data();

        assert_eq!(repository.branches, []);
        assert_eq!(repository.starred_branches, Vec::<String>::new());
        assert_eq!(repository.remotes, []);
        assert_eq!(repository.tags, Vec::<String>::new());
        assert_eq!(repository.stashes, Vec::<Value>::new());
        assert_eq!(repository.changes_count, 0);
        assert!(!repository.sidebar_loaded.tags);
        assert_eq!(repository.sidebar_loaded, SidebarFlags::default());
        assert_eq!(
            repository
                .operation_state
                .as_ref()
                .and_then(|state| state.kind),
            Some(OperationType::Merge)
        );
        assert_eq!(
            repository.repo_lifecycle_state,
            RepoLifecycleState::Conflicted
        );
        assert_eq!(repository.status.as_ref().unwrap().files[0].path, "a.txt");
    }

    #[test]
    fn invalidates_only_the_requested_sidebar_sections() {
        let mut repository = loaded_sidebar();

        repository.invalidate_sidebar_sections(&[SidebarSection::Tags, SidebarSection::Stashes]);

        assert_eq!(repository.branches, [branch("main", true)]);
        assert_eq!(repository.tags, Vec::<String>::new());
        assert_eq!(repository.stashes, Vec::<Value>::new());
        assert!(repository.sidebar_loaded.branches);
        assert!(!repository.sidebar_loaded.tags);
        assert!(!repository.sidebar_loaded.stashes);
        assert_eq!(repository.changes_count, 2);

        repository.invalidate_sidebar_sections(&[SidebarSection::ChangesCount]);
        assert_eq!(repository.changes_count, 0);
        assert!(!repository.sidebar_loaded.changes_count);
    }

    fn session(open: &[&str], active: Option<&str>) -> WorkspaceSession {
        WorkspaceSession {
            repository_workspaces: BTreeMap::new(),
            open_repositories: open.iter().map(|path| (*path).to_string()).collect(),
            active_repository_path: active.map(str::to_string),
            workspaces: Vec::new(),
            active_workspace_id: None,
        }
    }

    #[test]
    fn persists_the_workspace_session_when_adding_selecting_and_removing_repositories() {
        let mut store = Store::new();

        store.add_repository("/tmp/demo-a", AddRepositoryOptions::default());
        let added = store
            .add_repository("/tmp/demo-b", AddRepositoryOptions::default())
            .unwrap();

        assert_eq!(
            added.session,
            session(&["/tmp/demo-a", "/tmp/demo-b"], Some("/tmp/demo-b"))
        );
        assert_eq!(
            serde_json::to_value(&added.session).unwrap(),
            json!({
                "repositoryWorkspaces": {},
                "openRepositories": ["/tmp/demo-a", "/tmp/demo-b"],
                "activeRepositoryPath": "/tmp/demo-b"
            })
        );

        let selected = store.set_active_repository(Some(0));
        assert_eq!(
            selected,
            Some(session(
                &["/tmp/demo-a", "/tmp/demo-b"],
                Some("/tmp/demo-a")
            ))
        );

        let removed = store.remove_repository(0);
        assert_eq!(
            removed,
            Some(session(&["/tmp/demo-b"], Some("/tmp/demo-b")))
        );
        assert_eq!(
            store.workspace_session,
            session(&["/tmp/demo-b"], Some("/tmp/demo-b"))
        );
    }

    #[test]
    fn leaves_no_repository_active_when_the_last_one_closes_or_none_is_selected() {
        let mut store = Store::new();
        store.add_repository("/tmp/demo-a", AddRepositoryOptions::default());
        store.add_repository("/tmp/demo-b", AddRepositoryOptions::default());

        assert_eq!(store.set_active_repository(Some(5)), None);
        assert_eq!(store.active_repository_index, Some(1));
        assert_eq!(
            store.set_active_repository(None),
            Some(session(&["/tmp/demo-a", "/tmp/demo-b"], None))
        );
        assert!(store.active_repository().is_none());

        store.set_active_repository(Some(1));
        store.remove_repository("/tmp/demo-b");
        assert_eq!(store.active_repository_index, Some(0));
        assert_eq!(
            store.remove_repository("/tmp/demo-a"),
            Some(session(&[], None))
        );
        assert_eq!(store.active_repository_index, None);
        assert_eq!(store.remove_repository("/tmp/demo-a"), None);
    }

    #[test]
    fn distinguishes_repositories_with_the_same_name_by_the_shortest_parent_suffix() {
        let suffixes = distinguishing_parent_suffixes([
            ("demo", "/tmp/fx1/demo"),
            ("demo", "/tmp/fx2/demo"),
            ("api", "/work/api"),
            ("web", "/srv/a/x/web"),
            ("web", "/srv/b/x/web"),
            ("web", "/srv/fx1/web"),
        ]);

        assert_eq!(
            suffixes,
            [
                Some("fx1".to_string()),
                Some("fx2".to_string()),
                None,
                Some("a/x".to_string()),
                Some("b/x".to_string()),
                Some("fx1".to_string()),
            ]
        );
    }

    #[test]
    fn falls_back_to_the_whole_parent_path_when_one_is_a_suffix_of_another() {
        let suffixes = distinguishing_parent_suffixes([
            ("demo", "/p/demo"),
            ("demo", "/q/p/demo"),
            ("tool", "C:/work/tool"),
            ("tool", "D:/work/tool"),
        ]);

        assert_eq!(
            suffixes,
            [
                Some("/p".to_string()),
                Some("/q/p".to_string()),
                Some("C:/work".to_string()),
                Some("D:/work".to_string()),
            ]
        );
    }

    fn four_repositories_with_active(active: usize) -> Store {
        let mut store = Store::new();
        for name in ["a", "b", "c", "d"] {
            store.add_repository(&format!("/tmp/{name}"), AddRepositoryOptions::default());
        }
        store.set_active_repository(Some(active));
        store
    }

    fn active_path(store: &Store) -> Option<&str> {
        store.active_repository().map(|repo| repo.path.as_str())
    }

    // Antes: cerrar "a" con "c" activo dejaba activo "d" (el índice no se movía).
    #[test]
    fn closing_a_tab_left_of_the_active_one_keeps_the_same_repository_active() {
        let mut store = four_repositories_with_active(2);

        let session = store.remove_repository("/tmp/a").unwrap();

        assert_eq!(active_path(&store), Some("/tmp/c"));
        assert_eq!(store.active_repository_index, Some(1));
        assert_eq!(session.active_repository_path.as_deref(), Some("/tmp/c"));
    }

    #[test]
    fn closing_a_tab_right_of_the_active_one_keeps_it_active() {
        let mut store = four_repositories_with_active(1);
        store.remove_repository("/tmp/d");
        assert_eq!(active_path(&store), Some("/tmp/b"));
    }

    #[test]
    fn closing_the_active_tab_activates_its_right_neighbour_or_the_left_one_if_it_was_last() {
        let mut store = four_repositories_with_active(1);
        store.remove_repository("/tmp/b");
        assert_eq!(active_path(&store), Some("/tmp/c"));

        // [a, c, d] con "d" activo: era la última, pasa a la izquierda.
        store.set_active_repository(Some(2));
        store.remove_repository("/tmp/d");
        assert_eq!(active_path(&store), Some("/tmp/c"));
    }

    #[test]
    fn closing_a_tab_while_home_is_shown_keeps_home() {
        let mut store = four_repositories_with_active(2);
        store.set_active_repository(None);
        store.remove_repository("/tmp/a");
        assert_eq!(store.active_repository_index, None);
    }

    #[test]
    fn moving_a_tab_keeps_the_active_repository() {
        let mut store = four_repositories_with_active(2);

        let session = store.move_repository(0, 3).unwrap();

        assert_eq!(
            session.open_repositories,
            ["/tmp/b", "/tmp/c", "/tmp/d", "/tmp/a"]
        );
        assert_eq!(active_path(&store), Some("/tmp/c"));
        assert_eq!(store.active_repository_index, Some(1));
        assert_eq!(store.move_repository(1, 1), None);
        assert_eq!(store.move_repository(0, 9), None);
    }

    #[test]
    fn opens_a_repository_without_inventing_its_branch() {
        let mut store = Store::new();

        let outcome = store
            .add_repository(
                "/work/mergeon/",
                AddRepositoryOptions {
                    name: Some("Mergeon"),
                    is_github: true,
                    remote_url: Some("https://github.com/acme/mergeon.git"),
                    skip_recent_repository_update: true,
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(outcome.recent_repository, None);
        let repository = store.active_repository().unwrap();
        assert_eq!(repository.name, "Mergeon");
        assert_eq!(
            (repository.id.as_str(), repository.canonical_path.as_str()),
            ("/work/mergeon", "/work/mergeon")
        );
        assert_eq!(repository.current_branch, None);
        assert_eq!(repository.repo_lifecycle_state, RepoLifecycleState::Idle);
        assert_eq!(repository.mutation_queue, MutationQueue::default());
        assert_eq!(
            store.add_repository("   ", AddRepositoryOptions::default()),
            None
        );

        // Reabrirlo actualiza los recientes con lo que se sabe ahora.
        store
            .repository_mut(0)
            .unwrap()
            .set_status(Some(status("develop")));
        let reopened = store
            .add_repository(
                "/work/mergeon",
                AddRepositoryOptions {
                    is_github: true,
                    remote_url: Some("https://github.com/acme/mergeon.git"),
                    ..Default::default()
                },
            )
            .unwrap();
        let recent = reopened.recent_repository.unwrap();
        assert_eq!(recent.current_branch, "develop");
        assert!(recent.is_github);
        assert_eq!(recent.remote_url, "https://github.com/acme/mergeon.git");
        assert_eq!(recent.name, "Mergeon");
    }

    #[test]
    fn serializes_a_new_repository_with_the_same_fields_as_the_store() {
        let repository: Repo = Repository::new("/tmp/demo", None).unwrap();

        assert_eq!(
            serde_json::to_value(&repository).unwrap(),
            json!({
                "id": "/tmp/demo",
                "name": "demo",
                "path": "/tmp/demo",
                "canonicalPath": "/tmp/demo",
                "currentBranch": null,
                "detachedHeadRef": null,
                "headShortHash": null,
                "isDirty": false,
                "operationState": null,
                "conflictState": {
                    "inConflict": false,
                    "operationType": null,
                    "initialCount": 0,
                    "pendingCount": 0,
                    "resolvedCount": 0,
                    "pendingFiles": [],
                    "readyToContinue": false
                },
                "mutationQueue": { "active": null, "queued": [] },
                "repoLifecycleState": "idle",
                "trackingStatus": null,
                "lastMutationAt": null,
                "lastMutationOperation": null,
                "commits": [],
                "branches": [],
                "starredBranches": [],
                "remotes": [],
                "tags": [],
                "stashes": [],
                "submodules": [],
                "changesCount": 0,
                "sidebarLoading": {
                    "branches": false,
                    "starredBranches": false,
                    "remotes": false,
                    "tags": false,
                    "stashes": false,
                    "submodules": false,
                    "changesCount": false
                },
                "sidebarLoaded": {
                    "branches": false,
                    "starredBranches": false,
                    "remotes": false,
                    "tags": false,
                    "stashes": false,
                    "submodules": false,
                    "changesCount": false
                },
                "status": null
            })
        );
    }

    #[test]
    fn normalizes_repository_paths_for_comparison() {
        assert_eq!(
            normalize_repository_path("C:\\repos\\demo\\"),
            "C:/repos/demo"
        );
        assert_eq!(normalize_repository_path("  /tmp/demo//  "), "/tmp/demo");
        assert_eq!(normalize_repository_path("/"), "/");
        assert_eq!(normalize_repository_path("C:\\"), "C:/");
        assert_eq!(normalize_repository_path("c:/"), "c:/");
        assert_eq!(normalize_repository_path(""), "");
        assert_eq!(normalize_repository_path("   "), "");
        assert_eq!(normalize_repository_path("//"), "");
    }

    #[test]
    fn reads_the_current_branch_from_the_status() {
        assert_eq!(
            normalize_current_branch_from_status(None, Some("main")),
            "main"
        );
        assert_eq!(normalize_current_branch_from_status(None, None), "HEAD");
        assert_eq!(
            normalize_current_branch_from_status(Some(&status("develop")), Some("main")),
            "develop"
        );
        // Un estado sin rama es HEAD desanclado, no la rama anterior.
        assert_eq!(
            normalize_current_branch_from_status(Some(&status("")), Some("main")),
            "HEAD"
        );
        let no_branch = StructuredStatus::default();
        assert_eq!(
            normalize_current_branch_from_status(Some(&no_branch), Some("main")),
            "HEAD"
        );
    }

    #[test]
    fn restarts_conflict_progress_when_the_operation_changes_or_ends() {
        let previous = merge_conflict(3, &["a.txt", "b.txt"], false);
        let in_merge = |pending: &[&str]| StructuredStatus {
            operation_state: merge_in_progress(),
            conflict_state: merge_conflict(pending.len(), pending, false),
            ..status("main")
        };

        // Aparecen más conflictos de los que había: se vuelve a contar.
        let more =
            normalize_conflict_state(Some(&previous), Some(&in_merge(&["a", "b", "c", "d"])));
        assert_eq!((more.initial_count, more.resolved_count), (4, 0));

        // Otra operación distinta: nada que heredar.
        let rebase = StructuredStatus {
            operation_state: OperationState {
                kind: Some(OperationType::Rebase),
                ..merge_in_progress()
            },
            conflict_state: ConflictState {
                operation_type: Some(OperationType::Rebase),
                ..merge_conflict(1, &["a"], false)
            },
            ..status("main")
        };
        let switched = normalize_conflict_state(Some(&previous), Some(&rebase));
        assert_eq!((switched.initial_count, switched.resolved_count), (1, 0));

        // Sin estado anterior, o con la operación terminada.
        assert_eq!(
            normalize_conflict_state(None, Some(&in_merge(&["a"]))).initial_count,
            1
        );
        assert_eq!(
            normalize_conflict_state(Some(&previous), Some(&status("main"))),
            ConflictState::default()
        );
        assert_eq!(
            normalize_conflict_state(Some(&previous), None),
            ConflictState::default()
        );
    }

    #[test]
    fn derives_the_lifecycle_from_operation_and_conflicts() {
        let busy = StructuredStatus {
            operation_state: merge_in_progress(),
            ..status("main")
        };

        assert_eq!(
            derive_repo_lifecycle_state(None, None),
            RepoLifecycleState::Idle
        );
        assert_eq!(
            derive_repo_lifecycle_state(Some(&status("main")), None),
            RepoLifecycleState::Idle
        );
        assert_eq!(
            derive_repo_lifecycle_state(Some(&busy), None),
            RepoLifecycleState::Busy
        );
        assert_eq!(
            derive_repo_lifecycle_state(Some(&busy), Some(&merge_conflict(1, &["a"], false))),
            RepoLifecycleState::Conflicted
        );
        assert_eq!(
            derive_repo_lifecycle_state(Some(&busy), Some(&merge_conflict(1, &[], true))),
            RepoLifecycleState::ReadyToContinue
        );

        // Operación en curso sin conflictos: ni commit ni tocar archivos.
        let mut repository = demo_repo();
        repository.set_status(Some(busy));
        assert_eq!(repository.repo_lifecycle_state, RepoLifecycleState::Busy);
        assert!(repository.should_show_operation_banner());
        assert!(!repository.can_commit());
        assert!(!repository.can_mutate_files());
        assert!(!repository.should_auto_select_conflict());
    }

    #[test]
    fn keeps_a_detached_head_when_branch_data_arrives_later() {
        let mut repository = demo_repo();
        repository.set_status(Some(StructuredStatus {
            head_short_hash: Some("2eea100".to_string()),
            ..status("")
        }));

        // La lectura de ramas dice "main", pero el estado manda: HEAD desanclado.
        repository.set_branches_data(vec![branch("main", false)], Some("main"));
        assert_eq!(repository.current_branch.as_deref(), Some("HEAD"));
        assert_eq!(repository.detached_head_ref.as_deref(), Some("2eea100"));

        repository.update_branch(Some("main"));
        assert_eq!(repository.current_branch.as_deref(), Some("HEAD"));
        assert_eq!(repository.detached_head_ref.as_deref(), Some("2eea100"));

        // Ya en una rama, la lectura de ramas y el checkout sí la cambian.
        repository.set_status(Some(status("main")));
        assert_eq!(repository.detached_head_ref, None);
        repository.update_branch(Some("feature"));
        assert_eq!(repository.current_branch.as_deref(), Some("feature"));
        repository.status = None;
        repository.set_branches_data(vec![], Some(""));
        assert_eq!(repository.current_branch.as_deref(), Some("HEAD"));
        repository.update_branch(None);
        assert_eq!(repository.current_branch.as_deref(), Some("HEAD"));
    }

    #[test]
    fn clearing_the_status_keeps_the_branch_and_drops_the_rest() {
        let mut repository = demo_repo();
        repository.set_status(Some(StructuredStatus {
            tracking: Some("origin/main".to_string()),
            ahead: 2,
            behind: 1,
            files: vec![file("a.txt", " ", "M"), file("b.txt", "?", "?")],
            ..status("main")
        }));

        assert_eq!(
            repository.tracking_status,
            Some(TrackingStatus {
                current: "main".to_string(),
                tracking: Some("origin/main".to_string()),
                ahead: 2,
                behind: 1
            })
        );
        assert!(repository.is_dirty);
        assert_eq!(repository.changes_count, 2);
        assert!(repository.sidebar_loaded.changes_count);

        repository.set_status(None);

        assert_eq!(repository.current_branch.as_deref(), Some("main"));
        assert_eq!(repository.tracking_status, None);
        assert_eq!(repository.operation_state, None);
        assert!(!repository.is_dirty);
        assert_eq!(repository.changes_count, 0);
    }

    #[test]
    fn restores_a_saved_session_and_survives_a_corrupt_one() {
        let saved = json!({
            "openRepositories": ["/tmp/demo-a", "", "   ", 7, "/tmp/demo-b"],
            "activeRepositoryPath": "/tmp/demo-b",
            "repositoryWorkspaces": {
                "/tmp/demo-a": { "activeTab": "commits", "section": "changes" }
            }
        });

        let session = normalize_workspace_session(&saved);
        assert_eq!(session.open_repositories, ["/tmp/demo-a", "/tmp/demo-b"]);
        assert_eq!(
            session.active_repository_path.as_deref(),
            Some("/tmp/demo-b")
        );
        // Cada espacio de trabajo pasa por su propia normalización (y migración).
        assert_eq!(
            session.repository_workspaces["/tmp/demo-a"].active_tab,
            "changes"
        );

        assert_eq!(
            normalize_workspace_session(&Value::Null),
            WorkspaceSession::default()
        );
        assert_eq!(
            normalize_workspace_session(
                &json!({ "openRepositories": "no", "activeRepositoryPath": "  " })
            ),
            WorkspaceSession::default()
        );
        let reloaded: WorkspaceSession =
            serde_json::from_value(serde_json::to_value(&session).unwrap()).unwrap();
        assert_eq!(reloaded, session);
    }

    #[test]
    fn loads_recents_and_session_once() {
        let mut store = Store::new();
        let recent = RecentRepository {
            path: "/tmp/demo".to_string(),
            name: "demo".to_string(),
            ..Default::default()
        };
        let saved =
            json!({ "openRepositories": ["/tmp/demo"], "activeRepositoryPath": "/tmp/demo" });

        assert!(store.load_from_storage(Some(vec![recent.clone()]), Some(&saved)));
        assert!(store.loaded);
        assert_eq!(store.recent_repositories, [recent]);
        assert_eq!(store.workspace_session.open_repositories, ["/tmp/demo"]);

        // La segunda vez no pisa nada.
        assert!(!store.load_from_storage(Some(vec![]), None));
        assert_eq!(store.recent_repositories.len(), 1);

        store.remove_recent_repository("/tmp/other");
        assert_eq!(store.recent_repositories.len(), 1);
        store.remove_recent_repository("/tmp/demo");
        assert!(store.recent_repositories.is_empty());
        store.clear_recent_repositories();
        assert!(store.recent_repositories.is_empty());
    }

    #[test]
    fn keeps_the_tabs_of_each_repository_in_the_session() {
        let mut store = Store::new();
        store.add_repository("/tmp/demo", AddRepositoryOptions::default());

        let workspace = store.get_repository_workspace("/tmp/demo");
        open_file_tab(workspace, &FileTabDescriptor::working("src/app.js"));
        let workspace = workspace.clone();

        let saved = store.save_repository_workspace("/tmp/demo", workspace.clone());
        assert_eq!(saved.repository_workspaces["/tmp/demo"], workspace);
        assert_eq!(saved.open_repositories, ["/tmp/demo"]);

        // Cerrar el repositorio no olvida sus pestañas: vuelven al reabrirlo.
        let closed = store.remove_repository("/tmp/demo").unwrap();
        assert_eq!(closed.repository_workspaces["/tmp/demo"].tabs.len(), 1);
        assert!(closed.open_repositories.is_empty());
    }

    #[test]
    fn records_results_for_repositories_that_are_no_longer_open() {
        let mut store = Store::new();
        let mut log = OperationsLog::new();

        store.apply_repository_operation_result(
            "gone",
            &OperationResult {
                operation: Some("fetch".to_string()),
                outcome: Some("failed".to_string()),
                message: Some("fatal: not a git repository".to_string()),
                ..Default::default()
            },
            &mut log,
        );

        assert_eq!(log.entries.len(), 1);
        assert_eq!(log.entries[0].repo_name, None);
        assert_eq!(log.unseen_errors, 1);
    }

    #[test]
    fn manages_repo_workspaces_visibility_ordering_and_persistence() {
        let mut store = Store::new();
        store.add_repository("/tmp/a", AddRepositoryOptions::default());
        store.add_repository("/tmp/b", AddRepositoryOptions::default());
        store.add_repository("/tmp/c", AddRepositoryOptions::default());

        // Sin workspaces activos, visible_repositories devuelve todos.
        assert_eq!(store.visible_repositories().len(), 3);
        assert!(store.create_workspace("   ", &[]).is_none());

        let (ws1, session) = store
            .create_workspace(
                "  Work  ",
                &["/tmp/c/".into(), "/tmp/a".into(), "/tmp/c".into()],
            )
            .expect("workspace");
        assert_eq!(ws1.name, "Work");
        assert_eq!(ws1.id, "ws-work");
        assert_eq!(ws1.repositories, ["/tmp/c", "/tmp/a"]);
        assert_eq!(ws1.active_repository_path.as_deref(), Some("/tmp/c"));
        assert_eq!(session.workspaces.len(), 1);

        // Nombres duplicados producen ids únicos.
        let (ws2, _) = store
            .create_workspace("Work", &["/tmp/b".into()])
            .expect("second workspace");
        assert_eq!(ws2.id, "ws-work-2");

        // Activar ws1 filtra y ordena según el workspace.
        store.set_active_workspace(Some(&ws1.id)).expect("activate");
        let visible_paths: Vec<&str> = store
            .visible_repositories()
            .into_iter()
            .map(|repo| repo.path.as_str())
            .collect();
        assert_eq!(visible_paths, ["/tmp/c", "/tmp/a"]);

        // Reordenar dentro del workspace activo no cambia el orden global.
        store.move_visible_repository(0, 1).expect("reorder");
        let visible_paths: Vec<&str> = store
            .visible_repositories()
            .into_iter()
            .map(|repo| repo.path.as_str())
            .collect();
        assert_eq!(visible_paths, ["/tmp/a", "/tmp/c"]);
        let global_paths: Vec<&str> = store
            .repositories
            .iter()
            .map(|repo| repo.path.as_str())
            .collect();
        assert_eq!(global_paths, ["/tmp/a", "/tmp/b", "/tmp/c"]);

        // Abrir un repositorio nuevo con workspace activo lo añade automáticamente.
        store.add_repository("/tmp/d", AddRepositoryOptions::default());
        assert_eq!(
            store.workspace(&ws1.id).unwrap().repositories,
            ["/tmp/a", "/tmp/c", "/tmp/d"]
        );
        assert_eq!(
            store
                .workspace(&ws1.id)
                .unwrap()
                .active_repository_path
                .as_deref(),
            Some("/tmp/d")
        );

        // Con skip_active_workspace_assignment no se añade al workspace activo.
        store.add_repository(
            "/tmp/e",
            AddRepositoryOptions {
                skip_active_workspace_assignment: true,
                ..Default::default()
            },
        );
        assert!(!store.is_repository_in_workspace("/tmp/e", &ws1.id));

        // Renombrar conserva el id.
        assert!(store.rename_workspace(&ws1.id, "Backend").is_some());
        assert_eq!(store.workspace(&ws1.id).unwrap().name, "Backend");
        assert!(store.rename_workspace(&ws1.id, "  ").is_none());

        // Borrar el workspace activo vuelve a "All" sin cerrar repositorios.
        assert!(store.delete_workspace(&ws1.id).is_some());
        assert_eq!(store.active_workspace_id(), None);
        assert_eq!(store.repositories.len(), 5);
        assert_eq!(store.visible_repositories().len(), 5);
    }

    #[test]
    fn normalizes_workspaces_in_session_and_omits_empty_fields_when_serializing() {
        let empty = WorkspaceSession::default();
        let serialized = serde_json::to_value(&empty).unwrap();
        assert!(serialized.get("workspaces").is_none());
        assert!(serialized.get("activeWorkspaceId").is_none());

        let normalized = normalize_workspace_session(&json!({
            "openRepositories": ["/tmp/a"],
            "activeRepositoryPath": "/tmp/a",
            "workspaces": [
                { "id": "  ", "name": "Bad" },
                { "id": "w1", "name": "  " },
                {
                    "id": "w1",
                    "name": "  Core  ",
                    "repositories": ["/tmp/a/", "/tmp/a", "", 7, "/tmp/b"],
                    "activeRepositoryPath": "/tmp/missing"
                },
                {
                    "id": "w1",
                    "name": "Duplicate ID",
                    "repositories": ["/tmp/c"]
                },
                {
                    "id": "w2",
                    "name": "Tools",
                    "repositories": ["/tmp/b"],
                    "activeRepositoryPath": "/tmp/b/"
                }
            ],
            "activeWorkspaceId": "w2"
        }));

        assert_eq!(normalized.workspaces.len(), 2);
        assert_eq!(normalized.workspaces[0].id, "w1");
        assert_eq!(normalized.workspaces[0].name, "Core");
        assert_eq!(normalized.workspaces[0].repositories, ["/tmp/a", "/tmp/b"]);
        assert_eq!(normalized.workspaces[0].active_repository_path, None);
        assert_eq!(
            normalized.workspaces[1].active_repository_path.as_deref(),
            Some("/tmp/b")
        );
        assert_eq!(normalized.active_workspace_id.as_deref(), Some("w2"));

        // Un activeWorkspaceId inexistente se descarta a None.
        let dangling = normalize_workspace_session(&json!({
            "workspaces": [],
            "activeWorkspaceId": "ghost"
        }));
        assert_eq!(dangling.active_workspace_id, None);
    }
}
