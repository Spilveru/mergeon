//! Cabecera del commit seleccionado
//! (`src/components/repository/CommitSummaryHeader.vue`, rehecha con el `.dh`
//! de Señal): hash copiable y acciones, título, cuerpo del mensaje y la línea
//! de autor, correo, fecha y padres.

use std::rc::Rc;

use gpui_kit::base::{Button as BaseButton, TestSupportExt as _};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::*;
use gpui_kit::{
    Anchor, App, ClickEvent, FontWeight, Pixels, ScrollHandle, SharedString, Window, canvas, div,
    px,
};
use mergeon_model::commit_view_utils::CommitRow;
use mergeon_model::format::format_date_time;
use mergeon_ui::menu::MenuItem;
use mergeon_ui::{ActivePaletteExt as _, IconTone, Lucide, Radius};
use mergeon_view_diff::code_font_family;

use crate::commits_view::CommitCommand;
use crate::monogram::monogram;

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type HashHandler = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// Hash abreviado que enseña la cabecera (`shortHash`): 8 caracteres.
pub fn short_hash(full_hash: &str) -> &str {
    let end = full_hash
        .char_indices()
        .nth(8)
        .map_or(full_hash.len(), |(index, _)| index);
    &full_hash[..end]
}

/// Hashes de los padres de un commit (`parentHashes`).
pub fn parent_hashes(parents: &str) -> Vec<&str> {
    parents.split_whitespace().collect()
}

type CommandHandler = Rc<dyn Fn(CommitCommand, &mut Window, &mut App)>;
type MenuProvider = Rc<dyn Fn(&mut Window, &mut App) -> Vec<MenuItem>>;

/// Líneas visuales (las partidas por ancho cuentan) del cuerpo plegado.
pub const BODY_CLAMP_LINES: usize = 3;
/// Tipografía del cuerpo: con ella se cuentan sus líneas visuales.
const BODY_FONT_SIZE: f32 = 13.;
pub const BODY_LINE_HEIGHT: f32 = 19.;
/// Ancho máximo del cuerpo (Señal `.dh p { max-width: 560px }`).
const BODY_MAX_WIDTH: f32 = 560.;
/// El cuerpo desplegado crece hasta esta fracción de la ventana si quien
/// aloja la cabecera no dice otra cosa ([`CommitSummaryHeader::body_max_height`]).
const BODY_MAX_VIEWPORT_FRACTION: f32 = 0.4;

/// Un cuerpo de commit partido en el texto y su bloque final de trailers
/// (`Co-Authored-By: …`, `Signed-off-by: …`), los dos sin blancos al borde.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageBody<'a> {
    pub text: &'a str,
    pub trailers: &'a str,
}

/// `Token: valor`, con el token sin espacios (`Co-Authored-By`, `Fixes`).
fn trailer_token(line: &str) -> Option<&str> {
    let (token, value) = line.split_once(':')?;
    let well_formed = token
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && token
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        && (value.is_empty() || value.starts_with(' '));
    well_formed.then_some(token)
}

/// Un token que git o las forjas usan como trailer: `*-by` y unos pocos más.
/// Exigir uno evita tomar por trailers un párrafo como "Nota: …".
fn is_known_trailer(token: &str) -> bool {
    let token = token.to_ascii_lowercase();
    token.ends_with("-by")
        || matches!(
            token.as_str(),
            "cc" | "fixes" | "closes" | "refs" | "change-id" | "see-also" | "bug" | "link"
        )
}

/// Separa los trailers del final del cuerpo, como `git interpret-trailers`:
/// son el último párrafo si todas sus líneas son `Token: valor` (o
/// continuaciones sangradas) y al menos un token es de los conocidos.
pub fn split_trailers(body: &str) -> MessageBody<'_> {
    let body = body.trim_end();
    let mut paragraph_start = 0;
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        offset += line.len();
        if line.trim().is_empty() {
            paragraph_start = offset;
        }
    }
    let paragraph = &body[paragraph_start..];
    let mut known = false;
    let all_trailers = !paragraph.is_empty()
        && paragraph.lines().enumerate().all(|(index, line)| {
            if index > 0 && line.starts_with([' ', '\t']) {
                return true;
            }
            match trailer_token(line) {
                Some(token) => {
                    known |= is_known_trailer(token);
                    true
                }
                None => false,
            }
        });
    if all_trailers && known {
        MessageBody {
            text: body[..paragraph_start].trim(),
            trailers: paragraph.trim(),
        }
    } else {
        MessageBody {
            text: body.trim(),
            trailers: "",
        }
    }
}

