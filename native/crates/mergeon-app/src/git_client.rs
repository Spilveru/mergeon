//! Cliente git de la interfaz (`src/composables/useGit.js`).
//!
//! Un método por función de `useGit`, con el mismo nombre. Cada uno lanza la
//! llamada al backend en un hilo de fondo y, al volver, avisa del resultado
//! como lo hacía el original:
//!
//! - **Mutaciones**: devuelven siempre el resultado estructurado del backend
//!   (`Mutation<T>`), nunca un error, y muestran el aviso que le corresponde
//!   (`notifyOperationResult`). Dos peticiones idénticas en vuelo comparten
//!   una sola llamada y un solo aviso (`runSharedGitOperation`).
//! - **Lecturas**: devuelven `QueryResult<T>`. Las que en el original avisaban
//!   del fallo (`notify.error('Error loading ...', { error })`) lo siguen
//!   haciendo; el error se devuelve igualmente para que quien llama decida.
//!
//! Este módulo no toca el estado de los repositorios: aplicar un resultado
//! (`apply_repository_operation_result`) y releer (`sync_repository_read_model`)
//! es cosa de quien llama, como en el original.

use futures::future::pending;
use gpui_kit::{Context, Task};
use mergeon_backend::{
    AddToIgnoreResult, AddWorktreeOptions, ApplyPatchOptions, Backend, BlameResult,
    BranchComparison, BranchesOverview, CleanupBranchesOptions, CloneResult, Commit,
    CommitLogOptions, CommitOptions, CommitSearchOptions, CompareLimits, CompareMode, ComparedFile,
    ComparisonFiles, ComparisonRevisions, ConflictResolution, ConflictVersions, DiffOptions,
    DiffPayload, FetchOptions, FileAtCommit, FileChange, FileHistoryOptions, FileHistoryPage,
    GitIdentity, ImagePair, ImagePairRequest, InitRepositoryResult, InteractiveRebaseOptions,
    MergeConflictCheck, MergeOptions, MergedBranchesReport, Mutation, MutationResult,
    OperationResult, OperationState, PullOptions, PushOptions, QueryResult, RebaseActionParams,
    RebaseOptions, ReflogEntry, ReflogOptions, Remote, RemoteConnectionTest, RemoteTagOptions,
    RepositoryInspection, ResolveConflictResult, RevisionComparison, SequenceActionResult,
    SetIdentityOptions, SetIdentityResult, SetupTrackingResult, StashEntry, StashOptions,
    StructuredStatus, Submodule, SubmoduleUpdateOptions, TagDetails, TreeEntries,
    UncommittedChanges, Worktree, normalize_mutation_key_value,
};
use mergeon_model::notify::{NotifyOptions, NotifyType, ProgressToast, ToastAction, ToastId};
use mergeon_model::operation_types::OperationResult as OperationSummary;
use mergeon_model::workspace_tabs::WorkspaceSection;
use serde_json::{Value, json};

use crate::AppModel;
use crate::convert::StructuredOutcome;
use crate::tasks::{InFlightMap, flow, settled};
use crate::undo::{UndoIntent, short_sha};
use mergeon_backend::undo::{ResetMode, UndoStep};

/// Resultado de una mutación sobre el working tree, las ramas, los remotos,
/// los tags o los stashes.
pub type GitMutation = Mutation<MutationResult>;

/// Resultado de merge, rebase, revert y cherry-pick.
pub type GitOperation = Mutation<OperationResult>;

/// Estado que `useGit` guardaba en `isLoading` / `error` y el mapa de
/// operaciones compartidas.
#[derive(Debug, Default)]
pub(crate) struct GitClientState {
    pub shared_operations: InFlightMap,
    /// Operaciones en vuelo de las que en el original ponían `isLoading`.
    pub loading: usize,
    pub error: Option<String>,
}

fn shared_operations(model: &mut AppModel) -> &mut InFlightMap {
    &mut model.git.shared_operations
}

/// `buildSharedOperationKey`: operación, repositorio y argumentos, con las
/// claves de objeto ordenadas y los secretos sustituidos por su huella.
pub fn build_shared_operation_key(operation: &str, repo_path: &str, args: &Value) -> String {
    format!(
        "{operation}:{repo_path}:{}",
        normalize_mutation_key_value(args)
    )
}

/// "Push origin/main…": remoto y, si se conoce, la rama de destino.
fn push_progress_title(options: &PushOptions) -> String {
    let branch = options
        .target_branch
        .as_deref()
        .or(options.branch.as_deref())
        .filter(|branch| !branch.is_empty() && *branch != "HEAD");
    match branch {
        Some(branch) => format!("Push {}/{branch}…", options.remote),
        None => format!("Push {}…", options.remote),
    }
}

/// "cherry-pick of 3 commits" / "cherry-pick of abc1234".
fn commits_label(verb: &str, commits: &[String]) -> String {
    match commits {
        [one] => format!("{verb} of {}", short_sha(one)),
        many => format!("{verb} of {} commits", many.len()),
    }
}

/// Cómo se deshace un `git reset <mode>`: `soft` y `mixed` no tocan el
/// working tree y se deshacen con el mismo modo; `hard`, con `--keep` (lo que
/// borró del working tree no vuelve).
fn reset_undo_mode(mode: &str) -> ResetMode {
    match mode {
        "soft" => ResetMode::Soft,
        "mixed" => ResetMode::Mixed,
        _ => ResetMode::Keep,
    }
}

/// Nombre del repositorio que se clona, a partir de su URL.
fn clone_name(remote_url: &str) -> &str {
    let trimmed = remote_url.trim_end_matches('/');
    let name = trimmed.rsplit(['/', ':']).next().unwrap_or(trimmed);
    let name = name.strip_suffix(".git").unwrap_or(name);
    if name.is_empty() { remote_url } else { name }
}

/// Claves de las acciones del aviso de conflictos; les sigue la ruta del
/// repositorio.
const CONFLICTS_RESOLVE_ACTION: &str = "conflicts:resolve:";
const CONFLICTS_ABORT_ACTION: &str = "conflicts:abort:";

