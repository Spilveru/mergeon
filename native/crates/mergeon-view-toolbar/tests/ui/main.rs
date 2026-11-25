//! Tests de interacción de la barra, sus banners y sus diálogos: ventanas sin
//! pantalla con la misma raíz que la app, clics y teclas reales, un
//! `AppModel` sobre repositorios git de verdad y una interfaz global que
//! registra lo que se le pide.
//!
//! Portan `Toolbar.test.js`, `OperationBanner.test.js`,
//! `OperationStatusSlot.test.js`, `MutationQueueBanner.test.js`,
//! `OperationsLogDialog.test.js` y `ReflogDialog.test.js`. Lo que en ellos era
//! texto calculado (etiquetas, contadores, resultados) se prueba además junto
//! a cada módulo, sin ventana.

mod banners;
mod dialogs;
mod pull_requests;
mod repaints;
mod support;
mod toolbar;
