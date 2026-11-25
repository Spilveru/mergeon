//! Galería de los visores de `mergeon_view_diff`: una ventana con el visor que
//! se pida, sobre datos de ejemplo embebidos (los diffs del repo de
//! demostración de `native/tools/make-fixture.sh`).
//!
//! ```bash
//! cargo run -p mergeon-view-diff --example diff_gallery
//! MERGEON_THEME=light MERGEON_DIFF_CASE=staging cargo run -p mergeon-view-diff --example diff_gallery
//! ```
//!
//! Variables de entorno:
//!
//! - `MERGEON_THEME=dark|light|system` (por defecto `dark`).
//! - `MERGEON_DIFF_CASE=` qué se muestra:
//!   - `unified` (por defecto), `split`: diff de un commit, como la pestaña de
//!     archivo histórico;
//!   - `staging`, `staged`, `untracked`, `deleted`, `ignore-spaces`,
//!     `staging-split`, `long-lines`, `loading`, `too-large`, `empty`: el panel
//!     de cambios locales con staging por bloque;
//!   - `whitespace`, `whitespace-split`: diff que sobre todo reindenta;
//!   - `file`, `file-header`, `binary`, `file-too-large`, `file-empty`,
//!     `file-loading`, `file-error`: contenido de un archivo;
//!   - `blame`, `blame-loading`, `blame-error`, `blame-too-large`;
//!   - `tree`: filas de árbol de archivos;
//!   - `large`, `large-split`, `large-file`: 20.000 líneas.
//! - `MERGEON_DIFF_INPUT=`, pasos separados por `;` que se ejecutan después de
//!   arrancar (uno cada 300ms): `move:x,y`, `click:x,y`, `dclick:x,y`,
//!   `drag:x1,y1,x2,y2`, `scroll:x,y,dx,dy`, `key:<tecla>`. Coordenadas de la
//!   ventana.
//! - `MERGEON_DIFF_BENCH=1`: desplaza el visor durante 120 frames y escribe en
//!   stderr los milisegundos por frame.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mergeon_ui::{ActivePaletteExt as _, ThemePreference};
use mergeon_view_diff::{
    BlameLine, BlameState, BlameViewer, DiffPanel, DiffViewMode, DiffViewer, DiffViewerEvent,
    FileContent, FileContentViewer, FileContentViewerEvent, FileTreeRow, HunkAction, file_stats,
};

const LOGIN_DIFF: &str = r#"diff --git a/src/auth/login.js b/src/auth/login.js
index 430cbb7..cf60202 100644
--- a/src/auth/login.js
+++ b/src/auth/login.js
@@ -1,6 +1,6 @@
 export function login(user, password) {
   if (!user || !password) {
-    throw new Error("Missing credentials")
+    throw new Error("Missing user or password")
   }
   return { user, token: "demo" }
 }
"#;

const APP_DIFF: &str = r#"diff --git a/src/app.js b/src/app.js
index 0ae664c..ac2c05a 100644
--- a/src/app.js
+++ b/src/app.js
@@ -2,7 +2,8 @@ import { formatDate } from './lib/util.js'
 
 export function main() {
   const today = formatDate(new Date())
-  console.log(`Application started on ${today}`)
+  const mode = process.env.NODE_ENV || "development"
+  console.log(`Application started on ${today} (${mode})`)
   return 0
 }
 
"#;

const README_DIFF: &str = r#"diff --git a/README.md b/README.md
index af03efa..7bc5864 100644
--- a/README.md
+++ b/README.md
@@ -9,3 +9,5 @@ Run `npm install` and then `npm start`.
 ## License
 
 MIT
+
+See the guide in `docs/guide.md`.
"#;

const NOTES_DIFF: &str = r#"diff --git a/notes.txt b/notes.txt
new file mode 100644
index 0000000..e69de29
--- /dev/null
+++ b/notes.txt
@@ -0,0 +1,1 @@
+Remember to update the changelog.
"#;

const OLD_DIFF: &str = r#"diff --git a/docs/old.md b/docs/old.md
deleted file mode 100644
index 6503cbc..0000000
--- a/docs/old.md
+++ /dev/null
@@ -1,3 +0,0 @@
-# Old notes
-
-To be removed.
"#;

