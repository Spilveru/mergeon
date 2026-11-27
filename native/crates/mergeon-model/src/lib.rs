//! Lógica pura de la interfaz de Mergeon (sin UI): diffs, parches, árbol de
//! archivos, grafo de commits, formatos, pestañas y el estado de los
//! repositorios abiertos.
//!
//! Es el port de lo que en la app Electron vivía en `src/lib`, `src/utils` y
//! las partes sin Vue ni IPC de `src/stores`, `src/composables` y
//! `commitViewUtils.js`. Un módulo por archivo de origen; nada aquí depende de
//! GPUI ni de ninguna librería de interfaz.
//!
//! | Módulo | Origen |
//! |---|---|
//! | [`diff`] | `src/lib/diff.js` |
//! | [`patch`] | `src/lib/patch.js` |
//! | [`file_tree`] | `src/lib/fileTree.js` |
//! | [`format`] | `src/lib/format.js` |
//! | [`operation_labels`] | `src/lib/operationLabels.js` |
//! | [`workspace_tabs`] | `src/lib/workspaceTabs.js` |
//! | [`status_badge`] | `src/lib/ui/statusBadge.js` |
//! | [`notify`] | `src/lib/ui/notify.js` (+ `notifyOperationResult` de `useGit.js`) |
//! | [`dialogs`] | `src/lib/ui/dialogs.js` |
//! | [`repository_read_model`] | `src/utils/repositoryReadModel.js` |
//! | [`repository_read_instrumentation`] | `src/utils/repositoryReadInstrumentation.js` |
//! | [`commit_view_utils`] | `src/components/repository/commitViewUtils.js` |
//! | [`repositories`] | `src/stores/repositories.js` |
//! | [`operations_log`] | `src/stores/operationsLog.js` |
//! | [`diff_preferences`] | `src/composables/useDiffPreferences.js` |
//! | [`sidebar_width`] | `src/composables/useSidebarWidth.js` |
//! | [`file_list_width`] | `src/composables/useFileListWidth.js` |
//! | [`keyboard_shortcuts`] | `src/composables/useKeyboardShortcuts.js` |
//! | [`fuzzy`], [`command_palette`], [`text_search`] | sin original: paleta de comandos y búsqueda en los visores |
//!
//! [`operation_types`] y [`resizable_width`] no tienen archivo de origen
//! propio: reúnen lo que compartían varios de los anteriores.

pub mod branch_tree;
pub mod collate;
pub mod command_palette;
pub mod fuzzy;
mod jsstr;
pub mod text_search;

pub mod commit_view_utils;
pub mod dialogs;
pub mod diff;
pub mod diff_preferences;
pub mod file_list_width;
pub mod file_tree;
pub mod format;
pub mod keyboard_shortcuts;
pub mod notify;
pub mod operation_labels;
pub mod operation_types;
pub mod operations_log;
pub mod patch;
pub mod repositories;
pub mod repository_read_instrumentation;
pub mod repository_read_model;
pub mod resizable_width;
pub mod sidebar_width;
pub mod status_badge;
pub mod workspace_tabs;
