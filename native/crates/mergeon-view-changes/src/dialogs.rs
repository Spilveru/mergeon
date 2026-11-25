//! Los dos diálogos de solo lectura de la vista de cambios: el blame de un
//! archivo (`BlameViewer.vue` dentro de un `AppDialog`, como lo abre
//! `ChangesView.vue`) y el contenido de un stash (`StashDiffDialog.vue`).

use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ListSizingBehavior, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, TestSupportExt as _, UniformListScrollHandle, Window, div, px,
    rems, uniform_list,
};
use mergeon_app::AppModel;
use mergeon_backend::{DiffOptions, FileChange};
use mergeon_model::diff_preferences::DiffViewMode;
use mergeon_model::format::format_size;
use mergeon_ui::dialog::{AppDialog, DialogId, DialogWidth};
use mergeon_ui::{
    ActivePaletteExt as _, Lucide, MergeonStyled as _, Radius, SpinnerColor, SpinnerSize,
};
use mergeon_view_diff::{BlameLine, BlameState, BlameViewer, BlameViewerEvent, DiffViewer};

use crate::logic::file_name;

// ---------------------------------------------------------------------------
// Blame
// ---------------------------------------------------------------------------

/// Contenido del diálogo de blame: pide el blame del archivo y se lo da al
/// visor. Es navegable: el clic en un bloque cierra el diálogo y lleva al
/// commit en el historial; "Blame previous revision" pasa al blame del
/// archivo en el padre de ese commit, con "Back" para volver.
pub struct BlameDialog {
    viewer: Entity<BlameViewer>,
    repo_path: String,
    path: SharedString,
    /// Revisiones visitadas con "Blame previous revision" (`[hash, ruta]`),
    /// de la primera a la que se ve. Vacía: el working tree.
    revisions: Vec<(SharedString, SharedString)>,
    _load: Task<()>,
    _subscription: Subscription,
}

impl BlameDialog {
    pub fn new(
        repo_path: impl Into<String>,
        file_path: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let repo_path = repo_path.into();
        let path: SharedString = file_path.into();
        let viewer = cx.new(|cx| BlameViewer::new(window, cx));
        let subscription = cx.subscribe_in(
            &viewer,
            window,
            |this, _, event: &BlameViewerEvent, window, cx| match event {
                BlameViewerEvent::OpenCommit { hash } => this.show_commit(hash, window, cx),
                BlameViewerEvent::BlamePrevious { hash, path } => {
                    this.revisions.push((hash.clone(), path.clone()));
                    this.load(cx);
                }
            },
        );
        let mut this = Self {
            viewer,
            repo_path,
            path,
            revisions: Vec::new(),
            _load: Task::ready(()),
            _subscription: subscription,
        };
        this.load(cx);
        this
    }

    /// El archivo del que se muestra el blame.
    pub fn path(&self) -> &SharedString {
        &self.path
    }

    /// El visor, para consultar su estado.
    pub fn viewer(&self) -> &Entity<BlameViewer> {
        &self.viewer
    }

    /// La revisión que se ve (`[hash, ruta]`); `None` es el working tree.
    pub fn revision(&self) -> Option<&(SharedString, SharedString)> {
        self.revisions.last()
    }

