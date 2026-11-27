//! Barra de herramientas del repositorio activo
//! (`src/components/layout/Toolbar.vue`): deshacer, Fetch / Pull / Push y
//! Stash con el indicador de actividad a la izquierda, el buscador al centro
//! y, a la derecha, el botón de la rama actual (abre el selector de ramas) y
//! los de reflog, nueva rama, editor, terminal y explorador.
//!
//! El nombre del repositorio ya está en su pestaña y lo que la rama lleva de
//! ventaja o retraso, en las insignias de Pull y Push: la barra no lo repite.

use std::collections::BTreeMap;

use gpui_kit::base::ElementExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Div, Entity, EventEmitter, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, MouseButton, MouseDownEvent, ParentElement as _, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, TestSupportExt as _, Window, div,
    px,
};
use mergeon_app::{ActionDialogMode, AppModel, RemoteAction};
use mergeon_gitcmd::{ConflictState, OperationState};
use mergeon_model::keyboard_shortcuts::{
    ShortcutAction, combo_label_parts, effective_combo_for, effective_combo_label, is_customized,
};
use mergeon_model::repositories::{MutationQueue, RepoLifecycleState, Repository};
use mergeon_ui::style::{control_shadow, text};
use mergeon_ui::{ActivePaletteExt as _, IconTone, Lucide, Radius};

use crate::operation_indicator::{OperationActivity, OperationIndicator};
use crate::parts::{button_icon, mono_font, plain_icon};

/// Por debajo de este ancho la barra pasa a dos filas. El original lo hacía
/// a 900px (`@container repository-content`); la barra de Señal (rama con
/// ventaja/retraso, Fetch | Pull | Push con contadores y el buscador) necesita
/// unos 1000px en una fila sin cortar el nombre del repositorio ni la rama.
/// ancho el chip central baja a una segunda fila.
const COMPACT_MAX_WIDTH: Pixels = px(1040.);

/// Alto de la barra en una fila.
pub const TOOLBAR_HEIGHT: Pixels = px(40.);

/// Lo que la barra pide a quien la monta (la cáscara).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarEvent {
    /// Clic en la rama actual: el selector rápido de ramas de la paleta.
    OpenBranchPicker,
}

/// `activeBranchLabel`: la rama actual o, con HEAD desanclado, a qué commit
/// apunta.
pub fn toolbar_branch_label<S, M>(repository: &Repository<S, M>) -> String {
    let current = repository.current_branch.as_deref().unwrap_or_default();
    if current != "HEAD" {
        return if current.is_empty() {
            "HEAD".to_string()
        } else {
            current.to_string()
        };
    }

    match repository
        .detached_head_ref
        .as_deref()
        .filter(|reference| !reference.is_empty())
    {
        Some(reference) => format!("Detached · {reference}"),
        None => "Detached · HEAD".to_string(),
    }
}

/// `activeHeadHash`: con HEAD desanclado la etiqueta de rama ya muestra el
/// commit, así que no se repite; en una rama es el único sitio donde se ve
/// dónde quedó el working tree.
pub fn toolbar_head_hash<S, M>(repository: &Repository<S, M>) -> Option<String> {
    if repository.current_branch.as_deref() == Some("HEAD") {
        return None;
    }

    repository
        .head_short_hash
        .clone()
        .filter(|hash| !hash.is_empty())
}

/// Texto de ayuda del commit actual (el `title` de `.head-badge`).
pub fn toolbar_head_title(hash: &str) -> String {
    format!("Current commit · HEAD is at {hash}")
}

/// Cada botón de la barra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolbarAction {
    Fetch,
    Pull,
    Push,
    Stash,
    Reflog,
    NewBranch,
    OpenEditor,
    OpenTerminal,
    OpenExplorer,
}

/// Botones de solo ícono de la derecha.
const ICON_ACTIONS: [ToolbarAction; 5] = [
    ToolbarAction::Reflog,
    ToolbarAction::NewBranch,
    ToolbarAction::OpenEditor,
    ToolbarAction::OpenTerminal,
    ToolbarAction::OpenExplorer,
];

