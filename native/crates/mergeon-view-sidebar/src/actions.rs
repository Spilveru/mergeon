//! Lo que hace cada orden del sidebar: los `handle*` de `Sidebar.vue` que
//! abren un diálogo o necesitan estado de la vista. Los flujos que solo hablan
//! con el modelo están en [`crate::flows`].

use gpui_kit::{App, ClipboardItem, Context, Entity, Window};
use mergeon_app::{CheckoutTarget, CreateBranchRequest, LocalChangesMode, SyncRequest};
use mergeon_model::operation_types::OperationResult;
use mergeon_model::repositories::RepoLifecycleState;
use mergeon_model::repository_read_model::BranchItem;

use crate::dialogs::{
    AddRemoteDialog, AddRemoteDialogEvent, BranchActionCompleted, BranchActionDialogEvent,
    CheckoutBranchDialogEvent, CheckoutBranchOptions, CreateBranchDialogEvent, CreateBranchOptions,
    open_add_remote_dialog, open_branch_action_dialog, open_checkout_branch_dialog,
    open_cleanup_branches_dialog, open_create_branch_dialog, open_tag_details_dialog,
};
use crate::flows::{self, RepoRef};
use crate::logic::{
    UpstreamReference, branch_tracking_reference, normalize_remote_url, parse_upstream_reference,
};
use crate::menus::{self, BranchAction, MenuEntry, SidebarCommand};
use crate::sidebar::{ComparisonRequest, PendingCheckout, Sidebar, SidebarEvent};

impl Sidebar {
    // ------------------------------------------------------------------
    // Menús contextuales
    // ------------------------------------------------------------------

    /// Entradas del menú de una rama local (`branchContextMenuItems`). Vacío si
    /// la rama ya no existe.
    pub fn local_branch_menu(&self, branch_name: &str, cx: &App) -> Vec<MenuEntry> {
        let Some(repo) = self.model.read(cx).active_repository() else {
            return Vec::new();
        };
        let Some(branch) = repo
            .branches
            .iter()
            .find(|branch| branch.name == branch_name)
        else {
            return Vec::new();
        };
        let repo_id = repo.id.clone();
        let mut entries = menus::local_branch_menu(repo, branch, &|operation| {
            self.is_operation_running(&repo_id, operation, cx)
        });
        // Desprotegida en otro worktree: git no la deja desproteger aquí, así
        // que "Checkout" pasa a abrir ese worktree y no se ofrece otro.
        if let Some(path) = self.branch_worktree(branch_name) {
            for entry in &mut entries {
                match entry.command() {
                    Some(SidebarCommand::CheckoutBranch(_)) => {
                        *entry = MenuEntry::item(
                            format!("Open its worktree ({})", worktree_folder(path)),
                            mergeon_ui::Lucide::ExternalLink,
                            SidebarCommand::OpenWorktree(path.to_string()),
                        );
                    }
                    Some(SidebarCommand::CheckoutInNewWorktree(_)) => {
                        *entry = entry.clone().disabled(true);
                    }
                    _ => {}
                }
            }
        }
        entries
    }

    /// Entradas del menú de una rama remota (`buildRemoteBranchMenuItems`).
    pub fn remote_branch_menu(
        &self,
        remote_name: &str,
        branch_name: &str,
        cx: &App,
    ) -> Vec<MenuEntry> {
        let Some(repo) = self.model.read(cx).active_repository() else {
            return Vec::new();
        };
        let Some(remote) = repo
            .remotes
            .iter()
            .find(|remote| remote.name == remote_name)
        else {
            return Vec::new();
        };
        let repo_id = repo.id.clone();
        menus::remote_branch_menu(repo, remote, branch_name, &|operation| {
            self.is_operation_running(&repo_id, operation, cx)
        })
    }

    /// Entradas del menú de la sección Branches (clic derecho en su cabecera).
    pub fn branches_section_menu(&self, cx: &App) -> Vec<MenuEntry> {
        self.model
            .read(cx)
            .active_repository()
            .map(menus::branches_section_menu)
            .unwrap_or_default()
    }

    /// Entradas del menú de un tag (`tagContextMenuItems`).
    /// Entradas del menú de un stash (por su hash).
    pub fn stash_menu(&self, hash: &str, cx: &App) -> Vec<MenuEntry> {
        self.model
            .read(cx)
            .active_repository()
            .map(|repo| menus::stash_menu(repo, hash))
            .unwrap_or_default()
    }

