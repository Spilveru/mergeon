# `mergeon-view-toolbar`

Barra de herramientas del repositorio activo, franja de la operación de
secuencia, cola de mutaciones y los diálogos de fetch, pull, push, nueva rama,
log de operaciones y reflog. Convenciones generales en [`ui-port.md`](ui-port.md).

| Vue (`src/components/`) | Rust (`crates/mergeon-view-toolbar/src/`) |
|---|---|
| `layout/Toolbar.vue` | `toolbar.rs` → `ToolbarView` |
| `layout/OperationIndicator.vue` | `operation_indicator.rs` → `OperationIndicator` |
| `repository/OperationStatusSlot.vue` | `operation_status_slot.rs` → `OperationStatusSlot` |
| `repository/OperationBanner.vue` | `operation_banner.rs` → `OperationBanner`, `SequenceAction` |
| `repository/MutationQueueBanner.vue` | `mutation_queue_banner.rs` → `MutationQueueBanner` |
| `repository/FetchDialog.vue`, `PullDialog.vue`, `PushDialog.vue` + `git-action-dialog.css` | `dialogs/{fetch,pull,push}.rs` → `open_fetch_dialog`, `open_pull_dialog`, `open_push_dialog` |
| `repository/CreateBranchDialog.vue` | `dialogs/create_branch.rs` → `open_create_branch_dialog`, `CreateBranchDialogOptions` |
| `repository/OperationsLogDialog.vue` | `dialogs/operations_log.rs` → `open_operations_log` (lista virtual `list()`: filas escritas al llegar las entradas; el diálogo solo se repinta si el log cambió) |
| `repository/ReflogDialog.vue` | `dialogs/reflog.rs` → `open_reflog` (lista virtual `uniform_list`, fechas escritas al leer el reflog) |

## Montaje en la cáscara

```rust
use mergeon_app::{AppEvent, AppModel};
use mergeon_view_toolbar::{OperationStatusSlot, ToolbarView};

mergeon_view_toolbar::init(cx);                       // no registra nada; por uniformidad

// Vistas (una por ventana; siguen solas al repositorio activo):
let toolbar = cx.new(|cx| ToolbarView::new(window, cx));
let status = cx.new(|cx| OperationStatusSlot::new(window, cx));

// Columna del contenido, como en `App.vue`:
div().flex().flex_col()
    .child(toolbar.clone())                           // nada si no hay repositorio activo
    .child(status.clone())                            // nada si no hay operación de secuencia
    .child(workspace);

// Los diálogos que piden los flujos de `AppModel`:
cx.subscribe_in(&AppModel::global(cx), window, |_, _, event: &AppEvent, window, cx| {
    if let AppEvent::ShowDialog(request) = event {
        mergeon_view_toolbar::handle_dialog_request(request, window, cx);
    }
})
```

Las dos vistas observan `AppModel` y no guardan datos del repositorio. Hace
falta un `UiHost` instalado (`AppModel::set_ui_host`): la
confirmación de Abort y el prompt / confirm del reflog se piden por él.

## API pública

### Vistas

| Elemento | Qué es |
|---|---|
| `ToolbarView::new(window, cx)` | Vista (`Entity`). Barra del repositorio activo (Señal): repositorio · rama con ↑↓ · `Fetch \| Pull \| Push` segmentado con contadores (behind gris en Pull, ahead naranja en Push) · Stash · `Undo \| Redo` (íconos en un `.seg`; texto de ayuda "Undo commit 'Fix login' (Ctrl+Z)" o "Nothing to undo (Ctrl+Z)", apagados sin nada en la pila, con uno en curso o con una operación a medias; llaman a `AppModel::undo` / `redo`) · buscador `Search commits, branches, authors…` (Ctrl K / ⌘K) · botones de ícono. `is_compact()` dice si está en su disposición de dos filas (1040px o menos). |
| `StatusBarView::new(window, cx)` | Vista (`Entity`). Barra de estado inferior de 24px: repositorio, rama con ↑↓ y upstream, "fetched N min ago" (hora del último fetch/pull/push terminado, recogida del log de operaciones y guardada en memoria por repositorio), "<pos> / <total> commits" (lo da la cáscara con `set_history(Option<HistoryPosition>)`) y `git --version`. Nada sin repositorio activo. |
| `PullRequestIndicator::new(cx)` | Vista (`Entity`) que crea y pinta `StatusBarView` (`status_bar.pull_request_indicator()`) tras "fetched…": el PR de la rama actual en GitHub, `⎇ #123 · ✓ 5/5 checks` (ícono y color de estado: ✓ éxito, ✗ alguno falla, ● pendientes), tooltip con título, checks y revisión, y clic que abre el PR. Nada sin PR o sin cuenta. Pide `AppModel::watch_pull_requests`. `label(cx)` da el texto sin íconos. |
| `OperationStatusSlot::new(window, cx)` | Vista (`Entity`). Franja con el `OperationBanner` compacto de la operación de secuencia en curso; sus botones llaman a `AppModel::handle_sequence_action`. |