const WHITESPACE_DIFF: &str = r#"diff --git a/src/render.js b/src/render.js
index 3a1b2c4..9d8e7f6 100644
--- a/src/render.js
+++ b/src/render.js
@@ -1,9 +1,11 @@
 export function render(items) {
-  const rows = []
-  for (const item of items) {
-    rows.push(`<li>${item.name}</li>`)
-  }
-  return `<ul>${rows.join('')}</ul>`
+  if (items.length > 0) {
+    const rows = []
+    for (const item of items) {
+      rows.push(`<li class="item">${item.name}</li>`)
+    }
+    return `<ul>${rows.join('')}</ul>`
+  }
+  return ''
 }
\ No newline at end of file
"#;

const APP_JS: &str = r#"import { formatDate } from './lib/util.js'

export function main() {
  const today = formatDate(new Date())
  console.log(`Application started on ${today}`)
  return 0
}

main()
"#;

fn blame_lines() -> Vec<BlameLine> {
    // 2026-09-01T10:07:00Z y 2026-10-02T12:00:00Z.
    const INITIAL: i64 = 1_788_257_220_000;
    const TODAY: i64 = 1_790_942_400_000;
    let committed = |content: &str| {
        BlameLine::new(
            "e65ddb9",
            "Ana Torres",
            Some(INITIAL),
            "Initial commit",
            content,
        )
    };
    let pending = |content: &str| {
        BlameLine::new(
            "0000000",
            "Not Committed Yet",
            Some(TODAY),
            "Version of src/app.js from src/app.js",
            content,
        )
    };
    vec![
        committed("import { formatDate } from './lib/util.js'"),
        committed(""),
        committed("export function main() {"),
        committed("  const today = formatDate(new Date())"),
        pending("  const mode = process.env.NODE_ENV || \"development\""),
        pending("  console.log(`Application started on ${today} (${mode})`)"),
        committed("  return 0"),
        committed("}"),
        committed(""),
        committed("main()"),
    ]
}

/// Un diff de `lines` líneas repartidas en bloques de 200.
fn large_diff(lines: usize) -> String {
    let mut diff = String::from(
        "diff --git a/src/generated.js b/src/generated.js\nindex 1111111..2222222 100644\n--- a/src/generated.js\n+++ b/src/generated.js\n",
    );
    let mut line = 1;
    let mut written = 0;
    while written < lines {
        diff.push_str(&format!(
            "@@ -{line},200 +{line},200 @@ export function block{line}() {{\n"
        ));
        written += 1;
        for ix in 0..199 {
            if written >= lines {
                break;
            }
            let number = line + ix;
            match ix % 9 {
                3 => diff.push_str(&format!("-  const value{number} = compute({number}, \"before\")\n")),
                4 => diff.push_str(&format!("+  const value{number} = compute({number}, \"after\") // changed\n")),
                7 => diff.push_str(&format!("+  registry.set(`key-${{{number}}}`, value{number} ?? defaultValue({number}), {{ ttl: 60 * {number}, tags: [\"generated\", \"large\"] }})\n")),
                _ => diff.push_str(&format!("   if (value{number} > threshold) return finalize(value{number})\n")),
            }
            written += 1;
        }
        line += 200;
    }
    diff
}

/// Un diff con líneas que no caben en el visor: scroll horizontal.
fn long_lines_diff() -> String {
    let entries: Vec<String> = (0..40)
        .map(|ix| format!("option{ix}: \"value-{ix}\""))
        .collect();
    format!(
        "diff --git a/src/config.js b/src/config.js\nindex 1111111..2222222 100644\n--- a/src/config.js\n+++ b/src/config.js\n@@ -1,4 +1,4 @@ export const config = loadConfiguration(process.env.CONFIG_PATH ?? defaultConfigurationPath())\n export const defaults = {{\n-  legacy: {{ {} }},\n+  current: {{ {}, enabled: true }},\n   \tname: \"tabs\",\n }}\n",
        entries.join(", "),
        entries.join(", ")
    )
}