    pub fn tag_menu(&self, tag: &str, cx: &App) -> Vec<MenuEntry> {
        self.model
            .read(cx)
            .active_repository()
            .map(|repo| menus::tag_menu(repo, tag))
            .unwrap_or_default()
    }

    /// Entradas del menú de un remoto (`remoteContextMenuItems`).
    pub fn remote_menu(&self, remote_name: &str, cx: &App) -> Vec<MenuEntry> {
        self.model
            .read(cx)
            .active_repository()
            .map(|repo| menus::remote_menu(repo, remote_name))
            .unwrap_or_default()
    }

    // ------------------------------------------------------------------
    // Órdenes
    // ------------------------------------------------------------------

    /// La rama local con ese nombre o, si ya no existe (una destacada que se
    /// borró), lo poco que la fila sabe de ella.
    fn branch_or_stub(&self, branch_name: &str, cx: &App) -> Option<BranchItem> {
        let repo = self.model.read(cx).active_repository()?;
        Some(
            repo.branches
                .iter()
                .find(|branch| branch.name == branch_name)
                .cloned()
                .unwrap_or_else(|| BranchItem {
                    name: branch_name.to_string(),
                    active: repo.current_branch.as_deref() == Some(branch_name),
                    is_remote: false,
                    upstream: None,
                    has_tracking: false,
                    upstream_set: false,
                    ahead: 0,
                    behind: 0,
                }),
        )
    }

    /// El upstream de una rama partido en remoto y rama, reconociendo los
    /// remotos del repositorio (que pueden llevar `/`).
    fn branch_upstream(&self, branch_name: &str, cx: &App) -> Option<UpstreamReference> {
        let branch = self.branch_or_stub(branch_name, cx)?;
        let remotes = &self.model.read(cx).active_repository()?.remotes;
        parse_upstream_reference(branch_tracking_reference(&branch).as_deref(), remotes)
    }

