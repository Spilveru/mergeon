//! Capa de aplicación de Mergeon: el estado global y los flujos que en la app
//! Vue vivían en `src/stores`, `src/composables` y el `<script setup>` de
//! `App.vue`. No pinta nada: las vistas observan [`AppModel`] y llaman a sus
//! métodos. Guía para quien escriba vistas: `native/docs/app-api.md`.
//!
//! ```text
//! vistas ──observan y llaman──▶ AppModel ──hilos de fondo──▶ mergeon_backend::Backend
//!    ▲                              │
//!    └────── AppEvent / notify ─────┘        UiHost (avisos, confirmaciones, prompts)
//! ```
//!
//! # Módulos y su origen
//!
//! | Módulo | Origen |
//! |---|---|
//! | [`AppModel`], [`AppEvent`], [`backend_with_events`] | `App.vue` (`onMounted`, listeners), `electron/preload.js` (eventos) |
//! | [`git_client`] | `src/composables/useGit.js` |
//! | [`repository_sync`] | `src/composables/useRepositorySync.js` |
//! | [`repositories`] | `src/stores/repositories.js` |
//! | [`workspace`] | `src/components/repository/RepositoryWorkspace.vue` |
//! | [`shell`] | `<script setup>` de `App.vue`, `useSingleFlight.js` |
//! | [`flows`] | `useCheckoutFlow.js`, `useCreateBranchFlow.js` |
//! | [`github`] | `src/stores/github.js`, `useGitHubAuthFlow.js`, `useGitHubRepositories.js` |
//! | [`pull_requests`] | propio: PRs y checks de GitHub del repositorio (solo consulta) |
//! | [`settings`] | `useTheme.js`, `useAppSettings.js`, `useDiffPreferences.js`, `useSidebarWidth.js`, `useFileListWidth.js` |
//! | [`updater`] | `src/stores/updater.js`, `useUpdater.js` |
//! | [`summary`] | `useRepositorySummary.js` |
//! | [`tabs`] | propio: cerrar/reabrir/recorrer pestañas de repositorio, nombres sin ambigüedad, frescura al volver |
//! | [`history`] | carga del historial de `CommitView.vue` |
//! | [`ui_host`] | `src/lib/ui/notify.js`, `src/lib/ui/dialogs.js` (la parte que pinta) |
//! | [`testing`] | dobles para tests |
//!
//! # Convenciones
//!
//! - Cada acción de store, función de `useGit` y flujo de `App.vue` es un
//!   método de [`AppModel`] con el mismo nombre en snake_case. Donde dos
//!   stores tenían una acción con el mismo nombre se añade a quién pertenece
//!   (`load_repositories_from_storage`, `load_github_accounts_from_storage`,
//!   `initialize_github`).
//! - Git y disco nunca corren en el hilo de UI: los métodos lanzan el backend
//!   con `cx.background_spawn` y devuelven un `Task`.
//! - **Las acciones no se cancelan al soltar su `Task`.** Una mutación o un
//!   flujo, una vez lanzado, llega al final (como una promesa); el `Task` solo
//!   sirve para esperar el resultado. Las lecturas sí se abandonan al
//!   soltarlo.
//! - Los textos de los avisos, el orden de los pasos y las relecturas
//!   posteriores son los del original.
//!
//! # Una vista mínima
//!
//! Lista las ramas del repositorio activo y hace checkout al pulsar una. Es
//! el ejemplo de `native/docs/app-api.md`; se compila con los tests.
//!
//! ```no_run
//! use gpui_kit::component::button::Button;
//! use gpui_kit::prelude::*;
//! use gpui_kit::{Context, Entity, SharedString, Window, div};
//! use mergeon_app::{
//!     AppModel, CheckoutRequest, CheckoutTarget, FlowStatus, LocalChangesMode, SyncRequest,
//! };
//!
//! pub struct BranchList {
//!     model: Entity<AppModel>,
//! }
//!
//! impl BranchList {
//!     pub fn new(cx: &mut Context<Self>) -> Self {
//!         let model = AppModel::global(cx);
//!         // Volver a pintar cuando cambie el modelo.
//!         cx.observe(&model, |_, _, cx| cx.notify()).detach();
//!         Self { model }
//!     }
//!
//!     fn checkout(&mut self, branch: String, cx: &mut Context<Self>) {
//!         let model = self.model.clone();
//!         let Some(repo_id) = model.read(cx).active_repository().map(|repo| repo.id.clone())
//!         else {
//!             return;
//!         };
//!
//!         cx.spawn(async move |_, cx| {
//!             // El flujo aplica cada resultado al repositorio y muestra sus avisos.
//!             let outcome = model
//!                 .update(cx, |model, cx| {
//!                     model.run_checkout(
//!                         repo_id.as_str(),
//!                         CheckoutRequest {
//!                             label: branch.clone(),
//!                             target: CheckoutTarget::Reference(branch),
//!                             local_changes_mode: LocalChangesMode::Keep,
//!                         },
//!                         cx,
//!                     )
//!                 })
//!                 .await;
//!             if outcome.status == FlowStatus::Aborted {
//!                 return;
//!             }
//!
//!             // Qué releer lo decide la vista, como en Sidebar.vue.
//!             let needs_status = outcome
//!                 .result
//!                 .as_ref()
//!                 .is_none_or(|result| result.status().is_none());
//!             let mut request = SyncRequest::new()
//!                 .branches(true)
//!                 .status(needs_status)
//!                 .changes_count(needs_status)
//!                 .stashes(outcome.stashed_changes)
//!                 .operation("checkout");
//!             if let Some(result) = outcome.result {
//!                 request = request.result(result);
//!             }
//!             model
//!                 .update(cx, |model, cx| {
//!                     model.sync_repository_read_model(repo_id.as_str(), request, cx)
//!                 })
//!                 .await;
//!         })
//!         .detach();
//!     }
//! }
//!
//! impl Render for BranchList {
//!     fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
//!         let branches = self
//!             .model
//!             .read(cx)
//!             .active_repository()
//!             .map(|repo| repo.branches.clone())
//!             .unwrap_or_default();
//!
//!         div()
//!             .flex()
//!             .flex_col()
//!             .gap_1()
//!             .children(branches.into_iter().map(|branch| {
//!                 let name = branch.name.clone();
//!                 Button::new(SharedString::from(format!("checkout-{name}")))
//!                     .label(branch.name)
//!                     .on_click(cx.listener(move |this, _, _, cx| {
//!                         this.checkout(name.clone(), cx);
//!                     }))
//!             }))
//!     }
//! }
//! ```

