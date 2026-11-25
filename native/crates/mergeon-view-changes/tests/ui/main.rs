//! Tests de interacción de la vista de cambios: ventanas sin pantalla con la
//! misma raíz que la app, clics y teclas reales, un `AppModel` sobre
//! repositorios git de verdad y una interfaz global que registra lo que se le
//! pide.
//!
//! Portan `ChangesView.test.js`, `RepositoryChangesSidebar.test.js`,
//! `RepositoryCommitPanel.test.js`, `ConflictResolver.test.js` y
//! `StashDiffDialog.test.js`. Lo que en ellos era texto calculado se prueba
//! además junto a cada módulo, sin ventana.

mod changes_actions;
mod changes_view;
mod commit_panel;
mod conflict_kinds;
mod conflict_resolver;
mod dialogs;
mod local_changes;
mod local_changes_ux;
mod sidebar;
mod stash_fixup;
mod support;
mod whole_file_images;
