//! El inicio (`src/components/dashboard/Dashboard.vue`): repositorios abiertos
//! y recientes agrupados por carpeta, resumen del repositorio elegido (README
//! y estadísticas) y el estado de la cuenta de GitHub.

use std::collections::{HashMap, HashSet};

use gpui_kit::base::{Button as BaseButton, TestSupportExt as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ClickEvent, Context, Div, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, Role, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px, relative, rems,
    transparent_black,
};
use mergeon_app::AppModel;
use mergeon_app::backend::RepositorySummary;
use mergeon_app::model_types::dialogs::ConfirmOptions;
use mergeon_app::model_types::format::format_date_time;
use mergeon_ui::menu::{MenuItem, MenuTriggerExt as _};
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{
    ActivePaletteExt as _, AlertVariant, AppIcon, BadgeVariant, IconTone, Lucide,
    MergeonStyled as _, Radius, SpinnerColor, SpinnerSize, TabItem,
};

use crate::parts::{ch, github_app_icon, plain_button};

use crate::readme::{ReadmeDocument, readme_hash};
use crate::util::{
    format_repository_size, is_permanent_auth_error, is_transient_auth_error, locale_compare,
    parent_folder_name,
};

/// Elementos del submenú "Workspaces" de una fila de repositorio en el inicio.
pub(crate) fn home_repository_workspace_items(
    model: &Entity<AppModel>,
    repo_path: &str,
    cx: &App,
) -> Vec<MenuItem> {
    let workspaces: Vec<(String, String, bool)> = {
        let read = model.read(cx);
        read.workspaces()
            .iter()
            .map(|workspace| {
                (
                    workspace.id.clone(),
                    workspace.name.clone(),
                    read.is_repository_in_workspace(repo_path, &workspace.id),
                )
            })
            .collect()
    };

    let mut sub_items: Vec<MenuItem> = workspaces
        .into_iter()
        .map(|(ws_id, ws_name, is_member)| {
            let model = model.clone();
            let path = repo_path.to_string();
            MenuItem::new(ws_name)
                .checked(is_member)
                .on_click(move |_, cx| {
                    model.update(cx, |model, cx| {
                        if is_member {
                            model.remove_repository_from_workspace(&path, &ws_id, cx);
                        } else {
                            model.add_repository_to_workspace(&path, &ws_id, cx);
                        }
                    });
                })
        })
        .collect();
    if !sub_items.is_empty() {
        sub_items.push(MenuItem::separator());
    }
    let new_model = model.clone();
    let new_path = repo_path.to_string();
    sub_items.push(
        MenuItem::new("New workspace with this repository…")
            .icon(Lucide::Plus)
            .on_click(move |_, cx| {
                new_model.update(cx, |model, cx| {
                    model
                        .prompt_create_workspace(vec![new_path.clone()], cx)
                        .detach();
                });
            }),
    );
    sub_items
}

/// Menú contextual de una fila de repositorio en el inicio: abrirlo y
/// asignarlo (o quitarlo) de uno o varios workspaces.
pub(crate) fn home_repository_menu(
    model: &Entity<AppModel>,
    repo_path: &str,
    cx: &App,
) -> Vec<MenuItem> {
    let open_item = {
        let model = model.clone();
        let path = repo_path.to_string();
        MenuItem::new("Open repository")
            .icon(Lucide::FolderOpen)
            .on_click(move |_, cx| {
                model.update(cx, |model, cx| {
                    model.handle_open_repository(&path, cx).detach();
                });
            })
    };

    let has_workspaces = !model.read(cx).workspaces().is_empty();
    let workspace_entry = if !has_workspaces {
        let model = model.clone();
        let path = repo_path.to_string();
        MenuItem::new("New workspace with this repository…")
            .icon(Lucide::Folders)
            .on_click(move |_, cx| {
                model.update(cx, |model, cx| {
                    model
                        .prompt_create_workspace(vec![path.clone()], cx)
                        .detach();
                });
            })
    } else {
        let sub_items = home_repository_workspace_items(model, repo_path, cx);
        MenuItem::submenu("Workspaces", sub_items).icon(Lucide::Folders)
    };

    vec![open_item, MenuItem::separator(), workspace_entry]
}

/// Un repositorio de la lista del inicio: abierto o reciente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryEntry {
    pub path: String,
    pub name: String,
}

/// Los repositorios de una misma carpeta contenedora.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryGroup {
    pub name: String,
    pub items: Vec<RepositoryEntry>,
}

/// `recentEntries`: abiertos primero y luego recientes, sin repetir ruta.
pub fn recent_entries(model: &AppModel) -> Vec<RepositoryEntry> {
    let mut seen = HashSet::new();
    let open = model
        .open_repositories()
        .iter()
        .map(|repo| (&repo.path, &repo.name));
    let recent = model
        .recent_repositories()
        .iter()
        .map(|repo| (&repo.path, &repo.name));

    open.chain(recent)
        .filter(|(path, _)| !path.is_empty() && seen.insert((*path).clone()))
        .map(|(path, name)| RepositoryEntry {
            path: path.clone(),
            name: name.clone(),
        })
        .collect()
}

/// `repositoryGroups`: las entradas agrupadas por carpeta contenedora, con
/// grupos y repositorios en orden alfabético.
pub fn repository_groups(entries: &[RepositoryEntry]) -> Vec<RepositoryGroup> {
    let mut groups: Vec<RepositoryGroup> = Vec::new();
    for entry in entries {
        let name = parent_folder_name(&entry.path);
        match groups.iter_mut().find(|group| group.name == name) {
            Some(group) => group.items.push(entry.clone()),
            None => groups.push(RepositoryGroup {
                name,
                items: vec![entry.clone()],
            }),
        }
    }

    for group in &mut groups {
        group
            .items
            .sort_by(|left, right| locale_compare(&left.name, &right.name));
    }
    groups.sort_by(|left, right| locale_compare(&left.name, &right.name));
    groups
}

