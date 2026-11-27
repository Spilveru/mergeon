//! Diálogo «New Worktree»: una carpeta nueva con una rama desprotegida, sea
//! una rama que ya existe o una nueva desde un commit o rama. La carpeta se
//! sugiere junto al repositorio (`../<repo>-<rama>`) mientras no se edite a
//! mano. La validación es pura (nombres y ramas ya cargadas): que la carpeta
//! no exista lo comprueba git al crear, fuera del hilo de la interfaz.

use std::collections::{HashMap, HashSet};

use gpui_kit::prelude::*;
use gpui_kit::{
    App, Context, Entity, EventEmitter, SharedString, Subscription, TestSupportExt as _, Window,
    div, px, rems,
};
use mergeon_git_ops::AddWorktreeOptions;
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};
use mergeon_ui::{
    ActivePaletteExt as _, InputEvent, InputState, Lucide, MergeonStyled as _, TabItem,
};

use super::parts::{caption, icon_badge};

/// Qué se desprotege en el worktree nuevo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NewWorktreeMode {
    /// Una rama nueva desde un commit o rama.
    #[default]
    NewBranch,
    /// Una rama local que ya existe (y no está en otro worktree).
    ExistingBranch,
}

impl NewWorktreeMode {
    fn key(self) -> &'static str {
        match self {
            Self::NewBranch => "new",
            Self::ExistingBranch => "existing",
        }
    }
}

/// Lo que el diálogo necesita saber del repositorio.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct NewWorktreeOptions {
    pub repo_path: String,
    /// Ramas locales.
    pub branches: HashSet<String>,
    /// Ramas ya desprotegidas en algún worktree (este incluido) → su ruta.
    pub busy_branches: HashMap<String, String>,
    pub mode: NewWorktreeMode,
    /// Rama existente elegida de entrada ("Check out in new worktree…").
    pub branch: Option<String>,
    /// Desde dónde sale una rama nueva (la rama actual, o `HEAD`).
    pub start_point: String,
}

impl NewWorktreeOptions {
    pub fn new(repo_path: impl Into<String>) -> Self {
        Self {
            repo_path: repo_path.into(),
            start_point: "HEAD".to_string(),
            ..Default::default()
        }
    }

    pub fn branches(mut self, branches: impl IntoIterator<Item = String>) -> Self {
        self.branches = branches.into_iter().collect();
        self
    }

    pub fn busy_branches(mut self, busy: HashMap<String, String>) -> Self {
        self.busy_branches = busy;
        self
    }

    /// Abre en "Existing branch" con esa rama.
    pub fn existing_branch(mut self, branch: impl Into<String>) -> Self {
        self.mode = NewWorktreeMode::ExistingBranch;
        self.branch = Some(branch.into());
        self
    }

    pub fn start_point(mut self, start_point: impl Into<String>) -> Self {
        self.start_point = start_point.into();
        self
    }
}

/// `Submit` con lo que hay que crear, y si abrirlo después en una pestaña.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewWorktreeDialogEvent {
    Submit {
        options: AddWorktreeOptions,
        open_after: bool,
    },
    Close,
}

/// Carpeta sugerida para el worktree de `branch`: junto al repositorio,
/// `<padre>/<repo>-<rama>` (las `/` de la rama pasan a `-`). Respeta el
/// separador de la ruta del repositorio.
pub fn suggest_worktree_path(repo_path: &str, branch: &str) -> String {
    let separator = if repo_path.contains('\\') && !repo_path.contains('/') {
        '\\'
    } else {
        '/'
    };
    let trimmed = repo_path.trim_end_matches(['/', '\\']);
    let (parent, name) = match trimmed.rfind(['/', '\\']) {
        Some(at) => (&trimmed[..at], &trimmed[at + 1..]),
        None => ("", trimmed),
    };
    let slug: String = branch
        .trim()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    let folder = if slug.is_empty() {
        format!("{name}-worktree")
    } else {
        format!("{name}-{slug}")
    };
    if parent.is_empty() && !trimmed.starts_with(['/', '\\']) {
        folder
    } else {
        format!("{parent}{separator}{folder}")
    }
}