impl ToolbarAction {
    fn id(self) -> &'static str {
        match self {
            Self::Fetch => "toolbar-fetch",
            Self::Pull => "toolbar-pull",
            Self::Push => "toolbar-push",
            Self::Stash => "toolbar-stash",
            Self::Reflog => "toolbar-reflog",
            Self::NewBranch => "toolbar-new-branch",
            Self::OpenEditor => "toolbar-open-editor",
            Self::OpenTerminal => "toolbar-open-terminal",
            Self::OpenExplorer => "toolbar-open-explorer",
        }
    }

    /// Etiqueta del botón o, en los de solo ícono, su texto de ayuda.
    fn label(self) -> &'static str {
        match self {
            Self::Fetch => "Fetch",
            Self::Pull => "Pull",
            Self::Push => "Push",
            Self::Stash => "Stash",
            Self::Reflog => "Reflog",
            Self::NewBranch => "New Branch",
            Self::OpenEditor => "Open in Editor",
            Self::OpenTerminal => "Open Terminal",
            Self::OpenExplorer => "Show in Folder",
        }
    }

    fn icon(self) -> Lucide {
        match self {
            Self::Fetch => Lucide::ArrowDownToLine,
            Self::Pull => Lucide::Download,
            Self::Push => Lucide::Upload,
            Self::Stash => Lucide::Archive,
            // `History` de lucide-vue-next: el catálogo lo trae con su nombre nuevo.
            Self::Reflog => Lucide::RotateCcwClock,
            Self::NewBranch => Lucide::GitBranchPlus,
            Self::OpenEditor => Lucide::CodeXml,
            Self::OpenTerminal => Lucide::SquareTerminal,
            Self::OpenExplorer => Lucide::FolderSearch2,
        }
    }

    fn shortcut(self) -> Option<ShortcutAction> {
        match self {
            Self::Fetch => Some(ShortcutAction::Fetch),
            Self::Pull => Some(ShortcutAction::Pull),
            Self::Push => Some(ShortcutAction::Push),
            Self::Reflog => Some(ShortcutAction::ShowReflog),
            Self::NewBranch => Some(ShortcutAction::NewBranch),
            Self::OpenEditor => Some(ShortcutAction::OpenInEditor),
            Self::Stash | Self::OpenTerminal | Self::OpenExplorer => None,
        }
    }

    fn tooltip(
        self,
        shortcuts: &BTreeMap<String, String>,
        dialog_mode: Option<ActionDialogMode>,
        push_tags: bool,
        is_mac: bool,
    ) -> String {
        let mut text = match self
            .shortcut()
            .and_then(|action| effective_combo_label(action, shortcuts, is_mac))
        {
            Some(keys) => format!("{} ({keys})", self.label()),
            None => self.label().to_string(),
        };
        if self == Self::Push && push_tags {
            text.push_str(" · +tags");
        }
        if dialog_mode == Some(ActionDialogMode::Never)
            && matches!(self, Self::Fetch | Self::Pull | Self::Push)
        {
            text.push_str(" · Shift+click or right-click for options");
        }
        text
    }

    /// La operación de `busyToolbarOperation` que marca ocupado al botón. Las
    /// acciones que tienen una son las que mutan el repositorio, y se bloquean
    /// mientras haya una operación de secuencia a medias (`isRepoBusy`).
    fn operation(self) -> Option<&'static str> {
        match self {
            Self::Fetch => Some("fetch"),
            Self::Pull => Some("pull"),
            Self::Push => Some("push"),
            Self::Stash => Some("stash"),
            Self::NewBranch => Some("create-branch"),
            Self::Reflog | Self::OpenEditor | Self::OpenTerminal | Self::OpenExplorer => None,
        }
    }
}

/// Barra de herramientas del repositorio activo. Sin repositorio no dibuja
/// nada.
///
/// Cada botón llama al flujo de [`AppModel`] que le corresponde; los que abren
/// un diálogo lo piden con `AppEvent::ShowDialog`, que la cáscara delega en
/// [`crate::handle_dialog_request`].
pub struct ToolbarView {
    model: Entity<AppModel>,
    compact: bool,
    /// Lo último que se pintó (o se iba a pintar) del modelo.
    shown: Option<ToolbarState>,
    renders: usize,
    _observer: Subscription,
}

/// Lo que la barra enseña del modelo. La vista solo se repinta cuando cambia:
/// el modelo avisa por cualquier cosa (otra pestaña, el log, un ajuste...).
#[derive(Debug, Clone, PartialEq)]
struct ToolbarState {
    branch_label: SharedString,
    head_hash: Option<String>,
    repo_busy: bool,
    operation_state: Option<OperationState>,
    conflict_state: ConflictState,
    queue: MutationQueue,
    busy_operation: Option<&'static str>,
    ahead: u32,
    behind: u32,
    /// "Undo commit 'Fix login'" / "Redo …" de lo próximo en la pila.
    undo_label: Option<String>,
    redo_label: Option<String>,
    /// Hay un deshacer o rehacer en curso.
    undo_running: bool,
    /// Modo de diálogo de opciones por acción (`Fetch`, `Pull`, `Push`).
    fetch_dialog_mode: ActionDialogMode,
    pull_dialog_mode: ActionDialogMode,
    push_dialog_mode: ActionDialogMode,
    /// Si Push recuerda enviar tags (`action_defaults.push.push_tags`).
    push_tags: bool,
    /// Atajos personalizados para los tooltips y el buscador.
    keyboard_shortcuts: BTreeMap<String, String>,
}

impl ToolbarState {
    fn dialog_mode(&self, action: ToolbarAction) -> Option<ActionDialogMode> {
        match action {
            ToolbarAction::Fetch => Some(self.fetch_dialog_mode),
            ToolbarAction::Pull => Some(self.pull_dialog_mode),
            ToolbarAction::Push => Some(self.push_dialog_mode),
            _ => None,
        }
    }

