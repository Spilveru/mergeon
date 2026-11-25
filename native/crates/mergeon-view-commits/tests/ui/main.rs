//! Tests de interacción de la vista de commits: ventanas sin pantalla con la
//! misma raíz que la app, clics, teclas y arrastres reales, un `AppModel`
//! sobre repositorios git de verdad y una interfaz global que registra lo
//! que se le pide.
//!
//! Portan `CommitView.test.js`, `CommitHistoryList.test.js`,
//! `CommitInspector.test.js`, `CommitSummaryHeader.test.js`,
//! `CommitChangesTab.test.js`, `CommitTreeTab.test.js` e
//! `InteractiveRebaseDialog.test.js`, más la pestaña de archivo histórico.

mod changes_tab;
mod commit_view;
mod compare_view;
mod file_history;
mod historical_file_tab;
mod history_list;
mod inspector;
mod multi_select;
mod rebase_dialog;
mod support;
mod tree_tab;