fn large_file(lines: usize) -> String {
    (0..lines)
        .map(|ix| format!("export const value{ix} = compute({ix}, \"generated\") // line {ix}\n"))
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shown {
    Viewer,
    Panel,
    File,
    Blame,
    Tree,
}

struct Gallery {
    case: String,
    shown: Shown,
    diff_viewer: Entity<DiffViewer>,
    file_viewer: Entity<FileContentViewer>,
    blame_viewer: Entity<BlameViewer>,
    diff: SharedString,
    path: SharedString,
    status: SharedString,
    view_mode: DiffViewMode,
    ignore_whitespace: bool,
    hunk_action: Option<HunkAction>,
    loading: bool,
    too_large: Option<u64>,
    last_event: SharedString,
    expanded_src: bool,
    selected: SharedString,
    _subscriptions: Vec<Subscription>,
}

impl Gallery {
    fn new(case: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let diff_viewer = cx.new(|cx| DiffViewer::new(window, cx));
        let file_viewer = cx.new(|cx| FileContentViewer::new(window, cx));
        let blame_viewer = cx.new(|cx| BlameViewer::new(window, cx));

        let mut this = Self {
            case: case.to_string(),
            shown: Shown::Viewer,
            diff: LOGIN_DIFF.into(),
            path: "src/auth/login.js".into(),
            status: "M".into(),
            view_mode: DiffViewMode::Unified,
            ignore_whitespace: false,
            hunk_action: None,
            loading: false,
            too_large: None,
            last_event: "—".into(),
            expanded_src: true,
            selected: "src/app.js".into(),
            _subscriptions: vec![
                cx.subscribe(&diff_viewer, |this, _, event: &DiffViewerEvent, cx| {
                    if let DiffViewerEvent::ApplyHunk(request) = event {
                        this.last_event = format!(
                            "{} block {} ({} patch lines)",
                            request.action.as_str(),
                            request.index,
                            request
                                .patch
                                .as_deref()
                                .map_or(0, |patch| patch.lines().count())
                        )
                        .into();
                        cx.notify();
                    }
                }),
                cx.subscribe(
                    &file_viewer,
                    |this, _, event: &FileContentViewerEvent, cx| {
                        if matches!(event, FileContentViewerEvent::Close) {
                            this.last_event = "close file".into();
                            cx.notify();
                        }
                    },
                ),
            ],
            diff_viewer,
            file_viewer,
            blame_viewer,
        };
        this.apply_case(cx);
        this
    }

    fn panel(&mut self, diff: &str, path: &str, status: &str, action: Option<HunkAction>) {
        self.shown = Shown::Panel;
        self.diff = diff.to_string().into();
        self.path = path.to_string().into();
        self.status = status.to_string().into();
        self.hunk_action = action;
    }

    fn file(&mut self, path: &str, file: Option<FileContent>, cx: &mut Context<Self>) {
        self.shown = Shown::File;
        self.file_viewer.update(cx, |viewer, cx| {
            viewer.set_header_hidden(true, cx);
            viewer.set_file(path.to_string(), file, cx);
        });
    }

    fn blame(&mut self, state: BlameState, cx: &mut Context<Self>) {
        self.shown = Shown::Blame;
        self.blame_viewer
            .update(cx, |viewer, cx| viewer.set_state(state, cx));
    }

    fn apply_case(&mut self, cx: &mut Context<Self>) {
        match self.case.clone().as_str() {
            "split" => self.view_mode = DiffViewMode::Split,
            "whitespace" => {
                self.diff = WHITESPACE_DIFF.into();
                self.path = "src/render.js".into();
            }
            "whitespace-split" => {
                self.diff = WHITESPACE_DIFF.into();
                self.path = "src/render.js".into();
                self.view_mode = DiffViewMode::Split;
            }
            "staging" => self.panel(APP_DIFF, "src/app.js", "M", Some(HunkAction::Stage)),
            "staging-split" => {
                self.panel(APP_DIFF, "src/app.js", "M", Some(HunkAction::Stage));
                self.view_mode = DiffViewMode::Split;
            }
            "staged" => self.panel(README_DIFF, "README.md", " ", Some(HunkAction::Unstage)),
            "untracked" => self.panel(NOTES_DIFF, "notes.txt", "?", None),
            "deleted" => self.panel(OLD_DIFF, "docs/old.md", "D", Some(HunkAction::Stage)),
            "ignore-spaces" => {
                self.panel(APP_DIFF, "src/app.js", "M", None);
                self.ignore_whitespace = true;
            }
            "loading" => {
                self.panel("", "src/app.js", "M", None);
                self.loading = true;
            }
            "too-large" => {
                self.panel("", "src/app.js", "M", None);
                self.too_large = Some(3_400_000);
            }
            "empty" => self.panel("", "src/app.js", "M", None),
            "file" => self.file("src/app.js", Some(FileContent::text(APP_JS)), cx),
            "file-header" => {
                self.file("src/app.js", Some(FileContent::text(APP_JS)), cx);
                self.file_viewer
                    .update(cx, |viewer, cx| viewer.set_header_hidden(false, cx));
            }
            "binary" => self.file("assets/logo.png", Some(FileContent::binary(2_148)), cx),
            "file-too-large" => {
                self.file("data/dump.sql", Some(FileContent::too_large(8_400_000)), cx)
            }
            "file-empty" => self.file("src/empty.js", Some(FileContent::text("")), cx),
            "file-loading" => {
                self.file("src/app.js", None, cx);
                self.file_viewer
                    .update(cx, |viewer, cx| viewer.set_loading(true, cx));
            }
            "file-error" => {
                self.file("src/app.js", None, cx);
                self.file_viewer.update(cx, |viewer, cx| {
                    viewer.set_error("This file or commit is no longer available.", cx)
                });
            }
            "blame" => self.blame(BlameState::Lines(blame_lines()), cx),
            "blame-loading" => self.blame(BlameState::Loading, cx),
            "blame-error" => self.blame(BlameState::Error("fatal: no such path".into()), cx),
            "blame-too-large" => self.blame(BlameState::TooLarge(8_400_000), cx),
            "tree" => self.shown = Shown::Tree,
            "long-lines" => {
                self.panel(
                    &long_lines_diff(),
                    "src/config.js",
                    "M",
                    Some(HunkAction::Stage),
                );
            }
            "large" => {
                self.diff = large_diff(20_000).into();
                self.path = "src/generated.js".into();
                self.hunk_action = Some(HunkAction::Stage);
            }
            "large-split" => {
                self.diff = large_diff(20_000).into();
                self.path = "src/generated.js".into();
                self.view_mode = DiffViewMode::Split;
            }
            "large-file" => self.file(
                "src/generated.js",
                Some(FileContent::text(large_file(20_000))),
                cx,
            ),
            _ => {}
        }

        let (diff, path, mode, action) = (
            self.diff.clone(),
            self.path.clone(),
            self.view_mode,
            self.hunk_action,
        );
        self.diff_viewer.update(cx, |viewer, cx| {
            viewer.set_diff(diff, path, cx);
            viewer.set_view_mode(mode, cx);
            viewer.set_hunk_action(action, cx);
        });
    }

    fn toggle_view(&mut self, cx: &mut Context<Self>) {
        self.view_mode = match self.view_mode {
            DiffViewMode::Unified => DiffViewMode::Split,
            DiffViewMode::Split => DiffViewMode::Unified,
        };
        let mode = self.view_mode;
        self.diff_viewer
            .update(cx, |viewer, cx| viewer.set_view_mode(mode, cx));
        cx.notify();
    }

    /// La lista de archivos del lado izquierdo, hecha con `FileTreeRow`.
    fn render_files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let header = |title: &'static str, count: usize| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .min_h(rems(2.05))
                .py(rems(0.42))
                .px(rems(0.65))
                .border_b_1()
                .border_color(palette.border)
                .bg(palette.accent)
                .text_xs()
                .child(div().font_weight(FontWeight::MEDIUM).child(title))
                .child(
                    div()
                        .text_color(palette.muted_foreground)
                        .child(count.to_string()),
                )
        };
        let file = |path: &'static str, name: &'static str, depth: usize, status: &'static str| {
            FileTreeRow::file(SharedString::from(format!("file:{path}")), name)
                .depth(depth)
                .path(path)
                .selected(self.selected.as_ref() == path)
                .leading(mergeon_ui::status_badge(status))
                .on_activate(cx.listener(move |this, _, _, cx| {
                    this.selected = path.into();
                    this.last_event = format!("activate {path}").into();
                    cx.notify();
                }))
                .on_open_new(cx.listener(move |this, _, _, cx| {
                    this.last_event = format!("open-new {path}").into();
                    cx.notify();
                }))
        };

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(240.))
            .h_full()
            .bg(palette.card)
            .border_r_1()
            .border_color(palette.border)
            .child(header("Unstaged Changes", 3))
            .child(
                FileTreeRow::folder("folder:docs", "docs").on_activate(cx.listener(
                    |this, _, _, cx| {
                        this.last_event = "toggle docs".into();
                        cx.notify();
                    },
                )),
            )
            .child(file("docs/old.md", "old.md", 1, "D"))
            .child(
                FileTreeRow::folder("folder:src", "src")
                    .expanded(self.expanded_src)
                    .on_activate(cx.listener(|this, _, _, cx| {
                        this.expanded_src = !this.expanded_src;
                        cx.notify();
                    })),
            )
            .when(self.expanded_src, |this| {
                this.child(file("src/app.js", "app.js", 1, "M"))
            })
            .child(file("notes.txt", "notes.txt", 0, "?"))
            .child(header("Changed Files", 6))
            .children(
                [
                    ("docs/guide.md", "guide.md", "docs", "A", 3, 0),
                    ("README.md", "README.md", "", "M", 7, 2),
                    ("src/app.js", "app.js", "src", "M", 9, 0),
                    ("src/lib/util.js", "util.js", "src/lib", "D", 0, 3),
                    (
                        "styles/a-very-long-file-name-for-the-list.css",
                        "a-very-long-file-name-for-the-list.css",
                        "styles",
                        "R",
                        4,
                        4,
                    ),
                ]
                .into_iter()
                .map(|(path, name, dir, status, additions, deletions)| {
                    FileTreeRow::file(SharedString::from(format!("commit:{path}")), name)
                        .path(path)
                        .dir(dir)
                        .leading(mergeon_ui::status_badge(status))
                        .trailing(div().children(file_stats(additions, deletions)))
                }),
            )
            .child(
                div()
                    .mt_auto()
                    .p_2()
                    .text_xs()
                    .text_color(palette.muted_foreground)
                    .child(self.last_event.clone()),
            )
    }

    /// La barra de la pestaña de archivo histórico (`HistoricalFileTab.vue`),
    /// que no es de este crate: aquí solo sitúa al visor como en la referencia.
    fn render_viewer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .justify_end()
                    .p(px(5.))
                    .border_b_1()
                    .border_color(palette.border)
                    .child(
                        mergeon_ui::button("ignore-spaces")
                            .ghost()
                            .sm()
                            .label("Ignore spaces"),
                    )
                    .child(
                        mergeon_ui::button("toggle-view")
                            .ghost()
                            .sm()
                            .label(mergeon_view_diff::view_toggle_label(self.view_mode))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_view(cx))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(self.diff_viewer.clone()),
            )
    }

    fn render_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        DiffPanel::new(&self.diff_viewer)
            .file(self.path.clone(), self.status.clone())
            .diff(self.diff.clone())
            .loading(self.loading)
            .too_large(self.too_large)
            .view_mode(self.view_mode)
            .ignore_whitespace(self.ignore_whitespace)
            .hunk_action(if self.ignore_whitespace {
                None
            } else {
                self.hunk_action
            })
            .on_toggle_view(cx.listener(|this, _, _, cx| this.toggle_view(cx)))
            .on_toggle_ignore_whitespace(cx.listener(|this, _, _, cx| {
                this.ignore_whitespace = !this.ignore_whitespace;
                cx.notify();
            }))
            .on_discard(cx.listener(|this, _, _, cx| {
                this.last_event = "discard".into();
                cx.notify();
            }))
    }

    /// El blame va dentro de un diálogo en la app: aquí, en una caja de su
    /// mismo ancho.
    fn render_blame(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(1022.))
                    .p(px(24.))
                    .gap(px(16.))
                    .rounded(px(18.))
                    .border_1()
                    .border_color(palette.dialog_border)
                    .bg(palette.dialog_glass)
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Blame: app.js"),
                    )
                    .child(div().h(px(222.)).child(self.blame_viewer.clone())),
            )
    }
}

