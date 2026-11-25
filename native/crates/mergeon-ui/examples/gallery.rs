//! Galería del sistema de diseño de Mergeon: todos los componentes de
//! `mergeon_ui` en todas sus variantes y estados.
//!
//! ```bash
//! cargo run -p mergeon-ui --example gallery
//! MERGEON_THEME=light MERGEON_GALLERY_PAGE=forms cargo run -p mergeon-ui --example gallery
//! ```
//!
//! Variables de entorno:
//!
//! - `MERGEON_THEME=dark|light|system` (por defecto `dark`).
//! - `MERGEON_GALLERY_PAGE=controls|forms|feedback|overlays` (por defecto `controls`).
//! - `MERGEON_GALLERY_OPEN=`, lista separada por comas de lo que se abre al
//!   arrancar: `dialog`, `dialog-long`, `confirm`, `confirm-option`,
//!   `confirm-destructive`, `prompt`, `prompt-textarea`, `prompt-error`,
//!   `toasts`, `toast-action`, `toast-path`, `details`, `stacked`.
//! - `MERGEON_GALLERY_INPUT=`, pasos separados por `;` que se ejecutan después
//!   de arrancar (uno cada 300ms), para capturar estados que piden interacción:
//!   `key:<tecla>` (`key:tab`, `key:enter`), `focus:<n>` (avanza el foco de
//!   teclado n veces), `move:x,y`, `click:x,y`, `rclick:x,y`. Las coordenadas
//!   son de la ventana.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;
use mergeon_ui::dialog::{
    self, AppDialog, ConfirmOption, ConfirmOptions, ConfirmVariant, DialogWidth, PromptOptions,
};
use mergeon_ui::menu::{self, MenuItem, MenuTriggerExt as _};
use mergeon_ui::toast::{self, ToastKind, ToastOptions};
use mergeon_ui::{
    ActivePaletteExt as _, AlertVariant, BadgeSize, BadgeVariant, ButtonSize, ButtonVariant,
    HoverCardAlign, IconTone, InputState, Lucide, MergeonStyled as _, RadioOption, Radius,
    SelectOption, SpinnerColor, SpinnerSize, TabItem, TextareaState, ThemePreference,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Controls,
    Forms,
    Feedback,
    Overlays,
}

impl Page {
    fn parse(name: &str) -> Self {
        match name {
            "forms" => Self::Forms,
            "feedback" => Self::Feedback,
            "overlays" => Self::Overlays,
            _ => Self::Controls,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Controls => "controls",
            Self::Forms => "forms",
            Self::Feedback => "feedback",
            Self::Overlays => "overlays",
        }
    }
}

struct Gallery {
    page: Page,
    empty_input: Entity<InputState>,
    filled_input: Entity<InputState>,
    search_input: Entity<InputState>,
    disabled_input: Entity<InputState>,
    textarea: Entity<TextareaState>,
    fetch_all: bool,
    prune: bool,
    mode: SharedString,
    remote: SharedString,
    theme: SharedString,
    tab: SharedString,
    wide_tab: SharedString,
    menu_preview: Entity<PopupMenu>,
    select_preview: Entity<PopupMenu>,
    last_action: SharedString,
}

fn menu_items() -> Vec<MenuItem> {
    vec![
        MenuItem::new("Checkout").icon(Lucide::GitBranch),
        MenuItem::new("Merge into current branch…").icon(Lucide::GitMerge),
        MenuItem::new("Rebase current onto this").icon(Lucide::GitPullRequest),
        MenuItem::separator(),
        MenuItem::new("Copy branch name")
            .icon(Lucide::Copy)
            .shortcut("Ctrl+C"),
        MenuItem::new("Rename…").icon(Lucide::Pencil).disabled(true),
        MenuItem::separator(),
        MenuItem::new("Delete branch")
            .icon(Lucide::Trash)
            .danger()
            .shortcut("Del"),
    ]
}