/// Por qué git no aceptaría `name` como rama (lo esencial de
/// `git check-ref-format --branch`).
pub fn branch_name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("Enter a branch name.");
    }
    let bad_char = name.chars().any(|character| {
        character.is_whitespace()
            || character.is_control()
            || matches!(character, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
    });
    if bad_char
        || name.starts_with(['-', '/', '.'])
        || name.ends_with(['/', '.'])
        || name.ends_with(".lock")
        || name.contains("..")
        || name.contains("//")
        || name.contains("@{")
        || name.contains("/.")
        || name == "@"
    {
        return Some("That is not a valid branch name.");
    }
    None
}

/// Lo que se crearía con estos campos, o por qué no se puede.
pub fn validate_new_worktree(
    options: &NewWorktreeOptions,
    mode: NewWorktreeMode,
    branch: &str,
    start_point: &str,
    folder: &str,
) -> Result<AddWorktreeOptions, String> {
    let branch = branch.trim();
    let folder = folder.trim();
    match mode {
        NewWorktreeMode::NewBranch => {
            if let Some(problem) = branch_name_problem(branch) {
                return Err(problem.to_string());
            }
            if options.branches.contains(branch) {
                return Err(format!(
                    "A branch named '{branch}' already exists. Use \"Existing branch\" to check it out."
                ));
            }
        }
        NewWorktreeMode::ExistingBranch => {
            if branch.is_empty() {
                return Err("Enter the branch to check out.".to_string());
            }
            if !options.branches.contains(branch) {
                return Err(format!("There is no local branch named '{branch}'."));
            }
            if let Some(path) = options.busy_branches.get(branch) {
                return Err(format!(
                    "'{branch}' is already checked out in the worktree at {path}."
                ));
            }
        }
    }
    if folder.is_empty() {
        return Err("Choose a folder for the worktree.".to_string());
    }
    Ok(match mode {
        NewWorktreeMode::NewBranch => AddWorktreeOptions {
            path: folder.to_string(),
            new_branch: Some(branch.to_string()),
            start_point: Some(start_point.trim().to_string()).filter(|start| !start.is_empty()),
            ..Default::default()
        },
        NewWorktreeMode::ExistingBranch => AddWorktreeOptions {
            path: folder.to_string(),
            branch: Some(branch.to_string()),
            ..Default::default()
        },
    })
}