    /// "Back": la revisión anterior (o el working tree).
    pub fn back(&mut self, cx: &mut Context<Self>) {
        if self.revisions.pop().is_some() {
            self.load(cx);
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let model = AppModel::global(cx);
        let read = model.update(cx, |model, cx| match self.revisions.last() {
            Some((hash, path)) => model.blame_at_revision(&self.repo_path, hash, path, cx),
            None => model.blame(&self.repo_path, &self.path, cx),
        });
        self.viewer
            .update(cx, |viewer, cx| viewer.set_state(BlameState::Loading, cx));
        cx.notify();
        self._load = cx.spawn({
            let viewer = self.viewer.clone();
            async move |_, cx| {
                let state = match read.await {
                    Ok(result) if result.too_large => BlameState::TooLarge(result.size),
                    Ok(result) => BlameState::Lines(
                        result
                            .lines
                            .into_iter()
                            .map(|line| {
                                let previous = line.previous_hash.zip(line.previous_path);
                                let blame_line = BlameLine::new(
                                    line.short_hash,
                                    line.author,
                                    line.date,
                                    line.summary,
                                    line.content,
                                )
                                .commit(line.hash);
                                match previous {
                                    Some((hash, path)) => blame_line.previous(hash, path),
                                    None => blame_line,
                                }
                            })
                            .collect(),
                    ),
                    Err(error) => BlameState::Error(error.message().to_string().into()),
                };
                viewer.update(cx, |viewer, cx| viewer.set_state(state, cx));
            }
        });
    }

    /// Clic en un bloque: el commit en el historial (se cierra el diálogo).
    fn show_commit(&mut self, hash: &str, window: &mut Window, cx: &mut Context<Self>) {
        let model = AppModel::global(cx);
        let repo_id = model
            .read(cx)
            .open_repositories()
            .iter()
            .find_map(|repo| (repo.path == self.repo_path).then(|| repo.id.clone()));
        let Some(repo_id) = repo_id else {
            return;
        };
        mergeon_ui::dialog::close(window, cx);
        model.update(cx, |model, cx| {
            model.show_commit_in_history(&repo_id, hash, cx);
        });
    }
}

impl Render for BlameDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let line_count = self.viewer.read(cx).line_count();
        let rem = window.rem_size();
        // Una fila del visor: 0.75rem de texto con el renglón del diálogo
        // (20/14), 2px de relleno arriba y abajo y su borde.
        let row_height = rem * 0.75 * (20. / 14.) + px(5.);
        // Lo que deja el diálogo en una ventana baja: el visor hace su propio
        // scroll en lugar de alargar el cuerpo.
        let chrome = if self.revisions.is_empty() {
            140.
        } else {
            176.
        };
        let available = (window.viewport_size().height - px(chrome)).max(px(120.));
        let palette = *cx.palette();
        // En una revisión anterior: cuál es y cómo volver.
        let revision_bar = self.revision().map(|(hash, path)| {
            let short: String = hash.chars().take(7).collect();
            div()
                .flex()
                .items_center()
                .gap_2()
                .mb_2()
                .text_size(rems(0.75))
                .text_color(palette.muted_foreground)
                .child(
                    mergeon_ui::button("blame-back")
                        .outline()
                        .xs()
                        .icon(Lucide::ArrowLeft)
                        .label("Back")
                        .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                )
                .child(
                    div()
                        .id("blame-revision")
                        .test_support()
                        .aria_label(SharedString::from(format!("{short} {path}")))
                        .min_w_0()
                        .truncate()
                        .child(format!(
                            "Blame at {short} (parent of the selected commit) · {path}"
                        )),
                )
        });

        div()
            .id("blame-dialog-content")
            .test_support()
            .aria_label(self.path.clone())
            .w_full()
            .children(revision_bar)
            .child(
                div()
                    .w_full()
                    .when(line_count > 0, |this| {
                        // Los 2px de más: el visor redondea su alto de fila
                        // hacia arriba y, si no, enseña una barra de
                        // desplazamiento de nada.
                        this.h((row_height * line_count as f32 + px(2.)).min(available))
                    })
                    .child(self.viewer.clone()),
            )
    }
}

/// Abre el blame de `file_path` (relativa al repositorio) en un diálogo
/// (`handleBlame`). Devuelve `None` si el repositorio no está abierto.
pub fn open_blame_dialog(
    repo_id: &str,
    file_path: &str,
    window: &mut Window,
    cx: &mut App,
) -> Option<DialogId> {
    let repo_path = AppModel::global(cx)
        .read(cx)
        .repository(repo_id)
        .map(|repository| repository.path.clone())?;
    let file_path = file_path.to_string();
    let title = format!("Blame: {}", file_name(&file_path));
    let content = cx.new(|cx| BlameDialog::new(repo_path, file_path, window, cx));

    Some(
        AppDialog::new()
            .title(title)
            .width(DialogWidth::Xl5)
            .body(move |_, _| content.clone())
            .open(window, cx),
    )
}