    /// Ejecuta una orden: la acción de una entrada de menú, de un botón de fila
    /// o de un doble clic.
    pub fn run_command(
        &mut self,
        command: SidebarCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.active_repo(cx) else {
            return;
        };
        let model = self.model.clone();

        match command {
            SidebarCommand::CheckoutBranch(name) => self.checkout_branch(&name, window, cx),
            SidebarCommand::FastForwardBranch(name) => {
                let tracking = self
                    .branch_or_stub(&name, cx)
                    .and_then(|branch| branch_tracking_reference(&branch));
                if let Some(tracking) = tracking {
                    flows::fast_forward_branch(&model, repo, name, tracking, cx).detach();
                }
            }
            SidebarCommand::PullBranch(name) => {
                if let Some(parts) = self.branch_upstream(&name, cx) {
                    flows::pull(&model, repo, parts.remote_name, parts.branch_name, cx).detach();
                }
            }
            SidebarCommand::PushBranch(name) => {
                // `local` puede seguir a `origin/otro-nombre`: se empuja ahí,
                // no a `origin/local`.
                if let Some(parts) = self.branch_upstream(&name, cx) {
                    flows::push_branch(
                        &model,
                        repo,
                        parts.remote_name,
                        name,
                        Some(parts.branch_name),
                        cx,
                    )
                    .detach();
                }
            }
            SidebarCommand::CompareWithCurrent(reference) => {
                let current = model
                    .read(cx)
                    .repository(repo.id.as_str())
                    .and_then(|repository| repository.current_branch.clone())
                    .filter(|branch| !branch.is_empty())
                    .unwrap_or_else(|| "HEAD".to_string());
                cx.emit(SidebarEvent::OpenComparison(ComparisonRequest::new(
                    repo.id, repo.path, current, reference,
                )));
            }
            SidebarCommand::BranchAction { branch, action } => {
                self.open_branch_action(&branch, None, Some(action), window, cx);
            }
            SidebarCommand::RemoteBranchAction {
                remote,
                branch,
                action,
            } => {
                self.open_branch_action(
                    &format!("{remote}/{branch}"),
                    None,
                    Some(action),
                    window,
                    cx,
                );
            }
            SidebarCommand::NewBranchFromBranch(name) => {
                self.open_create_branch(name, window, cx);
            }
            SidebarCommand::NewBranchFromRemoteBranch { remote, branch } => {
                self.open_create_branch(format!("{remote}/{branch}"), window, cx);
            }
            SidebarCommand::NewTagFromBranch(name) => {
                let active = self
                    .branch_or_stub(&name, cx)
                    .is_some_and(|branch| branch.active);
                if !active {
                    model.update(cx, |model, cx| {
                        model.notify_warning(
                            "New tags are only available for the currently checked out branch.",
                            cx,
                        );
                    });
                    return;
                }
                flows::create_tag_from_branch(&model, repo, cx).detach();
            }
            SidebarCommand::NewTagFromRemoteBranch { remote, branch } => {
                cx.spawn(async move |_, cx| {
                    flows::create_tag_from_remote_branch(model, repo, remote, branch, cx).await;
                })
                .detach();
            }
            SidebarCommand::SetupTracking(name) => {
                flows::setup_tracking(&model, repo, name, cx).detach();
            }
            SidebarCommand::RenameBranch(name) => {
                flows::rename_branch(&model, repo, name, cx).detach();
            }
            SidebarCommand::DeleteBranch(name) => {
                let upstream = self.branch_upstream(&name, cx);
                cx.spawn(async move |_, cx| {
                    flows::delete_branch(model, repo, name, upstream, cx).await;
                })
                .detach();
            }
            SidebarCommand::CopyBranchName(name) => {
                cx.write_to_clipboard(ClipboardItem::new_string(name));
                model.update(cx, |model, cx| {
                    model.notify_success("Branch name copied to clipboard", cx);
                });
            }
            SidebarCommand::ToggleStarBranch(name) => {
                cx.spawn(async move |_, cx| {
                    flows::toggle_star_branch(model, repo, name, cx).await;
                })
                .detach();
            }
            SidebarCommand::CheckoutRemoteBranch { remote, branch } => {
                self.checkout_remote_branch(&remote, &branch, window, cx);
            }
            SidebarCommand::PullRemoteBranch { remote, branch } => {
                flows::pull(&model, repo, remote, branch, cx).detach();
            }
            SidebarCommand::OpenPullRequest(url) => {
                model
                    .update(cx, |model, cx| model.open_external(&url, cx))
                    .detach();
            }
            SidebarCommand::DeleteRemoteBranch { remote, branch } => {
                cx.spawn(async move |_, cx| {
                    flows::delete_remote_branch(model, repo, remote, branch, cx).await;
                })
                .detach();
            }
            // Las mutaciones de tags y stashes van de una en una: un doble
            // clic no lanza dos veces la misma.
            SidebarCommand::CreateTag => {
                flows::single_flight(
                    &model,
                    &repo.clone(),
                    "create-tag",
                    cx,
                    async move |model, cx| {
                        flows::create_tag(model, repo, cx).await;
                    },
                )
                .detach();
            }
            SidebarCommand::CheckoutTag(tag) => {
                flows::single_flight(
                    &model,
                    &repo.clone(),
                    "checkout-tag",
                    cx,
                    async move |model, cx| {
                        flows::checkout_tag(model, repo, tag, cx).await;
                    },
                )
                .detach();
            }
            SidebarCommand::ViewTagDetails(tag) => {
                self.dismiss_tag_details_dialog(window, cx);
                self.tag_details_dialog = Some(open_tag_details_dialog(
                    &model, &repo.path, &tag, window, cx,
                ));
            }
            SidebarCommand::CopyTagName(tag) => {
                cx.write_to_clipboard(ClipboardItem::new_string(tag.clone()));
                model.update(cx, |model, cx| {
                    model.notify_success(&format!("Copied '{tag}'"), cx);
                });
            }
            SidebarCommand::PushTag(tag) => {
                flows::single_flight(
                    &model,
                    &repo.clone(),
                    "push-tag",
                    cx,
                    async move |model, cx| {
                        flows::push_tag(model, repo, tag, cx).await;
                    },
                )
                .detach();
            }
            SidebarCommand::DeleteRemoteTag(tag) => {
                flows::single_flight(
                    &model,
                    &repo.clone(),
                    "delete-remote-tag",
                    cx,
                    async move |model, cx| {
                        flows::delete_remote_tag(model, repo, tag, cx).await;
                    },
                )
                .detach();
            }
            SidebarCommand::DeleteTag(tag) => {
                flows::single_flight(
                    &model,
                    &repo.clone(),
                    "delete-tag",
                    cx,
                    async move |model, cx| {
                        flows::delete_tag(model, repo, tag, cx).await;
                    },
                )
                .detach();
            }
            SidebarCommand::AddRemote => self.open_add_remote(window, cx),
            SidebarCommand::CleanupMergedBranches => {
                self.dismiss_cleanup_dialog(window, cx);
                self.cleanup_dialog = Some(open_cleanup_branches_dialog(
                    &model, &repo.id, &repo.path, window, cx,
                ));
            }
            SidebarCommand::EditRemoteUrl(remote) => {
                cx.spawn(async move |_, cx| flows::edit_remote_url(model, repo, remote, cx).await)
                    .detach();
            }
            SidebarCommand::RenameRemote(remote) => {
                cx.spawn(async move |_, cx| flows::rename_remote(model, repo, remote, cx).await)
                    .detach();
            }
            SidebarCommand::RemoveRemote(remote) => {
                cx.spawn(async move |_, cx| flows::remove_remote(model, repo, remote, cx).await)
                    .detach();
            }
            SidebarCommand::SelectStash(hash) => {
                let stash = model
                    .read(cx)
                    .repository(repo.id.as_str())
                    .and_then(|repository| {
                        repository
                            .stashes
                            .iter()
                            .find(|stash| stash.hash == hash)
                            .cloned()
                    });
                if let Some(stash) = stash.filter(|stash| !stash.hash.is_empty()) {
                    cx.emit(SidebarEvent::OpenStashDiff {
                        repo_path: repo.path,
                        stash,
                    });
                }
            }
            // Una sola mutación de stash a la vez: aplicar, sacar o borrar
            // cambia los índices `stash@{n}` de los demás.
            SidebarCommand::BranchFromStash(hash) => {
                flows::single_flight(
                    &model,
                    &repo.clone(),
                    "stash",
                    cx,
                    async move |model, cx| {
                        flows::branch_from_stash(model, repo, hash, cx).await;
                    },
                )
                .detach();
            }
            SidebarCommand::ApplyStash(hash) => {
                flows::single_flight(
                    &model,
                    &repo.clone(),
                    "stash",
                    cx,
                    async move |model, cx| {
                        flows::apply_stash(model, repo, hash, cx).await;
                    },
                )
                .detach();
            }
            SidebarCommand::PopStash(hash) => {
                flows::single_flight(
                    &model,
                    &repo.clone(),
                    "stash",
                    cx,
                    async move |model, cx| {
                        flows::pop_stash(model, repo, hash, cx).await;
                    },
                )
                .detach();
            }
            SidebarCommand::DropStash(hash) => {
                flows::single_flight(
                    &model,
                    &repo.clone(),
                    "stash",
                    cx,
                    async move |model, cx| {
                        flows::drop_stash(model, repo, hash, cx).await;
                    },
                )
                .detach();
            }
            SidebarCommand::UpdateAllSubmodules => {
                flows::update_submodules(&model, repo, Vec::new(), cx).detach();
            }
            SidebarCommand::UpdateSubmodule(path) => {
                if !path.is_empty() {
                    flows::update_submodules(&model, repo, vec![path], cx).detach();
                }
            }
            SidebarCommand::OpenSubmodule(path) => {
                let uninitialized = model
                    .read(cx)
                    .repository(repo.id.as_str())
                    .and_then(|repository| {
                        repository
                            .submodules
                            .iter()
                            .find(|submodule| submodule.path == path)
                    })
                    .is_some_and(|submodule| {
                        submodule.state == mergeon_git_query::SubmoduleState::Uninitialized
                    });
                if uninitialized {
                    model.update(cx, |model, cx| {
                        model.notify_warning("Initialize the submodule before opening it.", cx);
                    });
                    return;
                }
                flows::open_submodule(&model, &repo, &path, cx);
            }
            command @ (SidebarCommand::NewWorktree
            | SidebarCommand::CheckoutInNewWorktree(_)
            | SidebarCommand::OpenWorktree(_)
            | SidebarCommand::RemoveWorktree(_)
            | SidebarCommand::LockWorktree(_)
            | SidebarCommand::UnlockWorktree(_)
            | SidebarCommand::CopyWorktreePath(_)
            | SidebarCommand::PruneWorktrees) => {
                self.run_worktree_command(command, repo, window, cx);
            }
        }
    }