/// Estado del diálogo. Se abre con [`open_new_worktree_dialog`].
pub struct NewWorktreeDialog {
    options: NewWorktreeOptions,
    mode: NewWorktreeMode,
    branch: Entity<InputState>,
    start_point: Entity<InputState>,
    folder: Entity<InputState>,
    /// La última carpeta sugerida: si el campo la sigue mostrando, cambia
    /// con la rama; si el usuario la editó, se respeta.
    suggested_folder: String,
    folder_edited: bool,
    open_after: bool,
    /// El usuario ya tocó algo: desde entonces se dice qué falta.
    touched: bool,
    dialog_id: Option<DialogId>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<NewWorktreeDialogEvent> for NewWorktreeDialog {}

impl NewWorktreeDialog {
    fn new(options: NewWorktreeOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let initial_branch = options.branch.clone().unwrap_or_default();
        let branch = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("feature/my-change");
            if !initial_branch.is_empty() {
                state.set_value(initial_branch.clone(), window, cx);
            }
            state
        });
        let start_point = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("HEAD, a branch or a commit");
            state.set_value(options.start_point.clone(), window, cx);
            state
        });
        let suggested_folder = suggest_worktree_path(&options.repo_path, &initial_branch);
        let folder = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("Folder for the worktree");
            state.set_value(suggested_folder.clone(), window, cx);
            state
        });

        let subscriptions = vec![
            cx.subscribe_in(
                &branch,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    InputEvent::Change => {
                        this.touched = true;
                        this.branch_changed(window, cx);
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &start_point,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &folder,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    InputEvent::Change => {
                        let value = this.folder.read(cx).value().to_string();
                        if value != this.suggested_folder {
                            this.folder_edited = true;
                            this.touched = true;
                        }
                        cx.notify();
                    }
                    _ => {}
                },
            ),
        ];

        Self {
            mode: options.mode,
            options,
            branch,
            start_point,
            folder,
            suggested_folder,
            folder_edited: false,
            open_after: true,
            touched: false,
            dialog_id: None,
            _subscriptions: subscriptions,
        }
    }

    fn branch_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.folder_edited {
            let branch = self.branch.read(cx).value().to_string();
            self.suggested_folder = suggest_worktree_path(&self.options.repo_path, &branch);
            let suggested = self.suggested_folder.clone();
            self.folder
                .update(cx, |state, cx| state.set_value(suggested, window, cx));
        }
        cx.notify();
    }

    pub fn mode(&self) -> NewWorktreeMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: NewWorktreeMode, cx: &mut Context<Self>) {
        self.mode = mode;
        cx.notify();
    }

    pub fn branch_input(&self) -> &Entity<InputState> {
        &self.branch
    }

    pub fn start_point_input(&self) -> &Entity<InputState> {
        &self.start_point
    }

    pub fn folder_input(&self) -> &Entity<InputState> {
        &self.folder
    }

    pub fn open_after(&self) -> bool {
        self.open_after
    }

    pub fn set_open_after(&mut self, open: bool, cx: &mut Context<Self>) {
        self.open_after = open;
        cx.notify();
    }

    /// Lo que se crearía, o por qué todavía no.
    pub fn validation(&self, cx: &App) -> Result<AddWorktreeOptions, String> {
        validate_new_worktree(
            &self.options,
            self.mode,
            &self.branch.read(cx).value(),
            &self.start_point.read(cx).value(),
            &self.folder.read(cx).value(),
        )
    }

    pub fn is_submittable(&self, cx: &App) -> bool {
        self.validation(cx).is_ok()
    }

    fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.dialog_id.take() {
            dialog::close_id(window, cx, id);
        }
    }

    /// Cierra sin avisar a nadie (lo llama quien lo abrió: otro diálogo del
    /// mismo tipo o un cambio de repositorio).
    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.dialog_id.take() {
            dialog::close_id(window, cx, id);
        }
    }

    /// Crear. Con algo mal no hace nada (el aviso ya se ve en el diálogo).
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.touched = true;
        match self.validation(cx) {
            Ok(options) => {
                let Some(id) = self.dialog_id.take() else {
                    return;
                };
                dialog::close_id(window, cx, id);
                cx.emit(NewWorktreeDialogEvent::Submit {
                    options,
                    open_after: self.open_after,
                });
            }
            Err(_) => cx.notify(),
        }
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_dialog(window, cx);
        cx.emit(NewWorktreeDialogEvent::Close);
    }

    fn dismissed(&mut self, cx: &mut Context<Self>) {
        self.dialog_id = None;
        cx.emit(NewWorktreeDialogEvent::Close);
    }
}

fn field(label: &'static str, control: impl IntoElement, cx: &App) -> gpui_kit::Div {
    let palette = cx.palette();
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .w(px(112.))
                .flex_shrink_0()
                .tw_text_sm()
                .tw_font_medium()
                .text_color(palette.foreground)
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(control))
}

impl NewWorktreeDialog {
    /// Las ramas locales para "Existing branch", en orden; las que ya están
    /// en un worktree lo dicen.
    pub fn branch_choices(&self) -> Vec<(String, String)> {
        let mut branches: Vec<&String> = self.options.branches.iter().collect();
        branches.sort();
        branches
            .into_iter()
            .map(|branch| {
                let label = if self.options.busy_branches.contains_key(branch) {
                    format!("{branch} (in a worktree)")
                } else {
                    branch.clone()
                };
                (branch.clone(), label)
            })
            .collect()
    }