/// `statRows`: las filas de la pestaña Statistics.
pub fn stat_rows(summary: &RepositorySummary) -> Vec<(&'static str, String)> {
    vec![
        ("Changed Files", summary.changes_count.to_string()),
        (
            "Repository Size",
            format_repository_size(summary.repository_size),
        ),
        ("Commits", summary.commit_count.to_string()),
        ("Remotes", summary.remotes.len().to_string()),
        (
            "Initial Commit",
            format_date_time(summary.initial_commit_date.as_deref().unwrap_or_default()),
        ),
        (
            "Last Commit",
            format_date_time(summary.last_commit_date.as_deref().unwrap_or_default()),
        ),
    ]
}

/// Lo que el `watch` del original vigilaba: si cambia, se revisan los grupos
/// desplegados y el repositorio elegido.
type WatchKey = (Option<String>, Vec<String>, Vec<String>, Vec<String>);

/// Lo que el inicio enseña del modelo. El modelo avisa por cualquier cosa (un
/// toast, el progreso de git, lo que se prepara en un repositorio...) y el
/// inicio solo se repinta si cambia algo de esto. Se pinta desde aquí: leer
/// el modelo al pintar haría que la ventana repintara con cada aviso suyo.
#[derive(Debug, Clone, Default, PartialEq)]
struct HomeShown {
    entries: Vec<RepositoryEntry>,
    open_paths: Vec<String>,
    workspaces: Vec<(String, String, usize)>,
    active_workspace_id: Option<String>,
    has_recent: bool,
    has_github_accounts: bool,
    /// Usuario, "requiere reconectar", "requiere instalar la app" y si hay
    /// enlace de instalación, de la cuenta activa.
    github_account: Option<(String, bool, bool, bool)>,
    selected_repo_path: Option<String>,
    loading_summary_path: Option<String>,
    summary: Option<RepositorySummary>,
}

impl HomeShown {
    fn read(model: &AppModel, selected_repo_path: Option<&str>) -> Self {
        let entries = recent_entries(model);
        Self {
            entries,
            open_paths: model
                .open_repositories()
                .iter()
                .map(|repo| repo.path.clone())
                .collect(),
            workspaces: model
                .workspaces()
                .iter()
                .map(|workspace| {
                    (
                        workspace.id.clone(),
                        workspace.name.clone(),
                        workspace.repositories.len(),
                    )
                })
                .collect(),
            active_workspace_id: model.active_workspace_id().map(str::to_string),
            has_recent: !model.recent_repositories().is_empty(),
            has_github_accounts: model.has_github_accounts(),
            github_account: model.active_github_account().map(|account| {
                (
                    account.username.clone(),
                    account.requires_reconnect,
                    account.requires_installation == Some(true),
                    account
                        .install_url
                        .as_deref()
                        .is_some_and(|url| !url.is_empty()),
                )
            }),
            selected_repo_path: selected_repo_path.map(str::to_string),
            loading_summary_path: model.loading_summary_path().map(str::to_string),
            summary: selected_repo_path
                .and_then(|path| model.repository_summary(path))
                .cloned(),
        }
    }
}

/// El inicio de la aplicación. Se muestra cuando no hay repositorio activo.
///
/// Abrir un repositorio y gestionar las cuentas se piden a [`AppModel`]
/// (`handle_open_repository`, `open_settings("accounts")`); esta vista no
/// emite eventos propios.
pub struct HomeView {
    model: Entity<AppModel>,
    selected_repo_path: Option<String>,
    active_summary_tab: SharedString,
    expanded_groups: HashMap<String, bool>,
    refreshing_github_account: bool,
    watch_key: WatchKey,
    /// El README del repositorio elegido, interpretado una vez por contenido.
    readme: Option<ReadmeDocument>,
    panel_scroll: ScrollHandle,
    tree_scroll: ScrollHandle,
    /// Lo que se pinta del modelo.
    shown: HomeShown,
    /// Se pidió un repintado que aún no llegó. Mientras tanto no hace falta
    /// pedir otro ni releer el modelo: o el frame está al caer, o el inicio
    /// no está a la vista (hay un repositorio abierto y la cáscara no lo
    /// pinta). Lo que cambie entretanto marca `stale` y se relee al pintar,
    /// así que al volver al inicio sale al día.
    awaiting_render: bool,
    /// Hubo avisos del modelo sin recoger en `shown`.
    stale: bool,
    renders: usize,
    _subscriptions: Vec<Subscription>,
}

impl HomeView {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        let avatars = crate::avatar::AvatarCache::global(cx);
        let subscriptions = vec![
            cx.observe(&model, |this, _, cx| {
                this.sync_with_model(cx);
                this.model_changed(cx);
            }),
            cx.observe(&avatars, |this, _, cx| this.request_render(cx)),
        ];

        let mut this = Self {
            model,
            selected_repo_path: None,
            active_summary_tab: "summary".into(),
            expanded_groups: HashMap::new(),
            refreshing_github_account: false,
            watch_key: WatchKey::default(),
            readme: None,
            panel_scroll: ScrollHandle::new(),
            tree_scroll: ScrollHandle::new(),
            shown: HomeShown::default(),
            awaiting_render: false,
            stale: false,
            renders: 0,
            _subscriptions: subscriptions,
        };

