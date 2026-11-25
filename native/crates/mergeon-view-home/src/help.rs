//! Lo que Settings muestra en Help / About: la ficha de la app
//! (`AboutDialog.vue`), el estado de las actualizaciones
//! (`UpdateSettings.vue`) y la chuleta de atajos (`ShortcutsDialog.vue`).

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px, relative, rems,
};
use mergeon_app::AppModel;
use mergeon_app::model_types::keyboard_shortcuts::combo_label;
use mergeon_ui::dialog::{AppDialog, DialogId, DialogWidth};
use mergeon_ui::{ActivePaletteExt as _, MergeonStyled as _, Radius};
use mergeon_updater::{ErrorPhase, UpdateStatus, UpdaterState};

use crate::parts::HomeDialogs;

/// La página de versiones que abre "Releases on GitHub".
pub const RELEASES_PAGE: &str = "https://github.com/Spilveru/mergeon/releases";

// ---------------------------------------------------------------------------
// About
// ---------------------------------------------------------------------------

/// `.about`: versión, descripción y enlace a las versiones publicadas.
pub fn about_panel(cx: &App) -> impl IntoElement + use<> {
    let palette = *cx.palette();
    let version = AppModel::global(cx)
        .read(cx)
        .updater_state()
        .current_version
        .clone()
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| "dev".to_string());

    div()
        .flex()
        .flex_col()
        .items_start()
        .gap(rems(0.6))
        .child(
            div()
                .id("about-version")
                .aria_label(format!("Version {version}"))
                .test_support()
                .text_size(rems(0.8125))
                .line_height(relative(1.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.foreground)
                .child(SharedString::from(format!("Version {version}"))),
        )
        .child(
            div()
                .text_size(rems(0.8125))
                .line_height(relative(1.5))
                .text_color(palette.muted_foreground)
                .child("Cross-platform desktop git client."),
        )
        .child(
            mergeon_ui::button("about-releases")
                .outline()
                .sm()
                .label("Releases on GitHub")
                .on_click(|_, _, cx| {
                    AppModel::global(cx)
                        .update(cx, |model, cx| model.open_external(RELEASES_PAGE, cx))
                        .detach();
                }),
        )
}

/// Abre la ficha "Mergeon" como diálogo.
pub fn open_about_dialog(window: &mut Window, cx: &mut App) -> DialogId {
    const TITLE: &str = "Mergeon";
    if let Some((id, _)) = HomeDialogs::open_dialog(TITLE, window, cx) {
        return id;
    }
    let id = AppDialog::new()
        .title(TITLE)
        .width(DialogWidth::Sm)
        .body(|_, cx| about_panel(cx))
        .on_close(|_, cx| HomeDialogs::forget(TITLE, cx))
        .open(window, cx);
    HomeDialogs::register(TITLE, id, None, cx);
    id
}

// ---------------------------------------------------------------------------
// Updates
// ---------------------------------------------------------------------------

/// `statusText` de `UpdateSettings.vue`.
pub fn update_status_text(updater: &UpdaterState) -> String {
    let latest = match updater
        .latest_version
        .as_deref()
        .filter(|version| !version.is_empty())
    {
        Some(version) => format!("v{version}"),
        None => "A new version".to_string(),
    };

    match updater.status {
        UpdateStatus::Checking => "Checking for updates…".to_string(),
        UpdateStatus::Available => format!("{latest} is available."),
        UpdateStatus::Downloading => format!("Downloading {latest}… {}%", updater.percent),
        UpdateStatus::Downloaded => {
            if updater.can_auto_install {
                format!("{latest} is ready to install.")
            } else {
                format!("{latest} is available to download.")
            }
        }
        UpdateStatus::NotAvailable => "You are up to date.".to_string(),
        UpdateStatus::NotConfigured => "Download updates from the releases page.".to_string(),
        UpdateStatus::Unsupported => {
            "Update checks are available in the packaged application.".to_string()
        }
        UpdateStatus::Error => {
            if updater.is_up_to_date() {
                format!("Latest is {latest}. Could not check again. Please retry.")
            } else if updater.error_phase == Some(ErrorPhase::Download) {
                "Could not download the update. Please retry.".to_string()
            } else {
                "Could not check for updates. Please retry.".to_string()
            }
        }
        UpdateStatus::Idle => "Check whether a newer version of Mergeon is available.".to_string(),
    }
}