// ---------------------------------------------------------------------------
// Stash
// ---------------------------------------------------------------------------

/// Alto máximo de la lista de archivos y del visor (`max-height: 420px`).
const MAX_PANE_HEIGHT: f32 = 420.;

/// Contenido del diálogo de un stash: vista de solo lectura de lo que guarda
/// antes de aplicarlo o borrarlo. El stash se identifica por su SHA, que a
/// diferencia de `stash@{n}` no se corre cuando aparecen stashes nuevos.
pub struct StashDiffDialog {
    model: Entity<AppModel>,
    repo_path: String,
    stash_ref: String,
    files: Vec<FileChange>,
    selected_file: Option<FileChange>,
    file_diff: SharedString,
    diff_too_large: bool,
    diff_size: u64,
    loading_files: bool,
    loading_diff: bool,
    /// Carrera entre peticiones de diff: solo vale la última.
    diff_request: u64,
    viewer: Entity<DiffViewer>,
    files_scroll: UniformListScrollHandle,
    /// El modo del visor según el modelo, leído al avisar éste (pintar no
    /// lee el modelo: cada aviso suyo repintaría la ventana).
    view_mode: DiffViewMode,
    _load: Task<()>,
}

impl StashDiffDialog {
    pub fn new(
        repo_path: impl Into<String>,
        stash_ref: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let model = AppModel::global(cx);
        // Del modelo solo pinta el modo del visor (unificado / lado a lado):
        // el resto de sus avisos no cambian nada aquí.
        let view_mode = model.read(cx).diff_preferences().diff_view_mode;
        cx.observe(&model, |this: &mut Self, model, cx| {
            let view_mode = model.read(cx).diff_preferences().diff_view_mode;
            if view_mode != this.view_mode {
                this.view_mode = view_mode;
                cx.notify();
            }
        })
        .detach();
        let mut this = Self {
            view_mode,
            model,
            repo_path: repo_path.into(),
            stash_ref: stash_ref.into(),
            files: Vec::new(),
            selected_file: None,
            file_diff: SharedString::default(),
            diff_too_large: false,
            diff_size: 0,
            loading_files: false,
            loading_diff: false,
            diff_request: 0,
            viewer: cx.new(|cx| DiffViewer::new(window, cx)),
            files_scroll: UniformListScrollHandle::new(),
            _load: Task::ready(()),
        };
        this._load = this.load_stash_files(cx);
        this
    }

    pub fn files(&self) -> &[FileChange] {
        &self.files
    }

    pub fn selected_file(&self) -> Option<&FileChange> {
        self.selected_file.as_ref()
    }

    pub fn file_diff(&self) -> &SharedString {
        &self.file_diff
    }

    pub fn is_loading(&self) -> bool {
        self.loading_files || self.loading_diff
    }

    /// `loadStashFiles`: los archivos del stash y, de paso, el diff del primero.
    fn load_stash_files(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if self.repo_path.is_empty() || self.stash_ref.is_empty() {
            return Task::ready(());
        }

        self.loading_files = true;
        self.files.clear();
        self.selected_file = None;
        self.file_diff = SharedString::default();
        cx.notify();

        let read = self.model.update(cx, |model, cx| {
            model.get_stash_files(&self.repo_path, &self.stash_ref, cx)
        });
        cx.spawn(async move |this, cx| {
            let files = read.await.unwrap_or_default();
            let first_diff = this
                .update(cx, |this, cx| {
                    this.files = files;
                    let first = this.files.first().cloned();
                    cx.notify();
                    first.map(|file| this.select_file(file, cx))
                })
                .ok()
                .flatten();
            if let Some(first_diff) = first_diff {
                first_diff.await;
            }
            this.update(cx, |this, cx| {
                this.loading_files = false;
                cx.notify();
            })
            .ok();
        })
    }