impl Gallery {
    fn new(page: Page, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let menu_preview = PopupMenu::build(window, cx, |popup, window, cx| {
            menu::build(popup, menu_items(), window, cx)
        });
        let select_preview = PopupMenu::build(window, cx, |popup, window, cx| {
            let entries = ["origin", "upstream", "fork"]
                .into_iter()
                .map(|name| MenuItem::new(name).checked(name == "origin").list_box());
            menu::build(popup, entries, window, cx)
                .min_w(px(260.))
                .max_w(px(260.))
        });
        Self {
            page,
            empty_input: cx.new(|cx| InputState::new(window, cx).placeholder("Branch name")),
            filled_input: cx.new(|cx| InputState::new(window, cx).default_value("feature/login")),
            search_input: cx.new(|cx| InputState::new(window, cx).placeholder("Filter branches")),
            disabled_input: cx.new(|cx| InputState::new(window, cx).default_value("origin/main")),
            textarea: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(4)
                    .placeholder("Commit message")
            }),
            fetch_all: true,
            prune: false,
            mode: "merge".into(),
            remote: "origin".into(),
            theme: "".into(),
            tab: "changes".into(),
            wide_tab: "summary".into(),
            menu_preview,
            select_preview,
            last_action: "—".into(),
        }
    }

    fn section(title: &'static str, cx: &App) -> Div {
        div().flex().flex_col().gap_3().child(
            div()
                .tw_text_xs()
                .tw_font_medium()
                .text_color(cx.palette().muted_foreground)
                .child(title),
        )
    }

    fn row() -> Div {
        div().flex().flex_wrap().items_center().gap_2()
    }

    fn column() -> Div {
        div().flex().flex_col().flex_1().min_w_0().gap_6()
    }

    fn render_controls(&mut self, cx: &mut Context<Self>) -> Div {
        let variants = [
            ("Default", ButtonVariant::Default),
            ("Secondary", ButtonVariant::Secondary),
            ("Outline", ButtonVariant::Outline),
            ("Ghost", ButtonVariant::Ghost),
            ("Link", ButtonVariant::Link),
            ("Destructive", ButtonVariant::Destructive),
            ("Destructive outline", ButtonVariant::DestructiveOutline),
        ];
        let sizes = [
            ("xs", ButtonSize::Xs),
            ("sm", ButtonSize::Sm),
            ("default", ButtonSize::Default),
            ("lg", ButtonSize::Lg),
        ];

        let buttons = Self::section("Button · variants × sizes", cx).children(sizes.map(
            |(size_name, size)| {
                Self::row().children(variants.map(|(label, variant)| {
                    mergeon_ui::button(SharedString::from(format!("{size_name}-{label}")))
                        .variant(variant)
                        .size(size)
                        .label(label)
                }))
            },
        ));

        let with_icons = Self::section("Button · icons, loading, disabled", cx)
            .child(
                Self::row()
                    .child(
                        mergeon_ui::button("icon-fetch")
                            .outline()
                            .icon(Lucide::Download)
                            .label("Fetch"),
                    )
                    .child(
                        mergeon_ui::button("icon-pull")
                            .outline()
                            .sm()
                            .icon(Lucide::ArrowDownToLine)
                            .label("Pull"),
                    )
                    .child(
                        mergeon_ui::button("icon-push")
                            .icon(Lucide::Upload)
                            .label("Push"),
                    )
                    .child(
                        mergeon_ui::button("icon-stash")
                            .ghost()
                            .icon(Lucide::Archive)
                            .label("Stash"),
                    )
                    .child(
                        mergeon_ui::button("icon-branch")
                            .secondary()
                            .label("main")
                            .trailing_icon(Lucide::ChevronDown),
                    )
                    .child(
                        mergeon_ui::button("icon-delete")
                            .destructive_outline()
                            .icon(Lucide::Trash)
                            .label("Delete"),
                    ),
            )
            .child(
                Self::row()
                    .children(sizes.map(|(name, size)| {
                        mergeon_ui::button(SharedString::from(format!("only-{name}")))
                            .outline()
                            .size(size)
                            .icon(Lucide::RefreshCw)
                            .tooltip("Refresh")
                    }))
                    .children(sizes.map(|(name, size)| {
                        mergeon_ui::button(SharedString::from(format!("only-ghost-{name}")))
                            .ghost()
                            .size(size)
                            .icon(Lucide::Settings)
                    }))
                    .child(mergeon_ui::button("only-primary").icon(Lucide::Plus))
                    .child(
                        mergeon_ui::button("only-destructive")
                            .destructive()
                            .icon(Lucide::X),
                    ),
            )
            .child(
                Self::row()
                    .child(mergeon_ui::button("loading").loading(true).label("Pushing"))
                    .child(
                        mergeon_ui::button("loading-outline")
                            .outline()
                            .loading(true)
                            .label("Fetching"),
                    )
                    .child(
                        mergeon_ui::button("loading-sm")
                            .outline()
                            .sm()
                            .loading(true)
                            .label("Pulling"),
                    )
                    .children(variants.map(|(label, variant)| {
                        mergeon_ui::button(SharedString::from(format!("disabled-{label}")))
                            .variant(variant)
                            .disabled(true)
                            .label(label)
                    })),
            )
            .child(
                div().w(rems(22.)).child(
                    mergeon_ui::button("full")
                        .full_width(true)
                        .icon(Lucide::GitCommitHorizontal)
                        .label("Commit to main"),
                ),
            );

        let badge_variants = [
            ("Default", BadgeVariant::Default),
            ("Primary", BadgeVariant::Primary),
            ("Secondary", BadgeVariant::Secondary),
            ("Outline", BadgeVariant::Outline),
            ("Info", BadgeVariant::Info),
            ("Success", BadgeVariant::Success),
            ("Warning", BadgeVariant::Warning),
            ("Destructive", BadgeVariant::Destructive),
        ];
        let badges =
            Self::section("Badge · variants, sizes, solid, square", cx)
                .children([BadgeSize::Sm, BadgeSize::Md, BadgeSize::Lg].map(|size| {
                    Self::row().children(badge_variants.map(|(label, variant)| {
                        mergeon_ui::badge(label).variant(variant).size(size)
                    }))
                }))
                .child(Self::row().children(
                    badge_variants.map(|(label, variant)| {
                        mergeon_ui::badge(label).variant(variant).solid(true)
                    }),
                ))
                .child(
                    Self::row()
                        .child(
                            mergeon_ui::badge("3 ahead")
                                .variant(BadgeVariant::Info)
                                .icon(Lucide::ArrowUp),
                        )
                        .child(
                            mergeon_ui::badge("Private")
                                .variant(BadgeVariant::Secondary)
                                .icon(Lucide::Lock),
                        )
                        .child(
                            mergeon_ui::badge("12")
                                .variant(BadgeVariant::Primary)
                                .size(BadgeSize::Sm),
                        )
                        .child(mergeon_ui::badge("Active")),
                )
                .child(Self::row().gap_3().children(
                    ["M", "A", "D", "R", "C", "??", "U", "X"].map(|status| {
                        div()
                            .flex()
                            .items_center()
                            .gap(rems(0.375))
                            .child(mergeon_ui::status_badge(status))
                            .child(
                                div()
                                    .tw_text_xs()
                                    .text_color(cx.palette().muted_foreground)
                                    .child(status),
                            )
                    }),
                ));

        let small = Self::section("Kbd · Spinner · AppIcon", cx)
            .child(
                Self::row()
                    .child(mergeon_ui::kbd("Ctrl"))
                    .child(mergeon_ui::kbd("K"))
                    .child(mergeon_ui::kbd("Ctrl+Shift+P"))
                    .child(mergeon_ui::kbd("⌘P"))
                    .child(mergeon_ui::kbd("Del")),
            )
            .child(
                Self::row()
                    .gap_4()
                    .children(
                        [
                            SpinnerSize::Sm,
                            SpinnerSize::Md,
                            SpinnerSize::Lg,
                            SpinnerSize::Xl,
                        ]
                        .map(|size| mergeon_ui::spinner().size(size)),
                    )
                    .children(
                        [
                            SpinnerColor::Primary,
                            SpinnerColor::Danger,
                            SpinnerColor::Success,
                            SpinnerColor::Warning,
                        ]
                        .map(|color| mergeon_ui::spinner().size(SpinnerSize::Lg).color(color)),
                    ),
            )
            .child(
                Self::row()
                    .gap_4()
                    .children(
                        [
                            IconTone::Default,
                            IconTone::Muted,
                            IconTone::Active,
                            IconTone::Danger,
                            IconTone::Success,
                        ]
                        .map(|tone| mergeon_ui::icon(Lucide::GitBranch).tone(tone)),
                    )
                    .child(mergeon_ui::icon(Lucide::FolderGit2).size(px(20.)))
                    .child(
                        mergeon_ui::icon(Lucide::GitCommitHorizontal)
                            .size(px(24.))
                            .tone(IconTone::Muted),
                    )
                    .child(
                        mergeon_ui::icon(Lucide::LoaderCircle)
                            .tone(IconTone::Muted)
                            .spin(true),
                    ),
            );

        div()
            .flex()
            .items_start()
            .gap_10()
            .child(Self::column().child(buttons).child(with_icons))
            .child(Self::column().child(badges).child(small))
    }

    fn render_forms(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let _ = window;
        let label = |text: &'static str| {
            div()
                .tw_text_sm()
                .tw_font_medium()
                .text_color(cx.palette().foreground)
                .child(text)
        };
        let field = |text: &'static str, control: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(rems(0.375))
                .child(label(text))
                .child(control)
        };

        let inputs = Self::section("Input · Textarea", cx)
            .child(field(
                "Empty",
                mergeon_ui::input(&self.empty_input).into_any_element(),
            ))
            .child(field(
                "With value",
                mergeon_ui::input(&self.filled_input).into_any_element(),
            ))
            .child(field(
                "With prefix",
                mergeon_ui::input(&self.search_input)
                    .prefix(mergeon_ui::icon(Lucide::Search).tone(IconTone::Muted))
                    .cleanable(true)
                    .into_any_element(),
            ))
            .child(field(
                "Disabled",
                mergeon_ui::input(&self.disabled_input)
                    .disabled(true)
                    .into_any_element(),
            ))
            .child(field(
                "Textarea",
                mergeon_ui::textarea(&self.textarea).into_any_element(),
            ));

        let checks = Self::section("Checkbox", cx)
            .child(
                mergeon_ui::checkbox("fetch-all")
                    .label("Fetch all remotes")
                    .checked(self.fetch_all)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.fetch_all = *checked;
                        cx.notify();
                    })),
            )
            .child(
                mergeon_ui::checkbox("prune")
                    .label("Prune deleted branches")
                    .checked(self.prune)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.prune = *checked;
                        cx.notify();
                    })),
            )
            .child(
                mergeon_ui::checkbox("disabled-on")
                    .label("Disabled, checked")
                    .checked(true)
                    .disabled(true),
            )
            .child(
                mergeon_ui::checkbox("disabled-off")
                    .label("Disabled, unchecked")
                    .disabled(true),
            )
            .child(mergeon_ui::checkbox("bare").checked(true));

        let radios = Self::section("RadioGroup", cx).child(
            mergeon_ui::radio_group("mode")
                .label("Integration mode")
                .option(
                    RadioOption::new("merge", "Merge")
                        .description("Create a merge commit joining both histories"),
                )
                .option(
                    RadioOption::new("rebase", "Rebase")
                        .description("Replay your commits on top of the target"),
                )
                .option(RadioOption::new("squash", "Squash"))
                .option(
                    RadioOption::new("ff", "Fast-forward only")
                        .description("Not available: branches have diverged")
                        .disabled(true),
                )
                .selected(self.mode.clone())
                .on_change(cx.listener(|this, value: &SharedString, _, cx| {
                    this.mode = value.clone();
                    cx.notify();
                })),
        );

        let selects = Self::section("Select", cx)
            .child(field(
                "With value",
                mergeon_ui::select("remote")
                    .items(["origin", "upstream", "fork"])
                    .value(self.remote.clone())
                    .on_change(cx.listener(|this, value: &SharedString, _, cx| {
                        this.remote = value.clone();
                        cx.notify();
                    }))
                    .into_any_element(),
            ))
            .child(field(
                "Placeholder · { value, label }",
                mergeon_ui::select("theme")
                    .items([
                        SelectOption::new("light", "Light"),
                        SelectOption::new("dark", "Dark"),
                        SelectOption::new("system", "Follow the system"),
                    ])
                    .value(self.theme.clone())
                    .on_change(cx.listener(|this, value: &SharedString, _, cx| {
                        this.theme = value.clone();
                        cx.notify();
                    }))
                    .into_any_element(),
            ))
            .child(field(
                "Disabled",
                mergeon_ui::select("disabled")
                    .items(["main"])
                    .value("main")
                    .disabled(true)
                    .into_any_element(),
            ))
            .child(
                Self::row().child(label("Inline")).child(
                    mergeon_ui::select("inline")
                        .items(["Split", "Unified"])
                        .value("Split")
                        .full_width(false)
                        .w(rems(9.)),
                ),
            )
            .child(field(
                "Open list (preview)",
                div()
                    .flex()
                    .child(self.select_preview.clone())
                    .into_any_element(),
            ));

        let tabs = Self::section("Tabs", cx)
            .child(
                mergeon_ui::tabs("tabs")
                    .item(TabItem::new("changes", "Changes"))
                    .item(TabItem::new("tree", "Repo tree"))
                    .item(TabItem::new("stats", "Stats").disabled(true))
                    .selected(self.tab.clone())
                    .on_change(cx.listener(|this, key: &SharedString, _, cx| {
                        this.tab = key.clone();
                        cx.notify();
                    })),
            )
            .child(
                mergeon_ui::tabs("tabs-icons")
                    .item(TabItem::new("changes", "Commits").icon(Lucide::GitCommitHorizontal))
                    .item(TabItem::new("tree", "Local Changes").icon(Lucide::FileDiff))
                    .selected(self.tab.clone())
                    .on_change(cx.listener(|this, key: &SharedString, _, cx| {
                        this.tab = key.clone();
                        cx.notify();
                    })),
            )
            .child(
                mergeon_ui::tabs("tabs-wide")
                    .full_width(true)
                    .item(TabItem::new("summary", "Summary"))
                    .item(TabItem::new("statistics", "Statistics"))
                    .item(TabItem::new("contributors", "Contributors"))
                    .selected(self.wide_tab.clone())
                    .on_change(cx.listener(|this, key: &SharedString, _, cx| {
                        this.wide_tab = key.clone();
                        cx.notify();
                    })),
            );

        div()
            .flex()
            .items_start()
            .gap_10()
            .child(Self::column().child(inputs))
            .child(Self::column().child(checks).child(radios))
            .child(Self::column().child(selects).child(tabs))
    }

    fn render_feedback(&mut self, cx: &mut Context<Self>) -> Div {
        let alerts = Self::section("Alert", cx)
            .child(
                mergeon_ui::alert(AlertVariant::Default)
                    .title("Detached HEAD")
                    .description("You are not on a branch. New commits will not belong to any branch."),
            )
            .child(
                mergeon_ui::alert(AlertVariant::Info)
                    .title("Update available")
                    .description("Mergeon 0.9.1 is ready to install."),
            )
            .child(
                mergeon_ui::alert(AlertVariant::Success)
                    .title("No conflicts")
                    .description("These branches integrate cleanly."),
            )
            .child(
                mergeon_ui::alert(AlertVariant::Warning)
                    .title("Merge conflicts expected")
                    .description("Integrating these branches will conflict in 2 file(s) — you'll need to resolve them."),
            )
            .child(
                mergeon_ui::alert(AlertVariant::Error)
                    .title("Repository not available")
                    .description("This path no longer exists or is not a valid Git repository."),
            )
            .child(
                mergeon_ui::alert(AlertVariant::Warning)
                    .description("Only a description, with an action on the right.")
                    .action(mergeon_ui::button("alert-action").outline().xs().label("Resolve")),
            )
            .child(
                mergeon_ui::alert(AlertVariant::Info)
                    .no_icon()
                    .title("Without icon"),
            );

        let empty = Self::section("EmptyState", cx)
            .child(
                mergeon_ui::empty_state("No repository selected")
                    .description(
                        "Choose an open or recent repository from the sidebar to see its summary.",
                    )
                    .icon(Lucide::FolderGit2),
            )
            .child(
                mergeon_ui::empty_state("No GitHub accounts configured")
                    .description(
                        "Add an account to clone repositories and use GitHub App authentication.",
                    )
                    .icon(Lucide::Puzzle)
                    .child(mergeon_ui::button("add-account").label("Add Account"))
                    .child(
                        mergeon_ui::button("learn-more")
                            .outline()
                            .label("Learn more"),
                    ),
            )
            .child(
                mergeon_ui::empty_state("Loading repositories")
                    .description("Fetching repositories from all available installations.")
                    .icon(Lucide::LoaderCircle)
                    .spin_icon(true),
            )
            .child(mergeon_ui::empty_state("Nothing here").glyph("∅"));

        div()
            .flex()
            .items_start()
            .gap_10()
            .child(Self::column().child(alerts))
            .child(Self::column().child(empty))
    }

    fn render_overlays(&mut self, cx: &mut Context<Self>) -> Div {
        let open = |id: &'static str, label: &'static str, what: &'static str| {
            mergeon_ui::button(id)
                .outline()
                .sm()
                .label(label)
                .on_click(move |_, window, cx| open_overlay(what, window, cx))
        };

        let dialogs = Self::section("AppDialog · confirm · prompt", cx)
            .child(
                Self::row()
                    .child(open("open-dialog", "Dialog", "dialog"))
                    .child(open("open-dialog-long", "Scrolling body", "dialog-long"))
                    .child(open("open-stacked", "Confirm over a dialog", "stacked")),
            )
            .child(
                Self::row()
                    .child(open("open-confirm", "Confirm", "confirm"))
                    .child(open(
                        "open-confirm-destructive",
                        "Destructive",
                        "confirm-destructive",
                    ))
                    .child(open("open-confirm-option", "With option", "confirm-option"))
                    .child(open("open-prompt", "Prompt", "prompt"))
                    .child(open(
                        "open-prompt-textarea",
                        "Prompt (textarea)",
                        "prompt-textarea",
                    )),
            );

        let toasts = Self::section("Toasts", cx).child(
            Self::row()
                .child(open("toast-success", "Success", "toast-success"))
                .child(open("toast-info", "Info", "toast-info"))
                .child(open("toast-warning", "Warning", "toast-warning"))
                .child(open("toast-error", "Error + Details", "toast-error"))
                .child(open("toast-action", "With action", "toast-action"))
                .child(open("toast-all", "All", "toasts")),
        );

        let menus = Self::section("ContextMenu · dropdown", cx)
            .child(
                Self::row()
                    .child(
                        div()
                            .id("context-target")
                            .px_3()
                            .py_2()
                            .radius(Radius::Lg, cx)
                            .border_1()
                            .border_color(cx.palette().border)
                            .bg(cx.palette().card)
                            .tw_text_sm()
                            .child("Right-click me")
                            .context_menu_items(|_, _| menu_items()),
                    )
                    .child(
                        mergeon_ui::button("dropdown")
                            .outline()
                            .label("Branch actions")
                            .trailing_icon(Lucide::ChevronDown)
                            .dropdown_menu_items(|_, _| {
                                let mut items = menu_items();
                                items.push(MenuItem::separator());
                                items.push(MenuItem::submenu(
                                    "More",
                                    vec![
                                        MenuItem::new("Set upstream…"),
                                        MenuItem::new("Compare with…"),
                                    ],
                                ));
                                items
                            }),
                    ),
            )
            .child(div().flex().child(self.menu_preview.clone()));

        let hover = Self::section("HoverCard · tooltip", cx).child(
            Self::row()
                .gap_4()
                .child(
                    mergeon_ui::hover_card("author-card")
                        .align(HoverCardAlign::Start)
                        .trigger(
                            div()
                                .tw_text_sm()
                                .text_color(cx.palette().primary_soft_foreground)
                                .child("Ada Lovelace"),
                        )
                        .content(|_, cx| {
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .p_2()
                                .tw_text_sm()
                                .child("Ada Lovelace")
                                .child(
                                    div()
                                        .tw_text_xs()
                                        .text_color(cx.palette().muted_foreground)
                                        .child("ada@example.com · 42 commits"),
                                )
                        }),
                )
                .child(
                    mergeon_ui::button("tooltip-target")
                        .outline()
                        .icon(Lucide::RefreshCw)
                        .tooltip("Refresh"),
                )
                .child(
                    div()
                        .id("tooltip-shortcut")
                        .tw_text_sm()
                        .child("Hover for a shortcut")
                        .tooltip(mergeon_ui::tooltip_with_shortcut(
                            "Command palette",
                            "Ctrl+K",
                        )),
                ),
        );

        div()
            .flex()
            .items_start()
            .gap_10()
            .child(Self::column().child(dialogs).child(toasts).child(hover))
            .child(Self::column().child(menus))
            .child(
                Self::column().child(
                    Self::section("Last action", cx)
                        .child(div().tw_text_sm().child(self.last_action.clone())),
                ),
            )
    }
}