/// `actionLabel` de `UpdateSettings.vue`.
pub fn update_action_label(updater: &UpdaterState) -> &'static str {
    match updater.status {
        UpdateStatus::Checking => "Checking…",
        UpdateStatus::Downloading => "Downloading…",
        _ if updater.is_ready() && updater.can_auto_install => "Restart and install",
        UpdateStatus::Available | UpdateStatus::Downloaded => "Download update",
        UpdateStatus::NotConfigured => "Open downloads",
        UpdateStatus::Error => "Retry",
        _ => "Check for updates",
    }
}

/// Si el botón de actualizaciones está bloqueado: mientras se comprueba o se
/// descarga, y donde no hay actualizador.
pub fn update_action_disabled(updater: &UpdaterState) -> bool {
    matches!(
        updater.status,
        UpdateStatus::Checking | UpdateStatus::Downloading | UpdateStatus::Unsupported
    )
}

/// `<section class="update-settings">`: el botón llama a
/// `AppModel::activate_update`.
pub fn update_settings(cx: &App) -> impl IntoElement + use<> {
    let palette = *cx.palette();
    let updater = AppModel::global(cx).read(cx).updater_state().clone();
    let percent = f32::from(updater.percent.min(100)) / 100.;

    div()
        .flex()
        .flex_col()
        .items_start()
        .gap(px(12.))
        .child(
            div()
                .text_size(px(16.))
                .line_height(relative(1.5))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Updates"),
        )
        .child(
            div()
                .id("update-status")
                .role(Role::Status)
                .aria_label(update_status_text(&updater))
                .test_support()
                .text_size(px(14.))
                .line_height(relative(1.5))
                .text_color(palette.muted_foreground)
                .child(SharedString::from(update_status_text(&updater))),
        )
        .when(updater.status == UpdateStatus::Downloading, |this| {
            this.child(
                // `<progress>` con `width: 100%` (el ancho de la sección, que se
                // ajusta a su contenido) y `accent-color: var(--primary)`.
                div()
                    .id("update-progress")
                    .test_support()
                    .w_full()
                    .h(rems(0.875))
                    .overflow_hidden()
                    .bg(palette.muted)
                    .child(div().h_full().w(relative(percent)).bg(palette.primary)),
            )
        })
        .child(
            mergeon_ui::button("update-action")
                .outline()
                .disabled(update_action_disabled(&updater))
                .label(update_action_label(&updater))
                .on_click(|_, _, cx| {
                    AppModel::global(cx)
                        .update(cx, |model, cx| model.activate_update(cx))
                        .detach();
                }),
        )
}

// ---------------------------------------------------------------------------
// Shortcuts
// ---------------------------------------------------------------------------

/// Un grupo de atajos de la chuleta.
pub struct ShortcutSection {
    pub title: &'static str,
    /// Etiqueta y combinación (`mod+shift+f`).
    pub items: &'static [(&'static str, &'static str)],
}

/// `SECTIONS` de `ShortcutsDialog.vue`.
pub const SHORTCUT_SECTIONS: [ShortcutSection; 5] = [
    ShortcutSection {
        title: "Repository",
        items: &[
            ("Open repository", "mod+o"),
            ("Clone from GitHub", "mod+shift+c"),
            ("Clone from URL", "mod+shift+o"),
            ("Fetch", "mod+shift+f"),
            ("Pull", "mod+alt+p"),
            ("Push", "mod+shift+u"),
            ("New branch", "mod+shift+n"),
            ("Reflog", "mod+shift+r"),
            ("Open in editor", "mod+shift+e"),
        ],
    },
    ShortcutSection {
        title: "Navigation",
        items: &[
            ("Command palette", "mod+shift+p"),
            ("Commits view", "alt+1"),
            ("Changes view", "alt+2"),
            ("Search commits", "mod+f"),
            ("Search box in the toolbar", "mod+k"),
            ("Previous / next commit", "↑ / ↓"),
            ("Operation log", "mod+shift+l"),
            ("Next repository tab", "ctrl+tab"),
            ("Previous repository tab", "ctrl+shift+tab"),
            ("Repository tab N (9 = last)", "mod+1…9"),
            ("Next document", "alt+right"),
            ("Previous document", "alt+left"),
            ("Close document", "mod+w"),
            ("Close repository", "mod+shift+w"),
            ("Reopen closed repository", "mod+shift+t"),
            ("Reload application", "mod+r"),
            ("Minimize window", "mod+m"),
            ("Exit Mergeon", "mod+q"),
        ],
    },
    ShortcutSection {
        title: "Editing",
        items: &[
            ("Undo last Git operation", "mod+z"),
            ("Redo Git operation", "mod+shift+z"),
            ("Redo Git operation (alternative)", "mod+y"),
            ("Commit staged changes", "mod+enter"),
            ("Preferences", "mod+,"),
            ("This cheatsheet", "mod+/"),
        ],
    },
    ShortcutSection {
        title: "Local Changes",
        items: &[
            ("Stage / unstage file", "space"),
            ("Discard file", "d"),
            ("Previous / next file", "↑ / ↓"),
            ("Stage selected lines (diff)", "s"),
            ("Unstage selected lines (diff)", "u"),
            ("Discard selected lines (diff)", "d"),
        ],
    },
    ShortcutSection {
        title: "Diff and file viewer",
        items: &[
            ("Find in the diff or file", "mod+f"),
            ("Next match", "enter"),
            ("Previous match", "shift+enter"),
            ("Next change", "f7"),
            ("Previous change", "shift+f7"),
        ],
    },
];

