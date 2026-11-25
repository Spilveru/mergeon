//! Inspector del commit seleccionado
//! (`src/components/repository/CommitInspector.vue`): el porqué (la cabecera
//! con el mensaje), cuánto cambió (el bloque de cifras de Señal, con el modo
//! del diff al lado) y el qué (los archivos que cambió), juntos. El árbol del
//! repositorio en ese commit se abre en su propia pestaña.

use std::rc::Rc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClickEvent, Entity, FontWeight, Hsla, Pixels, SharedString, Window, canvas,
    div, px, relative,
};
use mergeon_model::commit_view_utils::CommitRow;
use mergeon_ui::menu::MenuItem;
use mergeon_ui::{ActivePaletteExt as _, Lucide};
use mergeon_view_diff::code_font_family;

use crate::changes_tab::CommitChangesTab;
use crate::commits_view::CommitCommand;
use crate::summary_header::CommitSummaryHeader;

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type HashHandler = Rc<dyn Fn(&str, &mut Window, &mut App)>;
type CommandHandler = Rc<dyn Fn(CommitCommand, &mut Window, &mut App)>;
type MenuProvider = Rc<dyn Fn(&mut Window, &mut App) -> Vec<MenuItem>>;

/// El cuerpo desplegado del mensaje crece hasta esta fracción del alto del
/// inspector; más largo, se desplaza dentro.
pub const BODY_MAX_FRACTION: f32 = 0.4;
/// Alto mínimo de la lista de archivos: su cabecera y cinco filas de 26px.
pub const MIN_FILES_HEIGHT: f32 = 34. + 5. * 26. + 4.;

/// Lo que dice el bloque de cifras: archivos, líneas añadidas y quitadas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CommitTotals {
    pub files: usize,
    pub additions: usize,
    pub deletions: usize,
}

impl CommitTotals {
    /// Suma las cifras de los archivos de la lista.
    pub fn of(changes: &CommitChangesTab) -> Self {
        changes
            .rows()
            .iter()
            .fold(Self::default(), |totals, row| Self {
                files: totals.files + 1,
                additions: totals.additions + row.file.additions as usize,
                deletions: totals.deletions + row.file.deletions as usize,
            })
    }
}

/// Inspector de un commit. Componente controlado sobre la lista de cambios
/// que guarda quien lo aloja.
#[derive(IntoElement)]
pub struct CommitInspector {
    commit: CommitRow,
    changes: Entity<CommitChangesTab>,
    expanded: bool,
    checkout_disabled: bool,
    on_toggle_details: Option<ClickHandler>,
    on_open_tree: Option<ClickHandler>,
    on_copy_sha: Option<HashHandler>,
    on_select_parent: Option<HashHandler>,
    on_command: Option<CommandHandler>,
    menu: Option<MenuProvider>,
}

impl CommitInspector {
    pub fn new(commit: &CommitRow, changes: &Entity<CommitChangesTab>) -> Self {
        Self {
            commit: commit.clone(),
            changes: changes.clone(),
            expanded: false,
            checkout_disabled: false,
            on_toggle_details: None,
            on_open_tree: None,
            on_copy_sha: None,
            on_select_parent: None,
            on_command: None,
            menu: None,
        }
    }

    /// El cuerpo del mensaje se ve entero.
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    pub fn on_toggle_details(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle_details = Some(Rc::new(handler));
        self
    }

    /// `open-tree`: botón "Repo tree".
    pub fn on_open_tree(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_tree = Some(Rc::new(handler));
        self
    }

    /// `copy-sha`: recibe el hash completo.
    pub fn on_copy_sha(mut self, handler: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_copy_sha = Some(Rc::new(handler));
        self
    }