impl Render for Gallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = self.page;
        let body = match page {
            Page::Controls => self.render_controls(cx),
            Page::Forms => self.render_forms(window, cx),
            Page::Feedback => self.render_feedback(cx),
            Page::Overlays => self.render_overlays(cx),
        };
        let dark = cx.theme().mode.is_dark();

        div()
            .flex()
            .flex_col()
            .size_full()
            .gap_6()
            .p_8()
            .bg(cx.palette().background)
            .text_color(cx.palette().foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .tw_text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Mergeon design system"),
                            )
                            .child(
                                div()
                                    .tw_text_sm()
                                    .text_color(cx.palette().muted_foreground)
                                    .child("mergeon_ui · every component, variant and state"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                mergeon_ui::tabs("pages")
                                    .item(TabItem::new("controls", "Controls"))
                                    .item(TabItem::new("forms", "Forms"))
                                    .item(TabItem::new("feedback", "Feedback"))
                                    .item(TabItem::new("overlays", "Overlays"))
                                    .selected(page.key())
                                    .on_change(cx.listener(|this, key: &SharedString, _, cx| {
                                        this.page = Page::parse(key);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                mergeon_ui::button("toggle-theme")
                                    .outline()
                                    .icon(if dark { Lucide::Sun } else { Lucide::Moon })
                                    .tooltip("Toggle theme")
                                    .on_click(move |_, _, cx| {
                                        mergeon_ui::theme::apply(
                                            if dark {
                                                ThemePreference::Light
                                            } else {
                                                ThemePreference::Dark
                                            },
                                            cx,
                                        );
                                    }),
                            ),
                    ),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}

fn long_body() -> impl IntoElement {
    div().flex().flex_col().gap_3().children((1..=24).map(|line| {
        div().child(format!(
            "{line}. A dialog body scrolls on its own when it does not fit; the header and the footer stay put."
        ))
    }))
}

fn fetch_dialog() -> AppDialog {
    AppDialog::new()
        .title("Fetch")
        .description("Fetch latest changes from remote repository")
        .width(DialogWidth::Lg)
        .body(|_, cx| {
            div()
                .flex()
                .items_start()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .justify_center()
                        .size_11()
                        .radius(Radius::Xl, cx)
                        .border_1()
                        .border_color(cx.palette().border)
                        .bg(cx.palette().primary_soft)
                        .child(
                            mergeon_ui::icon(Lucide::Download)
                                .size(px(22.))
                                .tone(IconTone::Active),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_4()
                                .child(div().w_16().tw_font_medium().child("Remote:"))
                                .child(
                                    mergeon_ui::select("fetch-remote")
                                        .items(["origin", "upstream"])
                                        .value("origin"),
                                ),
                        )
                        .child(
                            mergeon_ui::checkbox("fetch-all-remotes")
                                .label("Fetch all remotes")
                                .checked(true),
                        ),
                )
        })
        .footer(|_, _| {
            div()
                .flex()
                .gap_2()
                .child(
                    mergeon_ui::button("fetch-cancel")
                        .ghost()
                        .sm()
                        .label("Cancel")
                        .on_click(|_, window, cx| {
                            dialog::close(window, cx);
                        }),
                )
                .child(mergeon_ui::button("fetch-ok").sm().label("Fetch").on_click(
                    |_, window, cx| {
                        dialog::close(window, cx);
                        toast::success(window, cx, "Fetched origin.");
                    },
                ))
        })
}

fn open_overlay(what: &str, window: &mut Window, cx: &mut App) {
    match what {
        "dialog" => {
            fetch_dialog().open(window, cx);
        }
        "dialog-long" => {
            AppDialog::new()
                .title("Operation log")
                .description("Everything Mergeon ran in this repository")
                .width(DialogWidth::Xl2)
                .body(|_, _| long_body())
                .footer(|_, _| {
                    mergeon_ui::button("log-close")
                        .outline()
                        .sm()
                        .label("Close")
                        .on_click(|_, window, cx| {
                            dialog::close(window, cx);
                        })
                })
                .open(window, cx);
        }
        "stacked" => {
            fetch_dialog().open(window, cx);
            open_overlay("confirm-destructive", window, cx);
        }
        "confirm" => {
            dialog::confirm(
                window,
                cx,
                ConfirmOptions {
                    title: "Confirm merge".into(),
                    description: "Merge feature/login into main?".into(),
                    confirm_text: "Merge".into(),
                    ..Default::default()
                },
                |ok, window, cx| {
                    toast::info(window, cx, format!("Confirm answered: {ok}"));
                },
            );
        }
        "confirm-destructive" => {
            dialog::confirm(
                window,
                cx,
                ConfirmOptions {
                    title: "Delete tag".into(),
                    description:
                        "v0.9.0 will be removed from this repository. This cannot be undone.".into(),
                    confirm_text: "Delete".into(),
                    variant: ConfirmVariant::Destructive,
                    ..Default::default()
                },
                |ok, window, cx| {
                    toast::info(window, cx, format!("Confirm answered: {ok}"));
                },
            );
        }
        "confirm-option" => {
            dialog::confirm_with_option(
                window,
                cx,
                ConfirmOptions {
                    title: "Delete branch".into(),
                    description: "feature/login will be deleted from this repository.".into(),
                    confirm_text: "Delete".into(),
                    variant: ConfirmVariant::Destructive,
                    ..Default::default()
                },
                ConfirmOption {
                    label: "Also delete on origin".into(),
                    checked: false,
                },
                |outcome, window, cx| {
                    toast::info(
                        window,
                        cx,
                        format!(
                            "Confirmed: {}, option: {}",
                            outcome.confirmed, outcome.checked
                        ),
                    );
                },
            );
        }
        "prompt" | "prompt-error" => {
            let with_error = what == "prompt-error";
            dialog::prompt(
                window,
                cx,
                PromptOptions {
                    title: "Rename branch".into(),
                    description: "Enter the new name for feature/login".into(),
                    placeholder: "Branch name".into(),
                    default_value: if with_error {
                        "has spaces".into()
                    } else {
                        "feature/login".into()
                    },
                    confirm_text: "Rename".into(),
                    validate: Some(std::rc::Rc::new(|value: &str| {
                        !value.is_empty() && !value.contains(' ')
                    })),
                    input_error_message: "Branch names cannot contain spaces".into(),
                    ..Default::default()
                },
                |value, window, cx| {
                    toast::info(window, cx, format!("Prompt answered: {value:?}"));
                },
            );
        }
        "prompt-textarea" => {
            dialog::prompt(
                window,
                cx,
                PromptOptions {
                    title: "Stash message".into(),
                    placeholder: "What are you stashing?".into(),
                    textarea: true,
                    rows: 4,
                    ..Default::default()
                },
                |value, window, cx| {
                    toast::info(window, cx, format!("Prompt answered: {value:?}"));
                },
            );
        }
        "toast-success" => {
            toast::success(window, cx, "Pushed 3 commits to origin/main.");
        }
        "toast-info" => {
            toast::show(
                window,
                cx,
                ToastKind::Info,
                "Checking for updates…",
                ToastOptions::new().description("Mergeon 0.9.0 is the latest version."),
            );
        }
        "toast-warning" => {
            toast::warning(
                window,
                cx,
                "Could not save the sidebar layout. The change may not persist after restarting.",
            );
        }
        "toast-error" => {
            toast::show(
                window,
                cx,
                ToastKind::Error,
                "Could not push to origin",
                ToastOptions::new().error(
                    &"remote: Permission to mergeon/demo.git denied.\nfatal: unable to access 'https://github.com/mergeon/demo.git/': The requested URL returned error: 403",
                ),
            );
        }
        "toast-action" => {
            toast::show(
                window,
                cx,
                ToastKind::Info,
                "Update downloaded",
                ToastOptions::new()
                    .description("Restart Mergeon to finish installing 0.9.1.")
                    .details("Release notes: bug fixes and a faster commit graph.")
                    .action("Restart", |window, cx| {
                        toast::success(window, cx, "Restarting…");
                    }),
            );
        }
        "toasts" => {
            for kind in ["toast-success", "toast-warning", "toast-error"] {
                open_overlay(kind, window, cx);
            }
        }
        "toast-path" => {
            // Una ruta larga sin espacios: se corta tras `/` o `-`, no a mitad
            // de palabra. Con cuatro avisos el más antiguo se cierra.
            toast::success(window, cx, "Pushed 3 commits to origin/main.");
            toast::show(
                window,
                cx,
                ToastKind::Warning,
                "Fetch failed for 'origin'. Could not resolve host: github.com",
                ToastOptions::new()
                    .details("Fetch failed for 'origin'.\nCould not resolve host: github.com"),
            );
            toast::success(window, cx, "Fetch completed successfully for 'origin'.");
            toast::info(
                window,
                cx,
                "Opened apps/api/src/common/interceptors/multer-error.interceptor.ts",
            );
        }
        "details" => {
            open_overlay("toast-error", window, cx);
            AppDialog::new()
                .title("Notification details")
                .width(DialogWidth::Xl2)
                .body(|_, cx| {
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(rems(0.8))
                        .child("remote: Permission to mergeon/demo.git denied.\nfatal: unable to access 'https://github.com/mergeon/demo.git/': The requested URL returned error: 403")
                })
                .open(window, cx);
        }
        other => eprintln!("MERGEON_GALLERY_OPEN: no sé abrir `{other}`"),
    }
}

/// Ejecuta un paso de `MERGEON_GALLERY_INPUT` como si viniera del usuario.
fn run_step(step: &str, window: &mut Window, cx: &mut App) {
    let Some((kind, value)) = step.split_once(':') else {
        eprintln!("MERGEON_GALLERY_INPUT: paso inválido `{step}`");
        return;
    };
    if kind == "focus" {
        // Sin nada enfocado todavía, Tab no tiene desde dónde avanzar.
        for _ in 0..value.parse::<usize>().unwrap_or(1) {
            window.focus_next(cx);
        }
        return;
    }
    if kind == "key" {
        match Keystroke::parse(value) {
            Ok(keystroke) => {
                window.dispatch_keystroke(keystroke.clone(), cx);
                window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
            }
            Err(error) => eprintln!("MERGEON_GALLERY_INPUT: tecla inválida `{value}`: {error}"),
        }
        return;
    }
    let mut numbers = value
        .split(',')
        .filter_map(|n| n.trim().parse::<f32>().ok());
    let (Some(x), Some(y)) = (numbers.next(), numbers.next()) else {
        eprintln!("MERGEON_GALLERY_INPUT: coordenadas inválidas `{value}`");
        return;
    };
    let position = point(px(x), px(y));
    window.dispatch_event(
        MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers: Modifiers::default(),
        }
        .to_platform_input(),
        cx,
    );
    let button = match kind {
        "click" => MouseButton::Left,
        "rclick" => MouseButton::Right,
        _ => return,
    };
    window.dispatch_event(
        MouseDownEvent {
            button,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }
        .to_platform_input(),
        cx,
    );
    window.dispatch_event(
        MouseUpEvent {
            button,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
        }
        .to_platform_input(),
        cx,
    );
}

