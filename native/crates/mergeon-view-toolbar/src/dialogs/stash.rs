//! Diálogo Stash: mensaje y opciones de `git stash push` (lo que no se
//! sigue, conservar el índice, solo lo preparado), con un resumen de lo que
//! se va a guardar. Lo abre el botón Stash (todos los cambios) y "Stash
//! selected files…" de Local Changes (solo esas rutas).

use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, TestSupportExt as _, Window, div, px, rems,
};
use mergeon_app::{AppModel, StashRequest};
use mergeon_git_ops::{StashCapabilities, stash_capabilities};
use mergeon_gitcmd::{FileStatus, StructuredStatus};
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};
use mergeon_ui::{
    ActivePaletteExt as _, InputEvent, InputState, Lucide, MergeonStyled as _, Radius,
};

use super::{action_field, action_layout, is_open};

const TITLE: &str = "Stash changes";
const TITLE_SELECTED: &str = "Stash selected files";

/// Archivos que el resumen nombra uno a uno; del resto solo se dice cuántos.
const PREVIEW_FILES: usize = 6;

/// Lo que se elige en el diálogo.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StashChoice {
    pub include_untracked: bool,
    pub keep_index: bool,
    pub staged_only: bool,
}

/// Un archivo que se va a guardar y qué parte de él.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashEntryPreview {
    pub path: SharedString,
    /// La letra de estado (`M`, `A`, `D`, `R`, `?`…) para `status_badge`.
    pub status: SharedString,
    /// Qué se guarda de él: "staged + unstaged", "staged part only"…
    pub part: &'static str,
}

/// El resumen de lo que hará el stash con estas opciones.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StashSummary {
    /// Cuántos archivos se guardan.
    pub count: usize,
    /// Los primeros [`PREVIEW_FILES`].
    pub entries: Vec<StashEntryPreview>,
    /// Lo que se queda fuera o se conserva, en frases cortas.
    pub notes: Vec<String>,
    /// Alguna ruta elegida no se sigue (obliga a "Include untracked files").
    pub selected_untracked: bool,
    /// Por qué git no puede guardar estas rutas (un renombre o un borrado
    /// preparados no se pueden guardar por ruta).
    pub blocker: Option<String>,
}

impl StashSummary {
    pub fn headline(&self) -> String {
        match self.count {
            0 => "Nothing to stash with these options.".to_string(),
            1 => "1 file will be stashed:".to_string(),
            count => format!("{count} files will be stashed:"),
        }
    }
}

fn is_untracked(file: &FileStatus) -> bool {
    file.index == "?" || file.working_dir == "?"
}