    // ------------------------------------------------------------------
    // Ver una ref en el historial
    // ------------------------------------------------------------------

    /// `handleBranchNavigate`: el historial salta al tip de la rama local, que
    /// queda marcada como "la que estás viendo".
    pub fn view_branch(&mut self, branch_name: &str, cx: &mut Context<Self>) {
        if branch_name.is_empty() {
            return;
        }
        self.navigate_to_ref(
            format!("refs/heads/{branch_name}"),
            Some(branch_name.to_string()),
            cx,
        );
    }

    /// `handleRemoteBranchNavigate`.
    pub fn view_remote_branch(
        &mut self,
        remote_name: &str,
        branch_name: &str,
        cx: &mut Context<Self>,
    ) {
        if remote_name.is_empty() || branch_name.is_empty() {
            return;
        }
        let full_name = format!("{remote_name}/{branch_name}");
        self.navigate_to_ref(format!("refs/remotes/{full_name}"), Some(full_name), cx);
    }

    /// `handleTagNavigate`: el historial salta al commit del tag (sin marcar
    /// ninguna ref como vista).
    pub fn view_tag(&mut self, tag: &str, cx: &mut Context<Self>) {
        if tag.is_empty() {
            return;
        }
        self.navigate_to_ref(format!("refs/tags/{tag}"), None, cx);
    }

