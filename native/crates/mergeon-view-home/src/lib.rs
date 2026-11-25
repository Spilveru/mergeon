//! Home/Dashboard, lanzador, GitHub (cuentas y clone) y Settings.
//!
//! Port de `src/components/dashboard/Dashboard.vue`,
//! `src/components/layout/{RepositoryLauncher,SettingsView,UpdateSettings}.vue`,
//! `src/components/github/*.vue` y
//! `src/components/repository/{Preferences,Shortcuts,About}Dialog.vue`.
//! API pública y eventos: `native/docs/view-home.md`.
//!
//! ```ignore
//! mergeon_view_home::init(cx);
//! let home = cx.new(|cx| HomeView::new(window, cx));
//! let settings = cx.new(|cx| SettingsView::new(window, cx));
//!
//! // La cáscara reparte los diálogos que piden los flujos de `AppModel`:
//! cx.subscribe_in(&model, window, |_, _, event: &AppEvent, window, cx| {
//!     if let AppEvent::ShowDialog(request) = event {
//!         mergeon_view_home::handle_dialog_request(request, window, cx);
//!     }
//! });
//! ```

mod avatar;
mod clone_url;
mod credentials;
mod github_account;
mod github_accounts;
mod github_picker;
mod help;
mod home;
mod launcher;
mod parts;
mod path_field;
mod preferences;
mod readme;
mod settings;
mod shortcuts;

pub mod markdown;
pub mod util;

#[cfg(test)]
mod tests;

pub use avatar::{AvatarCache, AvatarFetcher, set_avatar_fetcher};
pub use clone_url::{CloneFromUrlDialog, open_clone_from_url_dialog};
pub use credentials::{CredentialsDialog, open_credentials_dialog};
pub use github_account::{GitHubAccountDialog, open_github_account_dialog};
pub use github_accounts::{
    AccountAction, GitHubAccountsEvent, GitHubAccountsManager, open_github_accounts_dialog,
};
pub use github_picker::{
    GitHubRepositoryPicker, filter_repositories, open_github_repository_picker,
};
pub use help::{
    RELEASES_PAGE, SHORTCUT_SECTIONS, ShortcutSection, about_panel, open_about_dialog,
    open_shortcuts_dialog, shortcuts_panel, update_action_disabled, update_action_label,
    update_settings, update_status_text,
};
pub use home::{
    HomeView, RepositoryEntry, RepositoryGroup, recent_entries, repository_groups, stat_rows,
};
pub use launcher::open_repository_launcher;
pub use path_field::{RepositoryPathField, open_repository_path_dialog};
pub use preferences::{
    PreferencesPanel, PreferencesSection, editor_options, open_preferences_dialog,
};
pub use settings::{SettingsEvent, SettingsSection, SettingsView};
pub use shortcuts::{SHORTCUT_RECORDER_CONTEXT, ShortcutsPanel, combo_from_keystroke};

use gpui_kit::{App, Window};
use mergeon_app::DialogRequest;

/// Prepara lo que las vistas de este crate comparten (la caché de avatares).
/// No registra acciones ni atajos: los globales los resuelve la cáscara con
/// `AppModel::handle_shortcut`.
pub fn init(cx: &mut App) {
    AvatarCache::global(cx);
}

/// Abre el diálogo de este crate que pide un `AppEvent::ShowDialog`: el
/// lanzador, el alta de cuenta de GitHub, el selector de repositorios o
/// "Clone from URL".
/// Devuelve `false` si la petición es de otro crate.
pub fn handle_dialog_request(request: &DialogRequest, window: &mut Window, cx: &mut App) -> bool {
    match request {
        DialogRequest::Launcher => {
            open_repository_launcher(window, cx);
        }
        DialogRequest::GitHubAccount => {
            open_github_account_dialog(window, cx);
        }
        DialogRequest::GitHubRepositoryPicker => {
            open_github_repository_picker(window, cx);
        }
        DialogRequest::CloneFromUrl => {
            open_clone_from_url_dialog(window, cx);
        }
        _ => return false,
    }
    true
}
