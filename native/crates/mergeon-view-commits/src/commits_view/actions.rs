//! Acciones sobre un commit del historial: el menú contextual de
//! `CommitView.vue` y lo que hace cada entrada (checkout, crear rama o tag,
//! squash, reword, drop, rebase interactivo, cherry-pick, revert, reset soft,
//! mixed o hard, copiar el hash y copiar el enlace de GitHub), con sus
//! confirmaciones y la relectura posterior.

use gpui_kit::{App, AsyncApp, Context, Entity, Task, WeakEntity, Window};
use mergeon_view_sidebar::ComparisonRequest;

use crate::compare_view::{CompareViewEvent, open_compare_view};
use crate::history_list::row_key;
use mergeon_app::{
    AppModel, CheckoutRequest, CheckoutTarget, CreateBranchRequest, FlowStatus, LocalChangesMode,
    StructuredOutcome, SyncRequest,
};
use mergeon_backend::{
    CommitOptions, InteractiveRebaseOptions, RebaseActionParams, RebaseTodoEntry,
};
use mergeon_model::commit_view_utils::{
    CommitRow, RefType, interactive_rebase_range, normalize_commit_view_branch,
};
use mergeon_model::dialogs::{ConfirmOptions, PromptOptions};
use mergeon_model::operation_types::OperationResult;
use mergeon_model::repositories::RepoLifecycleState;
use mergeon_model::workspace_tabs::WorkspaceSection;
use mergeon_ui::Lucide;
use mergeon_ui::menu::MenuItem;

use super::{
    CommitLoadOptions, CommitsView, CommitsViewEvent, PendingCheckout, PendingCreateBranch,
    RebaseSession, RepoRef,
};
use crate::checkout::{
    BRANCH_TARGET_PREFIX, CheckoutDialogRequest, CheckoutTargetKind, CreateBranchDialogRequest,
    build_checkout_targets,
};
use mergeon_view_sidebar::dialogs::{
    CheckoutBranchDialogEvent, CheckoutBranchOptions, CheckoutTargetOption as DialogTargetOption,
    CreateBranchDialogEvent, CreateBranchOptions, open_checkout_branch_dialog,
    open_create_branch_dialog,
};

use crate::interactive_rebase::{
    InteractiveRebaseDialogEvent, RebaseCommit, open_interactive_rebase_dialog,
};

/// Una acción del menú contextual de un commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommitCommand {
    Checkout,
    CreateBranch,
    CreateTag,
    Squash,
    Reword,
    Drop,
    InteractiveRebase,
    /// "Create fixup for this commit": `git commit --fixup=<sha>` con lo
    /// preparado.
    CreateFixup,
    /// "Autosquash fixups": `git rebase -i --autosquash` desde el padre del
    /// commit, sin editor.
    Autosquash,
    CherryPick,
    Revert,
    ResetSoft,
    ResetMixed,
    ResetHard,
    CopySha,
    /// "Copy GitHub link": `https://github.com/<owner>/<repo>/commit/<sha>`.
    /// Solo está en el menú si el repositorio tiene un remoto de GitHub.
    CopyGitHubLink,
}

/// Una entrada del menú contextual, tal como se pinta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct CommitMenuEntry {
    pub command: CommitCommand,
    pub label: &'static str,
    pub icon: Lucide,
    pub danger: bool,
    pub disabled: bool,
}

/// `contextMenuItems`: las entradas del menú de un commit, por grupos (entre
/// grupo y grupo va un separador). `current_branch` es la rama en la que está
/// el repositorio; `on_github` dice si tiene un remoto de GitHub (y con él la
/// entrada "Copy GitHub link").
pub fn commit_menu(
    commit: &CommitRow,
    current_branch: &str,
    on_github: bool,
) -> Vec<Vec<CommitMenuEntry>> {
    let is_current = is_checkout_disabled(commit, current_branch);
    let has_parent_commit = !commit.parents.trim().is_empty();
    let entry = |command, label, icon| CommitMenuEntry {
        command,
        label,
        icon,
        danger: false,
        disabled: false,
    };

    vec![
        vec![
            CommitMenuEntry {
                disabled: is_current,
                ..entry(CommitCommand::Checkout, "Checkout", Lucide::GitBranch)
            },
            entry(
                CommitCommand::CreateBranch,
                "Create Branch",
                Lucide::GitCommitVertical,
            ),
            entry(CommitCommand::CreateTag, "Create Tag…", Lucide::Tag),
        ],
        vec![
            CommitMenuEntry {
                disabled: !has_parent_commit,
                ..entry(
                    CommitCommand::Squash,
                    "Squash into parent",
                    Lucide::HardDriveDownload,
                )
            },
            entry(CommitCommand::Reword, "Reword", Lucide::Pencil),
            CommitMenuEntry {
                danger: true,
                ..entry(CommitCommand::Drop, "Drop", Lucide::Trash)
            },
            CommitMenuEntry {
                disabled: commit.in_current_branch == Some(false),
                ..entry(
                    CommitCommand::InteractiveRebase,
                    "Interactive rebase from here…",
                    Lucide::ListOrdered,
                )
            },
        ],
        vec![
            CommitMenuEntry {
                disabled: commit.in_current_branch == Some(false),
                ..entry(
                    CommitCommand::CreateFixup,
                    "Create fixup for this commit",
                    Lucide::Wrench,
                )
            },
            CommitMenuEntry {
                disabled: commit.in_current_branch == Some(false),
                ..entry(
                    CommitCommand::Autosquash,
                    "Autosquash fixups",
                    Lucide::Combine,
                )
            },
        ],
        vec![
            entry(CommitCommand::CherryPick, "Cherry-pick", Lucide::Cherry),
            entry(CommitCommand::Revert, "Revert", Lucide::RotateCcw),
        ],
        vec![
            entry(
                CommitCommand::ResetSoft,
                "Reset (Soft)",
                Lucide::RotateCcwClock,
            ),
            entry(
                CommitCommand::ResetMixed,
                "Reset (Mixed)",
                Lucide::GitPullRequestArrow,
            ),
            CommitMenuEntry {
                danger: true,
                ..entry(
                    CommitCommand::ResetHard,
                    "Reset (Hard)",
                    Lucide::TriangleAlert,
                )
            },
        ],
        std::iter::once(entry(CommitCommand::CopySha, "Copy SHA", Lucide::Copy))
            .chain(on_github.then(|| {
                entry(
                    CommitCommand::CopyGitHubLink,
                    "Copy GitHub link",
                    Lucide::Link2,
                )
            }))
            .collect(),
    ]
}

/// "Checkout" desactivado: la rama en la que está el repositorio ya apunta a
/// este commit. Es lo mismo que decide [`commit_menu`], sin construir el menú
/// (el inspector lo pregunta en cada frame).
pub fn is_checkout_disabled(commit: &CommitRow, current_branch: &str) -> bool {
    commit
        .refs
        .iter()
        .any(|commit_ref| commit_ref.kind == RefType::Local && commit_ref.name == current_branch)
}

/// El menú contextual de un commit, listo para `mergeon_ui::menu`.
pub(super) fn menu_items(
    view: &WeakEntity<CommitsView>,
    commit: &CommitRow,
    cx: &App,
) -> Vec<MenuItem> {
    let (current_branch, on_github, has_staged) = view
        .upgrade()
        .map(|view| {
            let view = view.read(cx);
            (
                view.current_branch.clone(),
                view.github_web_url(cx).is_some(),
                view.has_staged_changes(cx),
            )
        })
        .unwrap_or_default();

    // Clic derecho sobre uno de los commits de la selección múltiple: las
    // acciones en lote. Fuera de ella, el menú de ese commit, como siempre.
    let batch = view.upgrade().and_then(|view| {
        let view = view.read(cx);
        (view.multi.is_multi() && view.multi.contains(row_key(commit))).then(|| view.multi.len())
    });
    if let Some(count) = batch {
        return batch_menu_items(view, count);
    }

    let mut items = vec![MenuItem::header(commit_menu_header(commit))];
    for (index, group) in commit_menu(commit, &current_branch, on_github)
        .into_iter()
        .enumerate()
    {
        if index > 0 {
            items.push(MenuItem::separator());
        }
        for entry in group {
            let view = view.clone();
            let commit = commit.clone();
            // Un fixup se hace con lo preparado: sin nada, no hay qué hacer.
            let disabled =
                entry.disabled || (entry.command == CommitCommand::CreateFixup && !has_staged);
            let mut item = MenuItem::new(entry.label)
                .icon(entry.icon)
                .disabled(disabled);
            if let Some(key) = commit_menu_access_key(entry.command) {
                item = item.shortcut(key);
            }
            let item = item.on_click(move |window, cx| {
                let _ = view.update(cx, |view, cx| {
                    view.run_command(entry.command, &commit, window, cx);
                });
            });
            items.push(if entry.danger { item.danger() } else { item });
        }
    }
    items
}

