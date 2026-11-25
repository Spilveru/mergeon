//! La caja de commit (`src/components/repository/RepositoryCommitPanel.vue`,
//! con el compositor de Señal), pensada para caber en una columna de 400px:
//! la caja de atajos de la lista (cerrable), una línea discreta con la rama
//! a la que va el commit, el asunto con su contador por tramos (neutro hasta
//! 50, ámbar hasta 72, rojo después), el cuerpo (que crece con el texto), las
//! opciones compactas "Amend", "Push" y "Sign", y una fila de acciones con
//! "Fixup" y el botón "Commit N files" con su atajo.

use std::rc::Rc;

use gpui_kit::actions;
use gpui_kit::base::input::Enter;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{Input, InputState, Textarea};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, AnyElement, App, Entity, Focusable as _, InteractiveElement as _, IntoElement,
    KeyBinding, ParentElement as _, RenderOnce, SharedString, StatefulInteractiveElement as _,
    Styled as _, TestSupportExt as _, Window, div, px, relative, rems,
};
use mergeon_git_query::FixupCandidate;
use mergeon_ui::menu::MenuItem;
use mergeon_ui::style::input_focus_overlay;
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{ActivePaletteExt as _, Lucide, Radius, TextareaState};

use crate::logic::{
    SUBJECT_LIMIT, SubjectTone, amend_warning, commit_blocked_reason, commit_button_label,
    commit_shortcut_label, commit_target, compose_commit_message, subject_counter, subject_tone,
};

actions!(
    mergeon_changes,
    [
        /// Hace el commit con el mensaje escrito (el atajo de la caja de mensaje).
        SubmitCommit
    ]
);

/// Contexto de teclado de la caja de mensaje.
const CONTEXT: &str = "CommitMessage";

/// `@keydown.meta.enter` y `@keydown.ctrl.enter` de la caja de mensaje: valen
/// los dos en cualquier plataforma.
pub(crate) fn bind_keys(cx: &mut App) {
    let context = Some("CommitMessage > Input");
    cx.bind_keys([
        KeyBinding::new("ctrl-enter", SubmitCommit, context),
        KeyBinding::new("cmd-enter", SubmitCommit, context),
    ]);
}

/// Señal: el asunto mide 30px y el cuerpo 76px (renglón de 19px). El cuerpo
/// crece con sus renglones hasta [`BODY_MAX_LINES`]; luego hace scroll.
const TITLE_HEIGHT: f32 = 30.;
const BODY_HEIGHT: f32 = 76.;
const BODY_LINE: f32 = 19.;
const BODY_MAX_LINES: usize = 12;

/// Alto del cuerpo para `body`: el mínimo de diseño o lo que pidan sus
/// renglones (con su relleno), sin pasar de [`BODY_MAX_LINES`].
fn body_height(body: &str) -> f32 {
    let lines = body.split('\n').count().clamp(1, BODY_MAX_LINES);
    // 4 renglones caben en 76px (relleno incluido).
    (BODY_HEIGHT + lines.saturating_sub(3) as f32 * BODY_LINE).max(BODY_HEIGHT)
}

type ToggleHandler = Rc<dyn Fn(&bool, &mut Window, &mut App)>;
type Handler = Rc<dyn Fn(&mut Window, &mut App)>;
type FixupHandler = Rc<dyn Fn(&FixupCandidate, &mut Window, &mut App)>;

/// Alto máximo del menú "Fixup into…": con 30 commits hace scroll.
const FIXUP_MENU_MAX_HEIGHT: f32 = 380.;

