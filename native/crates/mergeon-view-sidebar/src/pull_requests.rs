//! Sección "Pull requests" del sidebar: los PRs abiertos del repositorio
//! activo en GitHub, con sus checks y su revisión (solo consulta). Los datos
//! los mantiene `AppModel` (`mergeon_app::pull_requests`); esta vista los
//! pinta y lanza sus órdenes.
//!
//! - Sin remoto de github.com (o sin acceso) la sección no se pinta.
//! - Sin cuenta: "Sign in to GitHub to see pull requests" (abre Settings).
//! - Clic en un PR: se abre en el navegador. Menú: abrir, checkout de su
//!   rama, copiar el enlace y, si la rama es del propio repositorio,
//!   compararla con la actual (Branch Actions).

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, Div, Entity, FontWeight, Hsla,
    SharedString, Subscription, TestSupportExt as _, WeakEntity, Window, div, px, relative, rems,
};
use mergeon_app::{
    AppModel, PullRequestCheckout, PullRequestItem, PullRequestsAvailability, RepoPullRequests,
};
use mergeon_github::{CheckState, CheckSummary, ReviewDecision, ReviewSummary};
use mergeon_ui::menu::{MenuItem, MenuTriggerExt as _};
use mergeon_ui::{ActivePaletteExt as _, IconTone, Lucide};

use crate::menus::SidebarCommand;
use crate::section;
use crate::sidebar::Sidebar;
use crate::virtual_rows::{RowHeights, virtual_rows};

/// Alto de una fila (dos líneas).
pub const PULL_REQUEST_ROW_HEIGHT: f32 = 42.;

/// Clave en `settings.sidebarSections` de si la sección está desplegada.
const EXPANDED_KEY: &str = "pullRequests";

/// Lo que hace una entrada del menú de un PR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestCommand {
    OpenInBrowser(u64),
    Checkout(u64),
    CopyLink(u64),
    CompareWithCurrent(u64),
}

/// Una entrada del menú de un PR (o un separador).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullRequestMenuEntry {
    Item {
        label: &'static str,
        icon: Lucide,
        command: PullRequestCommand,
    },
    Separator,
}

/// El menú de un PR. "Compare with current branch" solo si la rama vive en
/// el propio repositorio (la de un fork no tiene rama remota que comparar).
pub fn pull_request_menu(item: &PullRequestItem) -> Vec<PullRequestMenuEntry> {
    let number = item.pull.number;
    let entry = |label, icon, command| PullRequestMenuEntry::Item {
        label,
        icon,
        command,
    };
    let mut entries = vec![
        entry(
            "Open in browser",
            Lucide::ExternalLink,
            PullRequestCommand::OpenInBrowser(number),
        ),
        entry(
            "Checkout PR branch",
            Lucide::GitBranch,
            PullRequestCommand::Checkout(number),
        ),
        entry(
            "Copy link",
            Lucide::Link2,
            PullRequestCommand::CopyLink(number),
        ),
    ];
    if item.pull.same_repository && !item.pull.head_ref.is_empty() {
        entries.push(PullRequestMenuEntry::Separator);
        entries.push(entry(
            "Compare with current branch",
            Lucide::GitCompare,
            PullRequestCommand::CompareWithCurrent(number),
        ));
    }
    entries
}

/// El texto de la revisión para tooltips.
pub fn review_label(review: &ReviewSummary) -> Option<&'static str> {
    match review.decision {
        ReviewDecision::Approved => Some("Approved"),
        ReviewDecision::ChangesRequested => Some("Changes requested"),
        ReviewDecision::Pending => Some("Review pending"),
        ReviewDecision::None => None,
    }
}