Como en el original, la franja ya no incluye la cola de mutaciones: vive en el
indicador de la barra (`OperationIndicator`), y `MutationQueueBanner` queda
como pieza suelta.

### Piezas sin estado (`RenderOnce`)

```rust
OperationBanner::new(operation_state, Some(conflict_state))   // Option<OperationState>, Option<ConflictState>
    .compact(true)                       // sin bordes laterales ni radio
    .show_actions(true)                  // por defecto sí
    .busy_action(Some(SequenceAction::Continue))   // ese botón se bloquea y dice "Working…"
    .on_sequence_action(|action: SequenceAction, window, cx| { /* ... */ })

MutationQueueBanner::new(queue)          // mergeon_model::repositories::MutationQueue
    .framed(true)                        // fondo --card y borde inferior (por defecto)
    .separated(false)                    // línea superior, tras otra sección
    .on_cancel(|operation_id: &str, window, cx| { /* ... */ })

OperationIndicator::new(operation_state, Some(conflict_state), queue)
    .on_cancel(|operation_id: &str, window, cx| { /* ... */ })
```

- `SequenceAction { Continue, Skip, Abort }`, con `as_str()` (el nombre que
  entiende `handle_sequence_action`) y `parse(&str)`.
- Funciones puras: `operation_banner_title`, `operation_banner_detail`,
  `queue_operation_count`, `queue_phase_label`, `toolbar_branch_label`,
  `toolbar_head_hash`, `toolbar_head_title`, `sync_counts_label`,
  `search_shortcut_keys`, `undo_tooltip`, `history_position_label`, `fetched_ago_label`,
  `git_version_label`, `last_remote_sync`.

### Diálogos

Todos se abren sobre `mergeon_ui::dialog` y devuelven `Option<DialogId>`:
`None` si ya había uno igual abierto (en la app Vue eran un `v-if` sobre una
bandera: pedirlos dos veces no los duplicaba).

| Función | Al enviar |
|---|---|
| `open_fetch_dialog(window, cx)` | `AppModel::handle_fetch_submit(FetchRequest { remote, all_remotes })` |
| `open_pull_dialog(window, cx)` | `AppModel::handle_pull_submit(PullRequest { remote, branch, rebase, autostash })` |
| `open_push_dialog(window, cx)` | `AppModel::handle_push_submit(PushRequest { branch, destination, push_tags, force })` |
| `open_create_branch_dialog(options, window, cx)` | `options.on_submit(..)` o, sin él, `AppModel::handle_create_branch_submit(CreateBranchSubmission { .. })` |
| `open_operations_log(window, cx)` | — (al abrir: `mark_operations_seen`; "Clear": `clear_operations_log`) |
| `open_reflog(window, cx)` | — (lee con `get_reflog(path, { max_count: 100 })`) |
| `open_stash_dialog(paths, window, cx)` | `AppModel::handle_stash_submit(StashRequest { message, paths, include_untracked, keep_index, staged_only })` |
| `handle_dialog_request(&DialogRequest, window, cx) -> bool` | Abre el que corresponda; `false` para `Launcher`, `GitHubAccount` y `GitHubRepositoryPicker`. |

```rust
// Para la barra lateral y la vista de commits ("Create branch here…"), que en
// Vue montaban su propio CreateBranchDialog:
open_create_branch_dialog(
    CreateBranchDialogOptions::new("v1.1.0")          // lo que se lee en "Create branch at:"
        .has_local_changes(true)                      // habilita "Local changes"
        .suggested_name("hotfix/1.1.1")
        .on_submit(|submission: CreateBranchSubmission, window, cx| { /* submit_create_branch(...) */ }),
    window,
    cx,
);
```