impl Render for Gallery {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let content = match self.shown {
            Shown::Viewer => self.render_viewer(cx).into_any_element(),
            Shown::Panel => self.render_panel(cx).into_any_element(),
            Shown::File => div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .child(self.file_viewer.clone())
                .into_any_element(),
            Shown::Blame => self.render_blame(cx).into_any_element(),
            Shown::Tree => div().flex_1().into_any_element(),
        };

        div()
            .flex()
            .size_full()
            .bg(palette.background)
            .text_color(palette.foreground)
            .when(self.shown != Shown::Blame, |this| {
                this.child(self.render_files(cx))
            })
            .child(content)
    }
}

/// Ejecuta un paso de `MERGEON_DIFF_INPUT` como si viniera del usuario.
fn run_step(step: &str, window: &mut Window, cx: &mut App) {
    let Some((kind, value)) = step.split_once(':') else {
        eprintln!("MERGEON_DIFF_INPUT: paso inválido `{step}`");
        return;
    };
    if kind == "key" {
        match Keystroke::parse(value) {
            Ok(keystroke) => {
                window.dispatch_keystroke(keystroke.clone(), cx);
                window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
            }
            Err(error) => eprintln!("MERGEON_DIFF_INPUT: tecla inválida `{value}`: {error}"),
        }
        return;
    }

    let numbers: Vec<f32> = value
        .split(',')
        .filter_map(|number| number.trim().parse().ok())
        .collect();
    let at = |ix: usize| point(px(numbers[ix]), px(numbers[ix + 1]));
    let move_to = |position, pressed, window: &mut Window, cx: &mut App| {
        window.dispatch_event(
            MouseMoveEvent {
                position,
                pressed_button: pressed,
                modifiers: Modifiers::default(),
            }
            .to_platform_input(),
            cx,
        );
    };
    let down = |position, click_count, window: &mut Window, cx: &mut App| {
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Left,
                position,
                modifiers: Modifiers::default(),
                click_count,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
    };
    let up = |position, click_count, window: &mut Window, cx: &mut App| {
        window.dispatch_event(
            MouseUpEvent {
                button: MouseButton::Left,
                position,
                modifiers: Modifiers::default(),
                click_count,
            }
            .to_platform_input(),
            cx,
        );
    };

    match (kind, numbers.len()) {
        ("move", 2) => move_to(at(0), None, window, cx),
        ("click", 2) | ("dclick", 2) => {
            move_to(at(0), None, window, cx);
            for count in 1..=if kind == "dclick" { 2 } else { 1 } {
                down(at(0), count, window, cx);
                up(at(0), count, window, cx);
            }
        }
        ("drag", 4) => {
            move_to(at(0), None, window, cx);
            down(at(0), 1, window, cx);
            move_to(at(2), Some(MouseButton::Left), window, cx);
            up(at(2), 1, window, cx);
        }
        ("scroll", 4) => {
            move_to(at(0), None, window, cx);
            window.dispatch_event(
                ScrollWheelEvent {
                    position: at(0),
                    delta: ScrollDelta::Pixels(at(2)),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        }
        _ => eprintln!("MERGEON_DIFF_INPUT: paso inválido `{step}`"),
    }
}

fn main() {
    let preference = ThemePreference::parse(&std::env::var("MERGEON_THEME").unwrap_or_default());
    let case = std::env::var("MERGEON_DIFF_CASE").unwrap_or_default();
    let bench = std::env::var("MERGEON_DIFF_BENCH").is_ok();
    let steps: Vec<String> = std::env::var("MERGEON_DIFF_INPUT")
        .unwrap_or_default()
        .split(';')
        .map(|step| step.trim().to_string())
        .filter(|step| !step.is_empty())
        .collect();

    gpui_kit::application()
        .with_assets(mergeon_ui::Assets)
        .run(move |cx| {
            mergeon_ui::init(cx);
            mergeon_view_diff::init(cx);
            mergeon_ui::theme::apply(preference, cx);
            let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
            let (window, _gallery) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| Gallery::new(&case, window, cx)),
            )
            .expect("no se pudo abrir la ventana");

            cx.spawn(async move |cx| {
                for step in steps {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(300))
                        .await;
                    let _ = window.update(cx, |_, window, cx| run_step(&step, window, cx));
                }

                if bench {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(1500))
                        .await;
                    let _ = window.update(cx, |_, window, cx| {
                        bench_step(BENCH_FRAMES, std::time::Instant::now(), window, cx);
                    });
                }
            })
            .detach();
        });
}

const BENCH_FRAMES: usize = 120;

/// Un paso de scroll por frame: cada paso se dibuja antes de dar el siguiente.
fn bench_step(remaining: usize, started: std::time::Instant, window: &mut Window, cx: &mut App) {
    if remaining == 0 {
        let elapsed = started.elapsed();
        eprintln!(
            "bench: {BENCH_FRAMES} frames con scroll en {elapsed:?} ({:.1} ms por frame)",
            elapsed.as_secs_f64() * 1000. / BENCH_FRAMES as f64
        );
        return;
    }
    run_step("scroll:800,450,0,-730", window, cx);
    window.on_next_frame(move |window, cx| bench_step(remaining - 1, started, window, cx));
}