    /// Elige una rama existente (el selector).
    pub fn choose_branch(&mut self, branch: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.branch.update(cx, |state, cx| {
            state.set_value(branch.to_string(), window, cx)
        });
        self.touched = true;
        self.branch_changed(window, cx);
    }

    fn render_branch_picker(&self, cx: &mut Context<Self>) -> mergeon_ui::Select {
        let value = self.branch.read(cx).value().to_string();
        mergeon_ui::select("new-worktree-branch")
            .full_width(true)
            .placeholder("Choose a branch")
            .items(
                self.branch_choices()
                    .into_iter()
                    .map(|(branch, label)| mergeon_ui::SelectOption::new(branch, label)),
            )
            .when(!value.is_empty(), |select| select.value(value))
            .on_change(cx.listener(|this, value: &SharedString, window, cx| {
                this.choose_branch(value, window, cx);
            }))
    }
}

impl Render for NewWorktreeDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let new_branch = self.mode == NewWorktreeMode::NewBranch;
        let problem = self.validation(cx).err().filter(|_| self.touched);
        let picker = self.render_branch_picker(cx);

        div()
            .flex()
            .gap_3()
            .child(div().w(px(56.)).flex_shrink_0().child(icon_badge(
                Lucide::FolderGit2,
                2.85,
                30.,
                cx,
            )))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(rems(0.625))
                    .child(
                        div().flex().child(
                            mergeon_ui::tabs("new-worktree-mode")
                                .item(TabItem::new("new", "New branch"))
                                .item(TabItem::new("existing", "Existing branch"))
                                .selected(self.mode.key())
                                .on_change(cx.listener(|this, key: &SharedString, _, cx| {
                                    this.touched = false;
                                    this.set_mode(
                                        if key.as_ref() == "existing" {
                                            NewWorktreeMode::ExistingBranch
                                        } else {
                                            NewWorktreeMode::NewBranch
                                        },
                                        cx,
                                    );
                                })),
                        ),
                    )
                    .map(|this| {
                        if new_branch {
                            this.child(field("New branch:", mergeon_ui::input(&self.branch), cx))
                        } else {
                            // Una rama que ya existe se elige de la lista (ya
                            // cargada), no se teclea.
                            this.child(field("Branch:", picker, cx))
                        }
                    })
                    .when(new_branch, |this| {
                        this.child(field("From:", mergeon_ui::input(&self.start_point), cx))
                    })
                    .child(field("Folder:", mergeon_ui::input(&self.folder), cx))
                    .child(caption(
                        "The folder must not exist yet (or be empty). The branch stays when the \
                         worktree is removed.",
                        cx,
                    ))
                    .child(
                        div().flex().child(
                            mergeon_ui::checkbox("new-worktree-open")
                                .label("Open it in a new tab")
                                .checked(self.open_after)
                                .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                    this.set_open_after(*checked, cx);
                                })),
                        ),
                    )
                    .when_some(problem, |this, problem| {
                        this.child(
                            div()
                                .id("new-worktree-problem")
                                .test_support()
                                .aria_label(SharedString::from(problem.clone()))
                                .tw_text_sm()
                                .text_color(palette.destructive_foreground)
                                .child(problem),
                        )
                    }),
            )
    }
}