    fn read(model: &AppModel) -> Option<Self> {
        let repository = model.active_repository()?;
        // Lo que la rama actual lleva de ventaja y de retraso sobre su upstream.
        let (ahead, behind) = model
            .active_branch_entry()
            .map_or((0, 0), |branch| (branch.ahead, branch.behind));
        Some(Self {
            branch_label: toolbar_branch_label(repository).into(),
            head_hash: toolbar_head_hash(repository),
            repo_busy: repository.repo_lifecycle_state != RepoLifecycleState::Idle,
            operation_state: repository.operation_state.clone(),
            conflict_state: repository.conflict_state.clone(),
            queue: repository.mutation_queue.clone(),
            busy_operation: model.busy_toolbar_operation(),
            ahead,
            behind,
            undo_label: model.undo_action_label(&repository.path, false),
            redo_label: model.undo_action_label(&repository.path, true),
            undo_running: model.is_undo_running(&repository.path),
            fetch_dialog_mode: model.action_dialog_mode(RemoteAction::Fetch),
            pull_dialog_mode: model.action_dialog_mode(RemoteAction::Pull),
            push_dialog_mode: model.action_dialog_mode(RemoteAction::Push),
            push_tags: model.action_defaults().push.push_tags,
            keyboard_shortcuts: model.keyboard_shortcuts().clone(),
        })
    }
}

impl EventEmitter<ToolbarEvent> for ToolbarView {}

impl ToolbarView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        let observer = cx.observe(&model, |this: &mut Self, _, cx| this.sync_shown(cx));
        let shown = ToolbarState::read(model.read(cx));
        // El observador empieza a escuchar al final de este ciclo: un aviso
        // del modelo que ya estuviera en cola no le llegaría.
        let this = cx.weak_entity();
        cx.defer(move |cx| {
            this.update(cx, |this, cx| this.sync_shown(cx)).ok();
        });
        Self {
            model,
            // Hasta medir la barra, la ventana es la mejor pista de su ancho.
            compact: window.viewport_size().width <= COMPACT_MAX_WIDTH,
            shown,
            renders: 0,
            _observer: observer,
        }
    }

    /// Veces que se pintó (tests).
    #[doc(hidden)]
    pub fn render_count(&self) -> usize {
        self.renders
    }

    /// Texto de ayuda efectivo de Fetch / Pull / Push (tests).
    #[doc(hidden)]
    pub fn sync_button_tooltip(&self, id: &str) -> Option<String> {
        let state = self.shown.as_ref()?;
        let action = match id {
            "toolbar-fetch" => ToolbarAction::Fetch,
            "toolbar-pull" => ToolbarAction::Pull,
            "toolbar-push" => ToolbarAction::Push,
            _ => return None,
        };
        Some(action.tooltip(
            &state.keyboard_shortcuts,
            state.dialog_mode(action),
            state.push_tags,
            cfg!(target_os = "macos"),
        ))
    }

    /// Recoge lo que la barra enseña del modelo y repinta si cambió.
    fn sync_shown(&mut self, cx: &mut Context<Self>) {
        let state = ToolbarState::read(self.model.read(cx));
        if state != self.shown {
            self.shown = state;
            cx.notify();
        }
    }

    /// La barra está en su disposición de dos filas (1040px o menos).
    pub fn is_compact(&self) -> bool {
        self.compact
    }

    fn run(&mut self, action: ToolbarAction, shift: bool, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| match action {
            ToolbarAction::Fetch if shift => model.open_fetch_dialog(cx),
            ToolbarAction::Pull if shift => model.open_pull_dialog(cx),
            ToolbarAction::Push if shift => model.open_push_dialog(cx),
            ToolbarAction::Fetch => model.handle_fetch(cx),
            ToolbarAction::Pull => model.handle_pull(cx),
            ToolbarAction::Push => model.handle_push(cx),
            ToolbarAction::Stash => model.handle_stash(cx).detach(),
            ToolbarAction::Reflog => model.handle_shortcut(ShortcutAction::ShowReflog, cx),
            ToolbarAction::NewBranch => model.handle_new_branch(cx).detach(),
            ToolbarAction::OpenEditor => model.handle_open_in_editor(cx).detach(),
            ToolbarAction::OpenTerminal => model.handle_open_terminal(cx).detach(),
            ToolbarAction::OpenExplorer => model.handle_open_explorer(cx).detach(),
        });
    }

    fn open_options_dialog(&mut self, action: ToolbarAction, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| match action {
            ToolbarAction::Fetch => model.open_fetch_dialog(cx),
            ToolbarAction::Pull => model.open_pull_dialog(cx),
            ToolbarAction::Push => model.open_push_dialog(cx),
            _ => {}
        });
    }

    fn undo(&mut self, redo: bool, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| {
            if redo {
                model.redo(cx).detach();
            } else {
                model.undo(cx).detach();
            }
        });
    }

    /// Deshacer | Rehacer: dos botones de ícono en un `.seg`, con la
    /// etiqueta de lo que harían y el atajo en el texto de ayuda.
    fn undo_group(
        undo_label: Option<String>,
        redo_label: Option<String>,
        blocked_reason: Option<String>,
        running: bool,
        shortcuts: &BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) -> Div {
        let palette = *cx.palette();
        let inner_radius = (Radius::Md.px(cx) - px(1.)).max(px(0.));
        let mut group = segmented_group(cx);
        for redo in [false, true] {
            let label = if redo { &redo_label } else { &undo_label };
            let disabled = label.is_none() || blocked_reason.is_some() || running;
            let tooltip = undo_button_tooltip_with_shortcuts(
                label.as_deref(),
                redo,
                blocked_reason.as_deref(),
                running,
                shortcuts,
                cfg!(target_os = "macos"),
            );
            let (id, icon) = if redo {
                ("toolbar-redo", Lucide::Redo2)
            } else {
                ("toolbar-undo", Lucide::Undo2)
            };
            if redo {
                group = group.child(segment_divider(palette.border_hair));
            }
            group = group.child(
                mergeon_ui::button(id)
                    .outline()
                    .sm()
                    .h_full()
                    .w(px(30.))
                    .px_0()
                    .border_0()
                    .rounded_none()
                    .when(!redo, |this| this.rounded_l(inner_radius))
                    .when(redo, |this| this.rounded_r(inner_radius))
                    .shadow(Vec::new())
                    .tooltip(tooltip)
                    .disabled(disabled)
                    .when(disabled, |this| this.opacity(0.38))
                    .child(button_icon(icon, cx))
                    .on_click(cx.listener(move |this, _, _, cx| this.undo(redo, cx))),
            );
        }
        group
    }

    fn cancel_operation(&mut self, operation_id: &str, cx: &mut Context<Self>) {
        self.model
            .update(cx, |model, cx| {
                model.handle_cancel_operation(operation_id, cx)
            })
            .detach();
    }

    /// `.action-btn-icon`: botón de 30px solo con ícono. Ocupado, cambia el
    /// ícono por uno que gira.
    fn icon_button(
        action: ToolbarAction,
        busy: bool,
        disabled: bool,
        shortcuts: &BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) -> mergeon_ui::Button {
        let icon = if busy {
            plain_icon(Lucide::LoaderCircle, 16.).spin(true)
        } else {
            plain_icon(action.icon(), 16.)
        };
        let tooltip = action.tooltip(shortcuts, None, false, cfg!(target_os = "macos"));
        mergeon_ui::button(action.id())
            .ghost()
            .sm()
            .w(px(30.))
            .h(px(30.))
            .px_0()
            .rounded(Radius::Md.px(cx))
            .tooltip(tooltip)
            .disabled(disabled)
            // `.action-btn-icon:disabled { opacity: 0.38 }`
            .when(disabled, |this| this.opacity(0.38))
            .child(icon)
            .on_click(
                cx.listener(move |this, event: &gpui_kit::ClickEvent, _, cx| {
                    this.run(action, event.modifiers().shift, cx)
                }),
            )
    }
}