/// Un pull que se detuvo con conflictos (y dejó la fusión a medias).
fn is_pull_conflict(result: &OperationSummary) -> bool {
    result.operation.as_deref() == Some("pull")
        && (result.outcome.as_deref() == Some("conflicted")
            || result.code.as_deref() == Some("CONFLICT"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotifyMode {
    /// `notifyOperationResult`.
    Operation,
    /// `() => {}`: quien llama decide si avisa.
    Silent,
    /// `notifyStatusFlowResult`.
    StatusFlow,
    /// El aviso propio de `setupTracking`.
    SetupTracking,
    /// Resolver un conflicto: como `StatusFlow`, salvo el que resuelve el
    /// último (ver [`resolves_the_last_conflict`]).
    ResolveConflict,
}

/// Un conflicto resuelto que deja la operación lista para continuar. Su
/// aviso sobra: el banner de la operación ya lo dice ("All conflicts
/// resolved. Ready to continue …") y sus botones Continue / Abort quedan
/// justo bajo la pila de avisos (arriba a la derecha), así que el aviso
/// taparía el siguiente clic durante unos segundos.
fn resolves_the_last_conflict(result: &OperationSummary) -> bool {
    result.success
        && result.status.as_ref().is_some_and(|status| {
            status.conflict_state.ready_to_continue && status.operation_state.in_progress
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorRule {
    /// No toca `error`.
    Untouched,
    /// `result.success ? null : result.message`.
    SuccessOnly,
    /// `result.success || result.outcome === 'noop' ? null : result.message`.
    SuccessOrNoop,
}

#[derive(Clone)]
struct OperationSpec {
    operation: &'static str,
    repo_path: String,
    request_args: Value,
    notify: NotifyMode,
    loading: bool,
    error: ErrorRule,
    /// Título del aviso de progreso mientras la operación corre (fetch, pull,
    /// push, clone). Solo si la operación avisa de su resultado.
    progress: Option<String>,
    /// Qué es para la pila de deshacer, si se puede deshacer.
    undo: Option<UndoIntent>,
}

impl OperationSpec {
    fn new(operation: &'static str, repo_path: &str, request_args: Value) -> Self {
        Self {
            operation,
            repo_path: repo_path.to_string(),
            request_args,
            notify: NotifyMode::Operation,
            loading: false,
            error: ErrorRule::SuccessOrNoop,
            progress: None,
            undo: None,
        }
    }

    fn undo(mut self, intent: UndoIntent) -> Self {
        self.undo = Some(intent);
        self
    }

    /// Sin entrada en la pila de deshacer (los pasos internos de un flujo).
    fn without_undo(mut self, without: bool) -> Self {
        if without {
            self.undo = None;
        }
        self
    }

    fn progress(mut self, title: impl Into<String>) -> Self {
        self.progress = Some(title.into());
        self
    }

    fn loading(mut self) -> Self {
        self.loading = true;
        self
    }

    fn notify(mut self, notify: NotifyMode) -> Self {
        self.notify = notify;
        self
    }

    fn silent(self, silent: bool) -> Self {
        if silent {
            self.notify(NotifyMode::Silent)
        } else {
            self
        }
    }

    fn error(mut self, error: ErrorRule) -> Self {
        self.error = error;
        self
    }
}

impl AppModel {
    /// Hay alguna operación git en curso de las que `useGit` marcaba con
    /// `isLoading`.
    pub fn git_is_loading(&self) -> bool {
        self.git.loading > 0
    }

    /// `error` de `useGit`: el mensaje del último fallo, o `None` si la última
    /// operación terminó bien.
    pub fn git_error(&self) -> Option<&str> {
        self.git.error.as_deref()
    }

    /// `toast` es el aviso de esta operación en este repositorio: el resultado
    /// nuevo reescribe al anterior en vez de apilarse (un fetch que falló y
    /// el reintento que funcionó no son dos avisos que se contradicen).
    ///
    /// Con más de un repositorio abierto el aviso dice de cuál es.
    pub(crate) fn notify_git_result(
        &mut self,
        mode: NotifyMode,
        repo_path: &str,
        result: &OperationSummary,
        toast: ToastId,
        cx: &mut Context<Self>,
    ) {
        let mut labelled = result.clone();
        labelled.message = result
            .message
            .as_deref()
            .filter(|message| !message.is_empty())
            .map(|message| self.message_for_repository(repo_path, message));
        let result = &labelled;
        match mode {
            NotifyMode::Operation if is_pull_conflict(result) => {
                self.notify_pull_conflicts(repo_path, result, toast, cx);
            }
            NotifyMode::Operation => self.notify_operation_result(result, Some(toast), cx),
            NotifyMode::Silent => {}
            NotifyMode::StatusFlow => self.notify_status_flow_result(result, Some(toast), cx),
            // Se retira también el "Resolved …" del conflicto anterior (mismo
            // id), que podría seguir a la vista encima del banner.
            NotifyMode::ResolveConflict if resolves_the_last_conflict(result) => {
                self.dismiss_toast(&toast, cx);
            }
            NotifyMode::ResolveConflict => {
                self.notify_status_flow_result(result, Some(toast), cx);
            }
            NotifyMode::SetupTracking => {
                let Some(message) = result.message.as_deref().filter(|text| !text.is_empty())
                else {
                    return;
                };
                let outcome = result.outcome.as_deref();
                let code = result.code.as_deref().unwrap_or_default();
                let kind = if result.success && outcome == Some("completed") {
                    NotifyType::Success
                } else if result.success && outcome == Some("noop") {
                    NotifyType::Info
                } else if code == "REMOTE_BRANCH_NOT_FOUND" || code == "NO_BRANCH_SPECIFIED" {
                    NotifyType::Warning
                } else {
                    NotifyType::Error
                };
                let message = message.to_string();
                let options = NotifyOptions {
                    id: Some(toast),
                    ..Default::default()
                };
                self.notify(kind, &message, options, cx);
            }
        }
    }

    /// Pull detenido por conflictos: el aviso lo dice con el número de
    /// archivos, los nombra y ofrece resolverlos (Local Changes) o abortar la
    /// fusión (el "Abort" de la operación en curso).
    fn notify_pull_conflicts(
        &mut self,
        repo_path: &str,
        result: &OperationSummary,
        toast: ToastId,
        cx: &mut Context<Self>,
    ) {
        let status = result.status.as_ref();
        let files: Vec<&str> = status
            .map(|status| status.conflicted.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let title = match files.len() {
            0 => "Pull stopped: Git detected conflicts".to_string(),
            1 => "Pull stopped: 1 conflict".to_string(),
            count => format!("Pull stopped: {count} conflicts"),
        };
        let title = self.message_for_repository(repo_path, &title);
        let abort_label = match status.and_then(|status| status.operation_state.kind) {
            Some(mergeon_backend::OperationType::Rebase) => "Abort rebase",
            _ => "Abort merge",
        };
        let options = NotifyOptions {
            id: Some(toast),
            details: result.message.clone(),
            description: (!files.is_empty()).then(|| files.join(", ")),
            action: Some(ToastAction::custom(
                "Resolve",
                format!("{CONFLICTS_RESOLVE_ACTION}{repo_path}"),
            )),
            cancel: Some(ToastAction::custom(
                abort_label,
                format!("{CONFLICTS_ABORT_ACTION}{repo_path}"),
            )),
            ..Default::default()
        };
        self.notify(NotifyType::Error, &title, options, cx);
    }

    /// Acciones del aviso de conflictos. Devuelve `false` si la clave no es
    /// suya.
    pub(crate) fn run_git_toast_action(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        if self.run_progress_toast_action(key, cx) {
            return true;
        }
        let (abort, repo_path) = if let Some(path) = key.strip_prefix(CONFLICTS_RESOLVE_ACTION) {
            (false, path)
        } else if let Some(path) = key.strip_prefix(CONFLICTS_ABORT_ACTION) {
            (true, path)
        } else {
            return false;
        };
        // El aviso puede ser de un repositorio que ya no es el activo.
        let Some(index) = self
            .repositories
            .repositories
            .iter()
            .position(|repository| repository.path == repo_path)
        else {
            return true;
        };
        if self.active_repository_index() != Some(index) {
            self.select_repository_tab(index, cx);
        }
        if abort {
            self.handle_sequence_action("abort", cx).detach();
        } else {
            self.handle_change_view(WorkspaceSection::Changes, cx);
        }
        true
    }

    /// `invokeStructuredOperation`: ejecuta la mutación (compartida con las
    /// peticiones idénticas que ya estén en vuelo), avisa del resultado una
    /// vez y actualiza `isLoading` / `error`.
    fn invoke_structured_operation<T>(
        &mut self,
        spec: OperationSpec,
        cx: &mut Context<Self>,
        call: impl FnOnce(&Backend) -> T + Send + 'static,
    ) -> Task<T>
    where
        T: StructuredOutcome + Clone + Send + 'static,
    {
        let key = build_shared_operation_key(spec.operation, &spec.repo_path, &spec.request_args);
        let notify = spec.notify;
        let toast_key = crate::credentials::operation_toast_key(spec.operation, &spec.repo_path);
        let toast = ToastId::Named(toast_key.clone());
        let repo_path = spec.repo_path.clone();
        let repo_path_for_progress = spec.repo_path.clone();
        let undo = spec.undo.clone();
        let operation = spec.operation;
        let subject = spec
            .progress
            .as_deref()
            .map(|title| title.trim_end_matches('…').to_string());
        let progress = spec
            .progress
            .filter(|_| notify != NotifyMode::Silent)
            .map(|title| ProgressToast {
                id: toast.clone(),
                operation: spec.operation.to_string(),
                title: self.message_for_repository(&spec.repo_path, &title),
                description: None,
                // Indeterminada hasta que git informe (ver `progress.rs`).
                fraction: None,
                cancel: None,
            });
        let shared = self.run_shared(shared_operations, key, false, cx, move |this, cx| {
            let showed_progress = progress.is_some();
            if let Some(progress) = progress {
                this.begin_progress(progress, &repo_path_for_progress, cx);
            }
            let started = cx.background_executor().now();
            let work = this.blocking(cx, call);
            cx.spawn(async move |this, cx| {
                let result = work.await;
                this.update(cx, |this, cx| {
                    // El aviso del resultado reemplaza al de progreso (mismo
                    // id); si no hay nada que anunciar, el de progreso se va.
                    if showed_progress {
                        this.end_progress(&toast);
                        this.dismiss_toast(&toast, cx);
                    }
                    let summary = result.to_operation_result();
                    if let Some(intent) = &undo {
                        this.record_undo(&repo_path, intent, result.ref_capture(), &summary, cx);
                    }
                    // Si falló por credenciales y quien la lanzó va a
                    // pedirlas, el diálogo sustituye al aviso de error.
                    if this.suppresses_credentials_toast(&toast_key, &summary) {
                        return;
                    }
                    let elapsed = cx
                        .background_executor()
                        .now()
                        .saturating_duration_since(started);
                    this.notify_desktop_if_unfocused(
                        operation,
                        subject.as_deref(),
                        &repo_path,
                        &summary,
                        elapsed,
                        notify == NotifyMode::Silent,
                        cx,
                    );
                    this.notify_git_result(notify, &repo_path, &summary, toast, cx);
                })
                .ok();
                result
            })
        });

        if spec.loading {
            self.git.loading += 1;
            cx.notify();
        }
        let loading = spec.loading;
        let error_rule = spec.error;

        flow(cx, async move |this, cx| {
            let result = settled(shared).await;
            this.update(cx, |this, cx| {
                if loading {
                    this.git.loading = this.git.loading.saturating_sub(1);
                }
                if let Some(result) = &result {
                    let summary = result.to_operation_result();
                    let succeeded = match error_rule {
                        ErrorRule::Untouched => None,
                        ErrorRule::SuccessOnly => Some(summary.success),
                        ErrorRule::SuccessOrNoop => {
                            Some(summary.success || summary.outcome.as_deref() == Some("noop"))
                        }
                    };
                    match succeeded {
                        Some(true) => this.git.error = None,
                        Some(false) => this.git.error = summary.message,
                        None => {}
                    }
                }
                cx.notify();
            });

            match result {
                Some(result) => result,
                // La operación se abandonó sin resultado: la app se cierra.
                None => pending().await,
            }
        })
    }

    /// Lectura del backend. Si falla, guarda el mensaje en `error` y, cuando
    /// el original lo hacía, avisa con `failure_title`.
    pub(crate) fn read<R>(
        &mut self,
        failure_title: Option<&'static str>,
        cx: &mut Context<Self>,
        work: impl FnOnce(&Backend) -> QueryResult<R> + Send + 'static,
    ) -> Task<QueryResult<R>>
    where
        R: Send + 'static,
    {
        let work = self.blocking(cx, work);
        cx.spawn(async move |this, cx| {
            let result = work.await;
            if let Err(error) = &result {
                this.update(cx, |this, cx| {
                    this.git.error = Some(error.message().to_string());
                    if let Some(title) = failure_title {
                        this.notify_error_with(title, error, cx);
                    }
                    cx.notify();
                })
                .ok();
            }
            result
        })
    }

    // ------------------------------------------------------------------
    // Repositorio
    // ------------------------------------------------------------------

    /// `inspectRepository`.
    pub fn inspect_repository(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<RepositoryInspection> {
        let repo_path = repo_path.to_string();
        self.blocking(cx, move |backend| {
            backend.git().inspect_repository(&repo_path)
        })
    }

    /// `initRepository`. No avisa: quien llama decide el texto.
    pub fn init_repository(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<Mutation<InitRepositoryResult>> {
        let repo_path = repo_path.to_string();
        self.blocking(cx, move |backend| backend.git().init_repository(&repo_path))
    }

    /// `isValidRepository`.
    pub fn is_valid_repository(&mut self, repo_path: &str, cx: &mut Context<Self>) -> Task<bool> {
        let repo_path = repo_path.to_string();
        self.blocking(cx, move |backend| {
            backend.git().inspect_repository(&repo_path).valid
        })
    }

    /// `cloneRepository`. `auth_token` es el token del `authContext` del
    /// original.
    pub fn clone_repository(
        &mut self,
        remote_url: &str,
        target_path: &str,
        auth_token: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<CloneResult> {
        let spec = OperationSpec::new(
            "clone",
            target_path,
            json!([remote_url, target_path, { "authToken": auth_token }]),
        )
        .loading()
        .error(ErrorRule::SuccessOnly)
        .progress(format!(
            "Clone {}…",
            clone_name(&mergeon_gitcmd::credentials::redact_url_credentials(
                remote_url
            ))
        ));
        // Un remoto HTTPS que pide usuario y contraseña: se piden y se
        // reintenta (ver `credentials`). Quedan ligadas al destino.
        let target = target_path.to_string();
        let remote_url = remote_url.to_string();
        let target_path = target_path.to_string();
        let clone = move |this: &mut AppModel, cx: &mut Context<AppModel>| {
            let remote_url = remote_url.clone();
            let target_path = target_path.clone();
            let auth_token = auth_token.clone();
            this.invoke_structured_operation(spec.clone(), cx, move |backend| {
                backend
                    .git()
                    .clone(&remote_url, &target_path, auth_token.as_deref())
            })
        };
        self.with_http_credentials("clone", &target, &target, clone, cx)
    }

    // ------------------------------------------------------------------
    // Historial
    // ------------------------------------------------------------------

    /// `getCommits`.
    pub fn get_commits(
        &mut self,
        repo_path: &str,
        options: CommitLogOptions,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<Vec<Commit>>> {
        let repo_path = repo_path.to_string();
        self.git.loading += 1;
        self.git.error = None;
        cx.notify();
        let read = self.read(Some("Error loading commits"), cx, move |backend| {
            backend.git().get_commits(&repo_path, &options)
        });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            this.update(cx, |this, cx| {
                this.git.loading = this.git.loading.saturating_sub(1);
                cx.notify();
            })
            .ok();
            result
        })
    }

    /// `searchCommits`: búsqueda en el historial completo; quien llama decide
    /// cómo presentar el fallo.
    pub fn search_commits(
        &mut self,
        repo_path: &str,
        options: CommitSearchOptions,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<Vec<Commit>>> {
        let repo_path = repo_path.to_string();
        self.blocking(cx, move |backend| {
            backend.git().search_commits(&repo_path, &options)
        })
    }

    /// `resolveRef`: hash del commit de un ref.
    pub fn resolve_ref(
        &mut self,
        repo_path: &str,
        reference: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<String>> {
        let repo_path = repo_path.to_string();
        let reference = reference.to_string();
        self.blocking(cx, move |backend| {
            backend.git().resolve_ref(&repo_path, &reference)
        })
    }

    /// `getReflog`: reflog de HEAD.
    pub fn get_reflog(
        &mut self,
        repo_path: &str,
        options: ReflogOptions,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<Vec<ReflogEntry>>> {
        let repo_path = repo_path.to_string();
        self.blocking(cx, move |backend| {
            backend.git().get_reflog(&repo_path, &options)
        })
    }

    /// `getCommitFiles`.
    pub fn get_commit_files(
        &mut self,
        repo_path: &str,
        commit_hash: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<Vec<FileChange>>> {
        let repo_path = repo_path.to_string();
        let commit_hash = commit_hash.to_string();
        self.read(Some("Error loading commit files"), cx, move |backend| {
            backend.git().get_commit_files(&repo_path, &commit_hash)
        })
    }

    /// `getCommitDiff`.
    pub fn get_commit_diff(
        &mut self,
        repo_path: &str,
        commit_hash: &str,
        file_path: &str,
        options: DiffOptions,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<DiffPayload>> {
        let repo_path = repo_path.to_string();
        let commit_hash = commit_hash.to_string();
        let file_path = file_path.to_string();
        self.read(None, cx, move |backend| {
            backend
                .git()
                .get_commit_diff(&repo_path, &commit_hash, &file_path, &options)
        })
    }

    /// Las dos versiones de una imagen cambiada, para verlas lado a lado.
    /// Sin aviso: quien la pide enseña el error en su sitio.
    pub fn read_image_pair(
        &mut self,
        repo_path: &str,
        request: ImagePairRequest,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<ImagePair>> {
        let repo_path = repo_path.to_string();
        self.blocking(cx, move |backend| {
            backend.git().read_image_pair(&repo_path, &request)
        })
    }

    /// `getTreeEntries`: un nivel del árbol en ese commit. El error se
    /// devuelve como dato y no se avisa por pantalla: una carpeta que falla
    /// marca su rama, no interrumpe la navegación por el resto del árbol.
    pub fn get_tree_entries(
        &mut self,
        repo_path: &str,
        commit_hash: &str,
        directory: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<TreeEntries>> {
        let repo_path = repo_path.to_string();
        let commit_hash = commit_hash.to_string();
        let directory = directory.to_string();
        self.blocking(cx, move |backend| {
            backend
                .git()
                .get_tree_entries(&repo_path, &commit_hash, &directory)
        })
    }

    /// `getFileAtCommit`.
    pub fn get_file_at_commit(
        &mut self,
        repo_path: &str,
        commit_hash: &str,
        file_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<FileAtCommit>> {
        let repo_path = repo_path.to_string();
        let commit_hash = commit_hash.to_string();
        let file_path = file_path.to_string();
        self.blocking(cx, move |backend| {
            backend
                .git()
                .get_file_at_commit(&repo_path, &commit_hash, &file_path)
        })
    }

    /// `blame`.
    pub fn blame(
        &mut self,
        repo_path: &str,
        file_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<BlameResult>> {
        let repo_path = repo_path.to_string();
        let file_path = file_path.to_string();
        self.read(None, cx, move |backend| {
            backend.git().blame(&repo_path, &file_path)
        })
    }

    /// Blame del archivo en una revisión ("Blame previous revision"). El
    /// error se devuelve como dato: lo enseña el visor.
    pub fn blame_at_revision(
        &mut self,
        repo_path: &str,
        revision: &str,
        file_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<BlameResult>> {
        let repo_path = repo_path.to_string();
        let revision = revision.to_string();
        let file_path = file_path.to_string();
        self.blocking(cx, move |backend| {
            backend
                .git()
                .blame_at_revision(&repo_path, &revision, &file_path)
        })
    }

    /// Una página del historial de un archivo siguiendo sus renombres
    /// (pestaña "File history"). El error se devuelve como dato y no avisa al
    /// modelo: lo pinta la pestaña.
    pub fn get_file_history(
        &mut self,
        repo_path: &str,
        file_path: &str,
        options: FileHistoryOptions,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<FileHistoryPage>> {
        let repo_path = repo_path.to_string();
        let file_path = file_path.to_string();
        self.blocking(cx, move |backend| {
            backend
                .git()
                .get_file_history(&repo_path, &file_path, &options)
        })
    }

    /// El parche de un archivo en una de sus revisiones; con `previous_path`
    /// (la revisión lo renombró) se ve el renombre con sus cambios.
    pub fn get_file_revision_diff(
        &mut self,
        repo_path: &str,
        commit_hash: &str,
        file_path: &str,
        previous_path: Option<&str>,
        options: DiffOptions,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<DiffPayload>> {
        let repo_path = repo_path.to_string();
        let commit_hash = commit_hash.to_string();
        let file_path = file_path.to_string();
        let previous_path = previous_path.map(str::to_string);
        self.blocking(cx, move |backend| {
            backend.git().get_file_revision_diff(
                &repo_path,
                &commit_hash,
                &file_path,
                previous_path.as_deref(),
                &options,
            )
        })
    }

    /// `readFile`.
    pub fn read_file(
        &mut self,
        repo_path: &str,
        file_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<String>> {
        let repo_path = repo_path.to_string();
        let file_path = file_path.to_string();
        self.read(None, cx, move |backend| {
            backend.git().read_file(&repo_path, &file_path)
        })
    }

    /// Las tres versiones (etapas del índice) de un archivo en conflicto, para
    /// el resolvedor de tres vías.
    pub fn read_conflict_versions(
        &mut self,
        repo_path: &str,
        file_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<ConflictVersions>> {
        let repo_path = repo_path.to_string();
        let file_path = file_path.to_string();
        self.read(None, cx, move |backend| {
            backend.git().read_conflict_versions(&repo_path, &file_path)
        })
    }

    // ------------------------------------------------------------------
    // Identidad
    // ------------------------------------------------------------------

    /// `getIdentity`: identidad de commit (local y global).
    pub fn get_identity(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<GitIdentity>> {
        let repo_path = repo_path.to_string();
        self.blocking(cx, move |backend| backend.git().get_identity(&repo_path))
    }

    /// `setIdentity`.
    pub fn set_identity(
        &mut self,
        repo_path: &str,
        options: SetIdentityOptions,
        cx: &mut Context<Self>,
    ) -> Task<SetIdentityResult> {
        let repo_path = repo_path.to_string();
        let work = self.blocking(cx, move |backend| {
            backend.git().set_identity(&repo_path, &options)
        });
        flow(cx, async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                this.notify_operation_result(&result.to_operation_result(), None, cx);
            });
            result
        })
    }

    // ------------------------------------------------------------------
    // Estado y working tree
    // ------------------------------------------------------------------

    /// `getStatus`.
    pub fn get_status(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<StructuredStatus>> {
        let repo_path = repo_path.to_string();
        self.read(Some("Error loading status"), cx, move |backend| {
            backend.git().get_status(&repo_path)
        })
    }

    /// `getOperationState`.
    pub fn get_operation_state(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<OperationState>> {
        let repo_path = repo_path.to_string();
        self.read(Some("Error loading operation state"), cx, move |backend| {
            backend.git().get_operation_state(&repo_path)
        })
    }

    /// `hasUncommittedChanges`.
    pub fn has_uncommitted_changes(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<UncommittedChanges>> {
        let repo_path = repo_path.to_string();
        self.read(None, cx, move |backend| {
            backend.git().has_uncommitted_changes(&repo_path)
        })
    }

    /// `getDiff`.
    pub fn get_diff(
        &mut self,
        repo_path: &str,
        file_path: &str,
        is_staged: bool,
        is_untracked: bool,
        options: DiffOptions,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<DiffPayload>> {
        let repo_path = repo_path.to_string();
        let file_path = file_path.to_string();
        self.read(Some("Error loading diff"), cx, move |backend| {
            backend
                .git()
                .get_diff(&repo_path, &file_path, is_staged, is_untracked, &options)
        })
    }

    /// `stage`.
    pub fn stage(
        &mut self,
        repo_path: &str,
        file_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("stage", repo_path, json!([file_path])).loading();
        let repo_path = repo_path.to_string();
        let file_path = file_path.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().stage(&repo_path, &file_path)
        })
    }

    /// `stageAll`.
    pub fn stage_all(&mut self, repo_path: &str, cx: &mut Context<Self>) -> Task<GitMutation> {
        let spec = OperationSpec::new("stage-all", repo_path, json!([])).loading();
        let repo_path = repo_path.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().stage_all(&repo_path)
        })
    }

    /// `unstage`.
    pub fn unstage(
        &mut self,
        repo_path: &str,
        file_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("unstage", repo_path, json!([file_path])).loading();
        let repo_path = repo_path.to_string();
        let file_path = file_path.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().unstage(&repo_path, &file_path)
        })
    }

    /// `unstageAll`.
    pub fn unstage_all(&mut self, repo_path: &str, cx: &mut Context<Self>) -> Task<GitMutation> {
        let spec = OperationSpec::new("unstage-all", repo_path, json!([])).loading();
        let repo_path = repo_path.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().unstage_all(&repo_path)
        })
    }

    /// `discardChanges`. Guarda antes una copia para poder deshacerlo (ver
    /// [`crate::undo`]); si es demasiado grande, pregunta.
    pub fn discard_changes(
        &mut self,
        repo_path: &str,
        file_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("discard", repo_path, json!([file_path]))
            .loading()
            .undo(UndoIntent::Discard {
                label: format!("discard of changes in {file_path}"),
            });
        let repo = repo_path.to_string();
        let file_path = file_path.to_string();
        self.discard_with_backup(
            repo_path,
            Some(vec![file_path.clone()]),
            "discard",
            cx,
            move |this, backup, cx| {
                this.invoke_structured_operation(spec, cx, move |backend| {
                    backend
                        .git()
                        .discard_changes_backed_up(&repo, &file_path, backup)
                })
            },
        )
    }

    /// `discardAll`. `None` descarta todo; con rutas, solo esas. Con copia
    /// para deshacer, como [`AppModel::discard_changes`].
    pub fn discard_all(
        &mut self,
        repo_path: &str,
        file_paths: Option<Vec<String>>,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let label = match file_paths.as_deref() {
            Some([one]) => format!("discard of changes in {one}"),
            Some(paths) => format!("discard of changes in {} files", paths.len()),
            None => "discard of all changes".to_string(),
        };
        let spec = OperationSpec::new("discard-all", repo_path, json!([file_paths]))
            .loading()
            .undo(UndoIntent::Discard { label });
        let repo = repo_path.to_string();
        self.discard_with_backup(
            repo_path,
            file_paths.clone(),
            "discard-all",
            cx,
            move |this, backup, cx| {
                this.invoke_structured_operation(spec, cx, move |backend| {
                    backend
                        .git()
                        .discard_all_backed_up(&repo, file_paths.as_deref(), backup)
                })
            },
        )
    }

    /// `applyPatch`: aplica (o revierte) un bloque o unas líneas del diff
    /// sobre el índice; con `worktree`, sobre el working tree (descartar).
    pub fn apply_patch(
        &mut self,
        repo_path: &str,
        patch: &str,
        options: ApplyPatchOptions,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let operation = if options.worktree {
            "discard-hunk"
        } else if options.reverse {
            "unstage-hunk"
        } else {
            "stage-hunk"
        };
        let mut spec = OperationSpec::new(operation, repo_path, json!([patch, options]))
            .error(ErrorRule::Untouched);
        if options.worktree {
            // Descartar un bloque o unas líneas: el propio parche es la copia.
            let what = options
                .file_path
                .as_deref()
                .filter(|path| !path.is_empty())
                .map_or_else(String::new, |path| format!(" in {path}"));
            spec = spec.undo(UndoIntent::DiscardPatch {
                label: format!("discard of selected changes{what}"),
                patch: patch.to_string(),
                reverse: options.reverse,
            });
        }
        let repo_path = repo_path.to_string();
        let patch = patch.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().apply_patch(&repo_path, &patch, &options)
        })
    }

    /// `commit`. Con `silent` no avisa: quien llama presenta el resultado.
    pub fn commit(
        &mut self,
        repo_path: &str,
        message: &str,
        options: CommitOptions,
        silent: bool,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        // Enmendar y crear son operaciones distintas aunque compartan canal: si
        // no se distinguen, el gate las tomaría por la misma petición repetida.
        let spec = OperationSpec::new("commit", repo_path, json!([message, options]))
            .loading()
            .silent(silent)
            .undo(UndoIntent::Commit {
                message: message.to_string(),
                amend: options.amend,
                fixup: options.fixup.is_some(),
            });
        let repo_path = repo_path.to_string();
        let message = message.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().commit(&repo_path, &message, &options)
        })
    }

    /// `addToIgnore`.
    pub fn add_to_ignore(
        &mut self,
        repo_path: &str,
        file_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<Mutation<AddToIgnoreResult>> {
        let spec = OperationSpec::new("add-to-ignore", repo_path, json!([file_path]));
        let repo_path = repo_path.to_string();
        let file_path = file_path.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().add_to_ignore(&repo_path, &file_path)
        })
    }

    /// `resolveConflict`.
    pub fn resolve_conflict(
        &mut self,
        repo_path: &str,
        file_path: &str,
        content: &str,
        cx: &mut Context<Self>,
    ) -> Task<Mutation<ResolveConflictResult>> {
        let spec = OperationSpec::new("resolve-conflict", repo_path, json!([file_path, content]))
            .notify(NotifyMode::ResolveConflict)
            .error(ErrorRule::SuccessOnly);
        let repo_path = repo_path.to_string();
        let file_path = file_path.to_string();
        let content = content.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend
                .git()
                .resolve_conflict(&repo_path, &file_path, &content)
        })
    }

    /// [`Self::resolve_conflict`] con cualquier forma de resolución: el
    /// contenido, una versión entera o borrar el archivo.
    pub fn resolve_conflict_with(
        &mut self,
        repo_path: &str,
        file_path: &str,
        resolution: ConflictResolution,
        cx: &mut Context<Self>,
    ) -> Task<Mutation<ResolveConflictResult>> {
        let summary = match &resolution {
            ConflictResolution::Content(content) => json!({ "bytes": content.len() }),
            other => json!(other),
        };
        let spec = OperationSpec::new("resolve-conflict", repo_path, json!([file_path, summary]))
            .notify(NotifyMode::ResolveConflict)
            .error(ErrorRule::SuccessOnly);
        let repo_path = repo_path.to_string();
        let file_path = file_path.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend
                .git()
                .resolve_conflict_with(&repo_path, &file_path, &resolution)
        })
    }

    // ------------------------------------------------------------------
    // Ramas
    // ------------------------------------------------------------------

    /// `getBranches`.
    pub fn get_branches(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<BranchesOverview>> {
        let repo_path = repo_path.to_string();
        self.read(Some("Error loading branches"), cx, move |backend| {
            backend.git().get_branches(&repo_path)
        })
    }

    /// `checkout`: una rama local, un commit o cualquier ref.
    pub fn checkout(
        &mut self,
        repo_path: &str,
        branch_name: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("checkout", repo_path, json!([branch_name]))
            .loading()
            .undo(UndoIntent::Checkout {
                label: branch_name.to_string(),
            });
        let repo_path = repo_path.to_string();
        let branch_name = branch_name.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().checkout(&repo_path, &branch_name)
        })
    }

    /// `checkoutRemoteBranch`: checkout de una rama remota, con tracking.
    pub fn checkout_remote_branch(
        &mut self,
        repo_path: &str,
        remote_branch: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("checkout-remote-branch", repo_path, json!([remote_branch]))
            .loading()
            .undo(UndoIntent::CheckoutRemote {
                remote_branch: remote_branch.to_string(),
            });
        let repo_path = repo_path.to_string();
        let remote_branch = remote_branch.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend
                .git()
                .checkout_remote_branch(&repo_path, &remote_branch)
        })
    }

    /// `createBranch`.
    pub fn create_branch(
        &mut self,
        repo_path: &str,
        branch_name: &str,
        start_point: Option<&str>,
        checkout_after_create: bool,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new(
            "create-branch",
            repo_path,
            json!([branch_name, start_point, checkout_after_create]),
        )
        .loading()
        .undo(UndoIntent::CreateBranch {
            name: branch_name.to_string(),
        });
        let repo_path = repo_path.to_string();
        let branch_name = branch_name.to_string();
        let start_point = start_point.map(str::to_string);
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().create_branch(
                &repo_path,
                &branch_name,
                start_point.as_deref(),
                checkout_after_create,
            )
        })
    }

    /// `renameBranch`.
    pub fn rename_branch(
        &mut self,
        repo_path: &str,
        branch_name: &str,
        next_branch_name: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new(
            "rename-branch",
            repo_path,
            json!([branch_name, next_branch_name]),
        )
        .loading()
        .undo(UndoIntent::RenameBranch {
            from: branch_name.to_string(),
            to: next_branch_name.to_string(),
        });
        let repo_path = repo_path.to_string();
        let branch_name = branch_name.to_string();
        let next_branch_name = next_branch_name.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend
                .git()
                .rename_branch(&repo_path, &branch_name, &next_branch_name)
        })
    }

    /// `deleteBranch`.
    pub fn delete_branch(
        &mut self,
        repo_path: &str,
        branch_name: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("delete-branch", repo_path, json!([branch_name]))
            .loading()
            .undo(UndoIntent::DeleteBranch {
                name: branch_name.to_string(),
            });
        let repo_path = repo_path.to_string();
        let branch_name = branch_name.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().delete_branch(&repo_path, &branch_name)
        })
    }

    /// Ramas locales que se pueden limpiar (integradas en `base` o en la rama
    /// por defecto). Sin aviso si falla: el diálogo muestra el error.
    pub fn get_merged_branches(
        &mut self,
        repo_path: &str,
        base: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<MergedBranchesReport>> {
        let repo_path = repo_path.to_string();
        self.read(None, cx, move |backend| {
            backend
                .git()
                .get_merged_branches(&repo_path, base.as_deref())
        })
    }

    /// Limpieza en lote de ramas (diálogo "Clean up merged branches"). Con
    /// `delete_remote`, autentica con el token de la cuenta activa.
    pub fn cleanup_branches(
        &mut self,
        repo_path: &str,
        mut options: CleanupBranchesOptions,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        if options.delete_remote {
            options.auth_token = self.git_auth_token();
        }
        let spec = OperationSpec::new("cleanup-branches", repo_path, json!([options])).loading();
        let repo_path = repo_path.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().cleanup_branches(&repo_path, &options)
        })
    }

    /// `deleteRemoteBranch`.
    pub fn delete_remote_branch(
        &mut self,
        repo_path: &str,
        remote_name: &str,
        branch_name: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new(
            "delete-remote-branch",
            repo_path,
            json!([remote_name, branch_name]),
        )
        .loading();
        let repo_path = repo_path.to_string();
        let remote_name = remote_name.to_string();
        let branch_name = branch_name.to_string();
        // Como el resto de operaciones de red, con el token de la cuenta activa
        // (el original lo olvidaba aquí).
        let auth_token = self.git_auth_token();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().delete_remote_branch_with_auth(
                &repo_path,
                &remote_name,
                &branch_name,
                auth_token.as_deref(),
            )
        })
    }

    /// `fastForwardBranch`.
    pub fn fast_forward_branch(
        &mut self,
        repo_path: &str,
        branch_name: &str,
        upstream_branch: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new(
            "fast-forward-branch",
            repo_path,
            json!([branch_name, upstream_branch]),
        )
        .loading()
        .undo(UndoIntent::Pull {
            label: format!("fast-forward of '{branch_name}'"),
        });
        let repo_path = repo_path.to_string();
        let branch_name = branch_name.to_string();
        let upstream_branch = upstream_branch.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend
                .git()
                .fast_forward_branch(&repo_path, &branch_name, &upstream_branch)
        })
    }

    /// `setupTracking`.
    pub fn setup_tracking(
        &mut self,
        repo_path: &str,
        branch_name: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Task<Mutation<SetupTrackingResult>> {
        let spec = OperationSpec::new("setup-tracking", repo_path, json!([branch_name]))
            .notify(NotifyMode::SetupTracking);
        let repo_path = repo_path.to_string();
        let branch_name = branch_name.map(str::to_string);
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend
                .git()
                .setup_tracking(&repo_path, branch_name.as_deref())
        })
    }

    /// `compareBranches`.
    pub fn compare_branches(
        &mut self,
        repo_path: &str,
        branch1: &str,
        branch2: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<BranchComparison>> {
        let repo_path = repo_path.to_string();
        let branch1 = branch1.to_string();
        let branch2 = branch2.to_string();
        self.read(Some("Compare failed"), cx, move |backend| {
            backend
                .git()
                .compare_branches(&repo_path, &branch1, &branch2)
        })
    }

    /// Comparación navegable de dos revisiones (ramas, tags, SHAs): las fija
    /// a su SHA y trae commits y archivos según `mode`. No avisa ni toca el
    /// estado del modelo: la vista de la comparación presenta el fallo.
    pub fn compare_revisions(
        &mut self,
        repo_path: &str,
        base: &str,
        head: &str,
        mode: CompareMode,
        limits: CompareLimits,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<RevisionComparison>> {
        let (repo_path, base, head) = (repo_path.to_string(), base.to_string(), head.to_string());
        self.blocking(cx, move |backend| {
            backend
                .git()
                .compare_revisions(&repo_path, &base, &head, mode, limits)
        })
    }

    /// Los archivos de una comparación ya fijada en otro modo (sin avisar).
    pub fn get_comparison_files(
        &mut self,
        repo_path: &str,
        revisions: &ComparisonRevisions,
        mode: CompareMode,
        limit: usize,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<ComparisonFiles>> {
        let (repo_path, revisions) = (repo_path.to_string(), revisions.clone());
        self.blocking(cx, move |backend| {
            backend
                .git()
                .get_comparison_files(&repo_path, &revisions, mode, limit)
        })
    }

    /// Parche de un archivo de una comparación (sin avisar).
    pub fn get_comparison_diff(
        &mut self,
        repo_path: &str,
        revisions: &ComparisonRevisions,
        mode: CompareMode,
        file: &ComparedFile,
        options: DiffOptions,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<DiffPayload>> {
        let (repo_path, revisions, file) = (repo_path.to_string(), revisions.clone(), file.clone());
        self.blocking(cx, move |backend| {
            backend.git().get_comparison_diff(
                &repo_path,
                &revisions,
                mode,
                &file.path,
                file.old_path.as_deref(),
                &options,
            )
        })
    }

    /// `checkMergeConflicts`: predice conflictos de merge sin tocar el working
    /// tree.
    pub fn check_merge_conflicts(
        &mut self,
        repo_path: &str,
        source: &str,
        target: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<MergeConflictCheck>> {
        let repo_path = repo_path.to_string();
        let source = source.to_string();
        let target = target.to_string();
        self.read(None, cx, move |backend| {
            backend
                .git()
                .check_merge_conflicts(&repo_path, &source, &target)
        })
    }

    // ------------------------------------------------------------------
    // Operaciones de varios pasos
    // ------------------------------------------------------------------

    /// `merge`. La rama destino del original (`options.targetBranch`) va en
    /// `options.target_branch`.
    pub fn merge(
        &mut self,
        repo_path: &str,
        branch_name: &str,
        options: MergeOptions,
        silent: bool,
        cx: &mut Context<Self>,
    ) -> Task<GitOperation> {
        let spec = OperationSpec::new("merge", repo_path, json!([branch_name, options]))
            .loading()
            .silent(silent)
            .undo(UndoIntent::Sequence {
                label: format!("merge of {branch_name}"),
            });
        let repo_path = repo_path.to_string();
        let branch_name = branch_name.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().merge(&repo_path, &branch_name, &options)
        })
    }

    /// `rebase`.
    pub fn rebase(
        &mut self,
        repo_path: &str,
        branch_name: &str,
        options: RebaseOptions,
        cx: &mut Context<Self>,
    ) -> Task<GitOperation> {
        let spec = OperationSpec::new("rebase", repo_path, json!([branch_name, options]))
            .loading()
            .undo(UndoIntent::Sequence {
                label: format!("rebase onto {branch_name}"),
            });
        let repo_path = repo_path.to_string();
        let branch_name = branch_name.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().rebase(&repo_path, &branch_name, &options)
        })
    }

    /// `revert`.
    pub fn revert(
        &mut self,
        repo_path: &str,
        commit: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitOperation> {
        let spec = OperationSpec::new("revert", repo_path, json!([commit]))
            .loading()
            .undo(UndoIntent::Sequence {
                label: format!("revert of {}", short_sha(commit)),
            });
        let repo_path = repo_path.to_string();
        let commit = commit.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().revert(&repo_path, &commit)
        })
    }

    /// `cherryPick`.
    pub fn cherry_pick(
        &mut self,
        repo_path: &str,
        commit: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitOperation> {
        let spec = OperationSpec::new("cherry-pick", repo_path, json!([commit]))
            .loading()
            .undo(UndoIntent::Sequence {
                label: format!("cherry-pick of {}", short_sha(commit)),
            });
        let repo_path = repo_path.to_string();
        let commit = commit.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().cherry_pick(&repo_path, &commit)
        })
    }

    /// Cherry-pick de varios commits, en el orden dado (del más viejo al más
    /// nuevo). Se para en el primer conflicto (banner de la operación).
    pub fn cherry_pick_commits(
        &mut self,
        repo_path: &str,
        commits: &[String],
        cx: &mut Context<Self>,
    ) -> Task<GitOperation> {
        let spec = OperationSpec::new("cherry-pick", repo_path, json!([commits]))
            .loading()
            .undo(UndoIntent::Sequence {
                label: commits_label("cherry-pick", commits),
            });
        let repo_path = repo_path.to_string();
        let commits = commits.to_vec();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().cherry_pick_commits(&repo_path, &commits)
        })
    }

    /// Revert de varios commits, en el orden dado (del más nuevo al más
    /// viejo); con `single_commit`, todos en un único commit.
    pub fn revert_commits(
        &mut self,
        repo_path: &str,
        commits: &[String],
        single_commit: bool,
        cx: &mut Context<Self>,
    ) -> Task<GitOperation> {
        let spec = OperationSpec::new(
            "revert",
            repo_path,
            json!([commits, { "singleCommit": single_commit }]),
        )
        .loading()
        .undo(UndoIntent::Sequence {
            label: commits_label("revert", commits),
        });
        let repo_path = repo_path.to_string();
        let commits = commits.to_vec();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend
                .git()
                .revert_commits(&repo_path, &commits, single_commit)
        })
    }

    /// `reset`. `mode` es `soft`, `mixed` o `hard`.
    pub fn reset(
        &mut self,
        repo_path: &str,
        commit: &str,
        mode: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let discarded_changes = mode == "hard"
            && self
                .repositories
                .repositories
                .iter()
                .find(|repository| repository.path == repo_path)
                .and_then(|repository| repository.status.as_ref())
                .is_some_and(|status| {
                    status
                        .files
                        .iter()
                        .any(|file| !(file.index == "?" && file.working_dir == "?"))
                });
        let spec = OperationSpec::new("reset", repo_path, json!([commit, mode]))
            .loading()
            .undo(UndoIntent::HeadMove {
                label: format!("reset to {}", short_sha(commit)),
                mode: reset_undo_mode(mode),
                discarded_changes,
            });
        let repo_path = repo_path.to_string();
        let commit = commit.to_string();
        let mode = mode.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().reset(&repo_path, &commit, &mode)
        })
    }

    /// `interactiveRebase`.
    pub fn interactive_rebase(
        &mut self,
        repo_path: &str,
        options: InteractiveRebaseOptions,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("interactive-rebase", repo_path, json!([options]))
            .loading()
            .undo(UndoIntent::Sequence {
                label: "interactive rebase".to_string(),
            });
        let repo_path = repo_path.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().interactive_rebase(&repo_path, &options)
        })
    }

    /// `rebaseAction`: `drop`, `reword` o `squash` de un commit.
    pub fn rebase_action(
        &mut self,
        repo_path: &str,
        action: &str,
        commit_hash: &str,
        params: RebaseActionParams,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new(
            "rebase-action",
            repo_path,
            json!([action, commit_hash, params]),
        )
        .loading()
        .undo(UndoIntent::Sequence {
            label: format!("{action} of {}", short_sha(commit_hash)),
        });
        let repo_path = repo_path.to_string();
        let action = action.to_string();
        let commit_hash = commit_hash.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend
                .git()
                .rebase_action(&repo_path, &action, &commit_hash, &params)
        })
    }

    /// `sequenceAction`: `continue`, `abort` o `skip` de la operación en curso.
    pub fn sequence_action(
        &mut self,
        repo_path: &str,
        action: &str,
        cx: &mut Context<Self>,
    ) -> Task<Mutation<SequenceActionResult>> {
        let spec = OperationSpec::new("sequence-action", repo_path, json!([action]))
            .loading()
            .notify(NotifyMode::StatusFlow)
            .error(ErrorRule::SuccessOnly)
            .undo(UndoIntent::SequenceAction {
                action: action.to_string(),
            });
        let repo_path = repo_path.to_string();
        let action = action.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().sequence_action(&repo_path, &action)
        })
    }

    // ------------------------------------------------------------------
    // Remotos
    // ------------------------------------------------------------------

    /// `getRemotes`.
    pub fn get_remotes(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<Vec<Remote>>> {
        let repo_path = repo_path.to_string();
        self.read(Some("Error loading remotes"), cx, move |backend| {
            backend.git().get_remotes(&repo_path)
        })
    }

    /// `addRemote`.
    pub fn add_remote(
        &mut self,
        repo_path: &str,
        name: &str,
        url: &str,
        silent: bool,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("add-remote", repo_path, json!([name, url])).silent(silent);
        let repo_path = repo_path.to_string();
        let name = name.to_string();
        let url = url.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().add_remote(&repo_path, &name, &url)
        })
    }

    /// `removeRemote`.
    pub fn remove_remote(
        &mut self,
        repo_path: &str,
        name: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("remove-remote", repo_path, json!([name]));
        let repo_path = repo_path.to_string();
        let name = name.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().remove_remote(&repo_path, &name)
        })
    }

    /// `renameRemote`.
    pub fn rename_remote(
        &mut self,
        repo_path: &str,
        name: &str,
        new_name: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("rename-remote", repo_path, json!([name, new_name]));
        let repo_path = repo_path.to_string();
        let name = name.to_string();
        let new_name = new_name.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().rename_remote(&repo_path, &name, &new_name)
        })
    }

    /// `setRemoteUrl`.
    pub fn set_remote_url(
        &mut self,
        repo_path: &str,
        name: &str,
        url: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("set-remote-url", repo_path, json!([name, url]));
        let repo_path = repo_path.to_string();
        let name = name.to_string();
        let url = url.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().set_remote_url(&repo_path, &name, &url)
        })
    }

    /// `testRemoteConnection`.
    pub fn test_remote_connection(
        &mut self,
        repo_path: &str,
        url: &str,
        cx: &mut Context<Self>,
    ) -> Task<RemoteConnectionTest> {
        let repo_path = repo_path.to_string();
        let url = url.to_string();
        self.blocking(cx, move |backend| {
            backend.git().test_remote_connection(&repo_path, &url)
        })
    }

    /// `fetch`. Con `silent` no hay toasts (auto-fetch en segundo plano); quien
    /// llama decide si registra el resultado en el log de operaciones.
    pub fn fetch(
        &mut self,
        repo_path: &str,
        options: FetchOptions,
        silent: bool,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let target = if options.all_remotes {
            "all remotes"
        } else if options.remote.is_empty() {
            "origin"
        } else {
            options.remote.as_str()
        };
        let spec = OperationSpec::new("fetch", repo_path, json!([options]))
            .loading()
            .silent(silent)
            .progress(format!("Fetch {target}…"));
        let repo_path = repo_path.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().fetch(&repo_path, &options)
        })
    }

    /// Trae la rama de un pull request (`GitApi::fetch_pull_request`): como
    /// [`AppModel::fetch`], con su aviso de progreso y de resultado. Lo usa
    /// [`AppModel::fetch_pull_request`].
    pub fn fetch_pull_request_head(
        &mut self,
        repo_path: &str,
        number: u64,
        options: mergeon_backend::FetchPullRequestOptions,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("fetch", repo_path, json!(["pull-request", number]))
            .loading()
            .progress(format!("Fetch pull request #{number}…"));
        let repo_path = repo_path.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().fetch_pull_request(&repo_path, &options)
        })
    }

    /// `pull`.
    pub fn pull(
        &mut self,
        repo_path: &str,
        options: PullOptions,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let target = [options.remote.as_deref(), options.branch.as_deref()]
            .into_iter()
            .flatten()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("/");
        let spec = OperationSpec::new("pull", repo_path, json!([options]))
            .loading()
            .progress(if target.is_empty() {
                "Pull…".to_string()
            } else {
                format!("Pull {target}…")
            })
            .undo(UndoIntent::Pull {
                label: if target.is_empty() {
                    "pull".to_string()
                } else {
                    format!("pull from {target}")
                },
            });
        let repo_path = repo_path.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().pull(&repo_path, &options)
        })
    }

    /// `push`.
    pub fn push(
        &mut self,
        repo_path: &str,
        options: PushOptions,
        silent: bool,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("push", repo_path, json!([options]))
            .loading()
            .silent(silent)
            .progress(push_progress_title(&options));
        let repo_path = repo_path.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().push(&repo_path, &options)
        })
    }

    // ------------------------------------------------------------------
    // Tags
    // ------------------------------------------------------------------

    /// `getTags`.
    pub fn get_tags(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<Vec<String>>> {
        let repo_path = repo_path.to_string();
        self.read(Some("Error loading tags"), cx, move |backend| {
            backend.git().get_tags(&repo_path)
        })
    }

    /// `getTagDetails`: anotación, autor y commit destino de un tag.
    pub fn get_tag_details(
        &mut self,
        repo_path: &str,
        tag_name: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<TagDetails>> {
        let repo_path = repo_path.to_string();
        let tag_name = tag_name.to_string();
        self.read(Some("Error loading tag details"), cx, move |backend| {
            backend.git().get_tag_details(&repo_path, &tag_name)
        })
    }

    /// `createTag`. Sin `commit` el tag apunta a `HEAD`.
    pub fn create_tag(
        &mut self,
        repo_path: &str,
        tag_name: &str,
        commit: Option<&str>,
        message: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let commit = commit.unwrap_or("HEAD").to_string();
        let spec = OperationSpec::new("create-tag", repo_path, json!([tag_name, commit, message]))
            .undo(UndoIntent::CreateTag {
                name: tag_name.to_string(),
            });
        let repo_path = repo_path.to_string();
        let tag_name = tag_name.to_string();
        let message = message.map(str::to_string);
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend
                .git()
                .create_tag(&repo_path, &tag_name, Some(&commit), message.as_deref())
        })
    }

    /// `deleteTag`.
    pub fn delete_tag(
        &mut self,
        repo_path: &str,
        tag_name: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("delete-tag", repo_path, json!([tag_name])).undo(
            UndoIntent::DeleteTag {
                name: tag_name.to_string(),
            },
        );
        let repo_path = repo_path.to_string();
        let tag_name = tag_name.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().delete_tag(&repo_path, &tag_name)
        })
    }

    /// `pushTag`.
    pub fn push_tag(
        &mut self,
        repo_path: &str,
        tag_name: &str,
        options: RemoteTagOptions,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("push-tag", repo_path, json!([tag_name, options]))
            .error(ErrorRule::SuccessOnly);
        let repo_path = repo_path.to_string();
        let tag_name = tag_name.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().push_tag(&repo_path, &tag_name, &options)
        })
    }

    /// `deleteRemoteTag`.
    pub fn delete_remote_tag(
        &mut self,
        repo_path: &str,
        tag_name: &str,
        options: RemoteTagOptions,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("delete-remote-tag", repo_path, json!([tag_name, options]))
            .error(ErrorRule::SuccessOnly);
        let repo_path = repo_path.to_string();
        let tag_name = tag_name.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend
                .git()
                .delete_remote_tag(&repo_path, &tag_name, &options)
        })
    }

    /// `checkoutTag`.
    pub fn checkout_tag(
        &mut self,
        repo_path: &str,
        tag_name: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("checkout-tag", repo_path, json!([tag_name]))
            .loading()
            .undo(UndoIntent::Checkout {
                label: tag_name.to_string(),
            });
        let repo_path = repo_path.to_string();
        let tag_name = tag_name.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().checkout_tag(&repo_path, &tag_name)
        })
    }

    // ------------------------------------------------------------------
    // Stashes
    // ------------------------------------------------------------------

    /// `getStashes`.
    pub fn get_stashes(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<Vec<StashEntry>>> {
        let repo_path = repo_path.to_string();
        self.read(Some("Error loading stashes"), cx, move |backend| {
            backend.git().get_stashes(&repo_path)
        })
    }

    /// `getStashFiles`.
    pub fn get_stash_files(
        &mut self,
        repo_path: &str,
        stash_ref: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<Vec<FileChange>>> {
        let repo_path = repo_path.to_string();
        let stash_ref = stash_ref.to_string();
        self.read(Some("Error reading the stash"), cx, move |backend| {
            backend.git().get_stash_files(&repo_path, &stash_ref)
        })
    }

    /// `getStashDiff`.
    pub fn get_stash_diff(
        &mut self,
        repo_path: &str,
        stash_ref: &str,
        file_path: &str,
        options: DiffOptions,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<DiffPayload>> {
        let repo_path = repo_path.to_string();
        let stash_ref = stash_ref.to_string();
        let file_path = file_path.to_string();
        self.read(Some("Error reading the stash diff"), cx, move |backend| {
            backend
                .git()
                .get_stash_diff(&repo_path, &stash_ref, &file_path, &options)
        })
    }

    /// `stash`.
    pub fn stash(
        &mut self,
        repo_path: &str,
        message: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        self.stash_step(repo_path, message, false, cx)
    }

    /// `stash`; con `inner` es un paso de un flujo (guardar y devolver los
    /// cambios alrededor de un checkout) y no entra en la pila de deshacer.
    pub(crate) fn stash_step(
        &mut self,
        repo_path: &str,
        message: Option<&str>,
        inner: bool,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("stash", repo_path, json!([message]))
            .loading()
            .undo(UndoIntent::StashPush)
            .without_undo(inner);
        let repo_path = repo_path.to_string();
        let message = message.map(str::to_string);
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().stash(&repo_path, message.as_deref())
        })
    }

    /// `stash` con opciones: solo unas rutas, con lo que no se sigue,
    /// conservando el índice o solo lo preparado.
    pub fn stash_with_options(
        &mut self,
        repo_path: &str,
        options: StashOptions,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("stash", repo_path, json!([options]))
            .loading()
            .undo(UndoIntent::StashPush);
        let repo_path = repo_path.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().stash_with_options(&repo_path, &options)
        })
    }

    /// `applyStash`.
    pub fn apply_stash(
        &mut self,
        repo_path: &str,
        stash_ref: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("apply-stash", repo_path, json!([stash_ref]))
            .loading()
            .undo(UndoIntent::StashApply {
                stash_ref: stash_ref.to_string(),
                pop: false,
            });
        let repo_path = repo_path.to_string();
        let stash_ref = stash_ref.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().apply_stash(&repo_path, &stash_ref)
        })
    }

    /// `popStash`.
    pub fn pop_stash(
        &mut self,
        repo_path: &str,
        stash_ref: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        self.pop_stash_step(repo_path, stash_ref, false, cx)
    }

    /// `popStash`; con `inner`, sin entrada en la pila (ver
    /// [`AppModel::stash_step`]).
    pub(crate) fn pop_stash_step(
        &mut self,
        repo_path: &str,
        stash_ref: &str,
        inner: bool,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("pop-stash", repo_path, json!([stash_ref]))
            .loading()
            .undo(UndoIntent::StashApply {
                stash_ref: stash_ref.to_string(),
                pop: true,
            })
            .without_undo(inner);
        let repo_path = repo_path.to_string();
        let stash_ref = stash_ref.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().pop_stash(&repo_path, &stash_ref)
        })
    }

    /// `stashBranch`: crea una rama a partir de un stash.
    pub fn stash_branch(
        &mut self,
        repo_path: &str,
        stash_ref: &str,
        branch_name: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("stash-branch", repo_path, json!([stash_ref, branch_name]))
            .loading();
        let repo_path = repo_path.to_string();
        let stash_ref = stash_ref.to_string();
        let branch_name = branch_name.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend
                .git()
                .stash_branch(&repo_path, &stash_ref, &branch_name)
        })
    }

    /// `dropStash`.
    pub fn drop_stash(
        &mut self,
        repo_path: &str,
        stash_ref: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("drop-stash", repo_path, json!([stash_ref]))
            .loading()
            .undo(UndoIntent::StashDrop {
                stash_ref: stash_ref.to_string(),
            });
        let repo_path = repo_path.to_string();
        let stash_ref = stash_ref.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().drop_stash(&repo_path, &stash_ref)
        })
    }

    /// La mutación `undo` / `redo` (sin aviso: lo da [`AppModel::run_undo`]).
    pub(crate) fn undo_mutation(
        &mut self,
        repo_path: &str,
        label: &str,
        steps: Vec<UndoStep>,
        redo: bool,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let operation = if redo { "redo" } else { "undo" };
        let spec = OperationSpec::new(operation, repo_path, json!([label, redo]))
            .loading()
            .notify(NotifyMode::Silent);
        let repo_path = repo_path.to_string();
        let label = label.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().undo(&repo_path, &label, &steps, redo)
        })
    }

    // ------------------------------------------------------------------
    // Submódulos
    // ------------------------------------------------------------------

    /// Los worktrees del repositorio (sin avisar: la sección lo dice en línea).
    pub fn get_worktrees(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<Vec<Worktree>>> {
        let repo_path = repo_path.to_string();
        self.blocking(cx, move |backend| backend.git().get_worktrees(&repo_path))
    }

    /// Si un worktree tiene cambios sin commitear (lectura barata, sin avisar).
    pub fn worktree_has_changes(
        &mut self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<bool>> {
        let path = path.to_string();
        self.blocking(cx, move |backend| backend.git().worktree_has_changes(&path))
    }

    /// `git worktree add`.
    pub fn add_worktree(
        &mut self,
        repo_path: &str,
        options: AddWorktreeOptions,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("worktree-add", repo_path, json!([options])).loading();
        let repo_path = repo_path.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().add_worktree(&repo_path, &options)
        })
    }

    /// `git worktree remove`; con `force`, aunque tenga cambios.
    pub fn remove_worktree(
        &mut self,
        repo_path: &str,
        path: &str,
        force: bool,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("worktree-remove", repo_path, json!([path, force])).loading();
        let (repo_path, path) = (repo_path.to_string(), path.to_string());
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().remove_worktree(&repo_path, &path, force)
        })
    }

    /// `git worktree lock` / `unlock`.
    pub fn set_worktree_locked(
        &mut self,
        repo_path: &str,
        path: &str,
        lock: bool,
        reason: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let operation = if lock {
            "worktree-lock"
        } else {
            "worktree-unlock"
        };
        let spec = OperationSpec::new(operation, repo_path, json!([path, reason]));
        let (repo_path, path) = (repo_path.to_string(), path.to_string());
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend
                .git()
                .set_worktree_locked(&repo_path, &path, lock, reason.as_deref())
        })
    }

    /// `git worktree prune`.
    pub fn prune_worktrees(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("worktree-prune", repo_path, json!([]));
        let repo_path = repo_path.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().prune_worktrees(&repo_path)
        })
    }

    /// `getSubmodules`.
    pub fn get_submodules(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<QueryResult<Vec<Submodule>>> {
        let repo_path = repo_path.to_string();
        self.read(Some("Error loading submodules"), cx, move |backend| {
            backend.git().get_submodules(&repo_path)
        })
    }

    /// `updateSubmodules`.
    pub fn update_submodules(
        &mut self,
        repo_path: &str,
        options: SubmoduleUpdateOptions,
        cx: &mut Context<Self>,
    ) -> Task<GitMutation> {
        let spec = OperationSpec::new("submodule-update", repo_path, json!([options]));
        let repo_path = repo_path.to_string();
        self.invoke_structured_operation(spec, cx, move |backend| {
            backend.git().submodule_update(&repo_path, &options)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_keys_sort_object_keys_and_hide_secrets() {
        let key = build_shared_operation_key(
            "fetch",
            "/tmp/demo",
            &json!([{ "remote": "origin", "authToken": "abc", "allRemotes": false }]),
        );

        assert_eq!(
            key,
            r#"fetch:/tmp/demo:[{"allRemotes":false,"authToken":"[fingerprint:1a47e90b]","remote":"origin"}]"#
        );
    }

    #[test]
    fn different_requests_get_different_keys() {
        let stage = |file: &str| build_shared_operation_key("stage", "/repo", &json!([file]));

        assert_eq!(stage("a.txt"), stage("a.txt"));
        assert_ne!(stage("a.txt"), stage("b.txt"));
        assert_ne!(
            build_shared_operation_key("commit", "/repo", &json!(["msg", { "amend": false }])),
            build_shared_operation_key("commit", "/repo", &json!(["msg", { "amend": true }]))
        );
    }
}