/// Una acción del menú de varios commits seleccionados.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BatchCommand {
    /// `git cherry-pick a b c`, del más viejo al más nuevo.
    CherryPick,
    /// `git revert --no-edit c b a`, del más nuevo al más viejo: un commit de
    /// revert por cada uno.
    Revert,
    /// Los mismos reverts en un único commit (`--no-commit` y un commit).
    RevertAsOne,
    /// La comparación navegable de los dos (solo con dos seleccionados).
    CompareTwo,
    /// Los SHA completos, uno por línea, del más viejo al más nuevo.
    CopyShas,
}

/// Una entrada del menú de varios commits.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BatchMenuEntry {
    pub command: BatchCommand,
    pub label: String,
    pub icon: Lucide,
}

/// El menú de `count` commits seleccionados, por grupos.
pub fn batch_menu(count: usize) -> Vec<Vec<BatchMenuEntry>> {
    let entry = |command, label: String, icon| BatchMenuEntry {
        command,
        label,
        icon,
    };
    let mut groups = vec![vec![
        entry(
            BatchCommand::CherryPick,
            format!("Cherry-pick {count} commits"),
            Lucide::Cherry,
        ),
        entry(
            BatchCommand::Revert,
            format!("Revert {count} commits"),
            Lucide::RotateCcw,
        ),
        entry(
            BatchCommand::RevertAsOne,
            format!("Revert {count} commits as one commit"),
            Lucide::RotateCcw,
        ),
    ]];
    if count == 2 {
        groups.push(vec![entry(
            BatchCommand::CompareTwo,
            "Compare these two".to_string(),
            Lucide::GitCompareArrows,
        )]);
    }
    groups.push(vec![entry(
        BatchCommand::CopyShas,
        "Copy SHAs".to_string(),
        Lucide::Copy,
    )]);
    groups
}

/// El menú de la selección múltiple, listo para `mergeon_ui::menu`.
pub(super) fn batch_menu_items(view: &WeakEntity<CommitsView>, count: usize) -> Vec<MenuItem> {
    let mut items = vec![MenuItem::header(format!("{count} commits selected"))];
    for (index, group) in batch_menu(count).into_iter().enumerate() {
        if index > 0 {
            items.push(MenuItem::separator());
        }
        for entry in group {
            let view = view.clone();
            let mut item = MenuItem::new(entry.label).icon(entry.icon);
            if let Some(key) = batch_menu_access_key(entry.command) {
                item = item.shortcut(key);
            }
            items.push(item.on_click(move |window, cx| {
                let _ = view.update(cx, |view, cx| {
                    view.run_batch_command(entry.command, window, cx);
                });
            }));
        }
    }
    items
}

/// Letra de las entradas del menú de varios commits (las mismas que con uno).
pub fn batch_menu_access_key(command: BatchCommand) -> Option<&'static str> {
    match command {
        BatchCommand::CherryPick => Some("C"),
        BatchCommand::Revert => Some("R"),
        _ => None,
    }
}

/// Cabecera del menú de un commit: hash corto y asunto recortado
/// (`c4e1a07 · fix(cart): …`; el menú la pasa a mayúsculas).
pub fn commit_menu_header(commit: &CommitRow) -> String {
    const MAX_SUBJECT: usize = 32;
    let hash: String = if commit.hash.is_empty() {
        commit.full_hash.chars().take(7).collect()
    } else {
        commit.hash.clone()
    };
    let subject = commit.message.lines().next().unwrap_or_default().trim();
    let subject = if subject.chars().count() > MAX_SUBJECT {
        let head: String = subject.chars().take(MAX_SUBJECT - 1).collect();
        format!("{}…", head.trim_end())
    } else {
        subject.to_string()
    };
    if subject.is_empty() {
        hash
    } else {
        format!("{hash} · {subject}")
    }
}

/// Letra que ejecuta la entrada con el menú abierto (diseño Señal).
pub fn commit_menu_access_key(command: CommitCommand) -> Option<&'static str> {
    match command {
        CommitCommand::CreateBranch => Some("B"),
        CommitCommand::CherryPick => Some("C"),
        CommitCommand::Revert => Some("R"),
        CommitCommand::CreateTag => Some("T"),
        _ => None,
    }
}

/// `payload` de `commit-created`: lo que la vista de cambios locales cuenta
/// tras crear un commit.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct CommitCreated {
    /// Resultado de la operación de commit.
    pub result: Option<OperationResult>,
    /// Quien creó el commit ya aplicó el resultado y releyó el repositorio.
    pub sync_already_applied: bool,
}

impl CommitCreated {
    pub fn new(result: impl StructuredOutcome) -> Self {
        Self {
            result: Some(result.to_operation_result()),
            sync_already_applied: false,
        }
    }

    pub fn sync_already_applied(mut self, applied: bool) -> Self {
        self.sync_already_applied = applied;
        self
    }
}