/// El tooltip de una fila: título entero, autor, ramas, checks y revisión.
pub fn pull_request_tooltip(item: &PullRequestItem) -> String {
    let pull = &item.pull;
    let mut lines = vec![format!("#{} {}", pull.number, pull.title)];
    let mut origin = String::new();
    if let Some(author) = &pull.author {
        origin.push_str(&format!("by {author} · "));
    }
    let head = match (&pull.head_owner, pull.same_repository) {
        (Some(owner), false) => format!("{owner}:{}", pull.head_ref),
        _ => pull.head_ref.clone(),
    };
    origin.push_str(&format!("{head} → {}", pull.base_ref));
    if pull.draft {
        origin.push_str(" · Draft");
    }
    lines.push(origin);
    if let Some(checks) = item.checks.filter(|checks| checks.total > 0) {
        let detail = match checks.state() {
            Some(CheckState::Failure) => format!(", {} failing", checks.failed),
            Some(CheckState::Pending) => format!(", {} pending", checks.pending),
            _ => String::new(),
        };
        lines.push(format!("Checks: {} passed{detail}", checks.label()));
    }
    if let Some(review) = item.review.as_ref().and_then(review_label) {
        lines.push(format!("Review: {review}"));
    }
    lines.join("\n")
}

/// Ícono y color del estado de los checks; `None` sin checks.
pub(crate) fn check_icon(checks: Option<&CheckSummary>, cx: &App) -> Option<(Lucide, Hsla)> {
    let palette = cx.palette();
    match checks?.state()? {
        CheckState::Success => Some((Lucide::CircleCheck, palette.success_foreground.into())),
        CheckState::Failure => Some((Lucide::CircleX, palette.destructive_foreground.into())),
        CheckState::Pending => Some((Lucide::CircleDot, palette.warning_foreground.into())),
    }
}

/// Dos letras de un login (`octocat` → `OC`, `ana-maria` → `AM`).
fn initials(login: &str) -> String {
    let words: Vec<&str> = login
        .split(['-', '_', '.', ' '])
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .collect();
    let letters: String = match words.as_slice() {
        [] => return "?".to_string(),
        [word] => word
            .chars()
            .filter(|c| c.is_alphanumeric())
            .take(2)
            .collect(),
        [first, second, ..] => [first, second]
            .iter()
            .filter_map(|word| word.chars().find(|c| c.is_alphanumeric()))
            .collect(),
    };
    letters.to_uppercase()
}

/// Monograma Señal (`.mg`) del autor, como en la columna Author del
/// historial.
fn monogram(login: &str, cx: &App) -> Div {
    let palette = cx.palette();
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(14.))
        .rounded(px(2.))
        .border_1()
        .border_color(palette.border_hair)
        .bg(palette.bg_pressed)
        .text_color(palette.text_2)
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(7.5))
        .line_height(px(8.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(initials(login))
}

/// Lo que la sección enseñó la última vez: si cambia, se repinta.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Shown {
    repo_id: Option<String>,
    availability: PullRequestsAvailability,
    generation: u64,
    current_branch: Option<String>,
    expanded: bool,
}

/// La sección "Pull requests". La crea el [`Sidebar`] y la pinta entre sus
/// secciones.
pub struct PullRequestsSection {
    model: Entity<AppModel>,
    sidebar: WeakEntity<Sidebar>,
    shown: Option<Shown>,
    /// Lo que pinta `render`, sacado del modelo cuando cambia [`Shown`]:
    /// `render` no lee el modelo. `None`: la sección no se enseña.
    painted: Option<Rc<HeaderState>>,
    renders: usize,
    _observer: Subscription,
}

impl PullRequestsSection {
    pub fn new(sidebar: WeakEntity<Sidebar>, cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        let observer = cx.observe(&model, |this, model, cx| {
            // Recargar la aplicación detiene el vigilante: se vuelve a pedir.
            if !model.read(cx).is_watching_pull_requests() {
                model.update(cx, |model, cx| model.watch_pull_requests(cx));
            }
            this.model_changed(cx);
        });
        model.update(cx, |model, cx| model.watch_pull_requests(cx));
        // Un aviso que ya estuviera en cola antes de suscribirse no llega por
        // `observe`: se mira otra vez en cuanto termine este ciclo.
        let view = cx.weak_entity();
        cx.defer(move |cx| {
            view.update(cx, |this, cx| this.model_changed(cx)).ok();
        });
        let mut this = Self {
            model,
            sidebar,
            shown: None,
            painted: None,
            renders: 0,
            _observer: observer,
        };
        this.sync_from_model(cx);
        this
    }