fn main() {
    if std::env::var("MERGEON_LOG").is_ok() {
        env_logger::Builder::new().parse_filters("warn").init();
    }
    let preference = ThemePreference::parse(&std::env::var("MERGEON_THEME").unwrap_or_default());
    let page = Page::parse(&std::env::var("MERGEON_GALLERY_PAGE").unwrap_or_default());
    let open: Vec<String> = std::env::var("MERGEON_GALLERY_OPEN")
        .unwrap_or_default()
        .split(',')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect();
    let steps: Vec<String> = std::env::var("MERGEON_GALLERY_INPUT")
        .unwrap_or_default()
        .split(';')
        .map(|step| step.trim().to_string())
        .filter(|step| !step.is_empty())
        .collect();

    gpui_kit::application()
        .with_assets(mergeon_ui::Assets)
        .run(move |cx| {
            mergeon_ui::init(cx);
            mergeon_ui::theme::apply(preference, cx);
            let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
            let (window, gallery) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| Gallery::new(page, window, cx)),
            )
            .expect("no se pudo abrir la ventana");

            // La raíz de la ventana (y con ella la capa de diálogos y toasts)
            // existe recién después de `open_window`.
            window
                .update(cx, |_, window, cx| {
                    for what in &open {
                        open_overlay(what, window, cx);
                    }
                })
                .expect("la ventana se cerró al abrirse");
            let _ = gallery;

            cx.spawn(async move |cx| {
                for step in steps {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(300))
                        .await;
                    let _ = window.update(cx, |_, window, cx| run_step(&step, window, cx));
                }
            })
            .detach();
        });
}