/// Texto de ayuda de Deshacer / Rehacer con los atajos por defecto.
pub fn undo_tooltip(label: Option<&str>, redo: bool, is_mac: bool) -> String {
    undo_tooltip_with_shortcuts(label, redo, &BTreeMap::new(), is_mac)
}

/// Texto de ayuda de Deshacer / Rehacer con los atajos efectivos del usuario.
pub fn undo_tooltip_with_shortcuts(
    label: Option<&str>,
    redo: bool,
    shortcuts: &BTreeMap<String, String>,
    is_mac: bool,
) -> String {
    let action = if redo {
        ShortcutAction::Redo
    } else {
        ShortcutAction::Undo
    };
    let keys = effective_combo_label(action, shortcuts, is_mac);
    let base = match label {
        Some(label) => label,
        None if redo => "Nothing to redo",
        None => "Nothing to undo",
    };
    match keys {
        Some(keys) => format!("{base} ({keys})"),
        None => base.to_string(),
    }
}

/// Texto de ayuda real del botón ↶ / ↷: si está apagado por una operación a
/// medias o en curso, explica por qué en lugar de prometer una acción.
#[cfg_attr(not(test), allow(dead_code))]
pub fn undo_button_tooltip(
    label: Option<&str>,
    redo: bool,
    blocked_reason: Option<&str>,
    running: bool,
    is_mac: bool,
) -> String {
    undo_button_tooltip_with_shortcuts(
        label,
        redo,
        blocked_reason,
        running,
        &BTreeMap::new(),
        is_mac,
    )
}

/// Variante de [`undo_button_tooltip`] que refleja atajos personalizados.
pub fn undo_button_tooltip_with_shortcuts(
    label: Option<&str>,
    redo: bool,
    blocked_reason: Option<&str>,
    running: bool,
    shortcuts: &BTreeMap<String, String>,
    is_mac: bool,
) -> String {
    if running {
        return if redo {
            "Redoing…".to_string()
        } else {
            "Undoing…".to_string()
        };
    }
    if let Some(reason) = blocked_reason {
        return reason.to_string();
    }
    undo_tooltip_with_shortcuts(label, redo, shortcuts, is_mac)
}