        // `onMounted`
        this.watch_key = this.current_watch_key(cx);
        this.ensure_expanded_groups(cx);
        this.ensure_selected_repository(cx);
        this.load_selected_summary(cx);
        this.shown = HomeShown::read(this.model.read(cx), this.selected_repo_path.as_deref());
        // El observador empieza a escuchar al final de este ciclo: un aviso
        // del modelo que ya estuviera en cola no le llegaría.
        let view = cx.weak_entity();
        cx.defer(move |cx| {
            view.update(cx, |this, cx| this.model_changed(cx)).ok();
        });
        this
    }

    /// Veces que se pintó (tests).
    #[doc(hidden)]
    pub fn render_count(&self) -> usize {
        self.renders
    }

    /// Pide un repintado, salvo que ya haya uno pedido sin pintar (ver
    /// `awaiting_render`).
    fn request_render(&mut self, cx: &mut Context<Self>) {
        if !self.awaiting_render {
            self.awaiting_render = true;
            cx.notify();
        }
    }

    /// El modelo avisó: se repinta solo si cambió algo de lo que se ve.
    fn model_changed(&mut self, cx: &mut Context<Self>) {
        if self.awaiting_render {
            self.stale = true;
            return;
        }
        let shown = HomeShown::read(self.model.read(cx), self.selected_repo_path.as_deref());
        if shown != self.shown {
            self.shown = shown;
            self.request_render(cx);
        }
    }

    /// Recoge en `shown` lo que se pinta (tras elegir otro repositorio,
    /// fuera de `render`) y repinta si cambió.
    fn refresh_shown(&mut self, cx: &mut Context<Self>) {
        let shown = HomeShown::read(self.model.read(cx), self.selected_repo_path.as_deref());
        if shown != self.shown {
            self.shown = shown;
        }
        cx.notify();
    }

    /// Ruta del repositorio cuyo resumen se muestra.
    pub fn selected_repo_path(&self) -> Option<&str> {
        self.selected_repo_path.as_deref()
    }

    /// El README ya interpretado del repositorio elegido.
    #[cfg(test)]
    pub(crate) fn readme_document(&self) -> Option<&ReadmeDocument> {
        self.readme.as_ref()
    }

    /// Pestaña visible del resumen: `summary` o `statistics`.
    pub fn active_summary_tab(&self) -> &str {
        &self.active_summary_tab
    }

    /// Si el grupo `name` está desplegado.
    pub fn is_group_expanded(&self, name: &str) -> bool {
        self.expanded_groups.get(name).copied().unwrap_or(false)
    }

    /// Si hay un refresco de la cuenta de GitHub en curso.
    pub fn is_refreshing_github_account(&self) -> bool {
        self.refreshing_github_account
    }

    fn entries(&self, cx: &App) -> Vec<RepositoryEntry> {
        recent_entries(self.model.read(cx))
    }

    fn current_watch_key(&self, cx: &App) -> WatchKey {
        let model = self.model.read(cx);
        let entries = recent_entries(model);
        (
            model.active_repository().map(|repo| repo.path.clone()),
            model
                .open_repositories()
                .iter()
                .map(|repo| repo.path.clone())
                .collect(),
            entries.iter().map(|entry| entry.path.clone()).collect(),
            repository_groups(&entries)
                .into_iter()
                .map(|group| group.name)
                .collect(),
        )
    }

    /// El `watch` del original: al cambiar los repositorios se despliegan los
    /// grupos nuevos y, si el elegido ya no es candidato, se elige otro.
    fn sync_with_model(&mut self, cx: &mut Context<Self>) {
        let key = self.current_watch_key(cx);
        if key == self.watch_key {
            return;
        }
        self.watch_key = key;

        self.ensure_expanded_groups(cx);
        let previous = self.selected_repo_path.clone();
        self.ensure_selected_repository(cx);
        if self.selected_repo_path.is_some() && self.selected_repo_path != previous {
            self.load_selected_summary(cx);
        }
    }

    /// `ensureSelectedRepository`.
    fn ensure_selected_repository(&mut self, cx: &App) {
        let model = self.model.read(cx);
        let candidates: Vec<String> = [
            model.active_repository().map(|repo| repo.path.clone()),
            model
                .open_repositories()
                .first()
                .map(|repo| repo.path.clone()),
            recent_entries(model)
                .into_iter()
                .next()
                .map(|entry| entry.path),
        ]
        .into_iter()
        .flatten()
        .filter(|path| !path.is_empty())
        .collect();

        if self
            .selected_repo_path
            .as_ref()
            .is_some_and(|selected| candidates.contains(selected))
        {
            return;
        }
        self.selected_repo_path = candidates.into_iter().next();
    }

    /// `ensureExpandedGroups`: un grupo nuevo empieza desplegado.
    fn ensure_expanded_groups(&mut self, cx: &App) {
        for group in repository_groups(&self.entries(cx)) {
            self.expanded_groups.entry(group.name).or_insert(true);
        }
    }

    fn load_selected_summary(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selected_repo_path.clone() else {
            return;
        };
        self.model
            .update(cx, |model, cx| model.load_repository_summary(&path, cx))
            .detach();
    }

    /// `selectRepository`.
    pub fn select_repository(&mut self, repo_path: &str, cx: &mut Context<Self>) {
        self.selected_repo_path = Some(repo_path.to_string());
        self.active_summary_tab = "summary".into();
        if let Some(readme) = &self.readme {
            readme.scroll_to_top();
        }
        self.load_selected_summary(cx);
        self.refresh_shown(cx);
    }

    /// Cambia la pestaña del resumen (`summary` o `statistics`).
    pub fn set_summary_tab(&mut self, tab: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.active_summary_tab = tab.into();
        cx.notify();
    }

    /// `toggleRepositoryGroup`.
    pub fn toggle_repository_group(&mut self, group_name: &str, cx: &mut Context<Self>) {
        let expanded = self.is_group_expanded(group_name);
        self.expanded_groups
            .insert(group_name.to_string(), !expanded);
        cx.notify();
    }

    fn open_repository(&mut self, repo_path: &str, cx: &mut Context<Self>) {
        let repo_path = repo_path.to_string();
        self.model
            .update(cx, |model, cx| model.handle_open_repository(&repo_path, cx))
            .detach();
    }

    /// `handleOpenSelectedRepository`.
    fn handle_open_selected_repository(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.selected_repo_path.clone() {
            self.open_repository(&path, cx);
        }
    }

    /// `handleOpenRepositoryFromList`.
    fn handle_open_repository_from_list(&mut self, repo_path: &str, cx: &mut Context<Self>) {
        if repo_path.is_empty() {
            return;
        }
        self.selected_repo_path = Some(repo_path.to_string());
        self.open_repository(repo_path, cx);
        self.refresh_shown(cx);
    }

    fn manage_accounts(&mut self, cx: &mut Context<Self>) {
        self.model
            .update(cx, |model, cx| model.open_settings("accounts", cx));
    }

    /// `handleInstallGitHubApp`.
    fn handle_install_github_app(&mut self, cx: &mut Context<Self>) {
        let Some(install_url) = self
            .model
            .read(cx)
            .active_github_account()
            .and_then(|account| account.install_url.clone())
            .filter(|url| !url.is_empty())
        else {
            return;
        };
        self.model
            .update(cx, |model, cx| model.open_external(&install_url, cx))
            .detach();
    }

    /// `handleRefreshGitHubAccount`.
    fn handle_refresh_github_account(&mut self, cx: &mut Context<Self>) {
        if self.refreshing_github_account {
            return;
        }
        let Some(account_id) = self
            .model
            .read(cx)
            .active_github_account_id()
            .map(str::to_string)
        else {
            return;
        };

        self.refreshing_github_account = true;
        cx.notify();

        let model = self.model.clone();
        let refresh = model.update(cx, |model, cx| model.refresh_account(&account_id, cx));
        cx.spawn(async move |this, cx| {
            let result = refresh.await;
            model.update(cx, |model, cx| match result {
                Ok(_) => {
                    model.notify_success("GitHub account refreshed", cx);
                }
                Err(error) if is_permanent_auth_error(&error) => {
                    model.notify_error(
                        "GitHub session expired. Reconnect the account to continue.",
                        cx,
                    );
                }
                Err(error) if is_transient_auth_error(&error) => {
                    model.notify_error("GitHub is unreachable. Try again.", cx);
                }
                Err(error) => {
                    let message = if error.message.is_empty() {
                        "Unknown error"
                    } else {
                        &error.message
                    };
                    model.notify_error(&format!("Failed to refresh GitHub account: {message}"), cx);
                }
            });
            this.update(cx, |this, cx| {
                this.refreshing_github_account = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `handleRemoveRecent`.
    fn handle_remove_recent(&mut self, repo_path: &str, cx: &mut Context<Self>) {
        let repo_path = repo_path.to_string();
        let model = self.model.clone();
        let removal = model.update(cx, |model, cx| {
            model.remove_recent_repository(&repo_path, cx)
        });
        cx.spawn(async move |this, cx| {
            if let Err(error) = removal.await {
                model.update(cx, |model, cx| {
                    model.notify_error(&format!("Failed to remove repository: {error}"), cx);
                });
                return;
            }

            model.update(cx, |model, cx| {
                model.clear_repository_summary(&repo_path, cx);
            });
            this.update(cx, |this, cx| {
                if this.selected_repo_path.as_deref() == Some(repo_path.as_str()) {
                    this.selected_repo_path = None;
                    this.ensure_selected_repository(cx);
                    this.load_selected_summary(cx);
                }
                this.refresh_shown(cx);
            })
            .ok();
            model.update(cx, |model, cx| {
                model.notify_success("Repository removed from recent list", cx);
            });
        })
        .detach();
    }

    /// `handleClearRecent`.
    fn handle_clear_recent(&mut self, cx: &mut Context<Self>) {
        let model = self.model.clone();
        let confirmation = model.update(cx, |model, cx| {
            model.confirm(
                ConfirmOptions {
                    title: Some("Clear recent repositories".to_string()),
                    description: Some(
                        "This will remove every repository from the recent list.".to_string(),
                    ),
                    confirm_text: Some("Clear all".to_string()),
                    cancel_text: Some("Cancel".to_string()),
                    variant: Some("destructive".to_string()),
                    ..Default::default()
                },
                cx,
            )
        });
        cx.spawn(async move |this, cx| {
            if !confirmation.await {
                return;
            }

            let cleared = model
                .update(cx, |model, cx| model.clear_recent_repositories(cx))
                .await;
            if let Err(error) = cleared {
                log::error!("could not clear the recent repositories: {error}");
                return;
            }

            model.update(cx, |model, cx| {
                let open: HashSet<String> = model
                    .open_repositories()
                    .iter()
                    .map(|repo| repo.path.clone())
                    .collect();
                model.retain_repository_summaries(cx, |repo_path, _| open.contains(repo_path));
            });
            this.update(cx, |this, cx| {
                this.ensure_selected_repository(cx);
                this.load_selected_summary(cx);
                cx.notify();
            })
            .ok();
            model.update(cx, |model, cx| {
                model.notify_success("Recent repositories cleared", cx);
            });
        })
        .detach();
    }

    // ------------------------------------------------------------------
    // Render
    // ------------------------------------------------------------------

    /// `.group-header`
    fn group_header(label: &'static str, cx: &App) -> Div {
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(rems(0.5))
            .mb(rems(0.35))
            .text_size(rems(0.75))
            .line_height(relative(1.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(with_alpha(cx.palette().muted_foreground, 0.6))
            .child(label)
    }

    /// `.sidebar-empty`
    fn sidebar_note(text: impl Into<SharedString>, cx: &App) -> Div {
        div()
            .text_size(rems(0.75))
            .line_height(relative(1.5))
            .text_color(cx.palette().muted_foreground)
            .child(text.into())
    }

    /// `.repo-tree-group-toggle, .repo-tree-row`
    fn tree_row(id: SharedString, window: &mut Window, cx: &mut App) -> BaseButton {
        plain_button(id, Radius::Md.px(cx), window, cx)
            .w_full()
            .min_h(rems(1.625))
            .px(rems(0.36))
            .py(rems(0.2))
            .gap(rems(0.42))
            .justify_start()
            .line_height(relative(1.2))
            .border_1()
            .border_color(transparent_black())
            .text_color(cx.palette().foreground)
    }

    fn handle_create_workspace(&mut self, cx: &mut Context<Self>) {
        self.model
            .update(cx, |model, cx| {
                model.prompt_create_workspace(Vec::new(), cx)
            })
            .detach();
    }

    fn handle_select_workspace(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.model
            .update(cx, |model, cx| model.select_workspace(id, cx))
            .detach();
    }

    fn render_workspaces(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Div> {
        if self.shown.workspaces.is_empty() {
            return None;
        }
        let palette = *cx.palette();
        let active_id = self.shown.active_workspace_id.as_deref();
        let total_open = self.shown.open_paths.len();
        let workspaces = self.shown.workspaces.clone();
        let model = self.model.clone();

        let workspace_row = |id: SharedString,
                             icon: Lucide,
                             name: String,
                             count: usize,
                             selected: bool,
                             window: &mut Window,
                             cx: &mut App| {
            Self::tree_row(id, window, cx)
                .justify_between()
                .selected(selected)
                .map(|this| {
                    if selected {
                        this.bg(palette.sidebar_row_selected)
                            .font_weight(FontWeight::MEDIUM)
                    } else {
                        this.hover(|style| style.bg(palette.sidebar_row_hover))
                    }
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rems(0.38))
                        .min_w_0()
                        .child(mergeon_ui::icon(icon).size(px(13.)).tone(if selected {
                            IconTone::Active
                        } else {
                            IconTone::Muted
                        }))
                        .child(
                            div()
                                .truncate()
                                .text_size(rems(0.8125))
                                .font_weight(FontWeight::MEDIUM)
                                .child(SharedString::from(name)),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(rems(0.75))
                        .text_color(palette.muted_foreground)
                        .child(SharedString::from(count.to_string())),
                )
        };

        let all_row = workspace_row(
            "home-workspace-all".into(),
            Lucide::Folders,
            "All".to_string(),
            total_open,
            active_id.is_none(),
            window,
            cx,
        )
        .accessibility_label("Workspace All")
        .on_click(cx.listener(|this, _, _, cx| this.handle_select_workspace(None, cx)));

        Some(
            div()
                .mb(rems(0.75))
                .pb(rems(0.6))
                .border_b_1()
                .border_color(palette.border)
                .child(
                    Self::group_header("Workspaces", cx).child(
                        mergeon_ui::button("home-new-workspace")
                            .ghost()
                            .xs()
                            .icon(Lucide::Plus)
                            .tooltip("New workspace…")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.handle_create_workspace(cx);
                            })),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(rems(0.075))
                        .child(all_row)
                        .children(workspaces.into_iter().map(|(ws_id, ws_name, repo_count)| {
                            let selected = active_id == Some(ws_id.as_str());
                            let select_id = ws_id.clone();
                            let menu_id = ws_id.clone();
                            let menu_model = model.clone();
                            workspace_row(
                                SharedString::from(format!("home-workspace:{ws_id}")),
                                Lucide::Folder,
                                ws_name.clone(),
                                repo_count,
                                selected,
                                window,
                                cx,
                            )
                            .accessibility_label(format!("Workspace {ws_name}"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.handle_select_workspace(Some(select_id.clone()), cx);
                            }))
                            .context_menu_items(move |_, _| {
                                let rename_model = menu_model.clone();
                                let rename_id = menu_id.clone();
                                let delete_model = menu_model.clone();
                                let delete_id = menu_id.clone();
                                vec![
                                    MenuItem::new("Rename workspace…")
                                        .icon(Lucide::Pencil)
                                        .on_click(move |_, cx| {
                                            rename_model.update(cx, |model, cx| {
                                                model
                                                    .prompt_rename_workspace(&rename_id, cx)
                                                    .detach();
                                            });
                                        }),
                                    MenuItem::new("Delete workspace")
                                        .icon(Lucide::Trash)
                                        .danger()
                                        .on_click(move |_, cx| {
                                            delete_model.update(cx, |model, cx| {
                                                model
                                                    .confirm_delete_workspace(&delete_id, cx)
                                                    .detach();
                                            });
                                        }),
                                ]
                            })
                        })),
                )
                .text_color(palette.foreground),
        )
    }

    fn render_repositories(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let palette = *cx.palette();
        let groups = repository_groups(&self.shown.entries);
        let open_paths: HashSet<String> = self.shown.open_paths.iter().cloned().collect();
        let has_recent = self.shown.has_recent;
        let has_workspaces = !self.shown.workspaces.is_empty();

        let tree = if groups.is_empty() {
            Self::sidebar_note("No repositories available", cx).into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap(rems(0.1))
                .children(groups.into_iter().map(|group| {
                    let expanded = self.is_group_expanded(&group.name);
                    let name = group.name.clone();
                    div()
                        .flex()
                        .flex_col()
                        .gap(rems(0.075))
                        .child(
                            Self::tree_row(
                                SharedString::from(format!("repo-group:{}", group.name)),
                                window,
                                cx,
                            )
                            .accessibility_label(format!("Toggle repository group {}", group.name))
                            .text_size(rems(0.8125))
                            .font_weight(FontWeight::SEMIBOLD)
                            .hover(|style| style.bg(palette.sidebar_row_hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_repository_group(&name, cx);
                            }))
                            .child(
                                mergeon_ui::icon(if expanded {
                                    Lucide::ChevronDown
                                } else {
                                    Lucide::ChevronRight
                                })
                                .size(px(12.))
                                .tone(IconTone::Muted),
                            )
                            .child(
                                mergeon_ui::icon(Lucide::Folder)
                                    .size(px(14.))
                                    .tone(IconTone::Active),
                            )
                            .child(div().child(SharedString::from(group.name.clone()))),
                        )
                        .when(expanded, |this| {
                            this.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(rems(0.075))
                                    .pl(rems(1.1))
                                    .children(group.items.into_iter().map(|repo| {
                                        let open = open_paths.contains(&repo.path);
                                        self.render_repository_row(repo, open, window, cx)
                                    })),
                            )
                        })
                }))
                .into_any_element()
        };

        div()
            .child(
                Self::group_header("Repositories", cx).when(!has_workspaces, |header| {
                    header.child(
                        mergeon_ui::button("home-new-workspace")
                            .ghost()
                            .xs()
                            .icon(Lucide::Folders)
                            .tooltip("New workspace…")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.handle_create_workspace(cx);
                            })),
                    )
                }),
            )
            .child(tree)
            .when(has_recent, |this| {
                this.child(
                    div().flex().mt(rems(0.35)).child(
                        mergeon_ui::button("clear-recent")
                            .ghost()
                            .sm()
                            // `.clear-btn`
                            .h(rems(1.35))
                            .px(rems(0.4))
                            .text_size(rems(0.75))
                            .label("Clear recent")
                            .on_click(cx.listener(|this, _, _, cx| this.handle_clear_recent(cx))),
                    ),
                )
            })
            .text_color(palette.foreground)
    }

    fn render_repository_row(
        &self,
        repo: RepositoryEntry,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let palette = *cx.palette();
        let selected = self.selected_repo_path.as_deref() == Some(repo.path.as_str());
        let path = repo.path.clone();
        let menu_model = self.model.clone();
        let menu_path = repo.path.clone();

        Self::tree_row(
            SharedString::from(format!("repo:{}", repo.path)),
            window,
            cx,
        )
        .accessibility_label(format!("Repository {}", repo.name))
        .justify_between()
        .selected(selected)
        .map(|this| {
            // `.repo-tree-row.selected` va después de `:hover` en la hoja
            // de estilos: una fila elegida no cambia al pasar el cursor.
            if selected {
                this.bg(palette.sidebar_row_selected)
                    .font_weight(FontWeight::MEDIUM)
            } else {
                this.hover(|style| style.bg(palette.sidebar_row_hover))
            }
        })
        .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
            // `@click` selecciona; `@dblclick` además lo abre.
            this.select_repository(&path, cx);
            if event.click_count() == 2 {
                this.handle_open_repository_from_list(&path, cx);
            }
        }))
        .child(
            div()
                .flex()
                .items_center()
                .gap(rems(0.38))
                .min_w_0()
                .child(
                    mergeon_ui::icon(Lucide::FolderGit2)
                        .size(px(13.))
                        .tone(if selected {
                            IconTone::Active
                        } else {
                            IconTone::Muted
                        }),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(rems(0.8125))
                        .font_weight(FontWeight::MEDIUM)
                        .child(SharedString::from(repo.name)),
                ),
        )
        .when(open, |this| {
            this.child(
                // `.repo-open-dot`: punto con un cerco del color de la tarjeta.
                div()
                    .flex_shrink_0()
                    .size(rems(0.45))
                    .rounded_full()
                    .bg(palette.primary)
                    .shadow(vec![
                        gpui_kit::BoxShadow::new(px(0.), px(0.), palette.card.into())
                            .spread_radius(px(2.)),
                    ]),
            )
        })
        .context_menu_items(move |_, cx| home_repository_menu(&menu_model, &menu_path, cx))
    }

    fn render_github(&mut self, cx: &mut Context<Self>) -> Div {
        let has_accounts = self.shown.has_github_accounts;
        let (username, requires_reconnect, requires_installation, has_install_url) =
            match self.shown.github_account.clone() {
                Some((username, reconnect, installation, install_url)) => (
                    Some(username).filter(|username| !username.is_empty()),
                    reconnect,
                    installation,
                    install_url,
                ),
                None => (None, false, false, false),
            };
        let refreshing = self.refreshing_github_account;

        let note = if !has_accounts {
            "Add an account to clone repositories from GitHub.".to_string()
        } else if requires_reconnect {
            "Re-authentication required for this GitHub account.".to_string()
        } else if requires_installation {
            "GitHub App installation required.".to_string()
        } else {
            username
                .clone()
                .unwrap_or_else(|| "GitHub ready".to_string())
        };

        div()
            .child(Self::group_header("GitHub", cx).when(has_accounts, |this| {
                this.child(
                    mergeon_ui::badge(username.unwrap_or_else(|| "Connected".to_string()))
                        .variant(BadgeVariant::Secondary),
                )
            }))
            .child(
                Self::sidebar_note(note.clone(), cx)
                    .id("github-note")
                    .role(Role::Status)
                    .aria_label(note)
                    .test_support(),
            )
            .child(
                // `.github-actions`
                div()
                    .flex()
                    .flex_wrap()
                    .gap(rems(0.32))
                    .mt(rems(0.5))
                    .when(requires_reconnect, |this| {
                        this.child(
                            mergeon_ui::button("github-reconnect")
                                .sm()
                                .label("Reconnect")
                                .on_click(cx.listener(|this, _, _, cx| this.manage_accounts(cx))),
                        )
                    })
                    .when(requires_installation && has_install_url, |this| {
                        this.child(
                            mergeon_ui::button("github-install-app")
                                .outline()
                                .sm()
                                .label("Install App")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.handle_install_github_app(cx);
                                })),
                        )
                    })
                    .when(has_accounts, |this| {
                        this.child(
                            mergeon_ui::button("github-refresh")
                                .outline()
                                .sm()
                                .loading(refreshing)
                                .disabled(refreshing)
                                .label("Refresh")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.handle_refresh_github_account(cx);
                                })),
                        )
                    })
                    .child(
                        mergeon_ui::button("github-manage")
                            .ghost()
                            .sm()
                            .label("Manage")
                            .on_click(cx.listener(|this, _, _, cx| this.manage_accounts(cx))),
                    ),
            )
    }

    /// `.summary-topline`: rótulo, nombre y ruta.
    fn render_topline_copy(summary: &RepositorySummary, cx: &App) -> Div {
        let palette = *cx.palette();
        div()
            .min_w_0()
            .child(
                // `.dashboard-kicker`
                div()
                    .text_size(rems(0.6875))
                    // 16.5px en el original; GPUI redondea la línea a píxeles
                    // enteros y con el título (27.6px) suman lo mismo.
                    .line_height(rems(1.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(with_alpha(palette.muted_foreground, 0.7))
                    .child("Repository"),
            )
            .child(
                div()
                    .text_size(rems(1.5))
                    .line_height(rems(1.75))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.foreground)
                    .child(SharedString::from(summary.name.clone())),
            )
            .child(
                // `.summary-path`
                div()
                    .mt(rems(0.16))
                    .text_size(rems(0.875))
                    .line_height(relative(1.5))
                    .text_color(palette.muted_foreground)
                    .child(SharedString::from(summary.repo_path.clone())),
            )
    }

    /// `.summary-branch, .summary-remote`
    fn meta_chip(icon: AppIcon, tone: IconTone, label: String, cx: &App) -> Div {
        let palette = *cx.palette();
        div()
            .flex()
            .items_center()
            .gap(rems(0.35))
            .px(rems(0.4))
            .py(rems(0.16))
            .radius(Radius::Sm, cx)
            .bg(palette.accent)
            .text_color(palette.foreground)
            .text_size(rems(0.75))
            .line_height(relative(1.5))
            .font_weight(FontWeight::MEDIUM)
            .child(icon.size(px(13.)).tone(tone))
            .child(SharedString::from(label))
    }

    fn render_invalid(&self, summary: &RepositorySummary, cx: &Context<Self>) -> AnyElement {
        let repo_path = summary.repo_path.clone();
        div()
            .id("summary-invalid")
            .test_support()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(rems(0.8))
            .child(Self::render_topline_copy(summary, cx))
            .child(
                mergeon_ui::alert(AlertVariant::Error)
                    .title("Repository not available")
                    .description("This path no longer exists or is not a valid Git repository."),
            )
            .child(
                // `.summary-actions`
                div().flex().gap(rems(0.4)).pt(rems(0.1)).child(
                    mergeon_ui::button("remove-from-recent")
                        .outline()
                        .sm()
                        .label("Remove from Recent")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.handle_remove_recent(&repo_path, cx);
                        })),
                ),
            )
            .into_any_element()
    }

    fn render_statistics(summary: &RepositorySummary, cx: &App) -> Div {
        let palette = *cx.palette();
        let hairline = with_alpha(palette.border, 0.7);
        let rows = stat_rows(summary);
        let last = rows.len().saturating_sub(1);

        // `tableVariants`: `rounded-xl border border-border/70`, cabecera
        // `bg-muted/50`, filas `border-b` con `hover:bg-muted/50`.
        div()
            .w_full()
            .overflow_hidden()
            .radius(Radius::Xl, cx)
            .border_1()
            .border_color(hairline)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h_9()
                    .px_3()
                    .border_b_1()
                    .border_color(hairline)
                    .bg(with_alpha(palette.muted, 0.5))
                    .tw_text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.muted_foreground)
                    .child("Metric")
                    .child("Value"),
            )
            .children(rows.into_iter().enumerate().map(|(index, (label, value))| {
                div()
                    .id(label)
                    .test_support()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .when(index != last, |this| {
                        this.border_b_1().border_color(hairline)
                    })
                    .hover(|style| style.bg(with_alpha(palette.muted, 0.5)))
                    .tw_text_sm()
                    .whitespace_nowrap()
                    .child(label)
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(SharedString::from(value)),
                    )
            }))
    }

    fn render_summary(
        &mut self,
        summary: &RepositorySummary,
        is_open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *cx.palette();
        let tab = self.active_summary_tab.clone();

        let panel: AnyElement = if tab.as_ref() == "statistics" {
            Self::render_statistics(summary, cx).into_any_element()
        } else if let Some(readme) = summary
            .readme_content
            .as_deref()
            .filter(|readme| !readme.is_empty())
        {
            if self
                .readme
                .as_ref()
                .is_none_or(|document| document.hash() != readme_hash(readme))
            {
                self.readme = Some(ReadmeDocument::new(readme));
            }
            let document = self.readme.as_ref().expect("recién creado");

            // `.readme-panel`
            div()
                .relative()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .border_1()
                .border_color(palette.border)
                .radius(Radius::Xl, cx)
                .bg(with_alpha(palette.background, 0.55))
                .child(div().id("readme-panel").test_support().size_full().child(
                    // GPUI solo aplica el relleno vertical de una
                    // lista: el horizontal lo pone cada fila.
                    document.element().size_full().py(rems(0.75)),
                ))
                .vertical_scrollbar(document.list_state())
                .into_any_element()
        } else {
            // `.summary-text`
            div()
                .id("summary-text")
                .test_support()
                // `max-width: 64ch`
                .max_w(ch(64., window.rem_size() * 0.875, window, cx))
                .text_color(palette.muted_foreground)
                .text_size(rems(0.875))
                .line_height(relative(1.45))
                .child(SharedString::from(format!(
                    "{} is ready in this workspace. Review local changes, branch state, remotes \
                     and recent activity before opening it in the main Git views.",
                    summary.name
                )))
                .into_any_element()
        };

        div()
            .id("summary-content")
            .test_support()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(rems(0.8))
            .child(
                div()
                    .child(
                        // `.summary-topline-head`
                        div()
                            .flex()
                            .items_start()
                            .justify_between()
                            .gap(rems(0.75))
                            .child(Self::render_topline_copy(summary, cx))
                            .when(!is_open, |this| {
                                this.child(
                                    mergeon_ui::button("open-repository")
                                        .sm()
                                        .label("Open Repository")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.handle_open_selected_repository(cx);
                                        })),
                                )
                            }),
                    )
                    .child(
                        // `.summary-inline-meta`
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(rems(0.45))
                            .mt(rems(0.5))
                            .child(Self::meta_chip(
                                mergeon_ui::icon(Lucide::GitBranch),
                                IconTone::Active,
                                summary
                                    .current_branch
                                    .clone()
                                    .filter(|branch| !branch.is_empty())
                                    .unwrap_or_else(|| "HEAD".to_string()),
                                cx,
                            ))
                            .when(!summary.remotes.is_empty(), |this| {
                                this.child(Self::meta_chip(
                                    github_app_icon(),
                                    IconTone::Muted,
                                    summary.remotes.join(", "),
                                    cx,
                                ))
                            }),
                    ),
            )
            .child(
                div().flex().child(
                    mergeon_ui::tabs("summary-tabs")
                        .item(TabItem::new("summary", "Summary"))
                        .item(TabItem::new("statistics", "Statistics"))
                        .selected(tab)
                        .on_change(cx.listener(|this, key: &SharedString, _, cx| {
                            this.set_summary_tab(key.clone(), cx);
                        })),
                ),
            )
            .child(
                // `.summary-tab-panel`
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .gap(rems(0.8))
                    .child(panel),
            )
            .into_any_element()
    }

    fn render_main(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(selected) = self.selected_repo_path.clone() else {
            // `.dashboard-empty`
            return div()
                .flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .child(
                    mergeon_ui::empty_state("No repository selected")
                        .description(
                            "Choose an open or recent repository from the sidebar to see its \
                             summary.",
                        )
                        .icon(Lucide::FolderGit2),
                )
                .into_any_element();
        };

        let shown = &self.shown;
        if shown.loading_summary_path.as_deref() == Some(selected.as_str()) {
            return div()
                .id("summary-loading")
                .test_support()
                .flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .child(
                    mergeon_ui::spinner()
                        .size(SpinnerSize::Lg)
                        .color(SpinnerColor::Primary),
                )
                .into_any_element();
        }

        // `shown.summary` es la del elegido (`render` lo relee si cambió).
        let Some(summary) = shown.summary.clone() else {
            return div().into_any_element();
        };
        let is_open = shown.open_paths.contains(&selected);

        if summary.is_valid {
            self.render_summary(&summary, is_open, window, cx)
        } else {
            self.render_invalid(&summary, cx)
        }
    }
}