    // ------------------------------------------------------------------
    // Checkout
    // ------------------------------------------------------------------

    /// Con una operación a medias (merge, rebase...) no se cambia de rama.
    fn blocks_checkout(&self, cx: &mut Context<Self>) -> bool {
        let blocked = self.model.read(cx).active_repository().and_then(|repo| {
            (repo.repo_lifecycle_state != RepoLifecycleState::Idle).then(|| {
                repo.operation_state
                    .as_ref()
                    .map(|state| state.label.clone())
                    .filter(|label| !label.is_empty())
                    .unwrap_or_else(|| "Git operation".to_string())
            })
        });
        let Some(operation_label) = blocked else {
            return false;
        };

        self.model.update(cx, |model, cx| {
            model.notify_warning(
                &format!(
                    "Resolve or abort the current {} before switching branches.",
                    operation_label.to_lowercase()
                ),
                cx,
            );
        });
        true
    }

    /// `handleBranchClick`: checkout de una rama local. Sin cambios locales va
    /// directo; con ellos pregunta qué hacer.
    fn checkout_branch(&mut self, branch_name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo(cx) else {
            return;
        };
        let Some(branch) = self.branch_or_stub(branch_name, cx) else {
            return;
        };
        if branch.active || branch.is_remote {
            return;
        }
        // Git no deja una rama en dos worktrees: en vez de su error, se dice
        // dónde está y se ofrece abrir ese worktree.
        if let Some(path) = self.branch_worktree(&branch.name).map(str::to_string) {
            let confirm = self.model.update(cx, |model, cx| {
                model.confirm(
                    mergeon_model::dialogs::ConfirmOptions {
                        title: Some(format!("'{}' is open in another worktree", branch.name)),
                        description: Some(mergeon_git_ops::branch_in_other_worktree_message(
                            &branch.name,
                            &path,
                        )),
                        confirm_text: Some("Open that worktree".to_string()),
                        cancel_text: Some("Cancel".to_string()),
                        ..Default::default()
                    },
                    cx,
                )
            });
            let model = self.model.clone();
            cx.spawn(async move |_, cx| {
                if confirm.await {
                    cx.update(|cx| crate::worktrees::open_worktree(&model, &path, cx));
                }
            })
            .detach();
            return;
        }
        if self.blocks_checkout(cx) {
            return;
        }

        let name = branch.name;
        let target = CheckoutTarget::Reference(name.clone());
        self.inspect_then_checkout(repo, name, target, None, window, cx);
    }