fn undo_blocked_reason(
    operation_state: Option<&OperationState>,
    repo_busy: bool,
) -> Option<String> {
    if let Some(state) = operation_state.filter(|state| state.in_progress) {
        let what = state
            .label
            .trim()
            .to_lowercase()
            .trim_end_matches("in progress")
            .trim()
            .to_string();
        let what = if what.is_empty() {
            "operation".to_string()
        } else {
            what
        };
        return Some(format!("Finish or abort the {what} first"));
    }
    repo_busy.then(|| "Wait for the current operation to finish".to_string())
}

/// `.ab` de la rama actual: "↑2 ↓1", sin el lado que esté a cero. `None`
/// si va al día con su upstream (o no tiene).
pub fn sync_counts_label(ahead: u32, behind: u32) -> Option<String> {
    match (ahead, behind) {
        (0, 0) => None,
        (ahead, 0) => Some(format!("↑{ahead}")),
        (0, behind) => Some(format!("↓{behind}")),
        (ahead, behind) => Some(format!("↑{ahead} ↓{behind}")),
    }
}

/// Teclas del atajo del buscador tal como se pintan, una por `kbd`.
pub fn search_shortcut_keys(is_mac: bool) -> [&'static str; 2] {
    if is_mac { ["⌘", "K"] } else { ["Ctrl", "K"] }
}

/// Teclas efectivas del buscador de commits (si el usuario lo personalizó,
/// muestra su combo; si lo desvinculó, devuelve vacío).
pub fn effective_search_shortcut_keys(
    shortcuts: &BTreeMap<String, String>,
    is_mac: bool,
) -> Vec<String> {
    if !is_customized(ShortcutAction::SearchCommits, shortcuts) {
        return search_shortcut_keys(is_mac)
            .into_iter()
            .map(str::to_string)
            .collect();
    }
    effective_combo_for(ShortcutAction::SearchCommits, shortcuts)
        .map(|combo| combo_label_parts(&combo, is_mac))
        .unwrap_or_default()
}

/// Texto del buscador de la barra.
pub const SEARCH_PLACEHOLDER: &str = "Search commits, authors, hashes…";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CountTone {
    /// `.count`: gris (lo que hay por traer).
    Neutral,
    /// `.count.accent`: naranja relleno con texto tinta (lo que hay por enviar).
    Accent,
}

/// `.count` de Señal: contador mono de 16px junto a la etiqueta de un botón.
fn count_badge(id: &'static str, count: u32, tone: CountTone, cx: &App) -> AnyElement {
    let palette = cx.palette();
    let (bg, fg) = match tone {
        CountTone::Neutral => (palette.bg_pressed, palette.text_1),
        CountTone::Accent => (palette.primary, palette.primary_foreground),
    };
    let label: SharedString = count.to_string().into();
    div()
        .id(id)
        .test_support()
        .aria_label(label.clone())
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .h(px(16.))
        .min_w(px(16.))
        .px(px(4.))
        .rounded(Radius::Xs.px(cx))
        .bg(bg)
        .text_color(fg)
        .font_family(mono_font(cx))
        .text_size(px(11.))
        .line_height(px(16.))
        .font_weight(FontWeight::MEDIUM)
        .child(label)
        .into_any_element()
}

/// `.seg`: un solo borde con sombra de control alrededor de varios botones.
fn segmented_group(cx: &App) -> Div {
    let palette = cx.palette();
    let dark = cx.theme().mode.is_dark();
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .h(px(28.))
        .border_1()
        .border_color(palette.border_base)
        .rounded(Radius::Md.px(cx))
        .bg(palette.bg_base)
        .shadow(control_shadow(gpui_kit::black().opacity(if dark {
            0.3
        } else {
            0.04
        })))
}

/// Divisor de 1px entre los segmentos de un `.seg`.
fn segment_divider(color: impl Into<Hsla>) -> Div {
    div().flex_shrink_0().w(px(1.)).h_full().bg(color.into())
}