/// Abre «New Worktree» con el foco en la rama. Quien llama se suscribe al
/// [`NewWorktreeDialogEvent`] de la entidad devuelta y la conserva.
pub fn open_new_worktree_dialog(
    options: NewWorktreeOptions,
    window: &mut Window,
    cx: &mut App,
) -> Entity<NewWorktreeDialog> {
    let view = cx.new(|cx| NewWorktreeDialog::new(options, window, cx));

    let id = AppDialog::new()
        .title("New Worktree")
        .description("Check out a branch in its own folder, next to this one.")
        .width(DialogWidth::Lg)
        .body({
            let view = view.clone();
            move |_, _| view.clone()
        })
        .footer({
            let view = view.clone();
            move |_, cx| {
                let submittable = view.read(cx).is_submittable(cx);
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        mergeon_ui::button("new-worktree-cancel")
                            .ghost()
                            .sm()
                            .label("Cancel")
                            .on_click({
                                let view = view.clone();
                                move |_, window, cx| {
                                    view.update(cx, |view, cx| view.close(window, cx));
                                }
                            }),
                    )
                    .child(
                        mergeon_ui::button("new-worktree-submit")
                            .sm()
                            .disabled(!submittable)
                            .label("Create Worktree")
                            .on_click({
                                let view = view.clone();
                                move |_, window, cx| {
                                    view.update(cx, |view, cx| view.submit(window, cx));
                                }
                            }),
                    )
            }
        })
        .on_confirm({
            let view = view.clone();
            move |window, cx| {
                view.update(cx, |view, cx| view.submit(window, cx));
                false
            }
        })
        .on_close({
            let view = view.clone();
            move |_, cx| view.update(cx, |view, cx| view.dismissed(cx))
        })
        .open(window, cx);

    view.update(cx, |view, cx| {
        view.dialog_id = Some(id);
        match view.mode {
            NewWorktreeMode::NewBranch => {
                view.branch.update(cx, |state, cx| state.focus(window, cx));
            }
            NewWorktreeMode::ExistingBranch => {
                view.folder.update(cx, |state, cx| state.focus(window, cx));
            }
        }
    });
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> NewWorktreeOptions {
        NewWorktreeOptions::new("/home/me/code/shop")
            .branches(["main".to_string(), "topic".to_string(), "busy".to_string()])
            .busy_branches(HashMap::from([
                ("main".to_string(), "/home/me/code/shop".to_string()),
                ("busy".to_string(), "/home/me/code/shop-busy".to_string()),
            ]))
    }

    #[test]
    fn suggests_a_sibling_folder_named_after_the_branch() {
        assert_eq!(
            suggest_worktree_path("/home/me/code/shop", "feature/login"),
            "/home/me/code/shop-feature-login"
        );
        assert_eq!(
            suggest_worktree_path("/home/me/code/shop/", ""),
            "/home/me/code/shop-worktree"
        );
        assert_eq!(
            suggest_worktree_path(r"C:\work\shop", "fix"),
            r"C:\work\shop-fix"
        );
    }

    #[test]
    fn checks_branch_names_like_git() {
        assert_eq!(branch_name_problem("feature/login"), None);
        for bad in [
            "",
            "has space",
            "a..b",
            "-x",
            "x.lock",
            "x/",
            "a~b",
            "a:b",
            "a//b",
        ] {
            assert!(branch_name_problem(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn validates_new_and_existing_branches() {
        let options = options();
        let new = validate_new_worktree(
            &options,
            NewWorktreeMode::NewBranch,
            "feature",
            "main",
            "/tmp/wt",
        )
        .unwrap();
        assert_eq!(new.new_branch.as_deref(), Some("feature"));
        assert_eq!(new.start_point.as_deref(), Some("main"));

        assert!(
            validate_new_worktree(&options, NewWorktreeMode::NewBranch, "topic", "", "/tmp/wt")
                .unwrap_err()
                .contains("already exists")
        );
        let existing = validate_new_worktree(
            &options,
            NewWorktreeMode::ExistingBranch,
            "topic",
            "",
            "/tmp/wt",
        )
        .unwrap();
        assert_eq!(existing.branch.as_deref(), Some("topic"));
        assert!(
            validate_new_worktree(&options, NewWorktreeMode::ExistingBranch, "busy", "", "/x")
                .unwrap_err()
                .contains("already checked out in the worktree at /home/me/code/shop-busy")
        );
        assert!(
            validate_new_worktree(&options, NewWorktreeMode::ExistingBranch, "nope", "", "/x")
                .is_err()
        );
        assert_eq!(
            validate_new_worktree(&options, NewWorktreeMode::NewBranch, "ok", "", " ").unwrap_err(),
            "Choose a folder for the worktree."
        );
    }
}