    /// `handleRemoteBranchDoubleClick`.
    fn checkout_remote_branch(
        &mut self,
        remote_name: &str,
        branch_name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.active_repo(cx) else {
            return;
        };

        // Ya tienes esa rama en local: lo que quieres es cambiarte a ella, no
        // crear un duplicado.
        let local = self
            .model
            .read(cx)
            .active_repository()
            .and_then(|repository| {
                repository
                    .branches
                    .iter()
                    .find(|branch| branch.name == branch_name)
                    .map(|branch| branch.active)
            });
        match local {
            Some(true) => {
                self.model.update(cx, |model, cx| {
                    model.notify_info(&format!("You are already on '{branch_name}'."), cx);
                });
                return;
            }
            Some(false) => {
                self.checkout_branch(branch_name, window, cx);
                return;
            }
            None => {}
        }

        if self.blocks_checkout(cx) {
            return;
        }

        let full_remote_branch = format!("{remote_name}/{branch_name}");
        let target = CheckoutTarget::RemoteBranch(full_remote_branch.clone());
        self.inspect_then_checkout(
            repo,
            full_remote_branch.clone(),
            target,
            Some(full_remote_branch),
            window,
            cx,
        );
    }

    /// Mira si hay cambios locales y decide: checkout directo, o el diálogo.
    /// Una rama remota (`remote_branch`) pasa siempre por el diálogo, que
    /// confirma que se creará la rama local que la sigue.
    fn inspect_then_checkout(
        &mut self,
        repo: RepoRef,
        label: String,
        target: CheckoutTarget,
        remote_branch: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = self.model.clone();
        let inspect = model.update(cx, |model, cx| {
            model.has_uncommitted_changes(&repo.path, cx)
        });

        cx.spawn_in(window, async move |this, cx| {
            let local_changes = match inspect.await {
                Ok(local_changes) => local_changes,
                Err(error) => {
                    model.update(cx, |model, cx| {
                        let message = if error.message().is_empty() {
                            "Failed to inspect local changes"
                        } else {
                            error.message()
                        };
                        model.notify_error_with(message, &error, cx);
                    });
                    return;
                }
            };

            if !local_changes.has_changes && remote_branch.is_none() {
                cx.update(|_, cx| {
                    flows::checkout(&model, repo, label, target, LocalChangesMode::Keep, cx)
                        .detach();
                })
                .ok();
                return;
            }

            this.update_in(cx, |this, window, cx| {
                this.open_checkout_dialog(
                    PendingCheckout {
                        repo,
                        label,
                        target,
                    },
                    local_changes.has_changes,
                    remote_branch,
                    window,
                    cx,
                );
            })
            .ok();
        })
        .detach();
    }

    fn open_checkout_dialog(
        &mut self,
        pending: PendingCheckout,
        has_local_changes: bool,
        remote_branch: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let options = CheckoutBranchOptions::new(pending.label.clone())
            .has_local_changes(has_local_changes)
            .remote_branch(remote_branch.unwrap_or_default());
        // Uno a la vez: el anterior no se queda huérfano en la pila.
        self.dismiss_checkout_dialog(window, cx);
        let dialog = open_checkout_branch_dialog(options, window, cx);

        self.checkout_dialog_subscription = Some(cx.subscribe(
            &dialog,
            |this, dialog, event: &CheckoutBranchDialogEvent, cx| {
                let Some((open, pending)) = this.checkout_dialog.take() else {
                    return;
                };
                if open != dialog {
                    this.checkout_dialog = Some((open, pending));
                    return;
                }
                // `handleCheckoutBranchSubmit` / `handleCheckoutBranchDialogClose`.
                if let CheckoutBranchDialogEvent::Submit {
                    local_changes_mode, ..
                } = event
                {
                    flows::checkout(
                        &this.model,
                        pending.repo,
                        pending.label,
                        pending.target,
                        *local_changes_mode,
                        cx,
                    )
                    .detach();
                }
                cx.notify();
            },
        ));
        self.checkout_dialog = Some((dialog, pending));
        cx.notify();
    }

    // ------------------------------------------------------------------
    // Branch Actions
    // ------------------------------------------------------------------

    /// `openBranchActionDialog` / `openRemoteBranchActionDialog` / `handleDrop`.
    /// Sin `target`, la rama destino es la actual.
    fn open_branch_action(
        &mut self,
        source: &str,
        target: Option<&str>,
        action: Option<BranchAction>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repository) = self.model.read(cx).active_repository() else {
            return;
        };
        let repo = RepoRef {
            id: repository.id.clone(),
            path: repository.path.clone(),
        };
        let target = target
            .map(str::to_string)
            .unwrap_or_else(|| repository.current_branch.clone().unwrap_or_default());