impl ToolbarView {
    /// Un segmento de un `.seg`: el botón de contorno sin su propio borde ni
    /// sombra (los pone el grupo). `edges` dice si es el primero y/o el
    /// último, que redondean su lado contra el borde del grupo.
    fn segment_button(
        action: ToolbarAction,
        busy: bool,
        disabled: bool,
        badge: Option<AnyElement>,
        edges: (bool, bool),
        tooltip: String,
        cx: &mut Context<Self>,
    ) -> mergeon_ui::Button {
        let inner_radius = (Radius::Md.px(cx) - px(1.)).max(px(0.));
        let (first, last) = edges;
        let has_options_dialog = matches!(
            action,
            ToolbarAction::Fetch | ToolbarAction::Pull | ToolbarAction::Push
        );
        mergeon_ui::button(action.id())
            .outline()
            .sm()
            .h_full()
            .px(px(10.))
            .gap(px(6.))
            .border_0()
            .rounded_none()
            .when(first, |this| this.rounded_l(inner_radius))
            .when(last, |this| this.rounded_r(inner_radius))
            .shadow(Vec::new())
            .tooltip(tooltip)
            .disabled(disabled)
            .loading(busy)
            .when(!busy, |this| this.child(button_icon(action.icon(), cx)))
            .child(div().child(action.label()))
            .children(badge)
            .on_click(
                cx.listener(move |this, event: &gpui_kit::ClickEvent, _, cx| {
                    this.run(action, event.modifiers().shift, cx)
                }),
            )
            .when(has_options_dialog, |this| {
                this.on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        if disabled || busy {
                            return;
                        }
                        cx.stop_propagation();
                        this.open_options_dialog(action, cx);
                    }),
                )
            })
    }

    /// Abre la búsqueda de commits de la app (la del panel lateral), como
    /// el atajo.
    fn open_search(&mut self, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| {
            model.handle_shortcut(ShortcutAction::SearchCommits, cx);
        });
    }

    /// `.search`: parece un campo, pero no busca por sí mismo: lleva a la
    /// búsqueda de commits que ya existe (la lupa del panel lateral). Con
    /// `grow` se lleva el ancho que sobre (fila de abajo en la barra de dos
    /// filas).
    fn search_field(
        &self,
        grow: bool,
        shortcuts: &BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *cx.palette();
        let keys = effective_search_shortcut_keys(shortcuts, cfg!(target_os = "macos"));
        div()
            .id("toolbar-search")
            .test_support()
            .aria_label(SEARCH_PLACEHOLDER)
            .flex()
            .items_center()
            .gap(px(8.))
            .w(px(320.))
            .min_w(px(180.))
            .h(px(28.))
            .pl(px(8.))
            .pr(px(4.))
            .border_1()
            .border_color(palette.border_control)
            .rounded(Radius::Md.px(cx))
            .bg(palette.bg_base)
            .text_color(palette.text_3)
            .cursor_text()
            .hover(|style| style.border_color(palette.text_2))
            .child(
                mergeon_ui::icon(Lucide::Search)
                    .size(px(14.))
                    .text_color(palette.text_3),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SEARCH_PLACEHOLDER),
            )
            .when(!keys.is_empty(), |this| {
                this.child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .gap(px(2.))
                        .children(keys.into_iter().map(mergeon_ui::kbd)),
                )
            })
            .when(grow, |this| this.flex_1())
            .on_click(cx.listener(|this, _, _, cx| this.open_search(cx)))
            .into_any_element()
    }
}