/// Caja de commit. Es un componente controlado: se construye en cada `render`
/// con el estado actual y avisa con sus `on_*`. El asunto y el cuerpo viven en
/// el `InputState` y el `TextareaState` que guarda la vista; al hacer commit
/// se unen como `asunto\n\ncuerpo` ([`compose_commit_message`]).
///
/// ```ignore
/// // en `new`:
/// let title = cx.new(|cx| InputState::new(window, cx).placeholder("Summary"));
/// let body = cx.new(|cx| TextareaState::new(window, cx).placeholder("Description"));
///
/// // en `render`:
/// CommitPanel::new(&self.title, &self.body)
///     .branch(branch)
///     .staged_count(staged.len())
///     .committable(self.can_commit(cx))
///     .sign(self.sign)
///     .on_sign_change(cx.listener(|this, value: &bool, _, cx| { /* ... */ }))
///     .on_commit(cx.listener(|this, _, window, cx| this.commit(window, cx)))
/// ```
#[derive(IntoElement)]
pub struct CommitPanel {
    title: Entity<InputState>,
    body: Entity<TextareaState>,
    shortcut_hints: bool,
    push_after_commit: bool,
    busy: bool,
    disabled: bool,
    can_commit: bool,
    staged_count: usize,
    branch: SharedString,
    amend: bool,
    /// Merge, rebase… a medias (`"merge"`, `"rebase"`…): sin fixup hasta acabar.
    operation_in_progress: Option<&'static str>,
    can_amend: bool,
    amend_is_pushed: bool,
    upstream: SharedString,
    sign: bool,
    on_dismiss_hints: Option<Handler>,
    on_push_after_commit_change: Option<ToggleHandler>,
    on_amend_change: Option<ToggleHandler>,
    on_sign_change: Option<ToggleHandler>,
    on_commit: Option<Handler>,
    fixup_targets: Rc<Vec<FixupCandidate>>,
    on_fixup: Option<FixupHandler>,
}

impl CommitPanel {
    pub fn new(title: &Entity<InputState>, body: &Entity<TextareaState>) -> Self {
        Self {
            title: title.clone(),
            body: body.clone(),
            shortcut_hints: false,
            push_after_commit: false,
            busy: false,
            disabled: false,
            can_commit: false,
            staged_count: 0,
            branch: SharedString::default(),
            amend: false,
            operation_in_progress: None,
            can_amend: false,
            amend_is_pushed: false,
            upstream: SharedString::default(),
            sign: false,
            on_dismiss_hints: None,
            on_push_after_commit_change: None,
            on_amend_change: None,
            on_sign_change: None,
            on_commit: None,
            fixup_targets: Rc::default(),
            on_fixup: None,
        }
    }

    /// Encima de la caja, el recuadro con las teclas de la lista de archivos
    /// (`[Space] stage / unstage · [D] discard · [↑][↓] move`).
    pub fn shortcut_hints(mut self, shortcut_hints: bool) -> Self {
        self.shortcut_hints = shortcut_hints;
        self
    }

    /// La `×` del recuadro de atajos: quien aloja la caja lo recuerda.
    pub fn on_dismiss_hints(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss_hints = Some(Rc::new(handler));
        self
    }

    /// `pushAfterCommit`.
    pub fn push_after_commit(mut self, push_after_commit: bool) -> Self {
        self.push_after_commit = push_after_commit;
        self
    }

