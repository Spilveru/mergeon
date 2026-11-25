//! Diálogo de detalles de un tag (el `AppDialog` de `Sidebar.vue` con la clase
//! `tag-details`): tipo, commit, autor, fecha y mensaje de la anotación.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::*;
use gpui_kit::{App, Context, Entity, SharedString, Window, div, px, relative, rems};
use mergeon_app::AppModel;
use mergeon_backend::TagDetails;
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};
use mergeon_ui::{ActivePaletteExt as _, MergeonStyled as _};

/// Contenido del diálogo: los detalles, o "Loading…" mientras llegan.
pub struct TagDetailsDialog {
    name: SharedString,
    details: Option<TagDetails>,
    loading: bool,
    dialog_id: Option<DialogId>,
}

impl TagDetailsDialog {
    pub fn name(&self) -> &SharedString {
        &self.name
    }

    pub fn details(&self) -> Option<&TagDetails> {
        self.details.as_ref()
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn is_open(&self) -> bool {
        self.dialog_id.is_some()
    }

    /// Cierra sin avisar a nadie (lo llama quien lo abrió: otro diálogo del
    /// mismo tipo o un cambio de repositorio).
    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.dialog_id.take() {
            dialog::close_id(window, cx, id);
        }
    }

    fn row(label: &'static str, value: impl IntoElement, cx: &App) -> impl IntoElement {
        div()
            .flex()
            .gap_3()
            .child(
                div()
                    .w(px(64.))
                    .flex_shrink_0()
                    .tw_text_xs()
                    .tw_font_medium()
                    .text_color(cx.palette().muted_foreground)
                    .child(label),
            )
            .child(div().min_w_0().child(value))
    }
}

/// `(tagDetails?.targetHash || '').slice(0, 10)`.
fn short_target(hash: &str) -> String {
    hash.chars().take(10).collect()
}

impl Render for TagDetailsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let body = div()
            .flex()
            .flex_col()
            .gap(rems(0.6))
            .tw_text_sm()
            .text_color(palette.foreground);

        if self.loading {
            return body.child(div().text_color(palette.muted_foreground).child("Loading…"));
        }

        let details = self.details.as_ref();
        let is_annotated = details.is_some_and(|details| details.is_annotated);
        let target = details.map_or_else(String::new, |details| short_target(&details.target_hash));
        let tagger = details.and_then(|details| {
            let name = details
                .tagger_name
                .as_deref()
                .filter(|name| !name.is_empty())?;
            Some(
                match details
                    .tagger_email
                    .as_deref()
                    .filter(|email| !email.is_empty())
                {
                    Some(email) => format!("{name} · {email}"),
                    None => name.to_string(),
                },
            )
        });
        let date = details
            .and_then(|details| details.tagger_date.clone())
            .filter(|date| !date.is_empty());
        let message = details
            .map(|details| details.message.clone())
            .filter(|message| !message.is_empty());

        body.child(Self::row(
            "Type",
            if is_annotated {
                "Annotated"
            } else {
                "Lightweight"
            },
            cx,
        ))
        .child(Self::row(
            "Target",
            div()
                .font_family(cx.theme().mono_font_family.clone())
                .child(target),
            cx,
        ))
        .when_some(tagger, |this, tagger| {
            this.child(Self::row("Tagger", tagger, cx))
        })
        .when_some(date, |this, date| this.child(Self::row("Date", date, cx)))
        .map(|this| match message {
            Some(message) => this.child(
                // `white-space: pre-wrap`: GPUI respeta los saltos de línea del texto.
                div()
                    .pt(rems(0.6))
                    .border_t_1()
                    .border_color(palette.border)
                    .line_height(relative(1.5))
                    .child(message),
            ),
            None if !is_annotated => this.child(
                div()
                    .text_color(palette.muted_foreground)
                    .child("Lightweight tags have no description."),
            ),
            None => this,
        })
    }
}

/// `handleViewTagDetails`: abre el diálogo con el nombre del tag y carga sus
/// detalles. Si la lectura falla (el modelo ya avisó) el diálogo se cierra.
pub fn open_tag_details_dialog(
    model: &Entity<AppModel>,
    repo_path: &str,
    tag: &str,
    window: &mut Window,
    cx: &mut App,
) -> Entity<TagDetailsDialog> {
    let view = cx.new(|_| TagDetailsDialog {
        name: SharedString::from(tag.to_string()),
        details: None,
        loading: true,
        dialog_id: None,
    });

    let id = AppDialog::new()
        .title(if tag.is_empty() {
            "Tag".to_string()
        } else {
            tag.to_string()
        })
        .width(DialogWidth::Lg)
        .body({
            let view = view.clone();
            move |_, _| view.clone()
        })
        .on_close({
            let view = view.clone();
            move |_, cx| view.update(cx, |view, _| view.dialog_id = None)
        })
        .open(window, cx);
    view.update(cx, |view, _| view.dialog_id = Some(id));

    let read = model.update(cx, |model, cx| model.get_tag_details(repo_path, tag, cx));
    let handle = window.window_handle();
    let dialog = view.downgrade();
    cx.spawn(async move |cx| {
        let result = read.await;
        let _ = handle.update(cx, |_, window, cx| {
            let Some(dialog) = dialog.upgrade() else {
                return;
            };
            dialog.update(cx, |dialog, cx| {
                dialog.loading = false;
                match result {
                    Ok(details) => dialog.details = Some(details),
                    Err(_) => {
                        if let Some(id) = dialog.dialog_id.take() {
                            dialog::close_id(window, cx, id);
                        }
                    }
                }
                cx.notify();
            });
        });
    })
    .detach();

    view
}

#[cfg(test)]
mod tests {
    use super::short_target;

    #[test]
    fn the_target_shows_ten_characters() {
        assert_eq!(
            short_target("0ec902a4f6d1b2c3d4e5f60718293a4b5c6d7e8f"),
            "0ec902a4f6"
        );
        assert_eq!(short_target(""), "");
    }
}