/// Líneas que ocupa `text` pintado con la tipografía del cuerpo en `width`:
/// cada línea del texto más las veces que se parte por no caber.
pub fn visual_line_count(text: &str, width: Pixels, window: &Window) -> usize {
    if text.is_empty() {
        return 0;
    }
    let run = window.text_style().to_run(text.len());
    window
        .text_system()
        .shape_text(
            SharedString::from(text.to_string()),
            px(BODY_FONT_SIZE),
            &[run],
            Some(width),
            None,
        )
        .map(|lines| {
            lines
                .iter()
                .map(|line| line.wrap_boundaries.len() + 1)
                .sum()
        })
        .unwrap_or_else(|_| text.lines().count())
}

/// Lo que la cabecera recuerda entre frames: el ancho real del cuerpo (para
/// contar sus líneas visuales), el recuento hecho con él y el scroll del
/// cuerpo desplegado, que vuelve arriba al cambiar de commit.
struct BodyState {
    width: Option<Pixels>,
    count: Option<(SharedString, Pixels, usize)>,
    scroll: ScrollHandle,
    commit: String,
}

/// Cabecera del commit (Señal `.dh`): el hash en cápsula partida con su botón
/// de copiar, las acciones (Checkout, Cherry-pick y ⋯ con el menú del commit),
/// el título, el cuerpo y la línea de autor, fecha y padre.
///
/// Componente controlado: quien la aloja guarda si el cuerpo está desplegado
/// (empieza recortado a tres líneas visuales: el diff manda). Desplegado, el
/// cuerpo crece hasta [`CommitSummaryHeader::body_max_height`] y a partir de
/// ahí tiene su propio scroll; los trailers solo se ven entonces.
#[derive(IntoElement)]
pub struct CommitSummaryHeader {
    commit: CommitRow,
    expanded: bool,
    body_max_height: Option<Pixels>,
    checkout_disabled: bool,
    on_toggle: Option<ClickHandler>,
    on_copy_sha: Option<HashHandler>,
    on_select_parent: Option<HashHandler>,
    on_command: Option<CommandHandler>,
    menu: Option<MenuProvider>,
}

impl CommitSummaryHeader {
    pub fn new(commit: &CommitRow) -> Self {
        Self {
            commit: commit.clone(),
            expanded: false,
            body_max_height: None,
            checkout_disabled: false,
            on_toggle: None,
            on_copy_sha: None,
            on_select_parent: None,
            on_command: None,
            menu: None,
        }
    }

    /// El cuerpo del mensaje se ve entero (si no, recortado a tres líneas).
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    /// Alto máximo del cuerpo desplegado; más largo, se desplaza dentro. Sin
    /// él, el 40 % de la ventana.
    pub fn body_max_height(mut self, height: Pixels) -> Self {
        self.body_max_height = Some(height);
        self
    }

    /// Clic en "Show more" / "Show less" (solo con un cuerpo largo).
    pub fn on_toggle(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle = Some(Rc::new(handler));
        self
    }

    /// `copy-sha`: la cápsula del hash o su botón de copiar; recibe el hash
    /// completo.
    pub fn on_copy_sha(mut self, handler: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_copy_sha = Some(Rc::new(handler));
        self
    }