pub mod clone_url;
mod convert;
pub mod credentials;
pub mod desktop_notifications;
pub mod local_paths;
mod model;
mod storage;
mod tasks;

pub mod flows;
pub mod git_client;
pub mod github;
pub mod history;
pub mod progress;
pub mod pull_requests;
pub mod repositories;
pub mod repository_sync;
pub mod settings;
pub mod shell;
pub mod startup;
pub mod summary;
pub mod tabs;
pub mod testing;
pub mod ui_host;
pub mod undo;
pub mod updater;
pub mod workspace;

pub use convert::StructuredOutcome;
pub use credentials::{CredentialsAnswer, CredentialsRequest};
pub use flows::{
    CheckoutOutcome, CheckoutRequest, CheckoutTarget, CreateBranchOutcome, CreateBranchRequest,
    FlowStatus, LocalChangesMode,
};
pub use git_client::{GitMutation, GitOperation};
pub use mergeon_model::repositories::RepoWorkspace;
pub use mergeon_store::{
    ACTION_DIALOGS_KEY, ActionDefaults, ActionDialogMode, ActionDialogs, FetchActionDefaults,
    PullActionDefaults, PushActionDefaults, RemoteAction,
};
pub use model::{
    AppEvent, AppModel, EventReceiver, OpenRepository, Repositories, backend_with_events,
    event_channel,
};
pub use pull_requests::{
    GitHubRemote, PullRequestCheckout, PullRequestItem, PullRequestsAvailability, RepoPullRequests,
};
pub use repositories::AddRepository;
pub use repository_sync::{SyncOutcome, SyncRequest};
pub use shell::{
    CHOOSE_EDITOR_ACTION, CreateBranchSubmission, DialogRequest, FetchRequest, MetadataRefresh,
    PullRequest, PushRequest, StashRequest, StatusSyncReason, WindowRequest,
};
pub use tabs::RepositoryTabLabel;
pub use ui_host::{Answer, UiHost};
pub use updater::AppUpdaterBridge;

// Lo que aparece en la API de este crate, para que una vista no tenga que
// depender de los crates de abajo solo por nombrar un tipo.
pub use mergeon_backend as backend;
pub use mergeon_model as model_types;

#[cfg(test)]
mod tests;