/// Asunto de un commit que `--autosquash` mueve y combina.
pub fn is_autosquash_subject(subject: &str) -> bool {
    ["fixup! ", "squash! ", "amend! "]
        .iter()
        .any(|prefix| subject.starts_with(prefix))
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// `shouldSyncAfterMutation`.
fn should_sync_after_mutation(result: &OperationResult) -> bool {
    result.success || matches!(result.outcome.as_deref(), Some("noop" | "conflicted"))
}

/// Lo que un flujo de mutación necesita entre sus esperas.
#[derive(Clone)]
struct Flow {
    view: WeakEntity<CommitsView>,
    model: Entity<AppModel>,
    repo: RepoRef,
}

impl Flow {
    /// `applyOperationResult`.
    fn apply_result(&self, result: &impl StructuredOutcome, cx: &mut AsyncApp) {
        self.model.update(cx, |model, cx| {
            model.apply_repository_operation_result(self.repo.id.as_str(), result, cx);
        });
    }

    /// `syncRepositoryReadModel` de la vista: relee lo pedido y se queda con
    /// la rama actual que traiga la relectura.
    async fn sync(&self, request: SyncRequest, cx: &mut AsyncApp) {
        let outcome = self
            .model
            .update(cx, |model, cx| {
                model.sync_repository_read_model(self.repo.id.as_str(), request, cx)
            })
            .await;
        let Some(outcome) = outcome else {
            return;
        };

        let current = match (&outcome.status, &outcome.branch_data) {
            (Some(status), _) => Some(status.current.clone()),
            (None, Some(branch_data)) => Some(Some(branch_data.current.clone())),
            (None, None) => None,
        };
        if let Some(current) = current {
            let _ = self.view.update(cx, |view, cx| {
                view.current_branch = normalize_commit_view_branch(current.as_deref()).to_string();
                cx.notify();
            });
        }
    }

    /// La relectura habitual tras una mutación del historial: estado (si el
    /// resultado no lo trae) y ramas.
    async fn sync_after(&self, result: &OperationResult, cx: &mut AsyncApp) {
        let mut request = SyncRequest::new()
            .result(result.clone())
            .status(result.status.is_none())
            .branches(true);
        if let Some(operation) = result.operation.clone() {
            request = request.operation(operation);
        }
        self.sync(request, cx).await;
    }

    async fn reload(&self, options: CommitLoadOptions, cx: &mut AsyncApp) {
        if let Ok(load) = self
            .view
            .update(cx, |view, cx| view.load_commits(options, cx))
        {
            load.await;
        }
    }

    /// `loadCommits({ preferredBranch: activeRepo.currentBranch || currentBranch, forceDetailsReload: true })`.
    async fn reload_current_branch(&self, cx: &mut AsyncApp) {
        let preferred_branch = self
            .view
            .update(cx, |view, cx| view.branch_for_reload(cx))
            .ok()
            .flatten();
        self.reload(
            CommitLoadOptions {
                preferred_branch,
                force_details_reload: true,
                ..Default::default()
            },
            cx,
        )
        .await;
    }
}

impl CommitsView {
    /// Ejecuta una acción del menú contextual sobre un commit.
    pub fn run_command(
        &mut self,
        command: CommitCommand,
        commit: &CommitRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            CommitCommand::Checkout => self.handle_history_checkout_request(commit.clone(), cx),
            CommitCommand::CreateBranch => self.handle_create_branch(commit.clone(), cx),
            CommitCommand::CreateTag => self.handle_create_tag(commit.clone(), cx),
            CommitCommand::Squash => self.handle_squash(commit.clone(), cx),
            CommitCommand::Reword => self.handle_reword(commit.clone(), cx),
            CommitCommand::Drop => self.handle_drop(commit.clone(), cx),
            CommitCommand::InteractiveRebase => self.open_interactive_rebase(commit, window, cx),
            CommitCommand::CreateFixup => self.handle_create_fixup(commit.clone(), cx),
            CommitCommand::Autosquash => self.handle_autosquash(commit.clone(), cx),
            CommitCommand::CherryPick => self.handle_cherry_pick(commit.clone(), cx),
            CommitCommand::Revert => self.handle_revert(commit.clone(), cx),
            CommitCommand::ResetSoft => self.handle_reset(commit.clone(), "soft", cx),
            CommitCommand::ResetMixed => self.handle_reset(commit.clone(), "mixed", cx),
            CommitCommand::ResetHard => self.handle_reset(commit.clone(), "hard", cx),
            CommitCommand::CopySha => self.copy_to_clipboard(&commit.full_hash, cx),
            CommitCommand::CopyGitHubLink => self.copy_github_commit_link(commit, cx),
        }
    }

    /// `runCommitMutation`: una sola mutación de cada tipo a la vez por
    /// repositorio.
    fn run_commit_mutation(
        &mut self,
        operation: &str,
        cx: &mut Context<Self>,
        task: impl AsyncFnOnce(Flow, &mut AsyncApp) + 'static,
    ) -> Task<()> {
        let Some(repo) = self.active_repo(cx) else {
            return Task::ready(());
        };
        let flow = Flow {
            view: cx.weak_entity(),
            model: self.model.clone(),
            repo: repo.clone(),
        };
        self.model.update(cx, |model, cx| {
            model.run_repo_operation_single_flight(&repo.id, operation, cx, move |_, cx| {
                cx.spawn(async move |_, cx| task(flow, cx).await)
            })
        })
    }

    // ------------------------------------------------------------------
    // Checkout
    // ------------------------------------------------------------------

    /// `handleHistoryCheckoutRequest`: doble clic (o "Checkout" del menú) en
    /// una fila del historial.
    ///
    /// Solo se pide el diálogo cuando hay algo que decidir: qué hacer con los
    /// cambios sin commitear, entre qué ramas elegir, o si crear la rama que
    /// trackea la remota / desanclar HEAD. Cambiarse a una rama que ya se
    /// tiene en local, con el working tree limpio, no pregunta nada.
    pub fn handle_history_checkout_request(&mut self, commit: CommitRow, cx: &mut Context<Self>) {
        let is_current = commit.refs.iter().any(|commit_ref| {
            commit_ref.kind == RefType::Local && commit_ref.name == self.current_branch
        });
        if is_current {
            return;
        }
        self.request_checkout(commit, None, cx);
    }

    /// Doble clic en la etiqueta de una rama del historial: checkout de esa
    /// rama (local, o la local que sigue a la remota) por el mismo flujo que
    /// el doble clic en una fila: directo sin cambios locales, con el diálogo
    /// si los hay, y una rama remota nueva siempre con el diálogo que confirma
    /// que se creará la local (como el doble clic en el sidebar).
    pub fn handle_ref_checkout_request(
        &mut self,
        commit: CommitRow,
        name: &str,
        kind: RefType,
        cx: &mut Context<Self>,
    ) {
        let known_local_branches = self.known_local_branches(cx);
        let Some(target) = crate::checkout::ref_checkout_target(
            name,
            kind,
            &self.current_branch,
            &known_local_branches,
        ) else {
            let branch = match kind {
                RefType::Remote => name.split_once('/').map_or(name, |(_, branch)| branch),
                _ => name,
            };
            if !branch.is_empty() && branch == self.current_branch {
                self.model.update(cx, |model, cx| {
                    model.notify_info(&format!("You are already on '{branch}'."), cx);
                });
            }
            return;
        };
        self.request_checkout(commit, Some(vec![target]), cx);
    }

    /// Mira si hay una operación a medias y si hay cambios locales, y hace el
    /// checkout (directo a una sola rama sin cambios) o abre el diálogo.
    /// `targets`: los destinos a ofrecer; sin ellos, los del commit.
    fn request_checkout(
        &mut self,
        commit: CommitRow,
        targets: Option<Vec<crate::checkout::CheckoutTargetOption>>,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.active_repo(cx) else {
            return;
        };

        let busy_operation =
            self.model
                .read(cx)
                .repository(repo.id.as_str())
                .and_then(|repository| {
                    (repository.repo_lifecycle_state != RepoLifecycleState::Idle).then(|| {
                        repository
                            .operation_state
                            .as_ref()
                            .map(|operation| operation.label.clone())
                            .filter(|label| !label.is_empty())
                            .unwrap_or_else(|| "Git operation".to_string())
                    })
                });
        if let Some(operation_label) = busy_operation {
            self.model.update(cx, |model, cx| {
                model.notify_warning(
                    &format!(
                        "Resolve or abort the current {} before switching branches.",
                        operation_label.to_lowercase()
                    ),
                    cx,
                );
            });
            return;
        }

        let read = self.model.update(cx, |model, cx| {
            model.has_uncommitted_changes(&repo.path, cx)
        });
        Self::flow(cx, async move |this, cx| {
            let local_changes = read.await;
            let _ = this.update(cx, |this, cx| {
                let local_changes = match local_changes {
                    Ok(local_changes) => local_changes,
                    Err(error) => {
                        notify_inspection_failure(&this.model, &error, cx);
                        return;
                    }
                };

                let has_local_changes = local_changes.has_changes;
                let targets = targets.unwrap_or_else(|| this.build_checkout_targets(&commit, cx));
                let mut branch_targets = targets
                    .iter()
                    .filter(|target| target.value.starts_with(BRANCH_TARGET_PREFIX));

                if let (Some(only), None) = (branch_targets.next(), branch_targets.next())
                    && !has_local_changes
                {
                    let target = only.value.clone();
                    this.perform_history_checkout(commit, &target, LocalChangesMode::Keep, cx);
                    return;
                }

                let request = CheckoutDialogRequest {
                    commit_hash: commit.full_hash.clone(),
                    targets,
                    has_local_changes,
                };
                this.show_checkout_dialog(commit, request, cx);
            });
        })
        .detach();
    }

    /// Abre el diálogo de checkout (`CheckoutBranchDialog`) para esos
    /// destinos y espera su respuesta.
    fn show_checkout_dialog(
        &mut self,
        commit: CommitRow,
        request: CheckoutDialogRequest,
        cx: &mut Context<Self>,
    ) {
        let options = checkout_dialog_options(&request);
        self.pending_checkout = Some(PendingCheckout {
            commit,
            request,
            dialog: None,
        });
        cx.notify();

        let window = self.window;
        cx.spawn(async move |this, cx| {
            let Ok(dialog) = window.update(cx, |_, window, cx| {
                open_checkout_branch_dialog(options, window, cx)
            }) else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                let subscription =
                    cx.subscribe(&dialog, |this, _, event: &CheckoutBranchDialogEvent, cx| {
                        match event {
                            CheckoutBranchDialogEvent::Submit {
                                local_changes_mode,
                                target,
                            } => {
                                let target = target.as_ref().map(|target| target.to_string());
                                this.submit_checkout_dialog(
                                    *local_changes_mode,
                                    target.as_deref(),
                                    cx,
                                );
                            }
                            CheckoutBranchDialogEvent::Close => this.close_checkout_dialog(cx),
                        }
                    });
                match &mut this.pending_checkout {
                    Some(pending) => pending.dialog = Some((dialog, subscription)),
                    // Se respondió antes de poder escucharlo: nada que esperar.
                    None => drop(subscription),
                }
            });
        })
        .detach();
    }

    /// Abre el diálogo "Create Branch" con el commit como punto de partida.
    fn show_create_branch_dialog(
        &mut self,
        commit: CommitRow,
        request: CreateBranchDialogRequest,
        cx: &mut Context<Self>,
    ) {
        let options = CreateBranchOptions::new(request.base_label.clone())
            .has_local_changes(request.has_local_changes);
        self.pending_create_branch = Some(PendingCreateBranch {
            commit,
            request,
            dialog: None,
        });
        cx.notify();

        let window = self.window;
        cx.spawn(async move |this, cx| {
            let Ok(dialog) = window.update(cx, |_, window, cx| {
                open_create_branch_dialog(options, window, cx)
            }) else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                let subscription = cx.subscribe(
                    &dialog,
                    |this, _, event: &CreateBranchDialogEvent, cx| match event {
                        CreateBranchDialogEvent::Submit {
                            branch_name,
                            checkout_after_create,
                            local_changes_mode,
                        } => {
                            let branch_name = branch_name.clone();
                            this.submit_create_branch_dialog(
                                &branch_name,
                                *checkout_after_create,
                                *local_changes_mode,
                                cx,
                            );
                        }
                        CreateBranchDialogEvent::Close => this.close_create_branch_dialog(cx),
                    },
                );
                match &mut this.pending_create_branch {
                    Some(pending) => pending.dialog = Some((dialog, subscription)),
                    None => drop(subscription),
                }
            });
        })
        .detach();
    }

    /// Las ramas locales del repositorio.
    fn known_local_branches(&self, cx: &App) -> Vec<String> {
        self.active_repo(cx)
            .and_then(|repo| self.model.read(cx).repository(repo.id.as_str()))
            .map(|repository| {
                repository
                    .branches
                    .iter()
                    .map(|branch| branch.name.clone())
                    .filter(|name| !name.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn build_checkout_targets(
        &self,
        commit: &CommitRow,
        cx: &App,
    ) -> Vec<crate::checkout::CheckoutTargetOption> {
        let known_local_branches = self.known_local_branches(cx);
        build_checkout_targets(
            &commit.refs,
            &commit.hash,
            &self.current_branch,
            &known_local_branches,
        )
    }

    /// `handleCheckoutDialogSubmit`: la respuesta del diálogo de checkout que
    /// abrió la vista. `target` es
    /// el `value` del destino elegido; sin él vale el único destino que había.
    pub fn submit_checkout_dialog(
        &mut self,
        local_changes_mode: LocalChangesMode,
        target: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.pending_checkout.take() else {
            return;
        };
        cx.notify();

        let target =
            target
                .map(str::to_string)
                .or_else(|| match pending.request.targets.as_slice() {
                    [only] => Some(only.value.clone()),
                    _ => None,
                });
        let Some(target) = target else {
            return;
        };
        self.perform_history_checkout(pending.commit, &target, local_changes_mode, cx);
    }

    /// `closeCheckoutDialogState`: el diálogo se cerró sin confirmar.
    pub fn close_checkout_dialog(&mut self, cx: &mut Context<Self>) {
        if self.pending_checkout.take().is_some() {
            cx.notify();
        }
    }

    /// `performHistoryCheckout`.
    fn perform_history_checkout(
        &mut self,
        commit: CommitRow,
        target: &str,
        local_changes_mode: LocalChangesMode,
        cx: &mut Context<Self>,
    ) {
        if target.is_empty() {
            return;
        }

        let (label, checkout_target) = match CheckoutTargetKind::parse(target) {
            CheckoutTargetKind::Branch(reference) => {
                (reference.clone(), CheckoutTarget::Reference(reference))
            }
            CheckoutTargetKind::Remote(reference) => {
                (reference.clone(), CheckoutTarget::RemoteBranch(reference))
            }
            CheckoutTargetKind::Detached => (
                commit.hash.clone(),
                CheckoutTarget::Reference(commit.full_hash.clone()),
            ),
        };

        self.run_commit_mutation("checkout", cx, async move |flow, cx| {
            let outcome = flow
                .model
                .update(cx, |model, cx| {
                    model.run_checkout(
                        flow.repo.id.as_str(),
                        CheckoutRequest {
                            label,
                            target: checkout_target,
                            local_changes_mode,
                        },
                        cx,
                    )
                })
                .await;

            if outcome.status == FlowStatus::Aborted {
                return;
            }
            let Some(result) = outcome.result.map(|result| result.to_operation_result()) else {
                return;
            };
            if !should_sync_after_mutation(&result) {
                return;
            }

            // Un checkout que pasa por stash o descarte toca el working tree: sin
            // esto la lista de stashes y el contador de cambios quedan desfasados.
            let touched_working_tree = local_changes_mode != LocalChangesMode::Keep;
            let mut request = SyncRequest::new()
                .result(result.clone())
                .status(result.status.is_none() || touched_working_tree)
                .branches(true)
                .stashes(outcome.stashed_changes)
                .changes_count(touched_working_tree);
            if let Some(operation) = result.operation.clone() {
                request = request.operation(operation);
            }
            flow.sync(request, cx).await;
            flow.reload(
                CommitLoadOptions::new()
                    .preferred_commit_hash(commit.full_hash.clone())
                    .force_details_reload(true),
                cx,
            )
            .await;
        })
        .detach();
    }

    // ------------------------------------------------------------------
    // Crear rama y tag
    // ------------------------------------------------------------------

    /// `handleCreateBranch`: pide el diálogo "Create Branch" con el commit como
    /// punto de partida.
    pub fn handle_create_branch(&mut self, commit: CommitRow, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo(cx) else {
            return;
        };

        let read = self.model.update(cx, |model, cx| {
            model.has_uncommitted_changes(&repo.path, cx)
        });
        Self::flow(cx, async move |this, cx| {
            let local_changes = read.await;
            let _ = this.update(cx, |this, cx| {
                let local_changes = match local_changes {
                    Ok(local_changes) => local_changes,
                    Err(error) => {
                        notify_inspection_failure(&this.model, &error, cx);
                        return;
                    }
                };

                let request = CreateBranchDialogRequest {
                    base_reference: commit.full_hash.clone(),
                    base_label: commit.hash.clone(),
                    has_local_changes: local_changes.has_changes,
                };
                this.show_create_branch_dialog(commit, request, cx);
            });
        })
        .detach();
    }

    /// `handleCreateBranchDialogClose`.
    pub fn close_create_branch_dialog(&mut self, cx: &mut Context<Self>) {
        if self.pending_create_branch.take().is_some() {
            cx.notify();
        }
    }

    /// `handleCreateBranchSubmit`: la respuesta del diálogo "Create Branch" que
    /// abrió la vista.
    pub fn submit_create_branch_dialog(
        &mut self,
        branch_name: &str,
        checkout_after_create: bool,
        local_changes_mode: LocalChangesMode,
        cx: &mut Context<Self>,
    ) {
        if self.active_repo(cx).is_none() {
            return;
        }
        let Some(pending) = self.pending_create_branch.take() else {
            return;
        };
        cx.notify();

        let branch_name = branch_name.to_string();
        self.run_commit_mutation("create-branch", cx, async move |flow, cx| {
            let outcome = flow
                .model
                .update(cx, |model, cx| {
                    model.submit_create_branch(
                        flow.repo.id.as_str(),
                        CreateBranchRequest {
                            branch_name: branch_name.clone(),
                            checkout_after_create,
                            local_changes_mode,
                            base_reference: Some(pending.request.base_reference.clone()),
                        },
                        cx,
                    )
                })
                .await;

            let result = outcome
                .branch_result
                .as_ref()
                .map(StructuredOutcome::to_operation_result);
            let Some(result) = result.filter(|result| {
                outcome.status == FlowStatus::Completed && should_sync_after_mutation(result)
            }) else {
                return;
            };

            let mut request = SyncRequest::new()
                .result(result.clone())
                .status(checkout_after_create || local_changes_mode != LocalChangesMode::Keep)
                .branches(true);
            if let Some(operation) = result.operation.clone() {
                request = request.operation(operation);
            }
            flow.sync(request, cx).await;

            let options = if checkout_after_create {
                CommitLoadOptions::new().preferred_branch(branch_name)
            } else {
                let preferred_branch = flow
                    .view
                    .update(cx, |view, cx| view.branch_for_reload(cx))
                    .ok()
                    .flatten();
                CommitLoadOptions {
                    preferred_commit_hash: Some(pending.commit.full_hash.clone()),
                    preferred_branch,
                    ..Default::default()
                }
            };
            flow.reload(options.force_details_reload(true), cx).await;
        })
        .detach();
    }

    /// `handleCreateTag`: nombre, anotación opcional y creación del tag.
    pub fn handle_create_tag(&mut self, commit: CommitRow, cx: &mut Context<Self>) {
        if self.active_repo(cx).is_none() || commit.full_hash.is_empty() {
            return;
        }

        let model = self.model.clone();
        Self::flow(cx, async move |this, cx| {
            let name = model
                .update(cx, |model, cx| {
                    model.prompt(
                        PromptOptions {
                            title: Some("Create tag".to_string()),
                            description: Some(format!("Create a tag at commit {}.", commit.hash)),
                            confirm_text: Some("Next".to_string()),
                            cancel_text: Some("Cancel".to_string()),
                            pattern: Some(r"\S+".to_string()),
                            input_error_message: Some("Tag name is required.".to_string()),
                            ..Default::default()
                        },
                        cx,
                    )
                })
                .await;
            let Some(tag_name) = name
                .value
                .filter(|value| name.confirmed && !value.is_empty())
                .map(|value| value.trim().to_string())
            else {
                return;
            };

            let annotation = model
                .update(cx, |model, cx| {
                    model.prompt(
                        PromptOptions {
                            title: Some("Annotate tag (optional)".to_string()),
                            description: Some(format!(
                                "Add a message for '{tag_name}' to create an annotated tag. Leave it blank for a lightweight tag."
                            )),
                            confirm_text: Some("Create tag".to_string()),
                            cancel_text: Some("Skip".to_string()),
                            textarea: true,
                            rows: Some(4),
                            placeholder: Some(
                                "Release notes, context, or why this tag matters".to_string(),
                            ),
                            ..Default::default()
                        },
                        cx,
                    )
                })
                .await;
            let tag_message = annotation
                .value
                .filter(|_| annotation.confirmed)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());

            let mutation = this.update(cx, |this, cx| {
                this.run_commit_mutation("create-tag", cx, async move |flow, cx| {
                    let result = flow
                        .model
                        .update(cx, |model, cx| {
                            model.create_tag(
                                &flow.repo.path,
                                &tag_name,
                                Some(&commit.full_hash),
                                tag_message.as_deref(),
                                cx,
                            )
                        })
                        .await;
                    flow.apply_result(&result, cx);
                    if !result.success() {
                        return;
                    }

                    let result = result.to_operation_result();
                    let mut request = SyncRequest::new().tags(true).result(result.clone());
                    if let Some(operation) = result.operation {
                        request = request.operation(operation);
                    }
                    flow.model
                        .update(cx, |model, cx| {
                            model.sync_repository_read_model(flow.repo.id.as_str(), request, cx)
                        })
                        .await;
                    flow.reload_current_branch(cx).await;
                })
            });
            if let Ok(mutation) = mutation {
                mutation.await;
            }
        })
        .detach();
    }

    // ------------------------------------------------------------------
    // Reescritura del historial
    // ------------------------------------------------------------------

    /// `openInteractiveRebase`: el rango es ese commit (incluido) hasta HEAD,
    /// en orden de aplicación. Solo segmentos lineales: un merge en el rango
    /// se aplanaría, y es mejor avisar antes que sorprender.
    pub fn open_interactive_rebase(
        &mut self,
        commit: &CommitRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .commits
            .iter()
            .any(|candidate| candidate.full_hash == commit.full_hash)
        {
            return;
        }

        // El rango sale de seguir los padres desde HEAD: lo que `git log --all`
        // pone encima (otras ramas, el merge de `refs/stash`) no se reescribe.
        let range = match interactive_rebase_range(&self.commits, &commit.full_hash) {
            Ok(range) => range,
            Err(error) => {
                self.model
                    .update(cx, |model, cx| model.notify_warning(error.message(), cx));
                return;
            }
        };

        let has_parent = !commit.parents.trim().is_empty();
        let base = has_parent.then(|| format!("{}^", commit.full_hash));
        // Orden de aplicación: el más viejo primero.
        let commits: Vec<RebaseCommit> = range
            .iter()
            .rev()
            .map(|entry| RebaseCommit::new(entry.full_hash.clone(), entry.message.clone()))
            .collect();

        // Un diálogo anterior que siguiera abierto se sustituye.
        if let Some(previous) = self.rebase.take() {
            previous
                .dialog
                .update(cx, |dialog, cx| dialog.dismiss(window, cx));
        }
        let dialog = open_interactive_rebase_dialog(commits, window, cx);
        let subscription = cx.subscribe(
            &dialog,
            |this, _, event: &InteractiveRebaseDialogEvent, cx| match event {
                InteractiveRebaseDialogEvent::Submit(todo) => {
                    this.handle_interactive_rebase_submit(todo.clone(), cx);
                }
                InteractiveRebaseDialogEvent::Close => {
                    this.rebase = None;
                    cx.notify();
                }
            },
        );
        self.rebase = Some(RebaseSession {
            dialog,
            base,
            window: window.window_handle(),
            _subscription: subscription,
        });
        cx.notify();
    }

    /// `handleInteractiveRebaseSubmit`.
    fn handle_interactive_rebase_submit(
        &mut self,
        todo: Vec<RebaseTodoEntry>,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = &self.rebase else {
            return;
        };
        let (dialog, base, window) = (session.dialog.clone(), session.base.clone(), session.window);
        dialog.update(cx, |dialog, cx| dialog.set_busy(true, cx));

        let mutation = self.run_commit_mutation("interactive-rebase", cx, {
            let dialog = dialog.clone();
            async move |flow, cx| {
                let result = flow
                    .model
                    .update(cx, |model, cx| {
                        model.interactive_rebase(
                            &flow.repo.path,
                            InteractiveRebaseOptions {
                                base_commit: base,
                                todo,
                            },
                            cx,
                        )
                    })
                    .await;
                flow.apply_result(&result, cx);

                // `showInteractiveRebase.value = false`
                let _ = window.update(cx, |_, window, cx| {
                    dialog.update(cx, |dialog, cx| dialog.dismiss(window, cx));
                });
                let _ = flow.view.update(cx, |view, cx| {
                    view.rebase = None;
                    cx.notify();
                });

                let result = result.to_operation_result();
                if !should_sync_after_mutation(&result) {
                    return;
                }
                flow.sync_after(&result, cx).await;
                flow.reload_current_branch(cx).await;
            }
        });

        cx.spawn(async move |_, cx| {
            mutation.await;
            dialog.update(cx, |dialog, cx| dialog.set_busy(false, cx));
        })
        .detach();
    }

    /// `runRebaseHistoryAction`: squash, reword o drop de un commit.
    fn run_rebase_history_action(
        &mut self,
        commit: CommitRow,
        action: &'static str,
        params: RebaseActionParams,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        self.run_commit_mutation(
            &format!("rebase-action:{action}"),
            cx,
            async move |flow, cx| {
                let result = flow
                    .model
                    .update(cx, |model, cx| {
                        model.rebase_action(&flow.repo.path, action, &commit.full_hash, params, cx)
                    })
                    .await;
                flow.apply_result(&result, cx);

                let result = result.to_operation_result();
                if !should_sync_after_mutation(&result) {
                    return;
                }
                flow.sync_after(&result, cx).await;
                flow.reload_current_branch(cx).await;
            },
        )
    }

    /// `handleSquash`.
    pub fn handle_squash(&mut self, commit: CommitRow, cx: &mut Context<Self>) {
        let confirm = self.model.update(cx, |model, cx| {
            model.confirm(
                ConfirmOptions {
                    title: Some("Confirm squash".to_string()),
                    description: Some(format!("Squash {} into its parent?", commit.hash)),
                    confirm_text: Some("Squash".to_string()),
                    cancel_text: Some("Cancel".to_string()),
                    ..Default::default()
                },
                cx,
            )
        });
        Self::flow(cx, async move |this, cx| {
            if !confirm.await {
                return;
            }
            let action = this.update(cx, |this, cx| {
                this.run_rebase_history_action(commit, "squash", RebaseActionParams::default(), cx)
            });
            if let Ok(action) = action {
                action.await;
            }
        })
        .detach();
    }

    /// `handleReword`.
    pub fn handle_reword(&mut self, commit: CommitRow, cx: &mut Context<Self>) {
        let prompt = self.model.update(cx, |model, cx| {
            model.prompt(
                PromptOptions {
                    title: Some("Reword commit".to_string()),
                    description: Some("Enter the new commit message.".to_string()),
                    default_value: Some(commit.message.clone()),
                    confirm_text: Some("Reword".to_string()),
                    cancel_text: Some("Cancel".to_string()),
                    ..Default::default()
                },
                cx,
            )
        });
        Self::flow(cx, async move |this, cx| {
            let answer = prompt.await;
            let Some(new_message) = answer
                .value
                .filter(|value| answer.confirmed && !value.is_empty())
            else {
                return;
            };
            let action = this.update(cx, |this, cx| {
                this.run_rebase_history_action(
                    commit,
                    "reword",
                    RebaseActionParams {
                        new_message: Some(new_message),
                    },
                    cx,
                )
            });
            if let Ok(action) = action {
                action.await;
            }
        })
        .detach();
    }

    /// `handleDrop`.
    pub fn handle_drop(&mut self, commit: CommitRow, cx: &mut Context<Self>) {
        let confirm = self.model.update(cx, |model, cx| {
            model.confirm(
                ConfirmOptions {
                    title: Some("Confirm drop".to_string()),
                    description: Some(format!(
                        "Drop commit {}? This changes history.",
                        commit.hash
                    )),
                    confirm_text: Some("Drop".to_string()),
                    cancel_text: Some("Cancel".to_string()),
                    variant: Some("destructive".to_string()),
                    ..Default::default()
                },
                cx,
            )
        });
        Self::flow(cx, async move |this, cx| {
            if !confirm.await {
                return;
            }
            let action = this.update(cx, |this, cx| {
                this.run_rebase_history_action(commit, "drop", RebaseActionParams::default(), cx)
            });
            if let Ok(action) = action {
                action.await;
            }
        })
        .detach();
    }

    /// El repositorio activo tiene algo preparado (para "Create fixup").
    pub(super) fn has_staged_changes(&self, cx: &App) -> bool {
        let model = self.model.read(cx);
        let repository = match &self.repo_id {
            Some(repo_id) => model.repository(repo_id.as_str()),
            None => model.active_repository(),
        };
        repository
            .and_then(|repository| repository.status.as_ref())
            .is_some_and(|status| {
                status.files.iter().any(|file| {
                    !matches!(file.index.as_str(), "" | " " | "?" | "!" | "U")
                        && file.working_dir != "U"
                })
            })
    }

    /// "Create fixup for this commit": un `fixup! <asunto>` con lo preparado.
    /// Si el commit ya está en el upstream, pregunta antes.
    pub fn handle_create_fixup(&mut self, commit: CommitRow, cx: &mut Context<Self>) {
        let confirm = (commit.in_upstream == Some(true)).then(|| {
            self.model.update(cx, |model, cx| {
                model.confirm(
                    ConfirmOptions {
                        title: Some("Fix up a pushed commit?".to_string()),
                        description: Some(format!(
                            "{} is already on the upstream. Squashing the fixup into it later rewrites published history and needs a force push.",
                            commit.hash
                        )),
                        confirm_text: Some("Create fixup".to_string()),
                        cancel_text: Some("Cancel".to_string()),
                        ..Default::default()
                    },
                    cx,
                )
            })
        });
        Self::flow(cx, async move |this, cx| {
            if let Some(confirm) = confirm
                && !confirm.await
            {
                return;
            }
            let mutation = this.update(cx, |this, cx| {
                let sign = this
                    .active_repo(cx)
                    .is_some_and(|repo| this.model.read(cx).commit_sign(&repo.path));
                this.run_commit_mutation("commit", cx, async move |flow, cx| {
                    let options = CommitOptions {
                        sign,
                        fixup: Some(commit.full_hash.clone()),
                        ..Default::default()
                    };
                    let result = flow
                        .model
                        .update(cx, |model, cx| {
                            model.commit(&flow.repo.path, "", options, false, cx)
                        })
                        .await;
                    flow.apply_result(&result, cx);

                    let result = result.to_operation_result();
                    if !should_sync_after_mutation(&result) {
                        return;
                    }
                    flow.sync_after(&result, cx).await;
                    flow.reload_current_branch(cx).await;
                })
            });
            if let Ok(mutation) = mutation {
                mutation.await;
            }
        })
        .detach();
    }

    /// "Autosquash fixups": combina los `fixup!` / `squash!` / `amend!` que
    /// hay de este commit hasta HEAD en sus destinos. Cuenta los del rango
    /// cargado para la confirmación; si no hay ninguno, solo avisa. Un
    /// conflicto deja el rebase parado con su banner (Continue / Abort).
    pub fn handle_autosquash(&mut self, commit: CommitRow, cx: &mut Context<Self>) {
        let range = match interactive_rebase_range(&self.commits, &commit.full_hash) {
            Ok(range) => range,
            Err(error) => {
                self.model
                    .update(cx, |model, cx| model.notify_warning(error.message(), cx));
                return;
            }
        };
        let fixups = range
            .iter()
            .filter(|entry| is_autosquash_subject(&entry.message))
            .count();
        if fixups == 0 {
            self.model.update(cx, |model, cx| {
                model.notify_warning(
                    &format!(
                        "There are no fixup commits from {} up to HEAD.",
                        commit.hash
                    ),
                    cx,
                );
            });
            return;
        }
        let rewritten = range.len();
        let published = range.iter().any(|entry| entry.in_upstream == Some(true));
        let mut description = format!(
            "Fold {fixups} fixup commit{} into {} target{}? This rewrites the {rewritten} commit{} from {} up to HEAD.",
            plural(fixups),
            if fixups == 1 { "its" } else { "their" },
            plural(fixups),
            plural(rewritten),
            commit.hash
        );
        if published {
            description.push_str(
                " Some of them are already on the upstream: you will need to force push.",
            );
        }
        let confirm = self.model.update(cx, |model, cx| {
            model.confirm(
                ConfirmOptions {
                    title: Some("Autosquash fixups".to_string()),
                    description: Some(description),
                    confirm_text: Some("Autosquash".to_string()),
                    cancel_text: Some("Cancel".to_string()),
                    ..Default::default()
                },
                cx,
            )
        });
        Self::flow(cx, async move |this, cx| {
            if !confirm.await {
                return;
            }
            let action = this.update(cx, |this, cx| {
                this.run_rebase_history_action(
                    commit,
                    "autosquash",
                    RebaseActionParams::default(),
                    cx,
                )
            });
            if let Ok(action) = action {
                action.await;
            }
        })
        .detach();
    }

    /// `handleReset(commit, mode)`: `mode` es `soft`, `mixed` o `hard`.
    pub fn handle_reset(&mut self, commit: CommitRow, mode: &'static str, cx: &mut Context<Self>) {
        let tracked_changes = if mode == "hard" {
            self.model
                .read(cx)
                .active_repository()
                .and_then(|repo| repo.status.as_ref())
                .map_or(0, |status| {
                    status
                        .files
                        .iter()
                        .filter(|file| !(file.index == "?" && file.working_dir == "?"))
                        .count()
                })
        } else {
            0
        };
        let description = if tracked_changes > 0 {
            let noun = if tracked_changes == 1 {
                "uncommitted change"
            } else {
                "uncommitted changes"
            };
            format!(
                "Reset to {} using {mode} mode? {tracked_changes} {noun} in tracked files will be discarded and cannot be undone.",
                commit.hash
            )
        } else {
            format!("Reset to {} using {mode} mode?", commit.hash)
        };
        let confirm = self.model.update(cx, |model, cx| {
            model.confirm(
                ConfirmOptions {
                    title: Some("Confirm reset".to_string()),
                    description: Some(description),
                    confirm_text: Some("Reset".to_string()),
                    cancel_text: Some("Cancel".to_string()),
                    variant: (mode == "hard").then(|| "destructive".to_string()),
                    ..Default::default()
                },
                cx,
            )
        });
        Self::flow(cx, async move |this, cx| {
            if !confirm.await {
                return;
            }
            let mutation = this.update(cx, |this, cx| {
                this.run_commit_mutation(&format!("reset:{mode}"), cx, async move |flow, cx| {
                    let result = flow
                        .model
                        .update(cx, |model, cx| {
                            model.reset(&flow.repo.path, &commit.full_hash, mode, cx)
                        })
                        .await;
                    flow.apply_result(&result, cx);

                    let result = result.to_operation_result();
                    if !should_sync_after_mutation(&result) {
                        return;
                    }
                    flow.sync_after(&result, cx).await;
                    flow.reload_current_branch(cx).await;
                })
            });
            if let Ok(mutation) = mutation {
                mutation.await;
            }
        })
        .detach();
    }

    /// `handleRevert`: se ejecuta directo, sin diálogo.
    pub fn handle_revert(&mut self, commit: CommitRow, cx: &mut Context<Self>) {
        self.run_commit_mutation("revert", cx, async move |flow, cx| {
            let result = flow
                .model
                .update(cx, |model, cx| {
                    model.revert(&flow.repo.path, &commit.full_hash, cx)
                })
                .await;
            flow.apply_result(&result, cx);

            let result = result.to_operation_result();
            if !should_sync_after_mutation(&result) {
                return;
            }
            flow.sync_after(&result, cx).await;
            flow.reload_current_branch(cx).await;
        })
        .detach();
    }

    /// `handleCherryPick`: se ejecuta directo, sin diálogo.
    pub fn handle_cherry_pick(&mut self, commit: CommitRow, cx: &mut Context<Self>) {
        self.run_commit_mutation("cherry-pick", cx, async move |flow, cx| {
            let result = flow
                .model
                .update(cx, |model, cx| {
                    model.cherry_pick(&flow.repo.path, &commit.full_hash, cx)
                })
                .await;
            flow.apply_result(&result, cx);

            let result = result.to_operation_result();
            if !should_sync_after_mutation(&result) {
                return;
            }
            flow.sync_after(&result, cx).await;
            flow.reload_current_branch(cx).await;
        })
        .detach();
    }

    // ------------------------------------------------------------------
    // Varios commits
    // ------------------------------------------------------------------

    /// Ejecuta una acción del menú de la selección múltiple.
    pub fn run_batch_command(
        &mut self,
        command: BatchCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.multi.is_multi() {
            return;
        }
        match command {
            BatchCommand::CherryPick => self.handle_batch_cherry_pick(cx),
            BatchCommand::Revert => self.handle_batch_revert(false, cx),
            BatchCommand::RevertAsOne => self.handle_batch_revert(true, cx),
            BatchCommand::CompareTwo => self.compare_selected_commits(window, cx),
            BatchCommand::CopyShas => {
                let text = self.multi.oldest_first(&self.commits).join("\n");
                self.copy_to_clipboard(&text, cx);
            }
        }
    }

    /// "Cherry-pick N commits": confirma y aplica los seleccionados del más
    /// viejo al más nuevo sobre la rama actual. Un conflicto deja la secuencia
    /// a medias (banner: continuar, saltar o abortar).
    pub fn handle_batch_cherry_pick(&mut self, cx: &mut Context<Self>) {
        let commits = self.multi.oldest_first(&self.commits);
        let target = self.branch_label();
        let confirm = self.model.update(cx, |model, cx| {
            model.confirm(
                ConfirmOptions {
                    title: Some("Cherry-pick commits".to_string()),
                    description: Some(format!(
                        "Apply {} commits onto {target}, oldest first? Git stops at the first \
                         conflict so you can resolve it, skip that commit or abort.",
                        commits.len()
                    )),
                    confirm_text: Some("Cherry-pick".to_string()),
                    cancel_text: Some("Cancel".to_string()),
                    ..Default::default()
                },
                cx,
            )
        });
        self.run_confirmed_batch(confirm, "cherry-pick", cx, async move |flow, cx| {
            flow.model
                .update(cx, |model, cx| {
                    model.cherry_pick_commits(&flow.repo.path, &commits, cx)
                })
                .await
        });
    }

    /// "Revert N commits" (uno por commit) o, con `single_commit`, todos en un
    /// único commit de revert. Del más nuevo al más viejo.
    pub fn handle_batch_revert(&mut self, single_commit: bool, cx: &mut Context<Self>) {
        let commits = self.multi.newest_first(&self.commits);
        let target = self.branch_label();
        let description = if single_commit {
            format!(
                "Undo {} commits on {target} with a single new commit?",
                commits.len()
            )
        } else {
            format!(
                "Undo {} commits on {target} with one revert commit each, newest first?",
                commits.len()
            )
        };
        let confirm = self.model.update(cx, |model, cx| {
            model.confirm(
                ConfirmOptions {
                    title: Some("Revert commits".to_string()),
                    description: Some(description),
                    confirm_text: Some("Revert".to_string()),
                    cancel_text: Some("Cancel".to_string()),
                    ..Default::default()
                },
                cx,
            )
        });
        self.run_confirmed_batch(confirm, "revert", cx, async move |flow, cx| {
            flow.model
                .update(cx, |model, cx| {
                    model.revert_commits(&flow.repo.path, &commits, single_commit, cx)
                })
                .await
        });
    }

    /// La rama sobre la que caen los commits, para los textos.
    fn branch_label(&self) -> String {
        if self.current_branch.is_empty() || self.current_branch == "HEAD" {
            "the detached HEAD".to_string()
        } else {
            format!("'{}'", self.current_branch)
        }
    }

    /// Tras confirmar, vuelve a la selección simple y lanza la mutación con
    /// la relectura de siempre.
    fn run_confirmed_batch(
        &mut self,
        confirm: Task<bool>,
        operation: &'static str,
        cx: &mut Context<Self>,
        run: impl AsyncFnOnce(&Flow, &mut AsyncApp) -> mergeon_app::GitOperation + 'static,
    ) {
        Self::flow(cx, async move |this, cx| {
            if !confirm.await {
                return;
            }
            let mutation = this.update(cx, |this, cx| {
                this.collapse_multi_selection(cx);
                this.run_commit_mutation(operation, cx, async move |flow, cx| {
                    let result = run(&flow, cx).await;
                    flow.apply_result(&result, cx);

                    let result = result.to_operation_result();
                    if !should_sync_after_mutation(&result) {
                        return;
                    }
                    flow.sync_after(&result, cx).await;
                    flow.reload_current_branch(cx).await;
                })
            });
            if let Ok(mutation) = mutation {
                mutation.await;
            }
        })
        .detach();
    }

    /// "Compare these two": la comparación navegable del más viejo (A) con
    /// el más nuevo (B). Doble clic en un commit de la comparación la cierra y
    /// lo selecciona aquí.
    pub fn compare_selected_commits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hashes = self.multi.oldest_first(&self.commits);
        let [older, newer] = hashes.as_slice() else {
            return;
        };
        let Some(repo) = self.active_repo(cx) else {
            return;
        };
        let request = ComparisonRequest::new(repo.id, repo.path, older.clone(), newer.clone());
        let compare = open_compare_view(&request, window, cx);
        let subscription = cx.subscribe_in(
            &compare,
            window,
            |this, _, event: &CompareViewEvent, window, cx| {
                let CompareViewEvent::ShowCommit { full_hash } = event;
                mergeon_ui::dialog::close(window, cx);
                this.collapse_multi_selection(cx);
                this.request_commit_selection(full_hash, cx);
            },
        );
        self.compare = Some((compare, subscription));
    }

    /// `handleCommitCreated`: la vista de cambios locales creó un commit. Se
    /// relee el repositorio (si no lo hizo ya quien avisa), se recarga el
    /// historial y se vuelve a la vista de commits.
    pub fn handle_commit_created(
        &mut self,
        payload: Option<CommitCreated>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let Some(repo) = self.active_repo(cx) else {
            return Task::ready(());
        };
        let payload = payload.unwrap_or_default();
        let flow = Flow {
            view: cx.weak_entity(),
            model: self.model.clone(),
            repo,
        };

        Self::flow(cx, async move |this, cx| {
            if !payload.sync_already_applied {
                if let Some(result) = &payload.result {
                    flow.apply_result(result, cx);
                }
                let mut request = SyncRequest::new()
                    .status(
                        payload
                            .result
                            .as_ref()
                            .is_none_or(|result| result.status.is_none()),
                    )
                    .branches(true);
                if let Some(result) = payload.result.clone() {
                    if let Some(operation) = result.operation.clone() {
                        request = request.operation(operation);
                    }
                    request = request.result(result);
                }
                flow.sync(request, cx).await;
            }
            flow.reload_current_branch(cx).await;

            // `emit('change-view', 'commits')`
            let _ = this.update(cx, |this, cx| {
                if this.workspace_mode {
                    this.model.update(cx, |model, cx| {
                        model.show_workspace_home(
                            &flow.repo.path,
                            Some(WorkspaceSection::Commits),
                            cx,
                        );
                    });
                }
                cx.emit(CommitsViewEvent::ChangeView(WorkspaceSection::Commits));
            });
        })
    }
}

/// Las props de `CheckoutBranchDialog` para esos destinos, como las calcula
/// `CommitView.vue`.
fn checkout_dialog_options(request: &CheckoutDialogRequest) -> CheckoutBranchOptions {
    let targets = request
        .targets
        .iter()
        .map(|target| {
            let option = DialogTargetOption::new(target.value.clone(), target.label.clone())
                .description(target.description.clone())
                .confirm_label(target.confirm_label.clone());
            match &target.warning {
                Some(warning) => option.warning(warning.clone()),
                None => option,
            }
        })
        .collect();
    CheckoutBranchOptions::new(request.target_label().to_string())
        .has_local_changes(request.has_local_changes)
        .remote_branch(request.remote_branch().to_string())
        .target_options(targets)
        .title(request.title())
        .description(request.description())
}

/// `notify.error(error?.message || 'Failed to inspect local changes', { error })`.
fn notify_inspection_failure(
    model: &Entity<AppModel>,
    error: &mergeon_backend::QueryError,
    cx: &mut App,
) {
    let message = if error.message().is_empty() {
        "Failed to inspect local changes"
    } else {
        error.message()
    }
    .to_string();
    model.update(cx, |model, cx| {
        model.notify_error_with(&message, error, cx);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use mergeon_model::commit_view_utils::parse_refs;

    fn commit(refs: &str, parents: &str) -> CommitRow {
        CommitRow {
            hash: "2222222".to_string(),
            full_hash: "2222222222222222".to_string(),
            refs: parse_refs(refs),
            parents: parents.to_string(),
            ..Default::default()
        }
    }

    fn labels(menu: &[Vec<CommitMenuEntry>]) -> Vec<Vec<&'static str>> {
        menu.iter()
            .map(|group| group.iter().map(|entry| entry.label).collect())
            .collect()
    }

    fn entry(menu: &[Vec<CommitMenuEntry>], command: CommitCommand) -> CommitMenuEntry {
        *menu
            .iter()
            .flatten()
            .find(|entry| entry.command == command)
            .expect("the command is in the menu")
    }

    #[test]
    fn lists_every_commit_action_in_the_order_of_the_original_menu() {
        let menu = commit_menu(&commit("", "1111111111111111"), "main", false);

        assert_eq!(
            labels(&menu),
            [
                vec!["Checkout", "Create Branch", "Create Tag…"],
                vec![
                    "Squash into parent",
                    "Reword",
                    "Drop",
                    "Interactive rebase from here…"
                ],
                vec!["Create fixup for this commit", "Autosquash fixups"],
                vec!["Cherry-pick", "Revert"],
                vec!["Reset (Soft)", "Reset (Mixed)", "Reset (Hard)"],
                vec!["Copy SHA"],
            ]
        );
        let dangerous: Vec<&str> = menu
            .iter()
            .flatten()
            .filter(|entry| entry.danger)
            .map(|entry| entry.label)
            .collect();
        assert_eq!(dangerous, ["Drop", "Reset (Hard)"]);
        assert!(menu.iter().flatten().all(|entry| !entry.disabled));
    }

    #[test]
    fn offers_the_github_link_next_to_the_sha_only_on_github() {
        let menu = commit_menu(&commit("", "1111111111111111"), "main", true);
        assert_eq!(
            labels(&menu).last().map(Vec::as_slice),
            Some(["Copy SHA", "Copy GitHub link"].as_slice())
        );
        let menu = commit_menu(&commit("", "1111111111111111"), "main", false);
        assert!(
            menu.iter()
                .flatten()
                .all(|entry| entry.command != CommitCommand::CopyGitHubLink)
        );
    }

    #[test]
    fn heads_the_menu_with_the_short_hash_and_a_trimmed_subject() {
        let mut row = commit("", "1111111111111111");
        row.message = "fix(cart): round totals half-even before showing them".to_string();
        assert_eq!(
            commit_menu_header(&row),
            "2222222 · fix(cart): round totals half-ev…"
        );

        row.message = "fix(cart)".to_string();
        assert_eq!(commit_menu_header(&row), "2222222 · fix(cart)");
    }

    #[test]
    fn gives_the_main_actions_a_single_letter() {
        let keys: Vec<(CommitCommand, &str)> =
            commit_menu(&commit("", "1111111111111111"), "main", false)
                .iter()
                .flatten()
                .filter_map(|entry| Some((entry.command, commit_menu_access_key(entry.command)?)))
                .collect();
        assert_eq!(
            keys,
            [
                (CommitCommand::CreateBranch, "B"),
                (CommitCommand::CreateTag, "T"),
                (CommitCommand::CherryPick, "C"),
                (CommitCommand::Revert, "R"),
            ]
        );
    }

    #[test]
    fn cannot_check_out_the_commit_the_current_branch_points_at() {
        let menu = commit_menu(&commit("HEAD -> main", "1111111111111111"), "main", false);
        assert!(entry(&menu, CommitCommand::Checkout).disabled);

        let menu = commit_menu(&commit("feature", "1111111111111111"), "main", false);
        assert!(!entry(&menu, CommitCommand::Checkout).disabled);
    }

    /// El inspector pregunta por "Checkout" sin construir el menú: la
    /// respuesta tiene que ser siempre la del menú.
    #[test]
    fn checkout_disabled_matches_the_menu_entry_in_every_case() {
        let cases = [
            // HEAD en la rama actual.
            ("HEAD -> main", "main"),
            // La rama actual sin HEAD (otra vista del mismo commit).
            ("main", "main"),
            // HEAD separado sobre el commit.
            ("HEAD", "main"),
            // HEAD separado: no hay rama actual.
            ("HEAD -> main", ""),
            ("main", ""),
            // Otra rama local, otro commit cualquiera.
            ("feature", "main"),
            ("", "main"),
            // Una rama remota o un tag con el mismo nombre no cuentan.
            ("origin/main", "main"),
            ("tag: main", "main"),
            ("origin/main, tag: v1, feature, main", "main"),
            ("HEAD -> feature, main", "feature"),
        ];
        for (refs, branch) in cases {
            let row = commit(refs, "1111111111111111");
            let menu = commit_menu(&row, branch, false);
            assert_eq!(
                is_checkout_disabled(&row, branch),
                entry(&menu, CommitCommand::Checkout).disabled,
                "refs {refs:?} on branch {branch:?}"
            );
        }
        assert!(is_checkout_disabled(&commit("HEAD -> main", ""), "main"));
        assert!(!is_checkout_disabled(&commit("feature", ""), "main"));
        assert!(!is_checkout_disabled(&commit("origin/main", ""), "main"));
    }

    #[test]
    fn cannot_squash_the_root_commit_into_a_parent_it_does_not_have() {
        let menu = commit_menu(&commit("", "  "), "main", false);
        assert!(entry(&menu, CommitCommand::Squash).disabled);
    }

    #[test]
    fn interactive_rebase_needs_a_commit_of_the_current_branch() {
        let mut outside = commit("", "1111111111111111");
        outside.in_current_branch = Some(false);
        let menu = commit_menu(&outside, "main", false);
        assert!(entry(&menu, CommitCommand::InteractiveRebase).disabled);

        // Sin calcular la pertenencia no se afirma nada.
        let menu = commit_menu(&commit("", "1111111111111111"), "main", false);
        assert!(!entry(&menu, CommitCommand::InteractiveRebase).disabled);
    }

    #[test]
    fn fixup_actions_need_a_commit_of_the_current_branch() {
        let mut outside = commit("", "1111111111111111");
        outside.in_current_branch = Some(false);
        let menu = commit_menu(&outside, "main", false);
        assert!(entry(&menu, CommitCommand::CreateFixup).disabled);
        assert!(entry(&menu, CommitCommand::Autosquash).disabled);

        let menu = commit_menu(&commit("", "1111111111111111"), "main", false);
        assert!(!entry(&menu, CommitCommand::CreateFixup).disabled);
        assert!(!entry(&menu, CommitCommand::Autosquash).disabled);
    }

    #[test]
    fn recognizes_the_subjects_autosquash_moves() {
        assert!(is_autosquash_subject("fixup! feat: cart"));
        assert!(is_autosquash_subject("squash! feat: cart"));
        assert!(is_autosquash_subject("amend! feat: cart"));
        assert!(!is_autosquash_subject("fix: cart"));
        assert!(!is_autosquash_subject("fixup!cart"));
    }

    #[test]
    fn syncs_after_success_noop_or_conflict_only() {
        let result = |success: bool, outcome: &str| OperationResult {
            success,
            outcome: Some(outcome.to_string()),
            ..Default::default()
        };
        assert!(should_sync_after_mutation(&result(true, "completed")));
        assert!(should_sync_after_mutation(&result(false, "noop")));
        assert!(should_sync_after_mutation(&result(false, "conflicted")));
        assert!(!should_sync_after_mutation(&result(false, "failed")));
    }
}