impl Render for ToolbarView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        // Lo leído al avisar el modelo: pintar no lo lee, que repintaría la
        // ventana con cada aviso suyo.
        let state = self.shown.clone();
        let Some(ToolbarState {
            branch_label,
            head_hash,
            repo_busy,
            operation_state,
            conflict_state,
            queue,
            busy_operation,
            ahead,
            behind,
            undo_label,
            redo_label,
            undo_running,
            fetch_dialog_mode,
            pull_dialog_mode,
            push_dialog_mode,
            push_tags,
            keyboard_shortcuts,
        }) = state
        else {
            return div().into_any_element();
        };
        let has_activity =
            OperationActivity::new(operation_state.as_ref(), Some(&conflict_state), &queue)
                .has_activity();

        let palette = *cx.palette();
        let compact = self.compact;
        let this = cx.entity();

        // Fetch | Pull | Push: un solo control segmentado. Pull lleva lo que
        // hay por traer y Push lo que hay por enviar.
        let sync_actions = [
            (ToolbarAction::Fetch, fetch_dialog_mode, None),
            (
                ToolbarAction::Pull,
                pull_dialog_mode,
                Some(behind)
                    .filter(|count| *count > 0)
                    .map(|count| count_badge("toolbar-pull-count", count, CountTone::Neutral, cx)),
            ),
            (
                ToolbarAction::Push,
                push_dialog_mode,
                Some(ahead)
                    .filter(|count| *count > 0)
                    .map(|count| count_badge("toolbar-push-count", count, CountTone::Accent, cx)),
            ),
        ];
        let last_sync = sync_actions.len() - 1;
        let mut sync_group = segmented_group(cx);
        for (index, (action, dialog_mode, badge)) in sync_actions.into_iter().enumerate() {
            if index > 0 {
                sync_group = sync_group.child(segment_divider(palette.border_hair));
            }
            let busy = busy_operation == action.operation();
            let tooltip = action.tooltip(
                &keyboard_shortcuts,
                Some(dialog_mode),
                push_tags,
                cfg!(target_os = "macos"),
            );
            sync_group = sync_group.child(Self::segment_button(
                action,
                busy,
                repo_busy || busy,
                badge,
                (index == 0, index == last_sync),
                tooltip,
                cx,
            ));
        }
        let stash_busy = busy_operation == ToolbarAction::Stash.operation();
        let stash_tooltip = ToolbarAction::Stash.tooltip(
            &keyboard_shortcuts,
            None,
            false,
            cfg!(target_os = "macos"),
        );
        let stash_group = segmented_group(cx).child(Self::segment_button(
            ToolbarAction::Stash,
            stash_busy,
            repo_busy || stash_busy,
            None,
            (true, true),
            stash_tooltip,
            cx,
        ));

        let undo_blocked = undo_blocked_reason(operation_state.as_ref(), repo_busy);
        // ↶ ↷ van al principio (fijos al borde izquierdo): las insignias de
        // Pull y Push aparecen y desaparecen al deshacer/rehacer, así que si
        // fueran detrás los botones se moverían bajo el ratón.
        let actions = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(6.))
            .child(Self::undo_group(
                undo_label,
                redo_label,
                undo_blocked,
                undo_running,
                &keyboard_shortcuts,
                cx,
            ))
            .child(sync_group)
            .child(stash_group)
            // Sin actividad el indicador no existe: tampoco su hueco.
            .when(has_activity, |actions| {
                actions.child(
                    OperationIndicator::new(operation_state, Some(conflict_state), queue)
                        .on_cancel({
                            let this = this.clone();
                            move |operation_id, _, cx| {
                                this.update(cx, |this, cx| {
                                    this.cancel_operation(operation_id, cx);
                                });
                            }
                        }),
                )
            });

        // Señal: `.tb-btn` con la rama actual y el commit en el que está.
        // Es un botón: abre el selector rápido de ramas.
        let text_3 = palette.text_3;
        let location = div()
            .id("toolbar-branch-button")
            .test_support()
            .aria_label(SharedString::from(format!(
                "Switch branch ({branch_label})"
            )))
            .flex()
            .flex_shrink_1()
            .items_center()
            .gap(px(6.))
            .min_w(px(96.))
            .max_w(px(320.))
            .h(px(28.))
            .px(px(8.))
            .rounded(Radius::Md.px(cx))
            .cursor_pointer()
            .hover(move |button| button.bg(palette.bg_hover))
            .active(move |button| button.bg(palette.bg_pressed))
            .tooltip(mergeon_ui::tooltip("Switch branch…"))
            .child(
                mergeon_ui::icon(Lucide::GitBranch)
                    .size(px(14.))
                    .tone(IconTone::Muted),
            )
            .child(
                div()
                    .id("toolbar-branch")
                    .test_support()
                    .aria_label(branch_label.clone())
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.text_1)
                    .child(branch_label),
            )
            .when_some(head_hash, |this, hash| {
                let title: SharedString = toolbar_head_title(&hash).into();
                this.child(
                    div()
                        .id("toolbar-head")
                        .test_support()
                        .aria_label(title.clone())
                        .tooltip(mergeon_ui::tooltip(title))
                        .flex_shrink_0()
                        .font_family(mono_font(cx))
                        .text_size(text::CAPTION)
                        .text_color(text_3)
                        .child(hash),
                )
            })
            .child(
                mergeon_ui::icon(Lucide::ChevronDown)
                    .size(px(12.))
                    .text_color(text_3),
            )
            .on_click(cx.listener(|_, _, _, cx| cx.emit(ToolbarEvent::OpenBranchPicker)));

        let search = self.search_field(compact, &keyboard_shortcuts, cx);

        let right = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(2.))
            .children(ICON_ACTIONS.into_iter().map(|action| {
                let mutates = action.operation().is_some();
                let busy = mutates && busy_operation == action.operation();
                Self::icon_button(
                    action,
                    busy,
                    busy || (mutates && repo_busy),
                    &keyboard_shortcuts,
                    cx,
                )
            }));

        div()
            .id("toolbar")
            .test_support()
            .relative()
            .flex()
            .flex_shrink_0()
            .w_full()
            .pl(px(12.))
            .pr(px(6.))
            .bg(palette.bg_sunken)
            .border_b_1()
            .border_color(palette.border_hair)
            .map(|toolbar| {
                if compact {
                    // Dos filas: las acciones arriba y, debajo, dónde estás y
                    // el buscador, que se lleva el ancho que sobre.
                    toolbar
                        .flex_col()
                        .gap(px(6.))
                        .py(px(8.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(actions)
                                .child(div().flex_1())
                                .child(right),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(location)
                                .child(search),
                        )
                } else {
                    // Tres zonas: las acciones a la izquierda, el buscador al
                    // centro de la ventana (las zonas laterales crecen igual)
                    // y, a su derecha, la rama y luego los íconos. Así las
                    // dos zonas pesan parecido y la rama no se corta en
                    // ventanas normales.
                    toolbar
                        .items_center()
                        .gap(px(12.))
                        .h(TOOLBAR_HEIGHT)
                        .child(
                            div()
                                .id("toolbar-left")
                                .flex()
                                .flex_1()
                                .min_w_0()
                                .items_center()
                                .child(actions),
                        )
                        .child(search)
                        .child(
                            div()
                                .id("toolbar-right")
                                .flex()
                                .flex_1()
                                .min_w_0()
                                .items_center()
                                .gap(px(6.))
                                .child(location)
                                .child(div().flex_1().min_w(px(6.)))
                                .child(right),
                        )
                }
            })
            // La consulta de contenedor del original: la disposición depende
            // del ancho real de la barra, que solo se conoce tras el layout.
            .on_prepaint(move |bounds, window, cx| {
                let now_compact = bounds.size.width <= COMPACT_MAX_WIDTH;
                if now_compact != compact {
                    window.defer(cx, move |_, cx| {
                        this.update(cx, |this, cx| {
                            this.compact = now_compact;
                            cx.notify();
                        });
                    });
                }
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mergeon_model::repositories::Repository;
    use serde_json::Value;

    fn repository(
        branch: &str,
        detached: Option<&str>,
        hash: Option<&str>,
    ) -> Repository<Value, Value> {
        let mut repository =
            Repository::new("/tmp/git_gui", Some("git_gui")).expect("a valid repository path");
        repository.current_branch = Some(branch.to_string());
        repository.detached_head_ref = detached.map(str::to_string);
        repository.head_short_hash = hash.map(str::to_string);
        repository
    }

    /// `Toolbar.test.js`: "shows a detached label with the short hash instead
    /// of the literal HEAD label".
    #[test]
    fn a_detached_head_shows_the_commit_it_points_to() {
        let repository = repository("HEAD", Some("2eea100"), None);
        assert_eq!(toolbar_branch_label(&repository), "Detached · 2eea100");
    }

    /// "shows the commit HEAD is parked on while on a branch".
    #[test]
    fn a_branch_shows_its_name_and_the_commit_it_is_parked_on() {
        let repository = repository("main", None, Some("634a2d2"));

        assert_eq!(toolbar_branch_label(&repository), "main");
        assert_eq!(toolbar_head_hash(&repository).as_deref(), Some("634a2d2"));
        assert_eq!(
            toolbar_head_title("634a2d2"),
            "Current commit · HEAD is at 634a2d2"
        );
    }

    /// "does not repeat the hash when the detached label already carries it".
    #[test]
    fn a_detached_head_does_not_repeat_the_hash() {
        let repository = repository("HEAD", Some("2eea100"), Some("2eea100"));

        assert_eq!(toolbar_branch_label(&repository), "Detached · 2eea100");
        assert_eq!(toolbar_head_hash(&repository), None);
    }

    #[test]
    fn the_undo_tooltip_names_what_it_would_do_and_its_shortcut() {
        assert_eq!(
            undo_tooltip(Some("Undo commit 'Fix login'"), false, false),
            "Undo commit 'Fix login' (Ctrl+Z)"
        );
        assert_eq!(
            undo_tooltip(Some("Redo merge of feature/x"), true, true),
            "Redo merge of feature/x (⌘⇧Z)"
        );
        assert_eq!(undo_tooltip(None, false, false), "Nothing to undo (Ctrl+Z)");
        assert_eq!(
            undo_tooltip(None, true, false),
            "Nothing to redo (Ctrl+Shift+Z)"
        );
        assert_eq!(
            undo_button_tooltip(
                Some("Undo merge of feature/x"),
                false,
                Some("Finish or abort the merge first"),
                false,
                false,
            ),
            "Finish or abort the merge first"
        );
        assert_eq!(
            undo_button_tooltip(Some("Undo commit 'A'"), false, None, true, false),
            "Undoing…"
        );
    }

    #[test]
    fn a_detached_head_without_a_reference_falls_back_to_head() {
        let repository = repository("HEAD", None, None);
        assert_eq!(toolbar_branch_label(&repository), "Detached · HEAD");

        let mut unborn = repository;
        unborn.current_branch = None;
        assert_eq!(toolbar_branch_label(&unborn), "HEAD");
    }

    #[test]
    fn sync_tooltips_advertise_shift_and_right_click_in_direct_mode_and_push_tags() {
        let shortcuts = BTreeMap::new();
        for action in [
            ToolbarAction::Fetch,
            ToolbarAction::Pull,
            ToolbarAction::Push,
        ] {
            for mode in [ActionDialogMode::Toolbar, ActionDialogMode::Always] {
                let tip = action.tooltip(&shortcuts, Some(mode), false, false);
                assert!(
                    !tip.contains("Shift+click or right-click for options"),
                    "{mode:?}: {tip}"
                );
            }

            let direct_mode =
                action.tooltip(&shortcuts, Some(ActionDialogMode::Never), false, false);
            assert!(
                direct_mode.ends_with(" · Shift+click or right-click for options"),
                "{direct_mode}"
            );
        }

        assert!(
            !ToolbarAction::Stash
                .tooltip(&shortcuts, Some(ActionDialogMode::Never), false, false)
                .contains("Shift+click")
        );

        let push_with_tags =
            ToolbarAction::Push.tooltip(&shortcuts, Some(ActionDialogMode::Never), true, false);
        assert!(push_with_tags.contains(" · +tags"), "{push_with_tags}");
        assert!(
            push_with_tags.ends_with(" · Shift+click or right-click for options"),
            "{push_with_tags}"
        );

        let fetch_with_push_tags =
            ToolbarAction::Fetch.tooltip(&shortcuts, Some(ActionDialogMode::Never), true, false);
        assert!(
            !fetch_with_push_tags.contains("+tags"),
            "{fetch_with_push_tags}"
        );
    }
}