    /// `select-parent`: recibe el hash del padre elegido.
    pub fn on_select_parent(
        mut self,
        handler: impl Fn(&str, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select_parent = Some(Rc::new(handler));
        self
    }

    /// Los botones Checkout y Cherry-pick: la misma acción que la entrada del
    /// menú contextual del commit.
    pub fn on_command(
        mut self,
        handler: impl Fn(CommitCommand, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_command = Some(Rc::new(handler));
        self
    }

    /// El botón ⋯: abre estas entradas (el menú contextual de la fila). Sin
    /// menú, el botón no aparece.
    pub fn menu(
        mut self,
        items: impl Fn(&mut Window, &mut App) -> Vec<MenuItem> + 'static,
    ) -> Self {
        self.menu = Some(Rc::new(items));
        self
    }

    /// Checkout no tiene sentido (el commit ya es la punta de la rama actual).
    pub fn checkout_disabled(mut self, disabled: bool) -> Self {
        self.checkout_disabled = disabled;
        self
    }
}

impl RenderOnce for CommitSummaryHeader {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let mono = code_font_family(cx);
        let commit = self.commit;
        let expanded = self.expanded;
        let body_max_height = self
            .body_max_height
            .unwrap_or_else(|| window.viewport_size().height * BODY_MAX_VIEWPORT_FRACTION);

        // --- fila superior: hash, copiar, refs y acciones ----------------------
        let full_hash = commit.full_hash.clone();
        // Señal `.bighash`: cápsula partida, corto en negrita | resto apagado.
        let short = short_hash(&commit.full_hash).to_string();
        let rest: String = commit
            .full_hash
            .get(short.len()..)
            .unwrap_or_default()
            .chars()
            .take(12)
            .collect();
        let copy = self.on_copy_sha.clone();
        // Con el panel estrecho el resto del hash es lo primero que cede: el
        // corto, Checkout y ⋯ siguen a la vista.
        let sha = BaseButton::new("summary-sha")
            .accessibility_label(short.clone())
            .flex()
            .flex_shrink(1.)
            .min_w_0()
            .items_center()
            .h(px(22.))
            .border_1()
            .border_color(palette.border_base)
            .rounded(Radius::Xs.px(cx))
            .overflow_hidden()
            .font_family(mono.clone())
            .text_size(px(13.))
            .cursor_pointer()
            .hover(move |style| style.bg(palette.bg_hover))
            .tooltip(mergeon_ui::tooltip(format!(
                "{} \u{2014} click to copy",
                commit.full_hash
            )))
            .when_some(copy, |button, on_copy_sha| {
                let full_hash = full_hash.clone();
                button.on_click(move |_, window, cx| on_copy_sha(&full_hash, window, cx))
            })
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .px(px(7.))
                    .h_full()
                    .text_color(palette.text_1)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(short),
            )
            .when(!rest.is_empty(), |button| {
                button.child(
                    div()
                        .flex()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .items_center()
                        .px(px(7.))
                        .h_full()
                        .border_l_1()
                        .border_color(palette.border_hair)
                        .text_color(palette.text_3)
                        .child(format!("{rest}\u{b7}")),
                )
            });
        let copy_button = mergeon_ui::button("summary-copy")
            .ghost()
            .xs()
            .icon(Lucide::Copy)
            .tooltip("Copy SHA")
            .when_some(self.on_copy_sha, |button, on_copy_sha| {
                let full_hash = commit.full_hash.clone();
                button.on_click(move |_, window, cx| on_copy_sha(&full_hash, window, cx))
            });

        let ref_names: Vec<String> = commit
            .refs
            .iter()
            .map(|commit_ref| commit_ref.name.clone())
            .collect();
        let refs = commit.refs.iter().enumerate().map(|(index, commit_ref)| {
            div()
                .id(("summary-ref", index))
                .test_support()
                .aria_label(SharedString::from(commit_ref.name.clone()))
                .flex()
                .flex_shrink_0()
                .items_center()
                .px(px(6.))
                .h(px(18.))
                .rounded(Radius::Xs.px(cx))
                .border_1()
                .border_color(palette.primary_soft_border)
                .bg(palette.primary_soft)
                .text_color(palette.primary_text)
                .font_family(mono.clone())
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .child(commit_ref.name.clone())
        });

        let command =
            |id: &'static str, icon: Lucide, label: &'static str, which: CommitCommand| {
                let on_command = self.on_command.clone();
                mergeon_ui::button(id)
                    .outline()
                    .xs()
                    .icon(icon)
                    .label(label)
                    .when_some(on_command, move |button, on_command| {
                        button.on_click(move |_, window, cx| on_command(which, window, cx))
                    })
            };
        let checkout = command(
            "summary-checkout",
            Lucide::GitBranch,
            "Checkout",
            CommitCommand::Checkout,
        )
        .disabled(self.checkout_disabled)
        .tooltip(if self.checkout_disabled {
            "Already on this commit's branch"
        } else {
            "Check out this commit"
        });
        // Cherry-pick y el resto van en ⋯: con dos botones y las refs la fila
        // no cabía en un panel estrecho.
        let more = self.menu.map(|menu| {
            mergeon_ui::button("summary-more")
                .outline()
                .xs()
                .icon(Lucide::Ellipsis)
                .tooltip("More actions")
                .dropdown_menu_with_anchor(Anchor::TopRight, move |popup, window, cx| {
                    mergeon_ui::menu::build(popup, menu(window, cx), window, cx)
                })
        });

        let top = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(8.))
            .mb(px(10.))
            .min_w_0()
            .child(sha)
            .child(copy_button)
            .child(
                div()
                    .id("summary-refs")
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(px(4.))
                    .overflow_hidden()
                    // Las que no caben quedan cortadas: al pasar el ratón se
                    // leen todas.
                    // Siempre queda un trozo a la vista sobre el que pasar
                    // el ratón: el hash cede antes.
                    .when(!ref_names.is_empty(), |refs| {
                        refs.min_w(px(56.))
                            .tooltip(mergeon_ui::tooltip(ref_names.join("\n")))
                    })
                    .children(refs),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(6.))
                    .child(checkout)
                    .children(more),
            );

        // --- título y cuerpo -----------------------------------------------------
        let title = div()
            .id("summary-title")
            .test_support()
            .aria_label(SharedString::from(commit.message.clone()))
            .flex_shrink_0()
            .text_size(px(16.))
            .font_weight(FontWeight::SEMIBOLD)
            .line_height(px(22.))
            .text_color(palette.text_1)
            .child(commit.message.clone());

        // El texto cuenta para el recorte; los trailers van aparte y solo se
        // ven desplegado (o si el texto ya cabe entero y no hay botón).
        let message = split_trailers(&commit.message_body);
        let body_text = SharedString::from(message.text.to_string());
        let state = window.use_keyed_state("summary-body-state", cx, |_, _| BodyState {
            width: None,
            count: None,
            scroll: ScrollHandle::new(),
            commit: String::new(),
        });
        if state.read(cx).commit != commit.full_hash {
            state.update(cx, |state, _| {
                state.commit = commit.full_hash.clone();
                state.scroll = ScrollHandle::new();
            });
        }
        // Hasta medir el cuerpo se cuenta con su ancho máximo.
        let width = state
            .read(cx)
            .width
            .unwrap_or(px(BODY_MAX_WIDTH))
            .min(px(BODY_MAX_WIDTH));
        let lines = match &state.read(cx).count {
            Some((text, counted_width, lines)) if *text == body_text && *counted_width == width => {
                *lines
            }
            _ => {
                let lines = visual_line_count(&body_text, width, window);
                state.update(cx, |state, _| {
                    state.count = Some((body_text.clone(), width, lines));
                });
                lines
            }
        };
        let long_body = lines > BODY_CLAMP_LINES;
        let scroll = state.read(cx).scroll.clone();

        let trailers = (!message.trailers.is_empty() && (expanded || !long_body)).then(|| {
            div()
                .id("summary-trailers")
                .test_support()
                .aria_label(SharedString::from(message.trailers.to_string()))
                .flex_shrink_0()
                .mt(px(6.))
                .text_size(px(11.))
                .line_height(px(16.))
                .text_color(palette.text_3)
                .child(message.trailers.to_string())
        });
        let text = (!body_text.is_empty()).then(|| {
            div()
                .flex_shrink_0()
                .text_size(px(BODY_FONT_SIZE))
                .line_height(px(BODY_LINE_HEIGHT))
                .text_color(palette.text_2)
                .when(long_body && !expanded, |text| {
                    // `line_clamp` solo cuenta las líneas partidas por ancho
                    // si hay con qué truncar; el alto máximo asegura las tres.
                    text.max_h(px(BODY_LINE_HEIGHT * BODY_CLAMP_LINES as f32))
                        .overflow_hidden()
                        .text_ellipsis()
                        .line_clamp(BODY_CLAMP_LINES)
                })
                .child(body_text.clone())
        });
        let measure = {
            let state = state.clone();
            canvas(
                move |bounds, _, cx| {
                    let width = bounds.size.width;
                    if state.read(cx).width != Some(width) {
                        state.update(cx, |state, cx| {
                            state.width = Some(width);
                            cx.notify();
                        });
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };
        let body = (text.is_some() || trailers.is_some()).then(|| {
            div()
                .relative()
                .flex()
                .flex_col()
                .flex_shrink(1.)
                // Con la ventana muy baja cede alto, pero una línea queda.
                .min_h(px(BODY_LINE_HEIGHT))
                .mt(px(4.))
                .w_full()
                .max_w(px(BODY_MAX_WIDTH))
                .child(measure)
                .child(
                    div()
                        .id("summary-body")
                        .test_support()
                        .aria_label(body_text.clone())
                        .flex()
                        .flex_col()
                        .min_h_0()
                        .max_h(body_max_height)
                        .overflow_y_scroll()
                        .track_scroll(&scroll)
                        .children(text)
                        .children(trailers),
                )
                .vertical_scrollbar(&scroll)
        });
        let toggle = long_body.then(|| {
            BaseButton::new("summary-toggle")
                .accessibility_label(if expanded { "Show less" } else { "Show more" })
                .flex_shrink_0()
                .self_start()
                .mt(px(2.))
                .text_size(px(12.))
                .text_color(palette.text_3)
                .cursor_pointer()
                .hover(move |style| style.text_color(palette.text_1))
                .when_some(self.on_toggle, |button, on_toggle| {
                    button.on_click(move |event, window, cx| on_toggle(event, window, cx))
                })
                .child(if expanded { "Show less" } else { "Show more" })
        });

        // --- autor, fecha y padres -----------------------------------------------
        let person = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_w_0()
            .child(monogram(&commit.author, 22., 10., false, cx))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(5.))
                    .min_w_0()
                    .child(
                        div()
                            .id("summary-author")
                            .test_support()
                            .aria_label(SharedString::from(commit.author.clone()))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(palette.text_1)
                            .child(commit.author.clone()),
                    )
                    .when(!commit.email.is_empty(), |name| {
                        name.child(
                            div()
                                .id("summary-email")
                                .test_support()
                                .aria_label(SharedString::from(commit.email.clone()))
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(palette.text_3)
                                .child(commit.email.clone()),
                        )
                    }),
            );

        // Antes se pintaba el ISO tal cual; la fecha exacta queda en el tooltip.
        let date = div()
            .id("summary-date")
            .test_support()
            .aria_label(SharedString::from(commit.full_date.clone()))
            .tooltip(mergeon_ui::tooltip(commit.full_date.clone()))
            .font_family(mono.clone())
            .text_size(px(11.5))
            .text_color(palette.text_3)
            .child(format_date_time(commit.full_date.as_str()));

        let parents = parent_hashes(&commit.parents);
        let on_select_parent = self.on_select_parent;
        let parent_links = (!parents.is_empty()).then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .font_family(mono.clone())
                .text_size(px(11.5))
                .text_color(palette.text_3)
                .child(
                    mergeon_ui::icon(Lucide::GitCommitHorizontal)
                        .size(px(14.))
                        .tone(IconTone::Muted),
                )
                .child(if parents.len() > 1 {
                    "parents"
                } else {
                    "parent"
                })
                .children(parents.iter().map(|parent| {
                    let parent = parent.to_string();
                    let target = parent.clone();
                    let on_select_parent = on_select_parent.clone();
                    BaseButton::new(SharedString::from(format!("parent:{parent}")))
                        .accessibility_label(short_hash(&parent).to_string())
                        .text_color(palette.text_2)
                        .font_weight(FontWeight::MEDIUM)
                        .cursor_pointer()
                        .hover(move |style| style.text_color(palette.text_1).underline())
                        .tooltip(mergeon_ui::tooltip(format!("Go to {parent}")))
                        .when_some(on_select_parent, |button, on_select_parent| {
                            button.on_click(move |_, window, cx| {
                                on_select_parent(&target, window, cx)
                            })
                        })
                        .child(short_hash(&parent).to_string())
                }))
        });

        let meta = div()
            .flex()
            .flex_shrink_0()
            .flex_wrap()
            .items_center()
            .gap_x(px(18.))
            .gap_y(px(6.))
            .mt(px(12.))
            .text_size(px(13.))
            .child(person)
            .child(date)
            .children(parent_links);

        // Columna en la que solo el cuerpo cede alto: la fila del hash, el
        // título y la línea del autor no se comprimen.
        div()
            .id("commit-summary")
            .test_support()
            .flex()
            .flex_col()
            .flex_shrink(1.)
            .min_h_0()
            .overflow_hidden()
            .border_b_1()
            .border_color(palette.border_hair)
            .pt(px(14.))
            .pb(px(12.))
            .px(px(16.))
            .bg(palette.card)
            .child(top)
            .child(title)
            .children(body)
            .children(toggle)
            .child(meta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortens_the_hash_to_eight_characters() {
        assert_eq!(
            short_hash("b46d17dbf12a4565b5971f798f787e88c872502d"),
            "b46d17db"
        );
        assert_eq!(short_hash("abc"), "abc");
        assert_eq!(short_hash(""), "");
    }

    #[test]
    fn sets_the_trailers_apart_from_the_body() {
        let body = "Explica el cambio.\n\n- uno\n- dos\n\nCo-Authored-By: Claude <noreply@anthropic.com>\nSigned-off-by: Esteban <e@x.cl>\n";
        assert_eq!(
            split_trailers(body),
            MessageBody {
                text: "Explica el cambio.\n\n- uno\n- dos",
                trailers: "Co-Authored-By: Claude <noreply@anthropic.com>\nSigned-off-by: Esteban <e@x.cl>",
            }
        );
        // Un cuerpo que es solo trailers.
        assert_eq!(
            split_trailers("Co-authored-by: Ana <a@x.cl>"),
            MessageBody {
                text: "",
                trailers: "Co-authored-by: Ana <a@x.cl>",
            }
        );
        // Una continuación sangrada sigue siendo parte del trailer.
        assert_eq!(
            split_trailers("Texto.\n\nReviewed-by: Ana\n  y Luis\nFixes: #12").trailers,
            "Reviewed-by: Ana\n  y Luis\nFixes: #12"
        );
    }

    #[test]
    fn keeps_in_the_body_a_last_paragraph_that_only_looks_like_trailers() {
        for body in [
            "Texto.\n\nNota: esto no es un trailer",
            "Texto.\n\nCo-Authored-By: Claude\nY una línea normal",
            "Co-Authored-By: Claude\n\nTexto al final.",
            "Ver https://example.com/a: b",
        ] {
            let split = split_trailers(body);
            assert_eq!(split.trailers, "", "{body}");
            assert_eq!(split.text, body.trim(), "{body}");
        }
        assert_eq!(
            split_trailers(""),
            MessageBody {
                text: "",
                trailers: ""
            }
        );
    }

    #[test]
    fn splits_the_parents_of_a_merge() {
        assert_eq!(
            parent_hashes("27959da1111  aaaa2222"),
            ["27959da1111", "aaaa2222"]
        );
        assert!(parent_hashes("").is_empty());
        assert!(parent_hashes("   ").is_empty());
    }
}