    fn model_changed(&mut self, cx: &mut Context<Self>) {
        if self.sync_from_model(cx) {
            cx.notify();
        }
    }

    /// Recoge del modelo lo que se pinta. Devuelve si cambió.
    fn sync_from_model(&mut self, cx: &App) -> bool {
        let shown = self.shown(cx);
        if self.shown.as_ref() == Some(&shown) {
            return false;
        }
        // Solo se copia el cuerpo desplegada.
        let model = self.model.read(cx);
        self.painted = model
            .active_pull_requests()
            .filter(|state| state.availability.is_visible())
            .map(|state| {
                Rc::new(HeaderState {
                    count: state.pulls.len(),
                    available: state.availability == PullRequestsAvailability::Available,
                    loading: state.loading,
                    body: shown.expanded.then(|| state.clone()),
                })
            });
        self.shown = Some(shown);
        true
    }

    /// Veces que se pintó (tests).
    #[doc(hidden)]
    pub fn render_count(&self) -> usize {
        self.renders
    }

    fn shown(&self, cx: &App) -> Shown {
        let model = self.model.read(cx);
        let repository = model.active_repository();
        let state = model.active_pull_requests();
        Shown {
            repo_id: repository.map(|repository| repository.id.clone()),
            availability: state.map(|state| state.availability).unwrap_or_default(),
            generation: state.map_or(0, |state| state.generation),
            current_branch: repository.and_then(|repository| repository.current_branch.clone()),
            expanded: self.is_expanded(cx),
        }
    }