    /// `busy`: hay un commit en curso. El botón gira y todo lo demás se apaga.
    pub fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }

    /// `disabled`: otra mutación tiene la vista ocupada.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// `canCommit`.
    pub fn committable(mut self, can_commit: bool) -> Self {
        self.can_commit = can_commit;
        self
    }

    /// `stagedCount`: para poder decir qué falta en lugar de dejar el botón
    /// muerto.
    pub fn staged_count(mut self, staged_count: usize) -> Self {
        self.staged_count = staged_count;
        self
    }

    /// `branch`: dónde va a caer el commit ("Commit to main").
    pub fn branch(mut self, branch: impl Into<SharedString>) -> Self {
        self.branch = branch.into();
        self
    }

    /// `amend`.
    pub fn amend(mut self, amend: bool) -> Self {
        self.amend = amend;
        self
    }

    /// Hay un merge, rebase, cherry-pick o revert a medias: "Fixup into…" se
    /// apaga (el commit es el de la operación y el autosquash, después).
    pub fn operation_in_progress(mut self, operation: Option<&'static str>) -> Self {
        self.operation_in_progress = operation;
        self
    }

    /// `canAmend`: hay un commit que enmendar; sin él la casilla no se ofrece.
    pub fn amendable(mut self, can_amend: bool) -> Self {
        self.can_amend = can_amend;
        self
    }

    /// `amendIsPushed`: el último commit ya está en la rama de seguimiento.
    pub fn amend_is_pushed(mut self, amend_is_pushed: bool) -> Self {
        self.amend_is_pushed = amend_is_pushed;
        self
    }

    /// `upstream`: la rama de seguimiento (para el aviso de enmendar).
    pub fn upstream(mut self, upstream: impl Into<SharedString>) -> Self {
        self.upstream = upstream.into();
        self
    }

    /// "Sign (SSH/GPG)": el commit se firma (`git commit -S`).
    pub fn sign(mut self, sign: bool) -> Self {
        self.sign = sign;
        self
    }

    /// `@update:push-after-commit`.
    pub fn on_push_after_commit_change(
        mut self,
        handler: impl Fn(&bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_push_after_commit_change = Some(Rc::new(handler));
        self
    }

    /// `@update:amend`.
    pub fn on_amend_change(
        mut self,
        handler: impl Fn(&bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_amend_change = Some(Rc::new(handler));
        self
    }

    /// La casilla "Sign (SSH/GPG)".
    pub fn on_sign_change(
        mut self,
        handler: impl Fn(&bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_sign_change = Some(Rc::new(handler));
        self
    }

    /// Los commits que ofrece "Fixup into…" (los últimos de la rama, del más
    /// nuevo al más viejo). Sin ninguno el botón se apaga.
    pub fn fixup_targets(mut self, targets: Rc<Vec<FixupCandidate>>) -> Self {
        self.fixup_targets = targets;
        self
    }

    /// "Fixup into…": se eligió un commit del menú. Solo se ofrece con algo
    /// preparado y sin "Amend last commit".
    pub fn on_fixup(
        mut self,
        handler: impl Fn(&FixupCandidate, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_fixup = Some(Rc::new(handler));
        self
    }

    /// `@commit`: el botón o su atajo, solo cuando se puede hacer commit.
    pub fn on_commit(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_commit = Some(Rc::new(handler));
        self
    }
}

/// Las entradas del menú "Fixup into…": los commits sin publicar y, aparte,
/// los que ya están en el remoto (elegir uno de esos pide confirmación).
pub fn fixup_menu_items(
    targets: &[FixupCandidate],
    branch: &str,
    on_fixup: &FixupHandler,
) -> Vec<MenuItem> {
    let item = |target: &FixupCandidate| {
        let handler = on_fixup.clone();
        let candidate = target.clone();
        MenuItem::new(format!("{}  {}", target.short_hash, target.subject))
            .icon(if target.published {
                Lucide::CloudCheck
            } else {
                Lucide::GitCommitHorizontal
            })
            .on_click(move |window, cx| handler(&candidate, window, cx))
    };
    let (unpublished, published): (Vec<&FixupCandidate>, Vec<&FixupCandidate>) =
        targets.iter().partition(|target| !target.published);

    let mut items = Vec::with_capacity(targets.len() + 3);
    let heading = if branch.is_empty() {
        "Fixup into · not pushed".to_string()
    } else {
        format!("Fixup into · not pushed on {branch}")
    };
    items.push(MenuItem::header(heading));
    if unpublished.is_empty() {
        items.push(MenuItem::label("Every recent commit is already pushed."));
    }
    items.extend(unpublished.into_iter().map(item));
    if !published.is_empty() {
        items.push(MenuItem::separator());
        items.push(MenuItem::header("Already pushed"));
        items.extend(published.into_iter().map(item));
    }
    items
}

/// `.hintz`: las teclas de la lista en un recuadro de borde punteado, en una
/// línea y con una `×` para cerrarlo.
fn shortcut_hints(on_dismiss: Option<Handler>, cx: &App) -> impl IntoElement {
    let palette = *cx.palette();
    let close = on_dismiss.map(|handler| {
        crate::parts::PlainButton::new("changes-shortcut-hints-close")
            .label("Hide shortcuts")
            .tooltip("Hide these shortcuts (they stay in the checkbox tooltips)")
            .on_click(move |_, window, cx| handler(window, cx))
            .styled(|button, cx| {
                let palette = *cx.palette();
                button
                    .size(px(18.))
                    .rounded(Radius::Sm.px(cx))
                    .hover(move |style| style.bg(palette.bg_hover))
            })
            .child(
                mergeon_ui::icon(Lucide::X)
                    .size(px(12.))
                    .tone(mergeon_ui::IconTone::Muted),
            )
    });
    let hint = |keys: &[&'static str], text: &'static str| {
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .children(keys.iter().map(|key| mergeon_ui::kbd(*key)))
            .child(div().ml(px(2.)).child(text))
    };
    div()
        .id("changes-shortcut-hints")
        .test_support()
        .aria_label("Space stage / unstage · D discard · ↑ ↓ move")
        .flex()
        .items_center()
        .gap(px(8.))
        .py(px(5.))
        .pl(px(10.))
        .pr(px(5.))
        .border_1()
        .border_dashed()
        .border_color(palette.border_base)
        .rounded(Radius::Md.px(cx))
        .text_size(px(11.5))
        .text_color(palette.text_3)
        .child(
            div()
                .flex()
                .flex_1()
                .flex_wrap()
                .min_w_0()
                .gap_x(px(12.))
                .gap_y(px(4.))
                .child(hint(&["Space"], "stage / unstage"))
                .child(hint(&["D"], "discard"))
                .child(hint(&["↑", "↓"], "move")),
        )
        .children(close)
}

impl RenderOnce for CommitPanel {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let radius = Radius::Md.px(cx);
        let inert = self.disabled || self.busy;
        let title = self.title.read(cx).value();
        let body = self.body.read(cx).value();
        let message = compose_commit_message(&title, &body);
        let label: SharedString =
            commit_button_label(self.amend, self.push_after_commit, self.staged_count).into();
        let target: SharedString = commit_target(self.amend, &self.branch).into();
        let blocked_reason = commit_blocked_reason(
            self.can_commit,
            self.busy,
            self.staged_count,
            self.amend,
            &message,
        );
        let (subject_length, _) = subject_counter(&title);
        let small = rems(0.6875);
        let blocked = !self.can_commit || inert;

        // `submit`: ni el botón ni el atajo hacen commit cuando no se puede.
        let submit: Option<Handler> = self.on_commit.clone().filter(|_| self.can_commit && !inert);
        let title_focused = !inert
            && self
                .title
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx);
        let body_focused = !inert
            && self
                .body
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx);

        // `Ctrl+Enter` / `Cmd+Enter` en cualquiera de los dos campos.
        let message_keys = |id: &'static str, enter_moves_on: bool| {
            let body = self.body.clone();
            div()
                .id(id)
                .test_support()
                .key_context(CONTEXT)
                .relative()
                .w_full()
                .min_w_0()
                .on_action({
                    let submit = submit.clone();
                    move |_: &SubmitCommit, window, cx| {
                        if let Some(submit) = &submit {
                            submit(window, cx);
                        }
                    }
                })
                // Con el modificador secundario de la plataforma gpui-kit pide
                // un salto de línea: aquí es el atajo de commit. En el asunto,
                // un Enter a secas pasa al cuerpo.
                .capture_action({
                    let submit = submit.clone();
                    move |action: &Enter, window, cx| {
                        if action.secondary {
                            cx.stop_propagation();
                            if let Some(submit) = &submit {
                                submit(window, cx);
                            }
                        } else if enter_moves_on {
                            cx.stop_propagation();
                            let handle = body.read(cx).focus_handle(cx);
                            window.focus(&handle, cx);
                        }
                    }
                })
        };

        // El asunto: una línea con el contador dentro, a la derecha.
        let counter = div()
            .id("commit-title-counter")
            .test_support()
            .aria_label(SharedString::from(format!(
                "{subject_length}/{SUBJECT_LIMIT}"
            )))
            .flex_shrink_0()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(px(11.))
            .text_color(match subject_tone(subject_length) {
                SubjectTone::Neutral => palette.text_3,
                SubjectTone::Warning => palette.warning_foreground,
                SubjectTone::Danger => palette.destructive_foreground,
            })
            .child(format!("{subject_length}/{SUBJECT_LIMIT}"));
        let title_field = message_keys("commit-title", true)
            .child(
                Input::new(&self.title)
                    .focus_bordered(false)
                    .disabled(inert)
                    .suffix(counter)
                    .h(px(TITLE_HEIGHT))
                    .px(px(10.))
                    .text_size(px(13.))
                    .bg(palette.bg_base)
                    .rounded(radius),
            )
            .when(title_focused, |this| {
                this.child(input_focus_overlay(radius, cx))
            });

        let body_field = message_keys("commit-body", false)
            .child(
                Textarea::new(&self.body)
                    .disabled(inert)
                    .h(px(body_height(&body)))
                    .line_height(px(BODY_LINE))
                    .text_size(px(13.))
                    .bg(palette.bg_base)
                    .rounded(radius),
            )
            .when(body_focused, |this| {
                this.child(input_focus_overlay(radius, cx))
            });

        // `.chk`: casilla compacta con la etiqueta en `text-2`, 12px y lo que
        // hace en el tooltip.
        let checkbox = |id: &'static str,
                        text: &'static str,
                        tip: &'static str,
                        checked,
                        handler: Option<ToggleHandler>| {
            div()
                .id(SharedString::from(format!("{id}-tip")))
                .flex_shrink_0()
                .tooltip(mergeon_ui::tooltip(tip))
                .child(
                    mergeon_ui::checkbox(id)
                        .label(text)
                        .checked(checked)
                        .disabled(inert)
                        .gap(px(6.))
                        .text_size(px(12.))
                        .text_color(palette.text_2)
                        .when_some(handler, |this, handler| {
                            this.on_change(move |value, window, cx| handler(value, window, cx))
                        }),
                )
        };

        let [modifier, enter] = commit_shortcut_label();
        let shortcut_key = |key: &'static str| -> AnyElement {
            // Señal: el atajo dentro del primario va sin caja propia, en tinta.
            mergeon_ui::kbd(key)
                .bg(gpui_kit::transparent_black())
                .shadow_none()
                .border_color(if blocked {
                    palette.border.into()
                } else {
                    with_alpha(palette.primary_foreground, 0.28)
                })
                .text_color(if blocked {
                    palette.text_disabled
                } else {
                    palette.primary_foreground
                })
                .into_any_element()
        };
        let commit_button = div()
            // El nombre del botón (`mergeon_ui::button` no lo publica): lo que
            // hará, sin el atajo.
            .id("commit-action")
            .test_support()
            .aria_label(label.clone())
            .flex_1()
            .min_w_0()
            .child(
                mergeon_ui::button("commit")
                    .full_width(true)
                    .loading(self.busy)
                    .disabled(blocked)
                    .when(!self.busy, |this| this.icon(Lucide::Check))
                    .label(label)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(3.))
                            .ml(px(2.))
                            .child(shortcut_key(modifier))
                            .child(shortcut_key(enter)),
                    )
                    .when_some(submit, |this, submit| {
                        this.on_click(move |_, window, cx| submit(window, cx))
                    }),
            );

        // "Fixup into…": un commit `fixup!` con lo preparado para uno de los
        // últimos commits de la rama.
        let fixup_handler = self.on_fixup.clone().filter(|_| {
            !inert
                && !self.amend
                && self.operation_in_progress.is_none()
                && self.staged_count > 0
                && !self.fixup_targets.is_empty()
        });
        let fixup_tooltip: SharedString = if let Some(operation) = self.operation_in_progress {
            format!("Finish or abort the {operation} first").into()
        } else if self.amend {
            "Turn off Amend to create a fixup commit".into()
        } else if self.staged_count == 0 {
            "Stage the changes for the fixup first".into()
        } else {
            "Commit the staged changes as a fixup! of an earlier commit".into()
        };
        let fixup_button = mergeon_ui::button("commit-fixup")
            .outline()
            .icon(Lucide::Wrench)
            .label("Fixup")
            .trailing_icon(Lucide::ChevronDown)
            .tooltip(fixup_tooltip)
            .disabled(fixup_handler.is_none());
        let fixup_control = match fixup_handler {
            Some(handler) => {
                let targets = self.fixup_targets.clone();
                let branch = self.branch.to_string();
                fixup_button
                    .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, window, cx| {
                        mergeon_ui::menu::build(
                            menu.max_h(px(FIXUP_MENU_MAX_HEIGHT)).scrollable(true),
                            fixup_menu_items(&targets, &branch, &handler),
                            window,
                            cx,
                        )
                    })
                    .into_any_element()
            }
            None => fixup_button.into_any_element(),
        };
        // La fila de acciones: "Fixup" a la izquierda y el commit ocupando
        // el resto, a cualquier ancho.
        let actions = div()
            .flex()
            .w_full()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .id("commit-fixup-action")
                    .test_support()
                    .flex_shrink_0()
                    .child(fixup_control),
            )
            .child(commit_button);

        // Adónde va el commit, discreto, encima del asunto.
        let target_line = div()
            .id("commit-target")
            .test_support()
            .aria_label(target.clone())
            .flex()
            .items_center()
            .gap(px(6.))
            .min_w_0()
            .text_size(px(11.))
            .text_color(palette.text_3)
            .child(
                mergeon_ui::icon(Lucide::GitBranch)
                    .size(px(12.))
                    .tone(mergeon_ui::IconTone::Muted),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_family(cx.theme().mono_font_family.clone())
                    .child(target),
            )
            .when(self.staged_count > 0 || self.amend, |this| {
                this.child(
                    div()
                        .id("commit-staged-count")
                        .test_support()
                        .aria_label(SharedString::from(format!("{} staged", self.staged_count)))
                        .ml_auto()
                        .flex_shrink_0()
                        .child(format!("{} staged", self.staged_count)),
                )
            });

        div()
            .id("commit-panel")
            .test_support()
            .flex()
            .flex_col()
            .flex_none()
            .w_full()
            .when(self.shortcut_hints, |this| {
                this.child(
                    div()
                        .px(px(14.))
                        .pt(px(10.))
                        .pb(px(10.))
                        .child(shortcut_hints(self.on_dismiss_hints.clone(), cx)),
                )
            })
            .child(
                // `.composer`
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .pt(px(10.))
                    .px(px(14.))
                    .pb(px(12.))
                    .border_t_1()
                    .border_color(palette.border_hair)
                    .bg(palette.bg_sunken)
                    .child(target_line)
                    .child(title_field)
                    .child(body_field)
                    .child(
                        // `.crow2`: las opciones, compactas.
                        div()
                            .id("commit-options")
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_x(px(14.))
                            .gap_y(px(6.))
                            .when(self.can_amend, |this| {
                                this.child(checkbox(
                                    "amend-last-commit",
                                    "Amend",
                                    "Amend the last commit instead of creating a new one",
                                    self.amend,
                                    self.on_amend_change,
                                ))
                            })
                            .child(checkbox(
                                "push-after-commit",
                                "Push",
                                "Push the branch right after committing",
                                self.push_after_commit,
                                self.on_push_after_commit_change,
                            ))
                            .child(checkbox(
                                "sign-commit",
                                "Sign",
                                "Sign the commit with your SSH or GPG key (git commit -S)",
                                self.sign,
                                self.on_sign_change,
                            )),
                    )
                    .child(actions)
                    // Reescribir algo que ya está en el remoto es la única forma
                    // de que enmendar haga daño. Callarlo sería lo peor.
                    .when(self.amend && self.amend_is_pushed, |this| {
                        let warning: SharedString = amend_warning(&self.upstream).into();
                        this.child(
                            div()
                                .id("commit-warning")
                                .test_support()
                                .aria_label(warning.clone())
                                .text_size(small)
                                .line_height(relative(1.45))
                                .text_color(palette.warning_foreground)
                                .child(warning),
                        )
                    })
                    // Un botón apagado no dice qué falta. Sin nada que decir
                    // la línea se queda vacía pero ocupa su sitio: si no, al
                    // escribir el asunto el panel encogía y el botón bajaba
                    // justo cuando se iba a pulsar (QA 0.12.0).
                    .map(|this| {
                        if blocked_reason.is_empty() {
                            this.child(div().text_size(small).child("\u{a0}"))
                        } else {
                            this.child(
                                div()
                                    .id("commit-blocked")
                                    .test_support()
                                    .aria_label(blocked_reason)
                                    .flex()
                                    .justify_end()
                                    .text_size(small)
                                    .text_color(palette.text_3)
                                    .child(blocked_reason),
                            )
                        }
                    }),
            )
    }
}
