# `mergeon-view-commits`: historial, inspector, pestañas de archivo y rebase interactivo

Port de `CommitView.vue`, `CommitHistoryList.vue`, `CommitInspector.vue`,
`CommitSummaryHeader.vue`, `CommitChangesTab.vue`, `CommitTreeTab.vue`,
`HistoricalFileTab.vue` e `InteractiveRebaseDialog.vue`.

> Estado: completo. `cargo check --all-targets`, `clippy --all-targets -D
> warnings`, `fmt` y `cargo test -p mergeon-view-commits` (31 tests unitarios +
> 91 de interacción) pasan. Galería y capturas comparadas en claro y oscuro
> (ver [Galería y capturas](#galería-y-capturas)).

```toml
mergeon-view-commits.workspace = true
```

```rust
mergeon_ui::init(cx);
mergeon_view_commits::init(cx);   // hoy no registra nada
let commits = cx.new(|cx| CommitsView::new(repo_id, window, cx));
let file_tab = cx.new(|cx| HistoricalFileTabView::new(repo_id, tab_id, window, cx));
```

El crate depende de `mergeon-view-sidebar` para abrir los diálogos de checkout
y "Create Branch" (`mergeon_view_sidebar::dialogs`), que son los mismos que usa
el sidebar.

## Qué hay

| Vue | Rust | Tipo |
|---|---|---|
| `CommitView.vue` | `CommitsView` + `CommitsViewEvent` | `Entity` |
| `CommitHistoryList.vue` | `CommitHistoryList` + `CommitHistoryListEvent` | `Entity`, virtualizada |
| el `<svg>` de cada fila | módulo `graph` (`GraphPaint`, `lane_x`, `graph_width`) | lienzo |
| `CommitInspector.vue` | `CommitInspector` | `RenderOnce`, controlado |
| `CommitSummaryHeader.vue` | `CommitSummaryHeader` | `RenderOnce`, controlado |
| `CommitChangesTab.vue` | `CommitChangesTab` + `CommitChangesTabEvent` | `Entity` |
| `CommitTreeTab.vue` | `CommitTreeTab` + `CommitTreeTabEvent` | `Entity`, virtualizada |
| `HistoricalFileTab.vue` | `HistoricalFileTabView` | `Entity` |
| — ("File history", nuevo) | `FileHistoryList` + `FileHistoryListEvent` | `Entity`, virtualizada |
| `InteractiveRebaseDialog.vue` | `open_interactive_rebase_dialog` → `InteractiveRebaseDialog` | `Entity` sobre `AppDialog` |
| `buildCheckoutTargets` y las props del diálogo de checkout | `build_checkout_targets`, `CheckoutDialogRequest` | funciones puras |

## Lo que la cáscara conecta

1. **Una `CommitsView` por repositorio** (`CommitsView::new(repo_id, ..)`),
   montada en el panel de la pestaña Commits, a la derecha del sidebar y su
   divisor. Se guarda mientras el repositorio esté abierto: el historial se
   recarga solo y conserva el scroll.
2. **Una `HistoricalFileTabView` por pestaña de archivo** de fuente `commit`,
   `tree` o `history` (`HistoricalFileTabView::new(repo_id, tab_id, ..)`), con
   `set_files_open(bool)` desde el botón "Toggle file list" de la cabecera de
   la pestaña. Si la pestaña se reutiliza (`AppEvent::WorkspaceTabReplaced`),
   la vista se entera sola y pasa a mostrar la pestaña nueva: la cáscara solo
   tiene que mover la entidad de `from` a `to` en su mapa.
3. **Sidebar → historial**: con `SidebarEvent::CommitSelectionRequested { repo_id, request }`,
   `commits_view.update(cx, |view, cx| view.request_commit_selection(&request.hash, cx))`.
4. **Historial → sidebar**: con `CommitsViewEvent::CommitPicked { repo_id, .. }`,
   `sidebar.update(cx, |sidebar, cx| sidebar.clear_viewed_ref(&repo_id, cx))`.
5. Opcional: **commit creado en Local Changes** con
   `view.handle_commit_created(Some(CommitCreated::new(result)), cx)`. No hace
   falta: la vista recarga con el `AppEvent::MutationRecorded` de `commit` (y,
   si estaba oculta, al volver a verse).

Nada más: la vista abre sus diálogos (checkout, "Create Branch", rebase
interactivo, confirmaciones y prompts por `AppModel`), abre las pestañas de
archivo en `AppModel` (`open_workspace_file`) y guarda la selección en la
sesión (`set_workspace_selected_commit`).

## Comparación navegable (`CompareView`)

`open_compare_view(&ComparisonRequest, window, cx) -> Entity<CompareView>` abre la
comparación de dos ramas o revisiones en un `AppDialog` "Compare" que ocupa casi toda
la ventana (el diálogo conserva la entidad mientras está abierto). `CompareView::new`
sirve para montarla en otro sitio (una pestaña, si algún día se quiere).

- Cabecera: A y B con su SHA corto (fijados al abrir con `rev-parse`: si una rama se
  mueve la comparación no cambia; "Refresh" las vuelve a resolver), "⇄" para
  intercambiarlas sobre los mismos SHA, ↑ahead / ↓behind, el segmentado de modo
  (`tab-since` "Since diverged" = `A...B`, diff desde la base común, por defecto;
  `tab-direct` "Direct" = `A B`), la frase del modo ("Changes on B since it diverged
  from A" / "Direct diff A → B") y los totales (archivos, +, −).
- Columna de commits (`uniform_list`): "On B · N" y "Only on A · M", 200 por lado
  como mucho con "and N more commits". Doble clic → `CompareViewEvent::ShowCommit`
  (la cáscara cierra el diálogo y va al commit con `AppModel::show_commit_in_history`,
  lo mismo que el hash de "File history" o del blame: activa el repositorio y
  la pestaña Commits y lo selecciona).
- Archivos y diff: el mismo `CommitChangesTab` del inspector (lista virtual con
  estado, cifras y barritas, divisor y `DiffViewer`), con 1000 archivos como mucho y
  la nota "and N more files not listed" (`set_files_note`, añadido para esto). Los
  renombres piden el diff con sus dos rutas. Se elige el primero por ruta; al cambiar
  de modo o refrescar se conserva el elegido si sigue ahí.
- Cambiar de modo solo relee los archivos (`get_comparison_files`) sobre los SHA
  fijados. Sin historia común, "Since diverged" muestra un estado vacío que lo
  explica con "Show direct diff". Un ref que no existe sale en el panel.
- Ids: `compare-view`, `compare-base`, `compare-head`, `compare-swap`,
  `compare-refresh`, `compare-ahead`, `compare-behind`, `compare-description`,
  `compare-totals`, `compare-commits`, `compare-commit:<sha>`, `compare-commits-more`,
  `compare-no-common-history`, `compare-show-direct`, `compare-no-changes`,
  `compare-error`, `files-note`.
- Con dos commits seleccionados en el historial, "Compare these two" la abre con
  el más viejo como A y el más nuevo como B (ver más abajo).

## Selección múltiple del historial

- **Ratón**: Ctrl/⌘+clic añade o quita un commit (`ClickEvent::modifiers().secondary()`),
  Shift+clic selecciona el rango de la lista desde el ancla (el último clic
  simple o Ctrl+clic). La fila de cambios sin commitear no entra (con
  modificadores no hace nada). Un clic simple vuelve a la selección simple.
- **Teclado**: Shift+↑/↓ extienden desde el ancla; ↑/↓ solas vuelven a la
  selección simple y se mueven como siempre; Esc vuelve a la selección simple
  en el commit principal (el último pulsado, que sigue siendo `selected_commit`
  y el que se guarda en la sesión).
- **Estado**: `selection::MultiSelection` (lógica pura: `toggle`, `select_range`,
  `extend_by`, `retain_loaded`, `oldest_first`, `newest_first`,
  `contiguous_range`). Solo trabaja al pulsar; la lista recibe el conjunto por
  `Rc` (`CommitHistoryList::set_multi_selection`) y lo consulta por fila
  pintada. Tras recargar el historial se quedan los que siguen en la lista.
- **Resumen** en el sitio del inspector (`multi-selection`): "N commits selected"
  (`multi-selection-title`), botones `multi-cherry-pick`, `multi-revert`,
  `multi-compare` (solo con 2) y `multi-copy-shas`, `multi-clear`; los totales
  (`multi-stats`) con `git diff --shortstat <padre del más viejo> <más nuevo>`
  en segundo plano solo si la selección es un tramo seguido de una línea de
  historia (si no, `multi-stats-note` lo dice); y la lista compacta virtual
  (`uniform_list`, filas `multi-commit:<sha>`; un clic vuelve a ese commit).
- **Menú contextual** sobre una fila de la selección: `batch_menu(n)` —
  "Cherry-pick N commits" (C), "Revert N commits" (R), "Revert N commits as one
  commit", "Compare these two" (2), "Copy SHAs". Fuera de la selección, el
  menú de siempre de ese commit.
- **Operaciones**: confirman y van por la cola como el cherry-pick y el revert
  sueltos (mismos nombres de operación, `cherry-pick` / `revert`):
  `AppModel::cherry_pick_commits` (`git cherry-pick a b c`, del más viejo al más
  nuevo) y `AppModel::revert_commits` (`git revert --no-edit c b a`; con
  `single_commit`, `--no-commit` y un commit "Revert N commits" que lista cada
  SHA y asunto). Paran en el primer conflicto ("Cherry-pick stopped on a
  conflict after 1 of 3 commits…") dejando la secuencia de git; el banner ya
  ofrece Continue / Abort, y ahora también **Skip** para cherry-pick y revert
  con secuencia (`OperationState::can_skip` si existe `.git/sequencer`;
  `sequence_action_operation("skip")` → `git cherry-pick|revert --skip`).
- Copy SHAs copia los SHA completos del más viejo al más nuevo, uno por línea.
- "Create patch files…" no se hizo (era opcional).

## `CommitsView`

```rust
impl CommitsView {
    pub fn new(repo_id: impl Into<String>, window: &mut Window, cx: &mut Context<Self>) -> Self;   // modo espacio de trabajo
    pub fn for_active_repository(window: &mut Window, cx: &mut Context<Self>) -> Self;              // repo activo, diff incrustado

    // Props
    pub fn set_repository(&mut self, repo_id: Option<String>, cx: &mut Context<Self>);
    pub fn set_workspace_mode(&mut self, workspace_mode: bool, cx: &mut Context<Self>);
    pub fn set_active(&mut self, active: Option<bool>, cx: &mut Context<Self>);     // None: se deduce de AppModel

    // Peticiones de fuera
    pub fn request_commit_selection(&mut self, commit_hash: &str, cx: &mut Context<Self>);          // commitSelectionRequest
    pub fn load_commits(&mut self, options: CommitLoadOptions, cx: &mut Context<Self>) -> Shared<Task<()>>;
    pub fn reload_history(&mut self, options: CommitLoadOptions, cx: &mut Context<Self>);           // sin esperar
    pub fn handle_load_more_commits(&mut self, cx: &mut Context<Self>);
    pub fn handle_commit_created(&mut self, payload: Option<CommitCreated>, cx: &mut Context<Self>) -> Task<()>;

    // Acciones sobre un commit (las del menú contextual y el doble clic)
    pub fn run_command(&mut self, command: CommitCommand, commit: &CommitRow, window: &mut Window, cx: &mut Context<Self>);
    pub fn handle_history_select(&mut self, commit: CommitRow, cx: &mut Context<Self>);
    pub fn handle_history_checkout_request(&mut self, commit: CommitRow, cx: &mut Context<Self>);
    pub fn handle_create_branch(&mut self, commit: CommitRow, cx: &mut Context<Self>);
    pub fn handle_create_tag(&mut self, commit: CommitRow, cx: &mut Context<Self>);
    pub fn handle_squash / handle_reword / handle_drop(&mut self, commit: CommitRow, cx: &mut Context<Self>);
    pub fn handle_reset(&mut self, commit: CommitRow, mode: &'static str, cx: &mut Context<Self>);   // "soft" | "mixed" | "hard"
    pub fn github_web_url(&self, cx: &App) -> Option<String>;   // remoto del upstream u origin, si es de GitHub
    pub fn copy_github_commit_link(&mut self, commit: &CommitRow, cx: &mut Context<Self>);
    pub fn copy_github_file_link(&mut self, path: &str, cx: &mut Context<Self>);   // menú de "Changed Files"
    pub fn handle_revert / handle_cherry_pick(&mut self, commit: CommitRow, cx: &mut Context<Self>);
    pub fn open_interactive_rebase(&mut self, commit: &CommitRow, window: &mut Window, cx: &mut Context<Self>);
    pub fn handle_create_fixup(&mut self, commit: CommitRow, cx: &mut Context<Self>);   // "Create fixup for this commit"
    pub fn handle_autosquash(&mut self, commit: CommitRow, cx: &mut Context<Self>);     // "Autosquash fixups"
    pub fn select_parent_commit(&mut self, parent_hash: &str, cx: &mut Context<Self>);
    pub fn open_local_changes(&mut self, cx: &mut Context<Self>);   // fila de cambios sin commitear
    pub fn select_file(&mut self, file: FileChange, cx: &mut Context<Self>);
    pub fn open_repo_tree(&mut self, cx: &mut Context<Self>);
    pub fn copy_to_clipboard(&mut self, text: &str, cx: &mut Context<Self>);
    pub fn toggle_summary(&mut self, cx: &mut Context<Self>);
    pub fn handle_toggle_ignore_whitespace(&mut self, cx: &mut Context<Self>);

    // Respuesta de los diálogos que abre la vista (los llaman sus eventos;
    // públicos para quien quiera responder sin pasar por el diálogo)
    pub fn submit_checkout_dialog(&mut self, mode: LocalChangesMode, target: Option<&str>, cx: &mut Context<Self>);
    pub fn close_checkout_dialog(&mut self, cx: &mut Context<Self>);
    pub fn submit_create_branch_dialog(&mut self, branch_name: &str, checkout_after_create: bool, mode: LocalChangesMode, cx: &mut Context<Self>);
    pub fn close_create_branch_dialog(&mut self, cx: &mut Context<Self>);

    // Lectura
    pub fn repository_id(&self, cx: &App) -> Option<String>;
    pub fn is_active(&self, cx: &App) -> bool;
    pub fn is_workspace_mode(&self) -> bool;
    pub fn commits(&self) -> &Rc<Vec<CommitRow>>;
    pub fn selected_commit(&self) -> Option<&CommitRow>;
    pub fn commit_files(&self) -> &[FileChange];
    pub fn selected_file(&self) -> Option<&FileChange>;
    pub fn file_diff(&self) -> &str;
    pub fn current_branch(&self) -> &str;
    pub fn history_load_count(&self) -> usize;           // lecturas de git hechas (las fundidas cuentan una)
    pub fn has_more_commits / is_loading_more_commits / is_loading_initial_history / is_loading_history / is_loading_details / is_loading_diff(&self) -> bool;
    pub fn history_width(&self) -> f64;
    pub fn is_resizing_history / is_summary_expanded(&self) -> bool;
    pub fn checkout_dialog(&self) -> Option<&CheckoutDialogRequest>;          // el diálogo de checkout abierto
    pub fn create_branch_dialog(&self) -> Option<&CreateBranchDialogRequest>;
    pub fn interactive_rebase_dialog(&self) -> Option<&Entity<InteractiveRebaseDialog>>;
    pub fn history_list(&self) -> &Entity<CommitHistoryList>;
    pub fn changes_tab(&self) -> &Entity<CommitChangesTab>;
}

pub enum CommitsViewEvent {
    CommitSelected { hash: String },                 // select-commit (ya guardado en la sesión en modo espacio de trabajo)
    CommitPicked { repo_id: String, hash: String },  // clic o flechas: retirar la marca "viewing" del sidebar
    OpenFile(FileTabDescriptor),                     // open-file (ya abierto en AppModel en modo espacio de trabajo)
    ChangeView(WorkspaceSection),                    // change-view (ya hecho en modo espacio de trabajo);
                                                     // también Changes al pulsar la fila de cambios sin commitear
}

pub fn clamp_history_width(next_width: f64, container_width: f64) -> f64;   // clampHistoryWidth
pub fn commit_menu(commit: &CommitRow, current_branch: &str) -> Vec<Vec<CommitMenuEntry>>;   // grupos; entre grupos, separador
pub const DEFAULT_HISTORY_WIDTH: f64 = 250.;
```

`CommitLoadOptions` son las opciones de `loadCommits`
(`preferred_commit_hash`, `preferred_branch`, `force_details_reload`,
`preferred_file_path`, `ensure_commit_visible`, con constructores del mismo
nombre) y `merged_with` es `mergeCommitLoadOptions`.

Qué hace por su cuenta:

- **Carga y recarga del historial** (`AppModel::load_commit_history`, páginas de
  200, `load_more_commit_history` al llegar al final de la lista): al crearse,
  al cambiar la rama del repositorio y con cada `AppEvent::MutationRecorded` que
  pueda mover refs (ver [Diferencias](#diferencias-con-el-original)). Si no es
  la vista activa, la recarga queda pendiente hasta que vuelve a serlo. Las
  peticiones que llegan con una carga en curso se funden en una sola pasada
  posterior. Un refresco relee la ventana ya cargada (no encoge la lista) y no
  vacía las filas; solo la carga inicial de un repositorio pinta "Loading
  commits...".
- **Selección tras cada recarga**: el commit pedido, el que ya estaba, la punta
  de la rama preferida o el primero. La primera carga restaura
  `repository_workspace(path).selected_commit` (ampliando la ventana a 1000 si
  hace falta). Detalles (`get_commit_files`) y diff (`get_commit_diff`, solo
  fuera del espacio de trabajo) con descarte de respuestas de un repositorio,
  un commit o un archivo que ya no son los pedidos.
- **Modo espacio de trabajo** (`new`): guarda la selección con
  `set_workspace_selected_commit`, abre los archivos con `open_workspace_file`
  (clic en un archivo → pestaña `commit`; "Repo tree" → pestaña `tree`) y no
  pide diffs. "Activa" = Settings cerrado, repositorio activo y pestaña
  `HOME_TAB`.
- **Ancho del historial**: `load_history_width` al crearse, límites de
  `clamp_history_width` contra el ancho medido, `persist_history_width` al
  soltar el divisor.
- **Hash obsoleto** (`bad object`, `unknown revision`…): una sola recarga por
  hash.
- **Flechas arriba / abajo** con el foco en la vista (lo toma si la ventana no
  tiene nada enfocado). Solo recorren commits: la fila de cambios sin
  commitear no se selecciona.
- **Cambios sin commitear**: del último status del repositorio
  (`Repository::status`) saca cuántos archivos cambiaron y cuántos tienen algo
  en el índice y se lo da a la lista (`set_working_changes`). Al pulsar la fila
  pasa a Local Changes (`show_workspace_home(.., Changes)` en modo espacio de
  trabajo) y emite `ChangeView(Changes)`.
- **Inspector**: Checkout y Cherry-pick de la cabecera ejecutan
  `run_command` (Checkout desactivado como su entrada del menú); ⋯ abre
  `commit_menu`, el mismo menú que el clic derecho en la fila; el segmentado
  Unified | Split llama a `AppModel::toggle_diff_view`.
- **Diálogos**: checkout (`open_checkout_branch_dialog`, con selector de
  destino, aviso de HEAD desanclado o tracking de una remota, según
  `buildCheckoutTargets`), "Create Branch" (`open_create_branch_dialog`), rebase
  interactivo, y por `AppModel` los confirm de reset/squash/drop y los prompts
  de reword y de tag (nombre + anotación).

## `HistoricalFileTabView`

```rust
impl HistoricalFileTabView {
    pub fn new(repo_id: impl Into<String>, tab_id: impl Into<String>, window: &mut Window, cx: &mut Context<Self>) -> Self;
    pub fn set_files_open(&mut self, open: bool, cx: &mut Context<Self>);   // filesOpen
    pub fn load(&mut self, cx: &mut Context<Self>);                         // lo que hace "Retry"
    pub fn open_here / open_in_new_tab(&mut self, descriptor: FileTabDescriptor, cx: &mut Context<Self>);
    pub fn close(&mut self, cx: &mut Context<Self>);
    pub fn copy_path(&mut self, path: &str, cx: &mut Context<Self>);

    pub fn tab_id(&self) -> &str;                    // sigue a la pestaña si se reutiliza
    pub fn tab(&self, cx: &App) -> Option<FileTab>;
    pub fn is_active(&self, cx: &App) -> bool;
    pub fn shows_files(&self, cx: &App) -> bool;     // showFiles
    pub fn is_loading(&self) -> bool;
    pub fn error(&self) -> &str;
    pub fn diff(&self) -> &str;
    pub fn files(&self) -> &[FileChange];
    pub fn status_text(&self, cx: &App) -> Option<SharedString>;
    pub fn diff_viewer(&self) -> &Entity<DiffViewer>;
    pub fn file_viewer(&self) -> &Entity<FileContentViewer>;
    pub fn changes_list(&self) -> &Entity<CommitChangesTab>;
    pub fn tree(&self) -> &Entity<CommitTreeTab>;
}

pub fn file_menu_labels() -> [&'static str; 5];   // "Open here", "Open in new tab", "Open in editor", "File history", "Copy Path"

// Pestaña de historial
impl HistoricalFileTabView {
    pub fn history(&self) -> &Entity<FileHistoryList>;
    pub fn selected_revision(&self) -> Option<&FileRevision>;
    pub fn show_commit(&mut self, hash: &str, cx: &mut Context<Self>);                         // → AppModel::show_commit_in_history
    pub fn open_file_history(&mut self, descriptor: &FileTabDescriptor, cx: &mut Context<Self>); // pestaña nueva de historial
}
```

Carga cuando la pestaña está activa y cambia `[repo, pestaña, ignoreWhitespace]`
(los archivos del commit, una vez por commit; el diff o el archivo), restaura el
scroll guardado de la pestaña y guarda el nuevo (`remember_workspace_scroll`).
La lista lateral es `CommitChangesTab` (fuente `commit`) o `CommitTreeTab`
(fuente `tree`) en modo externo: clic → `open_here` (reutiliza la pestaña),
doble clic → `open_in_new_tab`, menú "Open here / Open in new tab / Open in
editor · File history · Copy Path" (+ "Copy GitHub link" a `blob/<sha>/<ruta>`
si el repositorio está en GitHub). El de "Changed Files" del inspector sigue el
mismo orden: "Open in editor · File history · Copy Path / Copy GitHub link".
El divisor usa el ancho compartido de `AppModel::file_list_width` (arrastre,
doble clic para volver al inicial, flechas con el foco en él). Estados:
"Loading diff…", el error con "Retry", "Diff too large to display (…)", "No
changes to display", "Pick a file on the left to see it as it was at this
commit.". La X del visor de archivo cierra la pestaña.

### "File history" (fuente `history`)

`FileTabDescriptor::history(ruta, commit)` abre el historial del archivo desde
`commit` (con la ruta que tenía en él) o, sin commit, desde `HEAD` (sigue a la
rama: se relee con cada `MutationRecorded` del repositorio, o al volver a la
pestaña si estaba oculta). Contexto de la pestaña: `History` o
`<hash> · History`; ícono `FileClock`. Se abre desde:

- el menú de un archivo de Local Changes ("File history", desde `HEAD`),
- el menú de "Changed Files" del inspector (desde el commit elegido),
- el menú de un archivo de la lista o del árbol de una pestaña de archivo y el
  menú de la propia pestaña en la tira (desde su commit),
- el botón "File history" de la barra del diff de una pestaña `commit`.

A la izquierda, `FileHistoryList` (siempre visible, aunque la lista de
archivos esté plegada): cabecera "History · <archivo>", ruta y cuántas
revisiones ("100+ revisions" si hay más), y una fila por revisión (46 px): asunto,
insignia ("renamed from x" —con la carpeta si cambió—, "copied from x", "added",
"deleted"), monograma y autor, fecha relativa y hash corto. Fila elegida en
tinta con la marca naranja. Clic → elige (y deja el foco en la lista); ↑/↓,
Inicio/Fin con el foco en ella; menú: "Show commit in history", "View file at
this revision" (pestaña `tree` con la ruta de esa revisión; apagado si la
revisión lo borró), "Copy commit hash". El hash es un enlace al commit.
Páginas de 100 (`AppModel::get_file_history`): la siguiente se pide al asomar
el final de la lista, con "Load more" como última fila o al llegar con Fin/↓.
La primera revisión se elige sola.

A la derecha, el diff del archivo en la revisión elegida
(`AppModel::get_file_revision_diff` con la ruta de esa revisión y, si fue un
renombre, la anterior, así que se ve el renombre con sus cambios) en el mismo
`DiffViewer`, con "Ignore spaces" y Unified/Split; la barra enseña el hash
(clic → commit en el historial), el asunto, `as <ruta>` si el archivo se
llamaba de otra forma en esa revisión, el autor y la fecha. Un renombre sin
cambios de contenido (diff sin bloques) lo dice en texto en vez de dejar el
visor en blanco. Sin revisión:
"Loading history…", el error, "No commit has touched this file." o "Pick a
revision on the left…". El scroll no se recuerda por pestaña (cambia con cada
revisión).

Ir a un commit (`show_commit` / clic en un hash) usa
`AppModel::show_commit_in_history`: activa el repositorio y su pestaña Commits
y emite `AppEvent::CommitSelectionRequested`, que la `CommitsView` del
repositorio atiende con `request_commit_selection` (recarga la ventana que haga
falta hasta tener el commit a la vista).

```rust
impl FileHistoryList {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self;
    pub fn set_source(&mut self, source: FileHistorySource, cx: &mut Context<Self>);  // { repo_path, path, revision: Option<String> }
    pub fn reload(&mut self, cx: &mut Context<Self>);      // relee lo cargado, conserva la elegida
    pub fn load_more(&mut self, cx: &mut Context<Self>);
    pub fn select(&mut self, index: usize, cx: &mut Context<Self>);
    pub fn rows(&self) -> &[HistoryRow];                   // revision + fecha relativa + insignia, calculadas al cargar
    pub fn selected_index / selected_revision / has_more / is_loading / is_loading_more / error / load_count / source(..);
}
pub enum FileHistoryListEvent { Select(FileRevision), ShowCommit(String), ViewFileAt(FileRevision) }
pub fn revision_badge(revision: &FileRevision) -> Option<SharedString>;
pub fn file_history_row_id(hash: &str) -> SharedString;   // "file-revision:<hash>"
```

## `CommitHistoryList`

Lista virtualizada (`v_virtual_list`): filas de 28px con el grafo en un lienzo
(carriles de 14px, escalón de metro hacia los padres, anillo de HEAD), badges
de refs agrupados (`main` y `origin/main` en el mismo commit: una sola
etiqueta con una nube por remoto, como el sidebar; tooltip "In sync with
origin/main") y recortados con "+N" (su tooltip lista **todas** las refs del
commit, una por línea: `refs_tooltip`),
mensaje, marca de pull, autor (monograma de iniciales + nombre), fecha y hash.
La columna del grafo mide al menos `GRAPH_COLUMN_MIN_WIDTH` (56px, el
`Math.max(g.W + 6, 56)` del diseño) para que la etiqueta "GRAPH" no se monte
sobre "MESSAGE" con uno o dos carriles.

Con cambios sin commitear (`set_working_changes(Some(..))`) hay una fila más
encima del primer commit: «5 uncommitted changes · 2 staged» en cursiva
apagada (Geist no trae cursiva: se ve recta), «now», «—» y, en el grafo, un
cuadrado hueco de borde punteado en el carril de HEAD unido a él por un tramo
punteado. No es un commit: no se selecciona y los índices de commit
(`painted_rows`, `row_bounds`, el ancla del scroll) no la cuentan. Lo que se
estaba viendo no se mueve cuando aparece o desaparece. Atenúa lo que
no está en la rama, ancla el scroll al refrescar si no se estaba en la punta y
trae a la vista la selección y el HEAD nuevo.

```rust
impl CommitHistoryList {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self;
    pub fn set_commits(&mut self, commits: Rc<Vec<CommitRow>>, cx: &mut Context<Self>);
    pub fn set_graph_lane_count(&mut self, lane_count: usize, cx: &mut Context<Self>);
    pub fn set_selected_commit_hash(&mut self, hash: Option<&str>, cx: &mut Context<Self>);
    pub fn set_loading / set_has_more / set_loading_more(&mut self, value: bool, cx: &mut Context<Self>);
    pub fn set_context_menu(&mut self, items: impl Fn(&CommitRow, &mut Window, &mut App) -> Vec<MenuItem> + 'static);
    pub fn set_working_changes(&mut self, changes: Option<WorkingChanges>, cx: &mut Context<Self>);

    pub fn commits(&self) -> &Rc<Vec<CommitRow>>;
    pub fn selected_commit_hash(&self) -> Option<&str>;
    pub fn graph_lane_count(&self) -> usize;
    pub fn graph_width(&self) -> f32;            // el del grafo (carriles)
    pub fn graph_column_width(&self) -> f32;     // max(graph_width, 56): la columna y su cabecera
    pub fn working_changes(&self) -> Option<WorkingChanges>;
    pub fn status_text(&self) -> Option<&'static str>;     // "Loading commits..." | "No commits found"
    pub fn painted_rows(&self) -> Range<usize>;
    pub fn scroll_top(&self) -> f32;
    pub fn set_scroll_top(&mut self, top: f32);
    pub fn row_bounds(&self, hash: &str) -> Option<Bounds<Pixels>>;   // en la ventana, según el último frame
    pub fn row_flags(&self, commit: &CommitRow) -> RowFlags;          // selected, is_head, not_in_branch, to_pull
    pub fn badge_layout(&self, commit: &CommitRow, measure_text: impl Fn(&str) -> f64) -> FittedBadges<RefBadge>;
}

pub enum CommitHistoryListEvent {
    SelectCommit(CommitRow), CheckoutCommit(CommitRow), LoadMore, OpenWorkingChanges,
    ToggleCommit(CommitRow), ExtendSelection(CommitRow),
    CheckoutRef { commit: CommitRow, name: String, kind: RefType },   // doble clic en una etiqueta de rama
}
pub fn refs_tooltip(refs: &[CommitRef]) -> String;          // el "+N": "main (HEAD)\norigin/main (remote)\nv1 (tag)"
pub fn ref_badge_title(badge: &RefBadge) -> Option<String>;  // badge_title + "Double-click to check out" en ramas
pub struct WorkingChanges { pub total: usize, pub staged: usize }   // ::new(total, staged) -> Option (None si 0); label()
pub fn row_key(commit: &CommitRow) -> &str;    // hash completo (o el corto)
pub const ROW_HEIGHT: f32 = 28.;
pub const GRAPH_COLUMN_MIN_WIDTH: f32 = 56.;
pub const GRAPH_LABEL_INSET: f32 = 10.;
pub const WORKING_CHANGES_ROW_ID: &str = "working-changes";
```

- **Doble clic en una etiqueta de rama** (local o remota) → `CheckoutRef`; el
  primer clic sigue seleccionando la fila y el segundo no llega a ella (no
  pide además el checkout del commit). `CommitsView::handle_ref_checkout_request`
  usa el flujo del doble clic en la fila (operación a medias → aviso; sin
  cambios locales y rama local → directo; con cambios, o rama remota sin local,
  el diálogo de checkout con ese único destino: `ref_checkout_target`). Una
  remota cuya local ya existe cambia a la local. La rama actual: "You are
  already on 'main'.". Tags y HEAD desanclado: el doble clic es el de la fila.
- **Cabeceras de columna**: qué columnas caben depende del ancho de la lista;
  la cabecera se arma en el prepaint del mismo frame con su ancho real (antes
  usaba el del frame anterior y, al arrastrar el divisor, AUTHOR/DATE se
  quedaban desfasadas de las filas hasta el siguiente repintado).

Ids: fila `commit:<hash>` (con su `aria-label` = mensaje), badges
`("ref", índice)` (etiqueta = nombre; la nube de cada remoto `ref-remote:<remoto>`), chip `ref-more`, marca `pull-marker`, estado
`commit-list-state`, fila `working-changes` (etiqueta = su texto), cabecera
`column:graph`, `column:message`, `column:author`, `column:date`,
`column:hash`.

## `CommitInspector` y `CommitSummaryHeader`

Controlados; quien los aloja guarda si el cuerpo del mensaje está desplegado
(`CommitsView` lo olvida al cambiar de commit: el siguiente empieza recortado).
Con el `.dh` de Señal: fila superior con el hash en cápsula partida (corto en
negrita | 12 caracteres más, apagados) y su botón de copiar, las refs y los
botones Checkout, Cherry-pick y ⋯; el título; el cuerpo; y la línea de autor
(monograma de 22px, nombre en negrita, correo apagado), fecha en mono y
"parent <hash>" (o "parents" en un merge) que lleva al padre. Debajo, el
bloque de cifras: `N FILES · +X LINES · −Y LINES` en mono de 20px separadas por
líneas de 1px, y a la derecha "Repo tree" y el segmentado Unified | Split
(`mergeon_ui::tabs`).

El cuerpo del mensaje:

- Los trailers del final (`Co-Authored-By:`, `Signed-off-by:`…, ver
  `split_trailers`) van aparte, en 11px y `text_3`, y no cuentan para el
  recorte.
- Plegado (por defecto) enseña como mucho 3 líneas *visuales*: las partidas
  por ancho cuentan (`visual_line_count`, con el ancho real del cuerpo medido
  al pintarlo y como mucho 560px). Si no cabe, se trunca con "…"
  (`line_clamp` + `text_ellipsis` y alto máximo de 3×19px; `line_clamp` solo no
  cuenta los saltos de línea de GPUI) y aparece "Show more". Si cabe, no hay
  botón y los trailers se ven debajo.
- Desplegado crece hasta el 40 % del alto del inspector (`BODY_MAX_FRACTION`,
  medido al pintarlo) y a partir de ahí se desplaza dentro con su barra;
  "Show less" queda fuera del scroll. Los trailers van al final.
- La fila del hash, el título, la línea del autor y las cifras no se encogen.
  La lista de archivos no baja de `MIN_FILES_HEIGHT` (su cabecera y 5 filas):
  con la ventana baja cede el cuerpo, hasta quedarse en una línea.

```rust
CommitInspector::new(&commit, &changes_tab)
    .expanded(self.summary_expanded)
    .checkout_disabled(bool)    // como la entrada Checkout del menú
    .diff_view_mode(mode)       // el segmento encendido
    .on_toggle_details(..)      // Fn(&ClickEvent, &mut Window, &mut App): "Show more"
    .on_open_tree(..)
    .on_copy_sha(..)            // Fn(&str, &mut Window, &mut App): hash completo
    .on_select_parent(..)       // Fn(&str, ..): hash del padre
    .on_command(..)             // Fn(CommitCommand, ..): Checkout / CherryPick
    .menu(..)                   // Fn(&mut Window, &mut App) -> Vec<MenuItem>: el botón ⋯
    .on_diff_view_mode(..)      // Fn(DiffViewMode, ..): solo al elegir el otro segmento

CommitSummaryHeader::new(&commit).expanded(bool).checkout_disabled(bool)
    .body_max_height(px)        // tope del cuerpo desplegado (sin él, 40 % de la ventana)
    .on_toggle(..).on_copy_sha(..).on_select_parent(..).on_command(..).menu(..)

pub struct CommitTotals { pub files: usize, pub additions: usize, pub deletions: usize }   // ::of(&CommitChangesTab)
pub fn short_hash(full_hash: &str) -> &str;        // 8 caracteres
pub fn parent_hashes(parents: &str) -> Vec<&str>;
pub fn split_trailers(body: &str) -> MessageBody<'_>;   // { text, trailers }, sin blancos al borde
pub fn visual_line_count(text: &str, width: Pixels, window: &Window) -> usize;
pub const BODY_CLAMP_LINES: usize = 3; pub const BODY_LINE_HEIGHT: f32 = 19.;
pub const BODY_MAX_FRACTION: f32 = 0.4; pub const MIN_FILES_HEIGHT: f32;
```

Ids: `summary-sha`, `summary-copy`, `("summary-ref", i)`, `summary-checkout`,
`summary-cherry-pick`, `summary-more`, `summary-title`, `summary-body`,
`summary-toggle` ("Show more" / "Show less", solo con cuerpo largo),
`summary-trailers` (etiqueta: los trailers), `commit-summary`, `commit-stats`,
`commit-files`, `commit-details`,
`summary-author`, `summary-email`, `summary-date` (etiqueta: la fecha ISO),
`parent:<hash>`, `stat-files` ("4 files"), `stat-additions` ("+65 lines"),
`stat-deletions` ("−9 lines"), `open-repo-tree`, `tab-unified`, `tab-split`.
Mientras cargan los archivos, las cifras muestran "–".

## `CommitChangesTab`

```rust
impl CommitChangesTab {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self;
    pub fn set_external_files(&mut self, external: bool, cx: &mut Context<Self>);   // solo la lista
    pub fn set_loading_details(&mut self, loading: bool, cx: &mut Context<Self>);
    pub fn set_commit_files(&mut self, files: &[FileChange], cx: &mut Context<Self>);
    pub fn set_selected_path(&mut self, path: Option<&str>, cx: &mut Context<Self>);
    pub fn set_loading_diff(&mut self, loading: bool, cx: &mut Context<Self>);
    pub fn set_diff(&mut self, diff: impl Into<SharedString>, too_large: bool, size: u64, cx: &mut Context<Self>);
    pub fn set_diff_preferences(&mut self, view_mode: DiffViewMode, ignore_whitespace: bool, cx: &mut Context<Self>);
    pub fn set_whole_file(&mut self, whole_file: bool, cx: &mut Context<Self>);   // enciende "Whole file"
    pub fn image_viewer(&self) -> &Entity<ImageDiffViewer>;   // quien aloja la lista lo llena; se pinta si la ruta es imagen
    pub fn set_file_context_menu(&mut self, items: impl Fn(&FileChange, &mut Window, &mut App) -> Vec<MenuItem> + 'static);
    pub fn set_show_header(&mut self, show: bool, cx: &mut Context<Self>);   // "Changed Files  N"; el inspector la quita

    pub fn rows(&self) -> &[FileRow];                 // lista plana ordenada (locale_compare): name, dir, file
    pub fn selected_path(&self) -> Option<&str>;
    pub fn status_text(&self) -> Option<SharedString>;
    pub fn viewer(&self) -> &Entity<DiffViewer>;
    pub fn files_pane_width(&self) -> f32;            // 260 por defecto, entre 160 y 420
    pub fn diff_view_mode / is_ignoring_whitespace / is_external_files / is_loading_details / is_resizing(..)
}

pub enum CommitChangesTabEvent { SelectFile(FileChange), OpenFile(FileChange), ToggleIgnoreWhitespace, ToggleWholeFile }
pub fn file_rows(files: &[FileChange]) -> Vec<FileRow>;
pub fn file_row_id(path: &str) -> SharedString;   // "file-row:<ruta>", el id de la fila en la ventana
pub fn clamp_files_pane_width(width: f32) -> f32;
```

Cada fila lleva sus cifras `+n −m` y las cinco barritas de proporción
(`FileStats::bars`, de `mergeon-view-diff`). El cambio Unified | Split ya no
está en la cabecera del diff: es el segmentado del inspector.

Ids: `changes-tab`, `file-row:<ruta>`, `change-bars` (dentro de cada fila),
`toggle-whole-file`, `toggle-ignore-whitespace`, `changes-resizer`, `file-count`.

**"Whole file" e imágenes** (también en `HistoricalFileTabView` y en
`CompareView`): el botón "Whole file" de la cabecera del diff (encendido con el
tinte `primary`) cambia la preferencia `whole_file` y quien aloja el visor
vuelve a pedir el diff con `DiffOptions::whole_file` (`--unified=999999999`;
mismo tope de 2 MiB: un archivo grande sale como "Diff too large"). Si la ruta
es una imagen (`is_image_path`: png, jpg/jpeg, gif, webp, bmp, ico, svg) se
pinta el `ImageDiffViewer` en lugar del diff y sin los botones del diff de
texto; las versiones se leen con `AppModel::read_image_pair` (commit: padre →
commit, con la ruta de origen de un renombre; comparación: base o merge-base →
B; historial de un archivo: con la ruta anterior de la revisión) y se preparan
en segundo plano (`crate::images`). Ids: `toggle-whole-file`, `image-diff`,
`image-before`, `image-after` (etiqueta = "W × H px · tamaño"),
`image-loading`, `image-error`.

## `CommitTreeTab`

```rust
impl CommitTreeTab {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self;
    pub fn set_source(&mut self, repo_path: &str, commit_hash: &str, cx: &mut Context<Self>);   // reinicia el árbol
    pub fn set_commit_files(&mut self, files: &[FileChange], cx: &mut Context<Self>);            // marcas de estado
    pub fn set_external_files(&mut self, external: bool, cx: &mut Context<Self>);
    pub fn set_file_context_menu(&mut self, items: impl Fn(&FileTabDescriptor, &mut Window, &mut App) -> Vec<MenuItem> + 'static);
    pub fn activate(&mut self, path: &str, cx: &mut Context<Self>);   // carpeta: plegar; archivo: abrir
    pub fn close_file(&mut self, cx: &mut Context<Self>);

    pub fn rows(&self) -> &[TreeRow];             // entry, depth, error
    pub fn is_expanded / is_loading(&self, path: &str) -> bool;
    pub fn is_loading_root(&self) -> bool;
    pub fn selected_path(&self) -> &str;
    pub fn omitted_total(&self) -> usize;
    pub fn changed_status(&self, row: &TreeRow) -> Option<&str>;
    pub fn status_text(&self) -> Option<SharedString>;
    pub fn viewer(&self) -> &Entity<FileContentViewer>;
    pub fn file_descriptor(&self, path: &str) -> FileTabDescriptor;
}

pub enum CommitTreeTabEvent { OpenFile(FileTabDescriptor), OpenFileNew(FileTabDescriptor) }
```

Pide los niveles con `AppModel::get_tree_entries` y los archivos con
`get_file_at_commit`. Ids: `tree:<ruta>`, `tree-state`, `tree-error`,
`tree-omitted`.

## Rebase interactivo

```rust
pub fn open_interactive_rebase_dialog(commits: Vec<RebaseCommit>, window: &mut Window, cx: &mut App) -> Entity<InteractiveRebaseDialog>;

impl InteractiveRebaseDialog {
    pub fn rows(&self) -> &[RebaseRow];
    pub fn move_row(&mut self, index: usize, delta: isize, cx: &mut Context<Self>);
    pub fn set_action(&mut self, index: usize, action: RebaseAction, window: &mut Window, cx: &mut Context<Self>);
    pub fn set_message(&mut self, index: usize, message: &str, window: &mut Window, cx: &mut Context<Self>);
    pub fn message_input(&self, index: usize) -> Option<&Entity<InputState>>;
    pub fn summary(&self) -> String;          // "2 pick · 1 squash"
    pub fn can_submit(&self) -> bool;
    pub fn todo(&self) -> Vec<RebaseTodoEntry>;
    pub fn submit(&mut self, cx: &mut Context<Self>);            // emite Submit; no cierra
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>);     // cierra y emite Close
    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>);   // cierra sin avisar
    pub fn is_busy(&self) -> bool;
    pub fn set_busy(&mut self, busy: bool, cx: &mut Context<Self>);
}

pub enum InteractiveRebaseDialogEvent { Submit(Vec<RebaseTodoEntry>), Close }
pub enum RebaseAction { Pick, Reword, Squash, Fixup, Drop }
pub fn available_actions(index: usize) -> Vec<RebaseAction>;   // la primera fila sin squash ni fixup
```

`CommitsView` lo abre desde "Interactive rebase from here…" con el rango de
`mergeon_model::commit_view_utils::interactive_rebase_range` y ejecuta el
rebase. Ids: `rebase-row:<hash>`, `rebase-action:<hash>`, `rebase-up:<hash>`,
`rebase-down:<hash>`, `rebase-summary`, `rebase-cancel`, `rebase-submit`. Las
filas van en una `uniform_list` (`rebase-rows`, `max-height` 420px): con un
rango de cientos de commits solo se construyen las que están a la vista.

### Fixups dirigidos

El menú contextual de un commit lleva un grupo tras el del rebase:

- **Create fixup for this commit** (`Lucide::Wrench`): `git commit
  --fixup=<sha>` con lo preparado (`CommitOptions { fixup: Some(sha), sign }`,
  la firma la que recuerde el repositorio), por la cola de mutaciones como un
  commit; relee estado y ramas y recarga el historial. Apagada sin nada
  preparado o si el commit no es de la rama actual; si el commit ya está en el
  upstream (`in_upstream`) pregunta antes ("Fix up a pushed commit?").
- **Autosquash fixups** (`Lucide::Combine`): cuenta los `fixup!` / `squash!` /
  `amend!` del rango (el de `interactive_rebase_range`, del commit a HEAD); sin
  ninguno solo avisa ("There are no fixup commits from <hash> up to HEAD.");
  con alguno confirma ("Fold N fixup commits into their targets? This rewrites
  the M commits from <hash> up to HEAD.", más un aviso si alguno ya está en el
  upstream) y lanza `rebase_action(.., "autosquash", hash)`: `git rebase -i
  --autosquash --autostash <hash>^` (o `--root`) con `GIT_SEQUENCE_EDITOR=:` y
  `GIT_EDITOR=:`, sin editor. Un rango con un merge se rechaza como en el
  rebase interactivo. Un conflicto deja el rebase parado y el banner de la
  operación ofrece Continue / Abort como en cualquier otro rebase.

## Galería y capturas

```bash
cd native
cargo build -p mergeon-view-commits --example commits_gallery
crates/mergeon-view-commits/shots.sh [dark|light|all] [regex de casos]   # → target-shots/commits-<caso>-<tema>.png
MERGEON_COMMITS_STEPS="select:Merge branch;expand" cargo run -p mergeon-view-commits --example commits_gallery
```

La galería arranca un `AppModel` real sobre el repositorio de
`tools/make-fixture.sh` (o `website` / `api` / `empty`, los mismos que creaba
el ya eliminado `tools/reference-shots.mjs`) y pinta la vista con el sidebar real a su
izquierda, el divisor, la tira de pestañas del espacio de trabajo y las
pestañas de archivo, en una ventana de 1440×900 recortada igual que las
capturas de referencia. Variables: `MERGEON_THEME`, `MERGEON_COMMITS_FIXTURE`,
`MERGEON_COMMITS_REPO`, `MERGEON_COMMITS_SETUP` (bash antes de abrir),
`MERGEON_HISTORY_WIDTH` y `MERGEON_COMMITS_STEPS` (lista en la cabecera de
`examples/commits_gallery.rs`).

| Caso (`commits-<caso>-<tema>.png`) | Referencia |
|---|---|
| `history` | 201 |
| `commit-selected`, `commit-many-files`, `merge-selected`, `summary-expanded` | 202–205 |
| `file-diff`, `file-diff-split`, `file-list-hidden`, `file-menu` | 206–209 |
| `whitespace`, `ignore-spaces`, `ignore-spaces-split` | 210–212 |
| `repo-tree`, `repo-tree-file`, `file-tabs`, `binary-file` | 213–216 |
| `branch-viewing` | 222 |
| `context-menu` | 230 |
| `checkout-commit`, `checkout-choice` | 411, 412 |
| `confirm-reset-mixed`, `confirm-reset-hard`, `confirm-squash`, `prompt-reword`, `confirm-drop` | 426–430 |
| `interactive-rebase`, `interactive-rebase-edited` | 431, 432 |
| `rebase-blocked`, `rebase-with-stash` | 531 (ver Diferencias) |
| `revert-completed`, `cherry-pick-completed` | 516, 517 |
| `ahead-behind`, `to-pull-selected` | 601, 602 |
| `empty-repository` | 606 |

## Tests

`cargo test -p mergeon-view-commits`: 31 unitarios (lógica pura) y 91 de
interacción en `tests/ui/` (`#[gpui_kit::test]`, ventanas sin pantalla,
clics, dobles clics, menús, teclas y arrastres reales, `AppModel` sobre
repositorios git reales y un `RecordingUiHost`).

| Original | Port | Notas |
|---|---|---|
| `CommitView.test.js` (27) | `tests/ui/commit_view.rs` (27 + 8) | Donde el original encadenaba promesas diferidas (coalescing, cambio de repo a media carga, hash obsoleto) se reproduce la situación con git real: varias peticiones en el mismo frame, cambio de repositorio antes del primer frame, objeto borrado con `gc --prune`. Los "se llamó a `getCommits` con…" se comprueban por su efecto (`history_load_count`, filas, selección, ramas de git, ajustes guardados). Añadidos: commit hecho con el historial oculto, flechas, rebase interactivo con stash y rango con merge, la fila de cambios sin commitear (lleva a Local Changes y las flechas la saltan). |
| `CommitHistoryList.test.js` (9) | `tests/ui/history_list.rs` (9 + 3) | El SVG no existe: el ancho del grafo y el alto de fila se miden en la ventana. Los anchos de badge se miden con una función fija. Añadidos: la etiqueta "GRAPH" con un solo carril (bug "GRAMESSAGE") y la fila de cambios sin commitear. |
| `CommitSummaryHeader.test.js` (7), `CommitInspector.test.js` (3) | `tests/ui/inspector.rs` (15) | Rehechos para la cabecera de Señal (cuerpo recortado, botones de acción, ⋯) más las cifras y el segmentado. |
| `CommitChangesTab.test.js` (7) | `tests/ui/changes_tab.rs` (9) | Los textos `+3` / `−1` se comprueban en `file_stats` (los pinta `mergeon-view-diff`); las barritas, por su etiqueta y que caben en la fila. El botón Split se movió al inspector. |
| `CommitTreeTab.test.js` (10) | `tests/ui/tree_tab.rs` (10) | La carpeta ilegible se provoca borrando su objeto de árbol; las 40 entradas omitidas, con 2040 archivos. |
| `InteractiveRebaseDialog.test.js` (4) | `tests/ui/rebase_dialog.rs` (4) | El mensaje de reword se escribe con `set_message` (el campo de gpui-kit no expone un id para escribir en él). |
| — | `tests/ui/historical_file_tab.rs` (6) | `HistoricalFileTab.vue` no tenía tests. |
| — | `tests/ui/multi_select.rs` (9) | Selección múltiple: Ctrl+clic (añadir, quitar, volver a simple), Shift+clic con totales del rango, Shift+flechas y Esc, la fila de cambios sin commitear no se selecciona, cherry-pick en lote (menú), revert en lote y en un único commit, cherry-pick que choca en el segundo de tres (secuencia con Skip), "Compare these two" (A = el más viejo; doble clic vuelve al historial) y Copy SHAs. |
| — | `tests/ui/file_history.rs` (6) | "File history": renombres dobles con insignias y diffs, flechas, hash → commit, historia desde un commit, paginación (106 revisiones, Fin carga la segunda página siguiendo el renombre), archivo sin commits, botón de la pestaña `commit`. |

## Diferencias con el original

- **Recarga tras cualquier mutación que mueva refs.** El original solo recargaba
  con `fetch`, `pull`, `push`, `merge`, `rebase` y `external-head-change`; un
  commit hecho en Local Changes llegaba por el watcher del sistema de archivos.
  Aquí recarga con todo menos `status-watch`, stage/unstage/discard,
  `add-to-ignore`, `resolve-conflict`, la configuración de remotos y la
  hidratación; sin releer los detalles cuando la operación solo mueve refs
  (fetch, pull, push, checkout, ramas, tags, stash).
- **"Interactive rebase from here…" con un stash guardado** ya no se niega: el
  rango sigue los padres desde HEAD (`interactive_rebase_range`). Con un merge en
  el rango sigue avisando (`rebase-blocked`).
- El menú del commit ofrece "Reset (Soft)", "Reset (Mixed)" y "Reset (Hard)"
  (el original no tenía Soft), cada uno con su confirmación.
- "Copy GitHub link" (módulo `github_link`): en el menú del commit
  (`…/commit/<sha>`) y en los menús de archivo de "Changed Files" y de la
  pestaña de archivo histórico (`…/blob/<sha>/<ruta>`). El repositorio sale del
  remoto del upstream de la rama actual o de `origin`; si ninguno es de GitHub
  la entrada no aparece. La lista "Changed Files" del inspector no tenía menú:
  ahora tiene "Copy Path" y, en GitHub, "Copy GitHub link".
- La lista del historial y el árbol están virtualizados; el ancla del scroll al
  refrescar se calcula con el índice del primer commit anterior.
- El ancho del historial se adopta en todas las vistas abiertas cuando cambia el
  ajuste guardado (en el original cada vista lo leía solo al montarse).
- Una respuesta de diff que llega tarde no pisa la del archivo elegido después.
- En la pestaña de archivo, "Ignore spaces" encendido no se distingue: igual que
  en el original, cuya regla `.active` vivía en el CSS de otro componente.
- La primera selección: el original elegía a veces la primera fila (el stash)
  porque cargaba el historial antes de conocer la rama actual (`201`); aquí se
  carga tras la primera lectura del repositorio y se elige la punta de la rama.
- Las barras de scroll son las superpuestas de gpui-kit (el original reservaba
  10px).
- Un autor que no cabe deja ver un trocito de la elipsis (en Chromium
  desaparece del todo).
- Los tooltips de las filas pueden quedar a la vista sobre un diálogo o un menú
  abierto con el cursor encima de la fila (los `title` nativos no).