    /// La sección está desplegada (por defecto, sí).
    pub fn is_expanded(&self, cx: &App) -> bool {
        self.model
            .read(cx)
            .sidebar_sections()
            .extra
            .get(EXPANDED_KEY)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true)
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        let expanded = !self.is_expanded(cx);
        self.model
            .update(cx, |model, cx| {
                model.update_sidebar_sections(cx, |sections| {
                    sections
                        .extra
                        .insert(EXPANDED_KEY.to_string(), serde_json::Value::Bool(expanded));
                })
            })
            .detach();
    }

    /// Vuelve a pedir los PRs del repositorio activo.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(repo_id) = self
            .model
            .read(cx)
            .active_repository()
            .map(|repository| repository.id.clone())
        else {
            return;
        };
        self.model
            .update(cx, |model, cx| {
                model.refresh_pull_requests(&repo_id, true, cx)
            })
            .detach();
    }

    /// El menú de un PR del repositorio activo, como datos.
    pub fn menu(&self, number: u64, cx: &App) -> Vec<PullRequestMenuEntry> {
        self.model
            .read(cx)
            .active_pull_requests()
            .and_then(|state| state.find(number))
            .map(pull_request_menu)
            .unwrap_or_default()
    }

    /// Ejecuta una orden sobre un PR del repositorio activo.
    pub fn run_command(
        &mut self,
        command: PullRequestCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = self.model.read(cx);
        let Some(repo_id) = model
            .active_repository()
            .map(|repository| repository.id.clone())
        else {
            return;
        };
        let number = match command {
            PullRequestCommand::OpenInBrowser(number)
            | PullRequestCommand::Checkout(number)
            | PullRequestCommand::CopyLink(number)
            | PullRequestCommand::CompareWithCurrent(number) => number,
        };
        let Some(item) = model
            .active_pull_requests()
            .and_then(|state| state.find(number))
            .cloned()
        else {
            return;
        };

        match command {
            PullRequestCommand::OpenInBrowser(_) => {
                let url = item.pull.html_url.clone();
                self.model
                    .update(cx, |model, cx| model.open_external(&url, cx))
                    .detach();
            }
            PullRequestCommand::CopyLink(_) => {
                cx.write_to_clipboard(ClipboardItem::new_string(item.pull.html_url.clone()));
                self.model.update(cx, |model, cx| {
                    model.notify_success("Pull request link copied to clipboard", cx);
                });
            }
            PullRequestCommand::Checkout(_) | PullRequestCommand::CompareWithCurrent(_) => {
                let compare = matches!(command, PullRequestCommand::CompareWithCurrent(_));
                let fetched = self.model.update(cx, |model, cx| {
                    model.fetch_pull_request(&repo_id, number, cx)
                });
                let sidebar = self.sidebar.clone();
                let model = self.model.clone();
                let window_handle = window.window_handle();
                cx.spawn(async move |_, cx| {
                    let Some(checkout) = fetched.await else {
                        return;
                    };
                    // Las órdenes del sidebar actúan sobre el repositorio
                    // activo: si cambió de pestaña mientras se traía el PR, no
                    // se hace nada en el otro.
                    let still_active = model.read_with(cx, |model, _| {
                        model
                            .active_repository()
                            .is_some_and(|repository| repository.id == repo_id)
                    });
                    if !still_active {
                        return;
                    }
                    let command = match (checkout, compare) {
                        // La comparación navegable, como "Compare with …" de
                        // una rama remota.
                        (PullRequestCheckout::RemoteBranch { remote, branch }, true) => {
                            SidebarCommand::CompareWithCurrent(format!("{remote}/{branch}"))
                        }
                        (PullRequestCheckout::RemoteBranch { remote, branch }, false) => {
                            SidebarCommand::CheckoutRemoteBranch { remote, branch }
                        }
                        (PullRequestCheckout::LocalBranch(branch), false) => {
                            SidebarCommand::CheckoutBranch(branch)
                        }
                        (PullRequestCheckout::LocalBranch(_), true) => return,
                    };
                    cx.update_window(window_handle, |_, window, cx| {
                        sidebar
                            .update(cx, |sidebar, cx| sidebar.run_command(command, window, cx))
                            .ok();
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    fn command_listener(
        command: PullRequestCommand,
        cx: &mut Context<Self>,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        cx.listener(move |this, _, window, cx| this.run_command(command, window, cx))
    }

    fn menu_items(section: &WeakEntity<Self>, entries: Vec<PullRequestMenuEntry>) -> Vec<MenuItem> {
        entries
            .into_iter()
            .map(|entry| match entry {
                PullRequestMenuEntry::Separator => MenuItem::separator(),
                PullRequestMenuEntry::Item {
                    label,
                    icon,
                    command,
                } => {
                    let section = section.clone();
                    MenuItem::new(label).icon(icon).on_click(move |window, cx| {
                        section
                            .update(cx, |this, cx| this.run_command(command, window, cx))
                            .ok();
                    })
                }
            })
            .collect()
    }

    fn rows(
        &mut self,
        range: Range<usize>,
        pulls: &[PullRequestItem],
        current: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        // Como las filas del sidebar: sin tooltip bajo un menú recién abierto.
        let tooltips = self
            .sidebar
            .upgrade()
            .is_none_or(|sidebar| sidebar.read(cx).row_tooltips_enabled());
        pulls[range]
            .iter()
            .map(|item| self.render_row(item, current == Some(item.pull.number), tooltips, cx))
            .collect()
    }

    fn render_row(
        &self,
        item: &PullRequestItem,
        is_current: bool,
        tooltips: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *cx.palette();
        let mono = cx.theme().mono_font_family.clone();
        let pull = &item.pull;
        let number = pull.number;
        let weak = cx.entity().downgrade();
        let check = check_icon(item.checks.as_ref(), cx);
        let author = pull.author.clone().unwrap_or_else(|| "ghost".to_string());
        let review = item.review.map(|review| review.decision);

        let first_line = div()
            .flex()
            .items_center()
            .gap(rems(0.4))
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .child(
                section::item_icon(
                    if pull.draft {
                        Lucide::GitPullRequestDraft
                    } else {
                        Lucide::GitPullRequest
                    },
                    IconTone::Muted,
                )
                .flex_shrink_0(),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .font_family(mono.clone())
                    .text_size(px(11.))
                    .text_color(palette.text_3)
                    .child(format!("#{number}")),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(pull.draft, |this| this.text_color(palette.text_2))
                    .child(pull.title.clone()),
            )
            .when_some(
                item.checks.filter(|checks| checks.total > 0),
                |line, checks| {
                    let (icon, color) = check.unwrap_or((Lucide::Circle, palette.text_3.into()));
                    line.child(
                        div()
                            .id(SharedString::from(format!("pull-request-checks:{number}")))
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap(px(3.))
                            .font_family(mono.clone())
                            .text_size(px(10.5))
                            .text_color(color)
                            .child(mergeon_ui::icon(icon).size(px(12.)).tone(IconTone::Current))
                            .child(format!("{}/{}", checks.passed, checks.total))
                            .test_support(),
                    )
                },
            );

        let second_line = div()
            .flex()
            .items_center()
            .gap(rems(0.35))
            .w_full()
            .min_w_0()
            .overflow_hidden()
            // Bajo el título: el ícono (13px) más el hueco.
            .pl(px(19.5))
            .font_family(mono)
            .text_size(px(10.5))
            .line_height(relative(1.2))
            .text_color(palette.text_3)
            .child(monogram(&author, cx))
            // Un autor largo se recorta antes de comerse la rama.
            .child(
                div()
                    .flex_shrink(1.)
                    .min_w(rems(1.5))
                    .max_w(relative(0.4))
                    .truncate()
                    .child(author),
            )
            .child(div().flex_shrink_0().child("·"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(pull.head_ref.clone()),
            )
            .when(pull.draft, |line| {
                line.child(
                    div()
                        .flex_shrink_0()
                        .px(px(4.))
                        .rounded(px(2.))
                        .border_1()
                        .border_color(palette.border_base)
                        .text_color(palette.text_2)
                        .child("Draft"),
                )
            })
            .map(|line| {
                let review = match review {
                    Some(ReviewDecision::Approved) => Some((Lucide::Check, IconTone::Success)),
                    Some(ReviewDecision::ChangesRequested) => {
                        Some((Lucide::FileDiff, IconTone::Danger))
                    }
                    Some(ReviewDecision::Pending) => Some((Lucide::Eye, IconTone::Muted)),
                    _ => None,
                };
                match review {
                    Some((icon, tone)) => line.child(
                        div()
                            .id(SharedString::from(format!("pull-request-review:{number}")))
                            .flex_shrink_0()
                            .child(mergeon_ui::icon(icon).size(px(11.)).tone(tone))
                            .test_support(),
                    ),
                    None => line,
                }
            });

        div()
            .id(SharedString::from(format!("pull-request:{number}")))
            .flex()
            .flex_col()
            .justify_center()
            .gap(px(3.))
            .w_full()
            .overflow_hidden()
            .h(px(PULL_REQUEST_ROW_HEIGHT))
            .pl(rems(1.0))
            .pr(rems(0.625))
            .text_size(rems(0.8125))
            .line_height(relative(1.2))
            .text_color(palette.foreground)
            .cursor_pointer()
            .map(|row| {
                if is_current {
                    // El PR de la rama actual: seleccionado sin foco.
                    row.bg(palette.sel_inactive)
                } else {
                    row.hover(|style| style.bg(palette.sidebar_row_hover))
                }
            })
            .when(tooltips, |row| {
                row.tooltip(mergeon_ui::tooltip(pull_request_tooltip(item)))
            })
            .on_click(Self::command_listener(
                PullRequestCommand::OpenInBrowser(number),
                cx,
            ))
            .child(first_line)
            .child(second_line)
            .test_support()
            .context_menu_items({
                let weak = weak.clone();
                move |_, cx| {
                    let entries = weak
                        .upgrade()
                        .map(|entity| entity.read(cx).menu(number, cx))
                        .unwrap_or_default();
                    Self::menu_items(&weak, entries)
                }
            })
            .into_any_element()
    }

    fn render_body(
        &self,
        state: &RepoPullRequests,
        current_branch: Option<&str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *cx.palette();
        let note = |icon: Lucide, tone: IconTone, text: String, spin: bool| {
            div()
                .flex()
                .items_center()
                .gap(rems(0.45))
                .pt(rems(0.3))
                .pr(rems(0.875))
                .pb(rems(0.45))
                .pl(rems(1.0))
                .text_size(rems(0.75))
                .line_height(relative(1.4))
                .text_color(palette.muted_foreground)
                .child(
                    mergeon_ui::icon(icon)
                        .size(px(12.))
                        .tone(tone)
                        .spin(spin)
                        .flex_shrink_0(),
                )
                .child(div().flex_1().min_w_0().child(text))
        };

        if state.availability == PullRequestsAvailability::SignedOut {
            return div()
                .id("pull-requests-sign-in")
                .flex()
                .items_center()
                .gap(rems(0.45))
                .pt(rems(0.3))
                .pr(rems(0.875))
                .pb(rems(0.45))
                .pl(rems(1.0))
                .text_size(rems(0.8125))
                .line_height(relative(1.4))
                .text_color(palette.muted_foreground)
                .cursor_pointer()
                .hover(|style| style.text_color(palette.foreground))
                .child(
                    mergeon_ui::icon(Lucide::LogIn)
                        .size(px(13.))
                        .tone(IconTone::Muted)
                        .flex_shrink_0(),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child("Sign in to GitHub to see pull requests"),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.model
                        .update(cx, |model, cx| model.open_settings("accounts", cx));
                }))
                .test_support()
                .into_any_element();
        }

        let pulls = state.pulls.clone();
        let current = current_branch
            .and_then(|branch| state.current_for(branch))
            .map(|item| item.pull.number);
        let view = cx.entity().downgrade();
        let count = pulls.len();

        div()
            .flex()
            .flex_col()
            .when_some(state.error.clone(), |body, error| {
                body.child(
                    note(Lucide::CircleAlert, IconTone::Muted, error, false)
                        .id("pull-requests-error")
                        .test_support(),
                )
            })
            .map(|body| {
                if state.loading && count == 0 {
                    body.child(note(
                        Lucide::LoaderCircle,
                        IconTone::Muted,
                        "Loading pull requests…".to_string(),
                        true,
                    ))
                } else if count == 0 && state.error.is_none() {
                    body.child(section::empty("No open pull requests", cx))
                } else {
                    body.child(virtual_rows(
                        "pull-request-rows",
                        RowHeights::uniform(count, px(PULL_REQUEST_ROW_HEIGHT)),
                        move |range: Range<usize>, _, cx| {
                            let pulls: Arc<[PullRequestItem]> = pulls.clone();
                            view.update(cx, |this, cx| this.rows(range, &pulls, current, cx))
                                .unwrap_or_default()
                        },
                    ))
                }
            })
            .into_any_element()
    }
}

impl Render for PullRequestsSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        // Todo sale de lo recogido en `model_changed`: pintar no lee el modelo.
        let (Some(shown), Some(state)) = (self.shown.clone(), self.painted.clone()) else {
            return div().into_any_element();
        };
        let expanded = shown.expanded;
        let count = state.count;
        let show_count = state.available;
        // Refrescando una lista que ya se ve: el botón gira (vacía, ya lo dice
        // el "Loading…" del cuerpo).
        let refreshing = state.loading && count > 0;

        section::section()
            .child(
                section::header("pull-requests", cx)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle(cx)))
                    .child(section::chevron(expanded))
                    .child(section::title("Pull requests", cx))
                    .when(show_count, |header| {
                        header
                            .child(
                                section::header_action_with(
                                    "pull-requests-refresh",
                                    section::HeaderActionState {
                                        enabled: true,
                                        busy: refreshing,
                                    },
                                    Lucide::RefreshCw,
                                    if refreshing {
                                        "Refreshing pull requests…"
                                    } else {
                                        "Refresh pull requests"
                                    },
                                    "pull-requests",
                                    cx,
                                    {
                                        let view = cx.entity().downgrade();
                                        move |_, _, cx| {
                                            view.update(cx, |this, cx| this.refresh(cx)).ok();
                                        }
                                    },
                                )
                                .test_support(),
                            )
                            .child(section::count(count, cx))
                    })
                    .test_support(),
            )
            .when_some(state.body.as_ref(), |this, body| {
                this.child(section::content(false).child(self.render_body(
                    body,
                    shown.current_branch.as_deref(),
                    cx,
                )))
            })
            .into_any_element()
    }
}

/// Lo que la cabecera necesita del estado de PRs; el cuerpo, solo desplegada.
struct HeaderState {
    count: usize,
    available: bool,
    loading: bool,
    body: Option<RepoPullRequests>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use mergeon_github::GitHubPullRequest;

    fn item(same_repository: bool) -> PullRequestItem {
        PullRequestItem {
            pull: GitHubPullRequest {
                number: 42,
                title: "Add search".into(),
                html_url: "https://github.com/acme/web/pull/42".into(),
                draft: true,
                author: Some("ana".into()),
                author_avatar_url: None,
                head_ref: "feat/search".into(),
                head_sha: "abc".into(),
                head_repository: Some("ana/web".into()),
                head_owner: Some("ana".into()),
                base_ref: "main".into(),
                same_repository,
                updated_at: None,
                requested_reviewers: 0,
            },
            checks: Some(CheckSummary {
                total: 5,
                passed: 3,
                failed: 1,
                pending: 1,
            }),
            review: Some(ReviewSummary {
                decision: ReviewDecision::ChangesRequested,
                approvals: 0,
                changes_requested: 1,
            }),
        }
    }

    fn labels(entries: &[PullRequestMenuEntry]) -> Vec<&'static str> {
        entries
            .iter()
            .map(|entry| match entry {
                PullRequestMenuEntry::Item { label, .. } => *label,
                PullRequestMenuEntry::Separator => "-",
            })
            .collect()
    }

    #[test]
    fn only_branches_of_the_same_repository_can_be_compared() {
        assert_eq!(
            labels(&pull_request_menu(&item(true))),
            [
                "Open in browser",
                "Checkout PR branch",
                "Copy link",
                "-",
                "Compare with current branch"
            ]
        );
        assert_eq!(
            labels(&pull_request_menu(&item(false))),
            ["Open in browser", "Checkout PR branch", "Copy link"]
        );
    }

    #[test]
    fn the_tooltip_tells_everything_the_row_abbreviates() {
        assert_eq!(
            pull_request_tooltip(&item(false)),
            "#42 Add search\nby ana · ana:feat/search → main · Draft\n\
             Checks: 3/5 checks passed, 1 failing\nReview: Changes requested"
        );
        let mut same = item(true);
        same.checks = None;
        same.review = None;
        same.pull.draft = false;
        assert_eq!(
            pull_request_tooltip(&same),
            "#42 Add search\nby ana · feat/search → main"
        );
    }

    #[test]
    fn labels_checks_and_initials() {
        let one = CheckSummary {
            total: 1,
            passed: 1,
            failed: 0,
            pending: 0,
        };
        assert_eq!(one.label(), "1/1 check");
        assert_eq!(initials("octocat"), "OC");
        assert_eq!(initials("ana-maria"), "AM");
        assert_eq!(initials("dependabot[bot]"), "DE");
        assert_eq!(initials("--"), "?");
    }
}