impl Render for HomeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        self.awaiting_render = false;
        // Si quedó algo sin recoger (avisos con el inicio tapado, otro
        // repositorio elegido) no se lee el modelo aquí, que registraría la
        // entidad y repintaría la ventana con cualquier aviso suyo: se recoge
        // justo después y, si cambió, se repinta.
        if self.stale || self.shown.selected_repo_path != self.selected_repo_path {
            self.stale = false;
            let view = cx.weak_entity();
            cx.defer(move |cx| {
                view.update(cx, |this, cx| this.model_changed(cx)).ok();
            });
        }
        let palette = *cx.palette();
        // `@media (max-width: 700px)` y `(max-width: 600px)` sobre la ventana.
        let viewport = window.viewport_size().width;
        let stacked = viewport <= px(600.);
        let sidebar_width = if viewport <= px(700.) {
            px(250.)
        } else {
            px(260.)
        };

        let workspaces = self.render_workspaces(window, cx);
        let repositories = self.render_repositories(window, cx);
        let github = self.render_github(cx);
        let main = self.render_main(window, cx);

        // `.dashboard-sidebar`
        let sidebar = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .min_h_0()
            .overflow_hidden()
            .bg(palette.sidebar)
            .map(|this| {
                if stacked {
                    this.w_full().border_b_1()
                } else {
                    this.w(sidebar_width).border_r_1()
                }
            })
            .border_color(palette.border)
            .child(
                // `.sidebar-group`
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .border_b_1()
                    .border_color(palette.border)
                    .child(
                        div()
                            .id("repositories")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.tree_scroll)
                            .px(px(12.))
                            .py(rems(0.5))
                            .children(workspaces)
                            .child(repositories),
                    )
                    .vertical_scrollbar(&self.tree_scroll),
            )
            .child(
                // `.github-compact`
                div()
                    .flex_shrink_0()
                    .px(px(12.))
                    .py(rems(0.5))
                    .child(github),
            );

        // `.dashboard-main` + `.dashboard-summary-panel`
        let content = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(
                div()
                    .id("summary-panel")
                    .flex()
                    .flex_col()
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.panel_scroll)
                    .pt(px(16.))
                    .px(px(16.))
                    .pb(px(24.))
                    .child(main),
            )
            .vertical_scrollbar(&self.panel_scroll);

        // `.dashboard` + `.dashboard-shell`
        div()
            .id("dashboard")
            .flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(palette.background)
            .text_color(palette.foreground)
            .child(
                div()
                    .flex()
                    .when(stacked, |this| this.flex_col())
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .child(sidebar)
                    .child(content),
            )
    }
}