        if let Some(previous) = self.branch_dialog.take() {
            previous.update(cx, |dialog, cx| dialog.dismiss(window, cx));
        }
        let dialog =
            open_branch_action_dialog(&self.model, &repo.path, source, &target, action, window, cx);

        self.branch_dialog_subscription = Some(cx.subscribe(
            &dialog,
            move |this, dialog, event: &BranchActionDialogEvent, cx| match event {
                BranchActionDialogEvent::ActionCompleted(completed) => {
                    this.branch_action_completed(repo.clone(), (**completed).clone(), cx);
                }
                BranchActionDialogEvent::BrowseComparison { base, head } => {
                    cx.emit(SidebarEvent::OpenComparison(ComparisonRequest::new(
                        repo.id.clone(),
                        repo.path.clone(),
                        base.clone(),
                        head.clone(),
                    )));
                }
                // `handleDialogClose`.
                BranchActionDialogEvent::Close => {
                    if this.branch_dialog.as_ref() == Some(&dialog) {
                        this.branch_dialog = None;
                    }
                    cx.notify();
                }
            },
        ));
        self.branch_dialog = Some(dialog);
        cx.notify();
    }

    /// `handleDrop`: una rama soltada sobre otra abre Branch Actions entre las
    /// dos, sin acción preseleccionada.
    pub fn drop_branch(
        &mut self,
        dragged: &str,
        target: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dragged_branch = None;
        if !dragged.is_empty() && dragged != target {
            self.open_branch_action(dragged, Some(target), None, window, cx);
        }
        cx.notify();
    }

    /// `handleActionCompleted`.
    fn branch_action_completed(
        &mut self,
        repo: RepoRef,
        completed: BranchActionCompleted,
        cx: &mut Context<Self>,
    ) {
        let model = self.model.clone();
        let result: OperationResult = completed.result;
        let operation = completed
            .mutation_operation
            .filter(|operation| !operation.is_empty())
            .or_else(|| result.operation.clone().filter(|name| !name.is_empty()))
            .unwrap_or_else(|| "branch-action".to_string());

        model.update(cx, |model, cx| {
            model.apply_repository_operation_result(repo.id.as_str(), &result, cx);
        });
        cx.spawn(async move |_, cx| {
            flows::refresh(
                &model,
                &repo,
                SyncRequest::new()
                    .status(true)
                    .branches(true)
                    .changes_count(true)
                    .result(result)
                    .operation(operation),
                cx,
            )
            .await;
        })
        .detach();
    }

    // ------------------------------------------------------------------
    // Nueva rama
    // ------------------------------------------------------------------

    /// `handleCreateBranchFromContext` / `handleCreateBranchFromRemoteContext`.
    fn open_create_branch(&mut self, base: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo(cx) else {
            return;
        };
        let model = self.model.clone();
        let inspect = model.update(cx, |model, cx| {
            model.has_uncommitted_changes(&repo.path, cx)
        });

        cx.spawn_in(window, async move |this, cx| {
            let local_changes = match inspect.await {
                Ok(local_changes) => local_changes,
                Err(error) => {
                    model.update(cx, |model, cx| {
                        let message = if error.message().is_empty() {
                            "Failed to inspect local changes"
                        } else {
                            error.message()
                        };
                        model.notify_error_with(message, &error, cx);
                    });
                    return;
                }
            };

            this.update_in(cx, |this, window, cx| {
                this.dismiss_create_branch_dialog(window, cx);
                let dialog = open_create_branch_dialog(
                    CreateBranchOptions::new(base.clone())
                        .has_local_changes(local_changes.has_changes),
                    window,
                    cx,
                );
                this.create_branch_dialog_subscription = Some(cx.subscribe(
                    &dialog,
                    |this, dialog, event: &CreateBranchDialogEvent, cx| {
                        let Some((open, repo, base)) = this.create_branch_dialog.take() else {
                            return;
                        };
                        if open != dialog {
                            this.create_branch_dialog = Some((open, repo, base));
                            return;
                        }
                        // `handleCreateBranchSubmit`.
                        if let CreateBranchDialogEvent::Submit {
                            branch_name,
                            checkout_after_create,
                            local_changes_mode,
                        } = event
                        {
                            flows::create_branch(
                                &this.model,
                                repo,
                                CreateBranchRequest {
                                    branch_name: branch_name.clone(),
                                    checkout_after_create: *checkout_after_create,
                                    local_changes_mode: *local_changes_mode,
                                    base_reference: Some(base),
                                },
                                cx,
                            )
                            .detach();
                        }
                        cx.notify();
                    },
                ));
                this.create_branch_dialog = Some((dialog, repo, base));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ------------------------------------------------------------------
    // Remotos
    // ------------------------------------------------------------------

    /// `handleAddRemote`.
    fn open_add_remote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo(cx) else {
            return;
        };
        if let Some(previous) = self.add_remote_dialog.take() {
            previous.update(cx, |dialog, cx| dialog.dismiss(window, cx));
        }

        // `origin` solo si todavía no existe: si no, el nombre se elige.
        let has_origin = self
            .model
            .read(cx)
            .active_repository()
            .is_some_and(|repository| {
                repository
                    .remotes
                    .iter()
                    .any(|remote| remote.name == "origin")
            });
        let dialog = open_add_remote_dialog(if has_origin { "" } else { "origin" }, "", window, cx);
        self.add_remote_dialog_subscription = Some(cx.subscribe_in(
            &dialog,
            window,
            move |this, dialog, event: &AddRemoteDialogEvent, window, cx| match event {
                AddRemoteDialogEvent::TestConnection { url, .. } => {
                    this.test_remote_connection(&repo, dialog, url, cx);
                }
                AddRemoteDialogEvent::Submit { name, url } => {
                    this.add_remote(repo.clone(), dialog.clone(), name, url, window, cx);
                }
                // `handleAddRemoteDialogClose`.
                AddRemoteDialogEvent::Close => {
                    if this.add_remote_dialog.as_ref() == Some(dialog) {
                        this.add_remote_dialog = None;
                    }
                    cx.notify();
                }
            },
        ));
        self.add_remote_dialog = Some(dialog);
        cx.notify();
    }

    /// `handleTestRemoteConnection`.
    fn test_remote_connection(
        &mut self,
        repo: &RepoRef,
        dialog: &Entity<AddRemoteDialog>,
        url: &str,
        cx: &mut Context<Self>,
    ) {
        if dialog.read(cx).is_submitting() {
            return;
        }

        // Cada prueba lleva su número: si la URL cambia y se prueba otra vez,
        // la respuesta de la anterior se descarta.
        let token = dialog.update(cx, |dialog, cx| dialog.begin_connection_test(cx));
        let url = normalize_remote_url(url);
        let test = self.model.update(cx, |model, cx| {
            model.test_remote_connection(&repo.path, &url, cx)
        });
        let dialog = dialog.downgrade();
        cx.spawn(async move |_, cx| {
            let result = test.await;
            dialog
                .update(cx, |dialog, cx| {
                    dialog.finish_connection_test(token, result, cx);
                })
                .ok();
        })
        .detach();
    }

    /// `handleAddRemoteSubmit`.
    fn add_remote(
        &mut self,
        repo: RepoRef,
        dialog: Entity<AddRemoteDialog>,
        name: &str,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = name.to_string();
        let url = normalize_remote_url(url);
        let window_handle = window.window_handle();

        flows::single_flight(
            &self.model,
            &repo.clone(),
            "add-remote",
            cx,
            async move |model, cx| {
                dialog.update(cx, |dialog, cx| dialog.set_submitting(true, cx));
                let result = model
                    .update(cx, |model, cx| {
                        model.add_remote(&repo.path, &name, &url, false, cx)
                    })
                    .await;
                dialog.update(cx, |dialog, cx| dialog.set_submitting(false, cx));
                // Al registro de operaciones y al estado, como las demás.
                flows::apply(&model, &repo, &result, cx);
                if !result.success() {
                    return;
                }

                // El remoto quedó añadido: se cierra el diálogo y se relee la sección.
                let _ = window_handle.update(cx, |_, window, cx| {
                    dialog.update(cx, |dialog, cx| dialog.close(window, cx));
                });
                flows::refresh(
                    &model,
                    &repo,
                    flows::with_result(SyncRequest::new().remotes(true), &result)
                        .operation("add-remote"),
                    cx,
                )
                .await;
            },
        )
        .detach();
    }
}

/// La carpeta (último tramo) de la ruta de un worktree.
fn worktree_folder(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
}