/// `.shortcuts`: la chuleta de atajos.
pub fn shortcuts_panel(cx: &App) -> impl IntoElement + use<> {
    let palette = *cx.palette();
    let is_mac = cfg!(target_os = "macos");

    let sections: Vec<AnyElement> = SHORTCUT_SECTIONS
        .iter()
        .map(|section| {
            let rows: Vec<AnyElement> = section
                .items
                .iter()
                .map(|(label, combo)| {
                    // `.shortcuts__row`
                    div()
                        .id(*label)
                        .test_support()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(rems(0.75))
                        .px(rems(0.35))
                        .py(rems(0.25))
                        .radius(Radius::Md, cx)
                        .hover(|style| style.bg(palette.sidebar_row_hover))
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(rems(0.8125))
                                .line_height(relative(1.5))
                                .text_color(palette.foreground)
                                .child(*label),
                        )
                        .child(mergeon_ui::kbd(combo_label(combo, is_mac)))
                        .into_any_element()
                })
                .collect();

            // `.shortcuts__section`
            div()
                .flex()
                .flex_col()
                .gap(rems(0.3))
                .child(
                    div()
                        .mb(rems(0.15))
                        .text_size(rems(0.75))
                        .line_height(relative(1.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(palette.muted_foreground)
                        .child(section.title),
                )
                .children(rows)
                .into_any_element()
        })
        .collect();

    // Dos columnas con más o menos las mismas filas, que se apilan cuando no
    // caben (ventana estrecha). Antes era una sola columna angosta.
    let split = column_split(&SHORTCUT_SECTIONS);
    let mut sections = sections.into_iter();
    let left: Vec<AnyElement> = sections.by_ref().take(split).collect();
    let right: Vec<AnyElement> = sections.collect();
    let column = |id: &'static str, sections: Vec<AnyElement>| {
        div()
            .id(id)
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(260.))
            .gap(rems(1.))
            .children(sections)
    };
    div()
        .id("shortcuts-columns")
        .w_full()
        .flex()
        .flex_wrap()
        .gap_x(rems(2.))
        .gap_y(rems(1.))
        .child(column("shortcuts-column-left", left))
        .child(column("shortcuts-column-right", right))
}

/// Cuántas secciones van en la columna izquierda: las primeras hasta llegar
/// a la mitad de las filas (contando el título de cada sección).
fn column_split(sections: &[ShortcutSection]) -> usize {
    let rows = |section: &ShortcutSection| section.items.len() + 1;
    let total: usize = sections.iter().map(rows).sum();
    let mut acc = 0;
    for (index, section) in sections.iter().enumerate() {
        if acc > 0 && acc + rows(section) / 2 > total / 2 {
            return index;
        }
        acc += rows(section);
    }
    sections.len()
}