    /// `selectFile`: muestra el diff de un archivo del stash.
    pub fn select_file(&mut self, file: FileChange, cx: &mut Context<Self>) -> Task<()> {
        if self.repo_path.is_empty() || self.stash_ref.is_empty() || file.path.is_empty() {
            return Task::ready(());
        }

        self.diff_request += 1;
        let request = self.diff_request;
        let path = file.path.clone();
        self.selected_file = Some(file);
        self.loading_diff = true;
        cx.notify();

        let options = DiffOptions {
            ignore_whitespace: self.model.read(cx).diff_preferences().ignore_whitespace,
            ..DiffOptions::default()
        };
        let read = self.model.update(cx, |model, cx| {
            model.get_stash_diff(&self.repo_path, &self.stash_ref, &path, options, cx)
        });
        cx.spawn(async move |this, cx| {
            let payload = read.await.unwrap_or_default();
            this.update(cx, |this, cx| {
                if this.diff_request != request {
                    return;
                }
                this.file_diff = payload.diff.into();
                this.diff_too_large = payload.too_large;
                this.diff_size = payload.size;
                this.loading_diff = false;
                cx.notify();
            })
            .ok();
        })
    }

    /// `.stash-diff__state`
    fn state(muted: bool, cx: &App) -> gpui_kit::Div {
        let palette = cx.palette();
        div()
            .flex()
            .items_center()
            .gap(rems(0.45))
            .py(rems(1.))
            .px(rems(0.25))
            .text_size(rems(0.8125))
            .text_color(if muted {
                palette.muted_foreground
            } else {
                palette.foreground
            })
    }

    fn render_file(
        file: &FileChange,
        active: bool,
        this: &Entity<Self>,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let palette = *cx.palette();
        let target = file.clone();
        let this = this.downgrade();
        div()
            // El hueco entre filas (`gap: 0.2rem`) va dentro de cada una: la
            // lista las mide todas iguales.
            .pb(rems(0.2))
            .child(
                div()
                    .id(SharedString::from(format!("stash-file:{}", file.path)))
                    .test_support()
                    .aria_label(SharedString::from(file.path.clone()))
                    .flex()
                    .items_center()
                    .gap(rems(0.45))
                    .w_full()
                    .py(rems(0.35))
                    .px(rems(0.45))
                    .radius(Radius::Md, cx)
                    .text_size(rems(0.8125))
                    .text_color(palette.foreground)
                    .cursor_pointer()
                    .map(|row| {
                        if active {
                            row.bg(palette.sidebar_row_selected)
                        } else {
                            row.hover(|style| style.bg(palette.sidebar_row_hover))
                        }
                    })
                    .on_click(move |_, _, cx| {
                        this.update(cx, |this, cx| {
                            this.select_file(target.clone(), cx).detach();
                        })
                        .ok();
                    })
                    .child(mergeon_ui::status_badge(&file.status))
                    .child(
                        // `direction: rtl`: si no cabe, se recorta por el principio.
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis_start()
                            .child(file.path.clone()),
                    )
                    .when(file.additions > 0 || file.deletions > 0, |row| {
                        row.child(
                            div()
                                .flex()
                                .flex_shrink_0()
                                .gap(rems(0.3))
                                .text_size(rems(0.75))
                                .when(file.additions > 0, |stats| {
                                    stats.child(
                                        div()
                                            .text_color(palette.success)
                                            .child(format!("+{}", file.additions)),
                                    )
                                })
                                .when(file.deletions > 0, |stats| {
                                    stats.child(
                                        div()
                                            .text_color(palette.destructive)
                                            .child(format!("\u{2212}{}", file.deletions)),
                                    )
                                }),
                        )
                    }),
            )
    }
}