**Stash** ("Stash changes"; con rutas, "Stash selected files"): mensaje
opcional, un resumen de lo que se guardará y tres casillas: "Include untracked
files", "Keep staged changes in the index" y "Only staged changes"
(`git stash push [-m] [--include-untracked] [--keep-index] [--staged] [rutas]`).
El resumen (`stash_summary(files, paths, choice)`, puro y probado) lista los
6 primeros archivos con su letra de estado y qué parte se guarda ("staged +
unstaged", "staged part only", "untracked"…), "and N more", y qué se queda
fuera ("1 untracked file stays in the working tree."). Se calcula al abrir, al
cambiar una opción o el estado del repositorio, nunca al pintar. Combinaciones
que git no admite se apagan: "Only staged changes" desmarca y apaga las otras
dos (`--staged` no va con `--include-untracked` ni tiene sentido con
`--keep-index`) y se apaga con git < 2.35 (la versión se lee una vez, en
segundo plano, con `mergeon_git_ops::stash_capabilities`); una ruta elegida
sin seguimiento marca y fija "Include untracked files". Un renombre o un
borrado preparados no se pueden guardar por ruta (git no los encuentra en el
índice): el diálogo lo dice (`stash-blocked`) y no deja enviar. "Stash" se
apaga si no hay nada que guardar. Enter en el mensaje envía.

El reflog llama directamente a `AppModel`: "Create branch here" → `prompt` +
`create_branch(path, name, Some(full_hash), false)` + relectura de ramas;
"Reset current branch here (hard)" → `confirm` destructivo + `reset(path,
full_hash, "hard")` + `apply_repository_operation_result` + relectura con
`operation("external-head-change")`, y cierra el diálogo.

## Eventos

El crate **no emite eventos propios**. Todo va por `AppModel`:

| Control | Llamada |
|---|---|
| Fetch / Pull / Push | `handle_fetch` / `handle_pull` / `handle_push` → `AppEvent::ShowDialog(Fetch \| Pull \| Push)` |
| Stash | `handle_stash` → `AppEvent::ShowDialog(Stash { paths: [] })` |
| Buscador | `handle_shortcut(ShortcutAction::SearchCommits)` (la búsqueda de commits del panel lateral, como Ctrl+F / Ctrl+K) |
| Reflog | `handle_shortcut(ShortcutAction::ShowReflog)` → `AppEvent::ShowDialog(Reflog)` |
| New Branch | `handle_new_branch` → `AppEvent::ShowDialog(CreateBranch { .. })` |
| Open in Editor / Open Terminal / Show in Folder | `handle_open_in_editor` / `handle_open_terminal` / `handle_open_explorer` |
| Cancel / Remove del popover | `handle_cancel_operation(operation_id)` |
| Continue / Skip / Abort | `handle_sequence_action("continue" \| "skip" \| "abort")` |

Lo que lee para pintar: `active_repository()` (`name`, `current_branch`,
`detached_head_ref`, `head_short_hash`, `repo_lifecycle_state`,
`operation_state`, `conflict_state`, `mutation_queue`),
`busy_toolbar_operation()`, `busy_sequence_action()`, `default_remote_name()`,
`default_remote_branch_name()`, `active_repo_current_branch()`,
`active_repo_has_local_changes()`, `repositories().active_repository_remotes()
/ _branches()` y `operations_log()`.

## Identificadores (tests e inspección)

Los elementos con comportamiento o texto calculado llevan `ElementId` estable
y etiqueta accesible (`aria_label`), que es lo que consultan los tests:

- Barra (40px en una fila): `toolbar`, `toolbar-fetch`, `toolbar-pull`,
  `toolbar-push`, `toolbar-stash`, `toolbar-reflog`, `toolbar-new-branch`,
  `toolbar-open-terminal`, `toolbar-open-explorer`, `toolbar-branch-button`
  (botón de la rama: emite `ToolbarEvent::OpenBranchPicker`), `toolbar-branch`,
  `toolbar-head`, `toolbar-pull-count`, `toolbar-push-count`,
  `toolbar-search`. El nombre del repositorio y la ventaja/retraso de la rama
  ya no se repiten (están en la pestaña y en las insignias de Pull/Push).
- Barra de estado: `status-bar`, `status-upstream`, `status-fetched` (clic:
  Fetch), `status-pull-request`, `status-history`. Sin repositorio, rama ni
  versión de git (ya están a la vista o en About).
- Actividad: `operation-indicator`, `operation-popover`, `mutation-queue`,
  `queue-row-<operationId>`, `queue-cancel-<operationId>`,
  `queue-remove-<operationId>`.
- Secuencia: `operation-status-slot`, `operation-banner`, `sequence-continue`,
  `sequence-skip`, `sequence-abort`.
- Diálogos: `fetch-remote`, `fetch-all-remotes`, `fetch-cancel`,
  `fetch-submit`; `pull-remote`, `pull-branch`, `pull-rebase`,
  `pull-autostash`, `pull-cancel`, `pull-submit`; `push-branch`,
  `push-destination`, `push-tags`, `push-force`, `push-cancel`, `push-submit`;
  `create-branch-name-label`, `create-branch-checkout`, `create-branch-local-changes` (`radio-keep`,
  `radio-stash`, `radio-discard`), `create-branch-cancel`,
  `create-branch-submit`; `ops-log-only-errors`, `ops-log-clear`,
  `ops-log-empty`, `ops-log-list`, `ops-log-row-<operationId>`,
  `ops-log-message-<operationId>`; `stash-summary` (etiqueta: "N files will
  be stashed:"), `stash-summary:<ruta>` ("<ruta> (<parte>)"), `stash-blocked`,
  `stash-include-untracked`, `stash-keep-index`, `stash-staged-only`,
  `stash-cancel`, `stash-submit`; `reflog-state`, `reflog-list`,
  `reflog-row-<selector>`, `reflog-branch-<selector>`,
  `reflog-reset-<selector>`.

## Galería y capturas

```bash
cd native
cargo build -p mergeon-view-toolbar --example toolbar_gallery
MERGEON_THEME=light MERGEON_TOOLBAR_STATE=merge cargo run -p mergeon-view-toolbar --example toolbar_gallery
crates/mergeon-view-toolbar/shots.sh [dark|light|all] [filtro]   # → target-shots/toolbar-<caso>-<tema>.png
```

La galería arranca un `AppModel` real (backend con configuración en memoria,
`mergeon_app::testing::TestBackend`) sobre el repositorio de
`tools/make-fixture.sh` en un temporal, instala un `UiHost` con los diálogos y
toasts de `mergeon_ui` y delega los diálogos como lo hará la cáscara. La barra
queda donde la pone la app (a la derecha de un hueco de 48px y bajo otro de
36px), así que las capturas se superponen con las de `target-shots/reference/`.

| Variable | Valores |
|---|---|
| `MERGEON_THEME` | `dark` (defecto) · `light` · `system` |
| `MERGEON_TOOLBAR_STATE` | lista: `merge`, `merge-partial`, `merge-ready`, `rebase`, `cherry-pick`, `revert`, `detached`, `empty`, `none`, `push-running`, `queue`, `log` |
| `MERGEON_TOOLBAR_OPEN` | lista: `fetch`, `pull`, `push`, `create-branch`, `new-branch`, `stash`, `operations-log`, `reflog` |
| `MERGEON_TOOLBAR_INPUT` | pasos separados por `;`: `key:enter`, `type:texto`, `focus:n`, `move:x,y`, `click:x,y`, `wait:ms` |
| `MERGEON_TOOLBAR_WIDTH` / `_HEIGHT` | tamaño de la ventana (1440×900) |
| `MERGEON_FIXTURE` | directorio ya creado con `make-fixture.sh` |

Capturas (`native/target-shots/toolbar-<caso>-{dark,light}.png`) y su
referencia:

| Caso | Referencia |
|---|---|
| `default` | `201-commits-history`, `601-shell-tabs-toolbar-ahead-behind` |
| `detached`, `empty-repository`, `no-repository` | `604-detached-head`, `606-empty-repository` |
| `head-tooltip` | `611-tooltip-toolbar-head` |
| `compact` | — (disposición de menos de 900px) |
| `merge`, `merge-popover`, `merge-partial`, `merge-ready` | `501`, `502`, `507`, `508` |
| `rebase`, `cherry-pick`, `revert`, `confirm-abort` | `512`, `514`, `515`, `509` |
| `push-running`, `queue`, `queue-popover` | `520`, `521`, `522` |
| `dialog-fetch`, `dialog-fetch-single-remote` | `401`, `402` |
| `dialog-pull`, `dialog-push`, `dialog-push-branch-select` | `403`, `404`, `405` |
| `prompt-create-stash` | `406` |
| `dialog-create-branch`, `-filled`, `-clean` | `407`, `408` |
| `dialog-operations-log-empty`, `dialog-operations-log`, `-expanded`, `-only-errors` | `400`, `541`, `542`, `543` |
| `dialog-reflog`, `confirm-reflog-reset`, `prompt-reflog-create-branch` | `449`, `450`, `451` |

## Tests

`cargo test -p mergeon-view-toolbar`: 28 unitarios (texto calculado, junto a
cada módulo) y 55 de interfaz en `tests/ui/` (ventana sin pantalla, clics y
teclas reales, `AppModel` sobre repositorios git de verdad y
`RecordingUiHost`).

| Vitest | Dónde |
|---|---|
| `Toolbar.test.js` (4) | `tests/ui/toolbar.rs` + `toolbar.rs::tests` |
| `OperationBanner.test.js` (3) | `tests/ui/banners.rs` + `operation_banner.rs::tests` |
| `OperationStatusSlot.test.js` (2) | `tests/ui/banners.rs` |
| `MutationQueueBanner.test.js` (1) | `tests/ui/banners.rs` |
| `OperationsLogDialog.test.js` (3) | `tests/ui/dialogs.rs` |
| `ReflogDialog.test.js` (4) | `tests/ui/dialogs.rs` |
| — (propio) | `tests/ui/pull_requests.rs` (2) + `pull_request_indicator.rs::tests`: indicador del PR con GitHub guionizado (`TestApp::with_backend`) y clic que abre el PR; sin PR no se pinta |

El diálogo de stash (`tests/ui/dialogs.rs`, sección Stash, y
`dialogs/stash.rs::tests`): resumen y casillas, "Only staged changes" apaga las
otras, rutas elegidas con un archivo sin seguimiento, cancelar; el botón abre
el diálogo una sola vez y se bloquea mientras corre un stash
(`tests/ui/toolbar.rs`).

`PushDialog`, `PullDialog`, `FetchDialog`, `CreateBranchDialog` y
`OperationIndicator` no tenían test en Vue; aquí sí (envío real contra un
remoto bare, opciones por defecto, validación, popover y cancelación).

## Diferencias con el original

- **Medidas subpíxel.** GPUI ajusta cada longitud al píxel antes del layout:
  una fila del reflog mide 34px en vez de 33.6 (cabe una menos a la vista) y
  los huecos de `0.6rem` de los diálogos miden 10px en vez de 9.6.
- **Barras de scroll.** Las listas del reflog y del log reservan los 6px de la
  barra cuando desbordan, pero la barra (la de gpui-kit) solo se ve al pasar el
  cursor o desplazar; en el original está siempre visible.
- **Indicador de actividad.** No es una parada de tabulación: su popover solo
  se abre con el cursor (el original también lo abría al enfocarlo).
- **Tooltips.** El `title` nativo pasa a ser el tooltip de Mergeon.
- **Create Branch.** El campo del nombre recibe el foco al abrir (lo que pide
  el `autofocus` del original, que en Electron no llega a aplicarse con el
  foco ya en el botón de la barra).
- **Texto.** El rasterizado de GPUI deja el texto una fracción de píxel más
  ancho (los cuatro botones de la izquierda suman ~3px más) y la familia
  monoespaciada es la del tema de gpui-kit, no la cadena de `--font-mono`
  (hashes, selectores del reflog, horas y mensajes del log).
- **De `mergeon_ui`.** Sin desenfoque del fondo de los diálogos y sombra de
  popovers (anotadas en `ui-components.md`); además, un radio pulsado con el
  ratón muestra su anillo de foco, que en el original solo sale con teclado.
