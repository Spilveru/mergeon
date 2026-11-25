//! Tests de la capa de aplicación: los de vitest de stores y composables,
//! portados sobre repositorios git reales en directorios temporales.
//!
//! Los backends de prueba no vigilan el disco ni lanzan procesos; los
//! eventos de la cola de mutaciones sí llegan desde sus hilos, por eso
//! `TestApp` permite al planificador de tests esperar a otros hilos.

mod support;

mod app;
mod checkout_flow;
mod create_branch_flow;
mod credentials;
mod desktop_notifications;
mod external_changes;
mod full_flow;
mod git_client;
mod git_client_smoke;
mod github_auth_flow;
mod github_repositories;
mod github_store;
mod history;
mod notifications;
mod pull_requests;
mod repo_workspaces;
mod repositories_store;
mod repository_sync;
mod settings;
mod shell;
mod summary;
mod tabs;
mod ui_host;
mod undo;
mod updater;
mod workspace;