impl Render for StashDiffDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        // `.stash-diff`
        let root = div().id("stash-diff").flex().flex_col().min_h(px(260.));

        if self.loading_files {
            return root.child(
                Self::state(false, cx)
                    .id("stash-loading")
                    .test_support()
                    .child(
                        mergeon_ui::spinner()
                            .color(SpinnerColor::Primary)
                            .size(SpinnerSize::Sm),
                    )
                    .child("Loading stash…"),
            );
        }

        if self.files.is_empty() {
            return root.child(
                Self::state(true, cx)
                    .id("stash-empty")
                    .test_support()
                    .child("This stash has no file changes."),
            );
        }

        let view_mode = self.view_mode;
        let path = self
            .selected_file
            .as_ref()
            .map(|file| file.path.clone())
            .unwrap_or_default();
        self.viewer.update(cx, |viewer, cx| {
            viewer.set_diff(self.file_diff.clone(), path, cx);
            viewer.set_view_mode(view_mode, cx);
        });
        let max_height = px(MAX_PANE_HEIGHT);
        let viewer_height = self
            .viewer
            .read(cx)
            .content_height()
            .unwrap_or(max_height)
            .min(max_height);

        let files = self.files.clone();
        let selected = self.selected_file.as_ref().map(|file| file.path.clone());
        let this = cx.entity();

        root.child(
            // `.stash-diff__layout`
            div()
                .flex()
                .items_start()
                .gap(rems(0.75))
                .min_h_0()
                .child(
                    // `.stash-diff__files`
                    div()
                        .relative()
                        .w(px(240.))
                        .flex_shrink_0()
                        .pr(rems(0.25))
                        .border_r_1()
                        .border_color(palette.border)
                        .child(
                            uniform_list("stash-files", files.len(), move |range, _, cx| {
                                range
                                    .filter_map(|ix| files.get(ix))
                                    .map(|file| {
                                        let active = selected.as_deref() == Some(&file.path);
                                        Self::render_file(file, active, &this, cx)
                                    })
                                    .collect()
                            })
                            .track_scroll(&self.files_scroll)
                            .with_sizing_behavior(ListSizingBehavior::Infer)
                            .max_h(max_height),
                        )
                        .vertical_scrollbar(&self.files_scroll),
                )
                .child(
                    // `.stash-diff__viewer`
                    div().flex_1().min_w_0().map(|pane| {
                        if self.loading_diff {
                            pane.child(
                                Self::state(false, cx)
                                    .id("stash-diff-loading")
                                    .child(
                                        mergeon_ui::spinner()
                                            .color(SpinnerColor::Primary)
                                            .size(SpinnerSize::Sm),
                                    )
                                    .child("Loading diff…"),
                            )
                        } else if self.diff_too_large {
                            pane.child(Self::state(true, cx).id("stash-diff-too-large").child(
                                format!(
                                    "Diff too large to display here (over {})",
                                    format_size(self.diff_size as f64)
                                ),
                            ))
                        } else {
                            pane.child(div().h(viewer_height).child(self.viewer.clone()))
                        }
                    }),
                ),
        )
    }
}

/// Abre el contenido de un stash en un diálogo. `stash_ref` es el SHA del
/// stash (también vale `stash@{n}`); el título es su mensaje, si el
/// repositorio lo tiene cargado. Devuelve `None` si el repositorio no está
/// abierto o falta la referencia.
pub fn open_stash_diff_dialog(
    repo_id: &str,
    stash_ref: &str,
    window: &mut Window,
    cx: &mut App,
) -> Option<DialogId> {
    if stash_ref.is_empty() {
        return None;
    }
    let (repo_path, message) = {
        let model = AppModel::global(cx);
        let model = model.read(cx);
        let repository = model.repository(repo_id)?;
        let message = repository
            .stashes
            .iter()
            .find(|stash| stash.hash == stash_ref)
            .map(|stash| stash.message.clone())
            .filter(|message| !message.is_empty());
        (repository.path.clone(), message)
    };
    let stash_ref = stash_ref.to_string();
    let content = cx.new(|cx| StashDiffDialog::new(repo_path, stash_ref, window, cx));

    Some(
        AppDialog::new()
            .title(message.unwrap_or_else(|| "Stash".to_string()))
            .width(DialogWidth::Xl4)
            .body(move |_, _| content.clone())
            .open(window, cx),
    )
}