fn has_side(column: &str) -> bool {
    !matches!(column, "" | " " | "?" | "!")
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// Qué guardará `git stash push` de `files` (el estado del repositorio) con
/// `paths` (vacío: todo) y `choice`. Recorre los archivos una vez: se calcula
/// al abrir el diálogo, al cambiar una opción o el estado, nunca al pintar.
pub fn stash_summary(files: &[FileStatus], paths: &[String], choice: StashChoice) -> StashSummary {
    let wanted: HashSet<&str> = paths.iter().map(String::as_str).collect();
    let in_scope = |file: &&FileStatus| wanted.is_empty() || wanted.contains(file.path.as_str());

    let mut summary = StashSummary::default();
    let mut untracked_left = 0;
    let mut unstaged_left = 0;
    for file in files.iter().filter(in_scope) {
        let untracked = is_untracked(file);
        if untracked && !wanted.is_empty() {
            summary.selected_untracked = true;
        }
        if !wanted.is_empty()
            && summary.blocker.is_none()
            && matches!(file.index.as_str(), "R" | "D")
        {
            let what = if file.index == "R" {
                "rename"
            } else {
                "deletion"
            };
            summary.blocker = Some(format!(
                "Git can't stash the staged {what} of {} by path. Unstage it first or stash all changes.",
                file.path
            ));
        }
        let staged = !untracked && has_side(&file.index);
        let unstaged = !untracked && has_side(&file.working_dir);
        let part = if untracked {
            if !choice.include_untracked || choice.staged_only {
                untracked_left += 1;
                continue;
            }
            "untracked"
        } else if choice.staged_only {
            if !staged {
                unstaged_left += 1;
                continue;
            }
            if unstaged {
                unstaged_left += 1;
                "staged part only"
            } else {
                "staged"
            }
        } else if staged && unstaged {
            "staged + unstaged"
        } else if staged {
            "staged"
        } else {
            "unstaged"
        };

        summary.count += 1;
        if summary.entries.len() < PREVIEW_FILES {
            let status = if untracked {
                "?"
            } else if staged {
                file.index.as_str()
            } else {
                file.working_dir.as_str()
            };
            summary.entries.push(StashEntryPreview {
                path: file.path.clone().into(),
                status: status.to_string().into(),
                part,
            });
        }
    }

    if untracked_left > 0 {
        summary.notes.push(format!(
            "{untracked_left} untracked file{} stay{} in the working tree.",
            plural(untracked_left),
            if untracked_left == 1 { "s" } else { "" }
        ));
    }
    if choice.staged_only && unstaged_left > 0 {
        summary
            .notes
            .push("Unstaged changes stay in the working tree.".to_string());
    }
    if choice.keep_index && !choice.staged_only {
        summary
            .notes
            .push("Staged changes are stashed and also stay in the index.".to_string());
    }
    summary
}

/// Formulario del diálogo.
pub(crate) struct StashDialog {
    model: Entity<AppModel>,
    paths: Rc<Vec<String>>,
    message: Entity<InputState>,
    choice: StashChoice,
    /// Lo que admite el git instalado; `None` mientras se lee.
    capabilities: Option<StashCapabilities>,
    /// El estado con el que se calculó `summary`.
    seen_status: Option<StructuredStatus>,
    summary: StashSummary,
    dialog_id: Option<DialogId>,
    _subscriptions: Vec<Subscription>,
}

impl StashDialog {
    fn new(paths: Vec<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        let message =
            cx.new(|cx| InputState::new(window, cx).placeholder("WIP: describe these changes"));
        let subscriptions = vec![
            cx.observe(&model, |this: &mut Self, _, cx| {
                if this.sync_status(cx) {
                    cx.notify();
                }
            }),
            cx.subscribe_in(
                &message,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.submit(window, cx);
                    }
                },
            ),
        ];

        // `git --version`, una vez por proceso y fuera del hilo de la interfaz.
        let read = cx.background_spawn(async { stash_capabilities() });
        cx.spawn(async move |this, cx| {
            let capabilities = read.await;
            this.update(cx, |this, cx| {
                this.capabilities = Some(capabilities);
                this.normalize_choice();
                cx.notify();
            })
            .ok();
        })
        .detach();

        let mut this = Self {
            model,
            paths: Rc::new(paths),
            message,
            choice: StashChoice::default(),
            capabilities: None,
            seen_status: None,
            summary: StashSummary::default(),
            dialog_id: None,
            _subscriptions: subscriptions,
        };
        this.sync_status(cx);
        this.normalize_choice();
        this.recompute();
        this
    }

    /// Recoge el estado del repositorio si cambió. Devuelve si cambió.
    fn sync_status(&mut self, cx: &App) -> bool {
        let model = self.model.read(cx);
        let status = model
            .repositories()
            .active_repository()
            .and_then(|repository| repository.status.as_ref());
        if status == self.seen_status.as_ref() {
            return false;
        }
        self.seen_status = status.cloned();
        self.recompute();
        true
    }

    fn summarize(&self) -> StashSummary {
        let files = self
            .seen_status
            .as_ref()
            .map(|status| status.files.as_slice())
            .unwrap_or_default();
        stash_summary(files, &self.paths, self.choice)
    }

    fn recompute(&mut self) {
        self.summary = self.summarize();
        // Una ruta elegida que no se sigue solo se guarda con
        // `--include-untracked`.
        if self.summary.selected_untracked && !self.choice.include_untracked {
            self.normalize_choice();
            self.summary = self.summarize();
        }
    }

    fn staged_only_supported(&self) -> bool {
        self.capabilities
            .is_none_or(|capabilities| capabilities.staged_only)
    }

    /// Deja la elección en una combinación que git admite.
    fn normalize_choice(&mut self) {
        if !self.staged_only_supported() || self.summary.selected_untracked {
            self.choice.staged_only = false;
        }
        if self.summary.selected_untracked {
            self.choice.include_untracked = true;
        }
        if self.choice.staged_only {
            self.choice.include_untracked = false;
            self.choice.keep_index = false;
        }
    }

    fn set_choice(&mut self, choice: StashChoice, cx: &mut Context<Self>) {
        self.choice = choice;
        self.normalize_choice();
        self.recompute();
        cx.notify();
    }

    fn can_submit(&self) -> bool {
        self.summary.count > 0 && self.summary.blocker.is_none()
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.dialog_id.take() {
            dialog::close_id(window, cx, id);
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_submit() {
            return;
        }
        let message = self.message.read(cx).value().trim().to_string();
        let request = StashRequest {
            message: (!message.is_empty()).then_some(message),
            paths: self.paths.to_vec(),
            include_untracked: self.choice.include_untracked,
            keep_index: self.choice.keep_index,
            staged_only: self.choice.staged_only,
        };
        self.close(window, cx);
        self.model
            .update(cx, |model, cx| model.handle_stash_submit(request, cx))
            .detach();
    }

    fn footer(form: &Entity<Self>, cx: &App) -> impl IntoElement + use<> {
        let can_submit = form.read(cx).can_submit();
        let cancel = form.clone();
        let submit = form.clone();
        div()
            .flex()
            .justify_end()
            .gap_2()
            .child(
                mergeon_ui::button("stash-cancel")
                    .ghost()
                    .sm()
                    .label("Cancel")
                    .on_click(move |_, window, cx| {
                        cancel.update(cx, |form, cx| form.close(window, cx));
                    }),
            )
            .child(
                mergeon_ui::button("stash-submit")
                    .sm()
                    .icon(Lucide::Archive)
                    .label("Stash")
                    .disabled(!can_submit)
                    .on_click(move |_, window, cx| {
                        submit.update(cx, |form, cx| form.submit(window, cx));
                    }),
            )
    }

    fn render_summary(&self, cx: &App) -> impl IntoElement + use<> {
        let palette = *cx.palette();
        let summary = &self.summary;
        let more = summary.count.saturating_sub(summary.entries.len());
        let headline = summary.headline();
        div()
            .id("stash-summary")
            .test_support()
            .aria_label(SharedString::from(headline.clone()))
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(rems(0.625))
            .border_1()
            .border_color(palette.border_hair)
            .radius(Radius::Lg, cx)
            .bg(palette.bg_sunken)
            .text_size(rems(0.8125))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if summary.count == 0 {
                        palette.text_3
                    } else {
                        palette.foreground
                    })
                    .child(headline),
            )
            .children(summary.entries.iter().map(|entry| {
                div()
                    .id(SharedString::from(format!("stash-summary:{}", entry.path)))
                    .test_support()
                    .aria_label(SharedString::from(format!(
                        "{} ({})",
                        entry.path, entry.part
                    )))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_w_0()
                    .child(mergeon_ui::status_badge(&entry.status))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(palette.foreground)
                            .child(entry.path.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(rems(0.75))
                            .text_color(palette.text_3)
                            .child(entry.part),
                    )
            }))
            .when(more > 0, |this| {
                this.child(
                    div()
                        .text_size(rems(0.75))
                        .text_color(palette.text_3)
                        .child(format!("and {more} more file{}", plural(more))),
                )
            })
            .children(summary.notes.iter().map(|note| {
                div()
                    .text_size(rems(0.75))
                    .text_color(palette.text_3)
                    .child(note.clone())
            }))
            .when_some(summary.blocker.clone(), |this, blocker| {
                this.child(
                    div()
                        .id("stash-blocked")
                        .test_support()
                        .aria_label(SharedString::from(blocker.clone()))
                        .text_size(rems(0.75))
                        .text_color(palette.warning_foreground)
                        .child(blocker),
                )
            })
    }
}