/// Abre la chuleta "Keyboard shortcuts" como diálogo.
pub fn open_shortcuts_dialog(window: &mut Window, cx: &mut App) -> DialogId {
    const TITLE: &str = "Keyboard shortcuts";
    if let Some((id, _)) = HomeDialogs::open_dialog(TITLE, window, cx) {
        return id;
    }
    let id = AppDialog::new()
        .title(TITLE)
        .width(DialogWidth::Lg)
        .body(|_, cx| shortcuts_panel(cx))
        .on_close(|_, cx| HomeDialogs::forget(TITLE, cx))
        .open(window, cx);
    HomeDialogs::register(TITLE, id, None, cx);
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(status: UpdateStatus) -> UpdaterState {
        UpdaterState {
            status,
            ..UpdaterState::default()
        }
    }

    /// `UpdateSettings.test.js`: tras fallar una comprobación posterior, el
    /// mensaje sigue diciendo cuál es la última versión y deja reintentar.
    #[test]
    fn a_failed_follow_up_check_keeps_the_latest_version_message() {
        let updater = UpdaterState {
            current_version: Some("0.7.0".to_string()),
            latest_version: Some("0.7.0".to_string()),
            error_phase: Some(ErrorPhase::Check),
            ..state(UpdateStatus::Error)
        };

        assert!(update_status_text(&updater).contains("Latest is v0.7.0. Could not check again."));
        assert_eq!(update_action_label(&updater), "Retry");
        assert!(!update_action_disabled(&updater));
    }

    /// `UpdateSettings.test.js`: mientras descarga no hay segunda acción;
    /// descargada y auto-instalable ofrece instalar.
    #[test]
    fn downloading_blocks_the_action_and_a_ready_update_offers_to_install() {
        let downloading = UpdaterState {
            percent: 42,
            latest_version: Some("0.8.0".to_string()),
            ..state(UpdateStatus::Downloading)
        };
        assert_eq!(update_status_text(&downloading), "Downloading v0.8.0… 42%");
        assert_eq!(update_action_label(&downloading), "Downloading…");
        assert!(update_action_disabled(&downloading));

        let ready = UpdaterState {
            latest_version: Some("0.8.0".to_string()),
            can_auto_install: true,
            ..state(UpdateStatus::Downloaded)
        };
        assert_eq!(update_status_text(&ready), "v0.8.0 is ready to install.");
        assert_eq!(update_action_label(&ready), "Restart and install");
        assert!(!update_action_disabled(&ready));
    }

    /// `UpdateSettings.test.js`: sin instalación automática se ofrece la
    /// descarga manual.
    #[test]
    fn platforms_without_auto_install_offer_a_manual_download() {
        let downloaded = UpdaterState {
            latest_version: Some("0.8.0".to_string()),
            can_auto_install: false,
            ..state(UpdateStatus::Downloaded)
        };

        assert_eq!(
            update_status_text(&downloaded),
            "v0.8.0 is available to download."
        );
        assert_eq!(update_action_label(&downloaded), "Download update");
        assert!(!update_action_disabled(&downloaded));
    }

    #[test]
    fn every_status_has_its_copy() {
        let cases = [
            (
                UpdateStatus::Idle,
                "Check whether a newer version of Mergeon is available.",
                "Check for updates",
                false,
            ),
            (
                UpdateStatus::Checking,
                "Checking for updates…",
                "Checking…",
                true,
            ),
            (
                UpdateStatus::Available,
                "A new version is available.",
                "Download update",
                false,
            ),
            (
                UpdateStatus::NotAvailable,
                "You are up to date.",
                "Check for updates",
                false,
            ),
            (
                UpdateStatus::NotConfigured,
                "Download updates from the releases page.",
                "Open downloads",
                false,
            ),
            (
                UpdateStatus::Unsupported,
                "Update checks are available in the packaged application.",
                "Check for updates",
                true,
            ),
            (
                UpdateStatus::Error,
                "Could not check for updates. Please retry.",
                "Retry",
                false,
            ),
        ];
        for (status, text, label, disabled) in cases {
            let updater = state(status);
            assert_eq!(update_status_text(&updater), text, "{status:?}");
            assert_eq!(update_action_label(&updater), label, "{status:?}");
            assert_eq!(update_action_disabled(&updater), disabled, "{status:?}");
        }

        let download_error = UpdaterState {
            error_phase: Some(ErrorPhase::Download),
            ..state(UpdateStatus::Error)
        };
        assert_eq!(
            update_status_text(&download_error),
            "Could not download the update. Please retry."
        );
    }

    #[test]
    fn the_cheatsheet_splits_into_two_balanced_columns() {
        let split = column_split(&SHORTCUT_SECTIONS);
        assert!(split > 0 && split < SHORTCUT_SECTIONS.len());
        let rows = |sections: &[ShortcutSection]| {
            sections
                .iter()
                .map(|section| section.items.len() + 1)
                .sum::<usize>()
        };
        let (left, right) = (
            rows(&SHORTCUT_SECTIONS[..split]),
            rows(&SHORTCUT_SECTIONS[split..]),
        );
        let largest = SHORTCUT_SECTIONS
            .iter()
            .map(|section| section.items.len() + 1)
            .max()
            .unwrap();
        assert!(left.abs_diff(right) <= largest, "{left} / {right}");
    }

    #[test]
    fn the_cheatsheet_lists_every_shortcut_once() {
        let labels: Vec<&str> = SHORTCUT_SECTIONS
            .iter()
            .flat_map(|section| section.items.iter().map(|(label, _)| *label))
            .collect();
        assert_eq!(labels.len(), 44);
        let mut unique = labels.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), labels.len());
        assert_eq!(combo_label("mod+shift+f", false), "Ctrl+Shift+F");
    }
}