    /// `select-parent`: recibe el hash del padre.
    pub fn on_select_parent(
        mut self,
        handler: impl Fn(&str, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select_parent = Some(Rc::new(handler));
        self
    }

    /// Botones Checkout y Cherry-pick de la cabecera.
    pub fn on_command(
        mut self,
        handler: impl Fn(CommitCommand, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_command = Some(Rc::new(handler));
        self
    }

    /// Entradas del botón ⋯ de la cabecera (el menú contextual del commit).
    pub fn menu(
        mut self,
        items: impl Fn(&mut Window, &mut App) -> Vec<MenuItem> + 'static,
    ) -> Self {
        self.menu = Some(Rc::new(items));
        self
    }

    pub fn checkout_disabled(mut self, disabled: bool) -> Self {
        self.checkout_disabled = disabled;
        self
    }
}

/// Una celda del bloque de cifras (Señal `.stat`): número grande en mono y
/// etiqueta pequeña en mayúsculas.
pub(crate) fn stat(
    id: &'static str,
    value: String,
    key: &'static str,
    color: Hsla,
    cx: &App,
) -> AnyElement {
    let palette = cx.palette();
    let mono = code_font_family(cx);
    div()
        .id(id)
        .test_support()
        .aria_label(SharedString::from(format!(
            "{value} {}",
            key.to_lowercase()
        )))
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap(px(2.))
        .py(px(8.))
        .px(px(16.))
        .border_r_1()
        .border_color(palette.border_hair)
        .font_family(mono)
        .child(
            div()
                .text_size(px(20.))
                .line_height(px(24.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .child(value),
        )
        .child(
            div()
                .text_size(px(10.))
                .line_height(px(14.))
                .text_color(palette.text_3)
                .child(key),
        )
        .into_any_element()
}

impl RenderOnce for CommitInspector {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();

        // Alto del inspector, medido al pintarlo: de él sale el tope del
        // cuerpo desplegado. Hasta medirlo vale el de la ventana.
        let height = window.use_keyed_state("commit-details-height", cx, |_, _| None::<Pixels>);
        let body_max_height = height
            .read(cx)
            .unwrap_or_else(|| window.viewport_size().height)
            * BODY_MAX_FRACTION;
        let measure = canvas(
            move |bounds, _, cx| {
                let measured = bounds.size.height;
                if *height.read(cx) != Some(measured) {
                    height.update(cx, |height, cx| {
                        *height = Some(measured);
                        cx.notify();
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();

        let header = CommitSummaryHeader::new(&self.commit)
            .expanded(self.expanded)
            .body_max_height(body_max_height)
            .checkout_disabled(self.checkout_disabled)
            .map(|header| match self.on_toggle_details {
                Some(handler) => {
                    header.on_toggle(move |event, window, cx| handler(event, window, cx))
                }
                None => header,
            })
            .map(|header| match self.on_copy_sha {
                Some(handler) => {
                    header.on_copy_sha(move |hash, window, cx| handler(hash, window, cx))
                }
                None => header,
            })
            .map(|header| match self.on_select_parent {
                Some(handler) => {
                    header.on_select_parent(move |hash, window, cx| handler(hash, window, cx))
                }
                None => header,
            })
            .map(|header| match self.on_command {
                Some(handler) => {
                    header.on_command(move |command, window, cx| handler(command, window, cx))
                }
                None => header,
            })
            .map(|header| match self.menu {
                Some(menu) => header.menu(move |window, cx| menu(window, cx)),
                None => header,
            });

        // Mientras llegan los archivos del commit las cifras serían las del
        // anterior: se dejan en blanco.
        let (loading, totals) = {
            let changes = self.changes.read(cx);
            (changes.is_loading_details(), CommitTotals::of(changes))
        };
        let blank = |value: String| {
            if loading {
                "\u{2013}".to_string()
            } else {
                value
            }
        };
        let files_key = if totals.files == 1 { "FILE" } else { "FILES" };
        let stats = div()
            .id("commit-stats")
            .test_support()
            .flex()
            .flex_shrink_0()
            .items_stretch()
            .border_b_1()
            .border_color(palette.border_hair)
            .child(stat(
                "stat-files",
                blank(totals.files.to_string()),
                files_key,
                palette.text_1.into(),
                cx,
            ))
            .child(stat(
                "stat-additions",
                blank(format!("+{}", totals.additions)),
                "LINES",
                palette.success_foreground.into(),
                cx,
            ))
            .child(stat(
                "stat-deletions",
                blank(format!("\u{2212}{}", totals.deletions)),
                "LINES",
                palette.destructive_foreground.into(),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .justify_end()
                    .gap(px(8.))
                    .px(px(12.))
                    // Con cifras anchas (−66057) el botón se recorta en vez de
                    // montarse encima de ellas.
                    .overflow_hidden()
                    .child(
                        mergeon_ui::button("open-repo-tree")
                            .ghost()
                            .xs()
                            .icon(Lucide::FolderTree)
                            .label("Repo tree")
                            .tooltip("Browse the repository at this commit")
                            .when_some(self.on_open_tree, |button, handler| {
                                button.on_click(move |event, window, cx| handler(event, window, cx))
                            }),
                    ),
            );

        div()
            .id("commit-details")
            .test_support()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .w_full()
            .h_full()
            .relative()
            .overflow_hidden()
            .bg(palette.card)
            .line_height(relative(1.5))
            .child(measure)
            .child(header)
            .child(stats)
            .child(
                // La lista no baja de unas cinco filas: si falta alto, cede
                // el cuerpo del mensaje.
                div()
                    .id("commit-files")
                    .test_support()
                    .flex_1()
                    .min_h(px(MIN_FILES_HEIGHT))
                    .overflow_hidden()
                    .child(self.changes),
            )
    }
}