impl Render for StashDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let choice = self.choice;
        let staged_supported = self.staged_only_supported();
        let forced_untracked = self.summary.selected_untracked;
        let hint = |text: &'static str| {
            div()
                .pl(px(24.))
                .text_size(rems(0.75))
                .text_color(palette.text_3)
                .child(text)
        };

        action_layout(
            Lucide::Archive,
            [
                action_field("Message:", mergeon_ui::input(&self.message), cx).into_any_element(),
                self.render_summary(cx).into_any_element(),
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap_2()
                    .child(
                        mergeon_ui::checkbox("stash-include-untracked")
                            .label("Include untracked files")
                            .checked(choice.include_untracked)
                            .disabled(choice.staged_only || forced_untracked)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                let choice = StashChoice {
                                    include_untracked: *checked,
                                    ..this.choice
                                };
                                this.set_choice(choice, cx);
                            })),
                    )
                    .when(forced_untracked, |this| {
                        this.child(hint("Some selected files are untracked."))
                    })
                    .child(
                        mergeon_ui::checkbox("stash-keep-index")
                            .label("Keep staged changes in the index")
                            .checked(choice.keep_index)
                            .disabled(choice.staged_only)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                let choice = StashChoice {
                                    keep_index: *checked,
                                    ..this.choice
                                };
                                this.set_choice(choice, cx);
                            })),
                    )
                    .child(
                        mergeon_ui::checkbox("stash-staged-only")
                            .label("Only staged changes")
                            .checked(choice.staged_only)
                            .disabled(!staged_supported || forced_untracked)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                let choice = StashChoice {
                                    staged_only: *checked,
                                    ..this.choice
                                };
                                this.set_choice(choice, cx);
                            })),
                    )
                    .when(!staged_supported, |this| {
                        this.child(hint("Needs Git 2.35 or newer."))
                    })
                    .into_any_element(),
            ],
            cx,
        )
    }
}

/// Abre el diálogo de stash del repositorio activo: `paths` vacío guarda
/// todo; si no, solo esas rutas. Al enviarlo llama a
/// `AppModel::handle_stash_submit`. Devuelve `None` si ya estaba abierto.
pub fn open_stash_dialog(
    paths: Vec<String>,
    window: &mut Window,
    cx: &mut App,
) -> Option<DialogId> {
    if is_open(TITLE, window, cx) || is_open(TITLE_SELECTED, window, cx) {
        return None;
    }
    let (title, description) = if paths.is_empty() {
        (
            TITLE,
            "Save your local changes in a new stash and clean them from the working tree",
        )
    } else {
        (
            TITLE_SELECTED,
            "Save the changes of the selected files in a new stash",
        )
    };

    let form = cx.new(|cx| StashDialog::new(paths, window, cx));
    let id = AppDialog::new()
        .title(title)
        .description(description)
        .width(DialogWidth::Lg)
        .body({
            let form = form.clone();
            move |_, _| form.clone()
        })
        .footer({
            let form = form.clone();
            move |_, cx| StashDialog::footer(&form, cx)
        })
        .open(window, cx);
    form.update(cx, |form, cx| {
        form.dialog_id = Some(id);
        form.message.update(cx, |state, cx| state.focus(window, cx));
    });
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, index: &str, working_dir: &str) -> FileStatus {
        FileStatus {
            path: path.to_string(),
            index: index.to_string(),
            working_dir: working_dir.to_string(),
            from: None,
        }
    }

    fn files() -> Vec<FileStatus> {
        vec![
            file("both.txt", "M", "M"),
            file("staged.txt", "A", " "),
            file("modified.txt", " ", "M"),
            file("new.txt", "?", "?"),
        ]
    }

    fn parts(summary: &StashSummary) -> Vec<(&str, &str)> {
        summary
            .entries
            .iter()
            .map(|entry| (entry.path.as_ref(), entry.part))
            .collect()
    }

    #[test]
    fn by_default_tracked_changes_go_and_untracked_files_stay() {
        let summary = stash_summary(&files(), &[], StashChoice::default());
        assert_eq!(summary.count, 3);
        assert_eq!(
            parts(&summary),
            [
                ("both.txt", "staged + unstaged"),
                ("staged.txt", "staged"),
                ("modified.txt", "unstaged"),
            ]
        );
        assert_eq!(
            summary.notes,
            ["1 untracked file stays in the working tree."]
        );
        assert_eq!(summary.headline(), "3 files will be stashed:");
    }

    #[test]
    fn options_change_what_is_stashed() {
        let with_untracked = stash_summary(
            &files(),
            &[],
            StashChoice {
                include_untracked: true,
                keep_index: true,
                ..Default::default()
            },
        );
        assert_eq!(with_untracked.count, 4);
        assert_eq!(with_untracked.entries[3].status.as_ref(), "?");
        assert_eq!(
            with_untracked.notes,
            ["Staged changes are stashed and also stay in the index."]
        );

        let staged = stash_summary(
            &files(),
            &[],
            StashChoice {
                staged_only: true,
                ..Default::default()
            },
        );
        assert_eq!(
            parts(&staged),
            [("both.txt", "staged part only"), ("staged.txt", "staged")]
        );
        assert_eq!(
            staged.notes,
            [
                "1 untracked file stays in the working tree.",
                "Unstaged changes stay in the working tree."
            ]
        );
    }

    #[test]
    fn selected_paths_limit_the_summary_and_flag_untracked_ones() {
        let paths = vec!["modified.txt".to_string(), "new.txt".to_string()];
        let summary = stash_summary(
            &files(),
            &paths,
            StashChoice {
                include_untracked: true,
                ..Default::default()
            },
        );
        assert!(summary.selected_untracked);
        assert_eq!(
            parts(&summary),
            [("modified.txt", "unstaged"), ("new.txt", "untracked")]
        );

        let renamed = vec![FileStatus {
            from: Some("old.txt".to_string()),
            ..file("new name.txt", "R", " ")
        }];
        let blocked = stash_summary(
            &renamed,
            &["new name.txt".to_string()],
            StashChoice::default(),
        );
        assert!(
            blocked
                .blocker
                .unwrap()
                .contains("staged rename of new name.txt")
        );
        assert!(
            stash_summary(&renamed, &[], StashChoice::default())
                .blocker
                .is_none()
        );

        let nothing = stash_summary(&files(), &["clean.txt".to_string()], StashChoice::default());
        assert_eq!(nothing.count, 0);
        assert_eq!(nothing.headline(), "Nothing to stash with these options.");
    }

    #[test]
    fn previews_only_the_first_files() {
        let many: Vec<FileStatus> = (0..40)
            .map(|index| file(&format!("file {index}.txt"), " ", "M"))
            .collect();
        let summary = stash_summary(&many, &[], StashChoice::default());
        assert_eq!(summary.count, 40);
        assert_eq!(summary.entries.len(), PREVIEW_FILES);
    }
}
