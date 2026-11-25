# `mergeon-view-changes`: cambios locales, commit, conflictos, stash y blame

Port de `ChangesView.vue`, `RepositoryChangesSidebar.vue`,
`RepositoryCommitPanel.vue`, `ConflictResolver.vue`, `StashDiffDialog.vue` y del
diálogo de blame que abre `ChangesView.vue`. Usa los visores de
`mergeon-view-diff` (`DiffViewer`, `DiffPanel`, `BlameViewer`, `FileTreeRow`).

> Estado: completo. `check --all-targets`, `clippy -D warnings`, `fmt` y
> `cargo test -p mergeon-view-changes` (41 tests unitarios + 89 de interfaz)
> pasan. Las 27 pantallas de referencia de este crate (`301`–`320`, `445`,
> `503`–`507`, `513`) se capturan con `shots.sh` y coinciden en claro y
> oscuro salvo lo listado en [Diferencias](#diferencias-deliberadas-con-el-original).

```toml
mergeon-view-changes.workspace = true
```

```rust
mergeon_ui::init(cx);
mergeon_view_changes::init(cx);   // registra Ctrl+Enter / Cmd+Enter de la caja de commit
```

## Qué hay

| Vue | Rust | Tipo |
|---|---|---|
| `ChangesView.vue` | `ChangesView`, `ChangesMode`, `ChangesViewEvent` | `Entity` (`impl Render`) |
| `RepositoryChangesSidebar.vue` | `ChangesSidebar` | `RenderOnce`, controlado |
| `RepositoryCommitPanel.vue` | `CommitPanel`, acción `SubmitCommit` | `RenderOnce`, controlado |
| `ConflictResolver.vue` | `ConflictResolver`, `ConflictResolverEvent`, `ConflictChoice`, `WholeChoice`, `MergeDocument`, `Pick` | `Entity` |
| `StashDiffDialog.vue` | `open_stash_diff_dialog`, `StashDiffDialog` | función + `Entity` |
| diálogo de blame de `ChangesView.vue` | `open_blame_dialog`, `BlameDialog` | función + `Entity` |
| `computed` de las tres vistas | funciones puras de `logic` (reexportadas en la raíz) | — |

## `ChangesView`

```rust
pub enum ChangesMode {
    List,                         // la pestaña "Local Changes": lista + caja de commit | diff
    FileTab { tab_id: String },   // `file-tab`: pestaña de un archivo (working / index)
    Full,                         // lista + diff + caja de commit (el modo por defecto del original)
}

impl ChangesView {
    pub fn new(repo_id: impl Into<String>, mode: ChangesMode, window: &mut Window, cx: &mut Context<Self>) -> Self;

    // Lo que pasa quien la aloja (las props del original)
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>);        // `active` (por defecto true)
    pub fn set_files_open(&mut self, files_open: bool, cx: &mut Context<Self>); // `files-open` (por defecto true)
    pub fn set_tab(&mut self, tab_id: impl Into<String>, cx: &mut Context<Self>);

    // Lectura
    pub fn repo_id(&self) -> &str;
    pub fn mode(&self) -> &ChangesMode;
    pub fn tab_id(&self) -> Option<&str>;
    pub fn is_files_open(&self) -> bool;
    pub fn files(&self) -> &[ChangedFile];
    pub fn selected_file(&self) -> Option<&ChangedFile>;
    pub fn file_diff(&self) -> &SharedString;
    pub fn file_content(&self) -> &SharedString;
    pub fn file_error(&self) -> &SharedString;
    pub fn is_loading_changes(&self) -> bool;
    pub fn is_loading_diff(&self) -> bool;
    pub fn busy_action(&self) -> Option<BusyAction>;
    pub fn is_interaction_locked(&self) -> bool;
    pub fn is_resolving_conflict(&self) -> bool;
    pub fn commit_message(&self, cx: &App) -> SharedString;   // "asunto\n\ncuerpo"
    pub fn commit_title(&self, cx: &App) -> SharedString;
    pub fn commit_body(&self, cx: &App) -> SharedString;
    pub fn is_sign(&self) -> bool;
    pub fn list_focus_handle(&self) -> &FocusHandle;
    pub fn is_push_after_commit(&self) -> bool;
    pub fn is_amend(&self) -> bool;
    pub fn hunk_action(&self, cx: &App) -> Option<HunkAction>;
    pub fn is_committable(&self, cx: &App) -> bool;
    pub fn sidebar_width(&self, cx: &App) -> f64;
    pub fn file_line_stats(&self, path: &str, staged: bool) -> Option<(u64, u64)>; // cifras +/− ya leídas
    pub fn diff_viewer(&self) -> &Entity<DiffViewer>;
    pub fn conflict_resolver(&self) -> &Entity<ConflictResolver>;

    // Acciones (lo mismo que hacen los controles)
    pub fn select_file(&mut self, file: &ChangedFile, cx: &mut Context<Self>);          // clic: su diff a la derecha
    pub fn move_selection(&mut self, file: &ChangedFile, cx: &mut Context<Self>);       // flechas (lo mismo que el clic)
    pub fn open_file_in_new_tab(&mut self, file: &ChangedFile, cx: &mut Context<Self>); // doble clic
    pub fn stage_file / unstage_file / discard_file / ignore_file / reveal_file / copy_path(&mut self, file, cx);
    pub fn stage_all / unstage_all / discard_all / discard_selected(&mut self, cx);
    pub fn handle_apply_hunk(&mut self, index: usize, cx: &mut Context<Self>);   // acción principal del bloque
    pub fn handle_hunk_request(&mut self, request: &HunkRequest, cx: &mut Context<Self>);   // Stage / Unstage / Discard hunk
    pub fn handle_lines_request(&mut self, request: &LinesRequest, cx: &mut Context<Self>); // Stage / Unstage / Discard lines
    pub fn toggle_diff_view / toggle_ignore_whitespace(&mut self, cx);
    pub fn handle_resolve(&mut self, resolution: ConflictResolution, cx: &mut Context<Self>);
    pub fn set_amend(&mut self, amend: bool, window: &mut Window, cx: &mut Context<Self>);
    pub fn set_push_after_commit(&mut self, push: bool, cx: &mut Context<Self>);
    pub fn set_sign(&mut self, sign: bool, cx: &mut Context<Self>);   // se recuerda por repositorio
    pub fn commit(&mut self, cx: &mut Context<Self>);
    pub fn run_file_menu_action(&mut self, action: FileMenuAction, file: &ChangedFile, window, cx);
}

pub enum ChangesViewEvent {
    OpenFile { descriptor: FileTabDescriptor, replace: bool },              // @open-file
    CommitCreated { result: Box<GitMutation>, sync_already_applied: bool }, // @commit-created
}
```

Tras un commit la vista **relee ella misma** estado y ramas (con push, lo que
pedía `ChangesView.vue`: `status`, `branches`, `changesCount`, `operation:
'push'`; sin push, lo que hacía `handleCommitCreated` de `CommitView.vue`:
`result`, `branches`, `status` si el resultado no lo trae, `operation:
'commit'`). Por eso `sync_already_applied` llega siempre a `true`: quien lo
reciba no tiene que releer nada.

### "Local Changes" (`ChangesMode::List`)

Disposición de Señal (`changes.png`): a la izquierda la lista de archivos y,
debajo, la caja de commit (fila de atajos, asunto, cuerpo, casillas y botón);
a la derecha, tras el divisor, el diff del archivo elegido (`DiffPanel`, o el
resolvedor si está en conflicto). El clic y las flechas eligen el archivo y
cargan su diff; el doble clic (y el menú) abren su pestaña. Al entrar no se
elige ningún archivo: la columna derecha dice "Select a file to see its
changes" (`changes-no-selection`). Tras una relectura, el archivo elegido
sigue elegido en su mismo lado (el lado sin preparar de un archivo a medio
preparar no salta al preparado); si desaparece, la selección se vacía.

- **Ancho**: la columna izquierda empieza en 680px
  (`DEFAULT_LOCAL_CHANGES_WIDTH`) y se arrastra entre 320 y 900 dejando al
  diff al menos 360 (`clamp_local_changes_width`, que también se aplica al
  pintar si la ventana encoge). Doble clic en el divisor: vuelve a 680.
- **Ventana angosta** (≤ 1100px): las columnas se apilan: lista, caja de
  commit y, debajo, el diff (mín. 160px). Sin divisor.
- **Cifras `+N −N` y barritas** en cada archivo de "Unstaged" (con lo que no
  se sigue) y "Staged" (`file_stats(..).bars(true).on_ink(selected)` de
  `mergeon-view-diff`, ids `stats:<unstaged|staged>:<ruta>` con la etiqueta
  `+N −N`). Salen de `mergeon_git_query::get_working_tree_stats`
  (`git diff --numstat -z` y `git diff --cached --numstat -z`, con
  `GIT_OPTIONAL_LOCKS=0`; lo que no se sigue cuenta sus líneas como añadidas
  si es texto y ≤ 1 MiB). Se leen en segundo plano al cambiar el estado, con
  cada `MutationRecorded` del repositorio (el watcher incluido: un archivo que
  vuelve a cambiar sin cambiar el estado) y al volver a verse la lista; una
  sola lectura a la vez y las peticiones intermedias se juntan en una. Las
  filas salen sin cifras hasta que llegan; al pintar solo se busca la fila
  visible en un `HashMap` (nada se recorre por frame). Binarios y conflictos,
  sin cifras. También en las pestañas de archivo con la lista abierta.
- **Plantilla de commit**: si el repositorio (o la configuración global) tiene
  `commit.template` (`mergeon_git_query::get_commit_template`: ruta con `~` o
  relativa al repositorio), al abrir la vista sin borrador el asunto y el
  cuerpo se rellenan con la plantilla sin sus líneas `#`
  (`logic::commit_template_message`). Nunca pisa un borrador ni lo que se
  escriba mientras se lee; no se guarda como borrador. Tras un commit la caja
  vuelve a la plantilla, y con la plantilla sin tocar "Amend last commit"
  parte del mensaje de HEAD como con la caja vacía. No se cachea toda la
  sesión: al abrir la vista, al volver a "Local Changes", tras un commit y
  al quedar la caja vacía se relee en un hilo de fondo
  (`mergeon_git_query::read_commit_template_if_changed`: un `git config` y la
  fecha y el tamaño del archivo; el archivo solo se lee si cambió). Si la
  plantilla cambió o se quitó y la caja está vacía o con la plantilla anterior
  sin tocar, se pone la vigente (o se vacía); un borrador nunca se pisa.

### Orden del menú contextual de un archivo

`logic::file_menu_entries`, por grupos separados: (en una pestaña de archivo,
"Open here / Open in new tab") · "Stage" o "Unstage" y "Discard Changes" ·
("Ignore" si no tiene seguimiento) · "Blame / Open in editor / Reveal in
Explorer" · "File history" · "Stash selected files…" (no en un conflicto) ·
"Copy Path". Ver y abrir antes, copiar al final.

### "Stash selected files…" y "Fixup into…"

- **Stash selected files…** (menú contextual de un archivo, en su grupo tras
  "File history"; no en un conflicto): si la casilla del archivo está marcada (tiene algo preparado),
  todos los archivos marcados; si no, solo ese (`logic::stash_selection`). Abre
  el diálogo de stash de `mergeon-view-toolbar` con esas rutas
  (`AppModel::handle_stash_paths` → `DialogRequest::Stash { paths }`), que
  resume qué se guardará.
- **Fixup into…** (`commit-fixup`, botón de contorno junto al de commit, con
  su menú `.max_h(380).scrollable(true)`): los 30 últimos commits de la rama
  sin merges (`mergeon_git_query::get_fixup_candidates`), primero los no
  publicados y, bajo "Already pushed", los que ya están en el upstream o en
  una rama remota (ícono de nube). Elegir uno hace `git commit --fixup=<sha>`
  con lo preparado (`ChangesView::commit_fixup`; uno publicado pregunta antes
  "Fix up a pushed commit?"); la caja de mensaje no se toca. Apagado sin nada
  preparado, con "Amend last commit" o mientras se lee la lista. La lista se
  lee en segundo plano (una lectura a la vez) al abrir la vista y cuando
  cambian la rama, HEAD, el upstream o el `ahead` del estado; nunca en las
  pestañas de archivo. `fixup_targets()` la expone (tests).

### Bloques y líneas sueltas

En `src/changes_view/patch_actions.rs`. El visor emite `ApplyHunk` /
`ApplyLines` con una `PatchAction`; la vista solo atiende las que ofrece su
`hunk_action()` (lo preparado no se descarta). Preparar y quitar del stage van
directos; descartar un bloque o unas líneas pregunta antes ("Discard hunk" /
"Discard lines", "This action cannot be undone.", variante destructiva) y
aplica el parche al revés sobre el working tree (`ApplyPatchOptions { reverse:
true, worktree: true }`, operación `discard-hunk`). git-ops valida siempre con
`git apply --check`: si el parche ya no encaja avisa "The selected changes no
longer apply to <ruta>: … Reload the diff and try again." sin tocar nada.

### Cómo se coordina con `AppModel`

La vista **aplica ella misma** lo que en el original recogía
`RepositoryWorkspace.vue`; los eventos son solo informativos.

| Original | Lo que hace la vista |
|---|---|
| `emit('open-file', descriptor)` | `AppModel::open_workspace_file(path, &descriptor, replace, cx)` y después `ChangesViewEvent::OpenFile` |
| `emit('view-scroll', pos)` | `AppModel::remember_workspace_scroll(path, tab_id, Scroll)` (no hay evento) |
| `props.fileTab` | lo lee de `AppModel::repository_workspace(path).tabs` por `tab_id` |
| un clic reutiliza la pestaña (`onReplace`) | escucha `AppEvent::WorkspaceTabReplaced { from, to }`: si `from` es su pestaña, pasa a ser `to` (ver `tab_id()`) |
| `props.active` | `set_active(..)` **y además**, en una pestaña de archivo, que su `tab_id` sea el `active_tab` del repositorio (lo mira sola) |
| `props.filesOpen` | `set_files_open(..)` |
| `watch(activeRepo.status)` | `cx.observe(&model)` comparando el estado por valor, más `AppEvent::MutationRecorded` con `status-watch` / `external-head-change` para recargar el diff cuando el estado queda igual |
| ancho de la lista | en pestaña de archivo, `AppModel::file_list_width` (`start/drag/stop/reset_file_list_resize`); en `List`, propio de la vista (680, entre 320 y 900); en `Full`, propio (330, entre 160 y 560) |
| `commit-created` → `CommitView.handleCommitCreated` | la relectura de ramas y estado la hace la vista (ver arriba) |
| `confirm(...)`, `notify.*` | `AppModel::confirm`, `notify_success`, `notify_error`, `notify` (pasan por el `UiHost`) |

### Lo que la cáscara debe hacer

- Crear **una entidad por panel** y conservarla mientras la pestaña exista
  (guarda selección, mensaje de commit, carpetas plegadas y scroll):
  `ChangesMode::List` para "Local Changes", `ChangesMode::FileTab { tab_id }`
  para cada pestaña `working` / `index`.
- Al recibir `AppEvent::WorkspaceTabReplaced { from, to }`, mover su entidad
  de la clave `from` a la clave `to` (la vista ya cambió su `tab_id`).
- Llamar a `set_files_open(files_open)` con el estado del botón "Toggle file
  list" y a `set_active(false/true)` cuando el espacio de trabajo del
  repositorio deja de verse o vuelve.
- Atender el clic de un stash del sidebar (`SidebarEvent::OpenStashDiff
  { repo_path, stash }`) con `open_stash_diff_dialog(&repo_path, &stash.hash, window, cx)`.
- `ChangesViewEvent::CommitCreated` no necesita atención: las ramas ya están
  releídas y el historial se entera por `AppEvent::MutationRecorded`.

## `ChangesSidebar`

Controlado. Guarda en estado de elemento (`window.use_keyed_state`) las
carpetas plegadas y el desplazamiento de cada sección. Las filas son
`FileTreeRow` dentro de `uniform_list` (solo se construyen las visibles).

```rust
ChangesSidebar::new(id)
    .source(SidebarSource::All | Working | Index)
    .loading(bool).has_changes(bool)
    .conflict_state(Option<ConflictState>)
    .busy_action(Option<BusyAction>)
    .conflicted_files(Vec<ChangedFile>).unstaged_files(..).untracked_files(..).staged_files(..)
    .unstaged_stats(StatsByPath).staged_stats(StatsByPath) // cifras +N −N por ruta (Rc<HashMap<String, LineStats>>)
    .right_border(bool)                      // filete derecho (por defecto sí; "Local Changes" lo quita)
    .selected_file_path(Option<impl Into<SharedString>>)
    .stageable(bool).unstageable(bool).discardable(bool)
    .on_select_file(|file, window, cx| ..)   // clic
    .on_open_file(|file, window, cx| ..)     // doble clic
    .on_stage_file(..).on_unstage_file(..)   // casilla del archivo o Espacio
    .on_discard_file(..)                     // D (quien lo recibe confirma)
    .on_move_selection(..)                   // flechas (sin esto, `on_select_file`)
    .track_focus(&focus_handle)              // con el foco, Espacio / D / flechas
    .on_stage_all(|window, cx| ..).on_unstage_all(..).on_discard_all(..)
    .file_menu(|file, window, cx| -> Vec<MenuItem>)   // menú contextual
    .head_hash(Option<impl Into<SharedString>>)       // estado vacío: "No changes since <hash>."
    .on_open_terminal(|window, cx| ..).on_open_folder(..)  // sus dos botones
```

Cada archivo de "Unstaged" y "Staged" lleva una casilla (vacía / marcada; mixta
en las dos secciones si el archivo está a medio preparar) que lo prepara o lo
quita del stage sin elegir la fila. Cada cabecera lleva su casilla de sección
(clic = "Stage all" / "Unstage all") y, a la derecha, "+ Stage all" /
"− Unstage all" (y la papelera de "Discard all" en "Unstaged"). Las teclas son
las de `mergeon_model::keyboard_shortcuts::CHANGES_LIST_SHORTCUTS` (Espacio, D,
↑, ↓; D también en mayúscula, con Shift o Bloq Mayús, como la enseña la
pista), solo con el foco en la propia lista; las flechas recorren los archivos
visibles en el orden de la lista (sin los de carpetas plegadas) y distinguen
las dos filas de un archivo a medio preparar. D solo actúa en "Unstaged", y
Espacio no hace nada sobre un conflicto (se resuelve en el resolvedor).

**Conflictos**: cualquiera de los siete códigos sin fusionar de `git status`
(`UU`, `AA`, `DD`, `AU`, `UA`, `DU`, `UD`; `logic::is_conflicted`) va a
"Conflicted files" y solo ahí: ni a "Unstaged" ni a "Staged" (antes solo `UU`;
`AA`, `DD`, `UD`… salían dos veces, sin diff, y `UD` no se podía preparar).
"Open in editor" se apaga solo en `DD` (en `UD` / `DU` git deja el archivo del
lado que lo conserva). La
vista enfoca la lista si nada tiene el foco, y en "Local Changes" las flechas
solo mueven la selección (el clic sigue abriendo la pestaña del archivo).

Ids: secciones `conflict-section`, `unstaged-section`, `staged-section`;
cabeceras `unstaged-header`, `staged-header` con sus casillas `unstaged-check`,
`staged-check`; botones `stage-all`, `discard-all`, `unstage-all`; filas
`"<conflict|unstaged|staged>:<file|folder>:<ruta>"` y sus casillas
`"check:<unstaged|staged>:file:<ruta>"`; estados `changes-loading`,
`changes-empty` ("Working tree clean", con `changes-empty-head`,
`changes-empty-terminal` y `changes-empty-folder`; la vista los enlaza a
`handle_open_terminal` / `handle_open_explorer`).

## `CommitPanel`

Controlado; el asunto vive en el `InputState` y el cuerpo en el
`TextareaState` de quien lo aloja. Al hacer commit se unen como
`asunto\n\ncuerpo` (`compose_commit_message`). El borrador del repositorio se
sigue guardando como una sola cadena con ese formato; al cargarlo,
`split_commit_message` lo reparte (asunto hasta el primer salto de línea, el
resto al cuerpo), así que los borradores de la caja única de antes migran
solos.

```rust
CommitPanel::new(&title_state, &body_state)
    .shortcut_hints(bool)              // recuadro "[Space] stage / unstage · [D] discard · [↑][↓] move"
    .push_after_commit(bool).busy(bool).disabled(bool).committable(bool)
    .staged_count(usize).branch(..).upstream(..)
    .amend(bool).amendable(bool).amend_is_pushed(bool)
    .sign(bool)
    .on_push_after_commit_change(|&bool, window, cx| ..)
    .on_amend_change(|&bool, window, cx| ..)
    .on_sign_change(|&bool, window, cx| ..)
    .on_commit(|window, cx| ..)        // botón o Ctrl/Cmd+Enter, solo si se puede
    .fixup_targets(Rc<Vec<FixupCandidate>>)
    .on_fixup(|candidate, window, cx| ..)   // "Fixup into…": el commit elegido
```

El asunto lleva dentro el contador `n/72` (`subject_counter`; en gris, en rojo
al pasar de 72). Enter en el asunto pasa al cuerpo. El botón es el primario
naranja grande: "Commit to main" / "Commit & Push to main" / "Amend on main" /
"Amend & Push on main" (`commit_label`), con las teclas del atajo dentro.
"Sign (SSH/GPG)" hace `git commit -S` (`CommitOptions::sign`); sin clave el
commit falla con `SIGNING_FAILED` y el aviso empieza por cómo arreglarlo.

Ids: `commit-panel` (registrado para los tests), `changes-shortcut-hints`, `commit-title`,
`commit-title-counter`, `commit-body`, `amend-last-commit`, `sign-commit`,
`push-after-commit`, `commit-warning`, `commit-blocked`, `commit` (el botón) y
`commit-action` (su envoltura, con el nombre del botón como etiqueta
accesible: `mergeon_ui::button` no la publica), `commit-fixup` y su envoltura
`commit-fixup-action`.

El atajo es `Ctrl+Enter` y `Cmd+Enter` (en cualquier plataforma, como el
original): una acción propia (`SubmitCommit`) en el contexto
`CommitMessage > Input`, que gana a la de salto de línea del área de texto, y
además un `capture_action::<Enter>` para el `secondary-enter` de gpui-kit. Ni
inserta salto de línea ni dispara el commit dos veces (tests
`commits_with_the_keyboard` y `a_commit_rereads_the_branches`).

## `ConflictResolver`

Resolvedor de tres vías. Parte de las etapas del índice (`:1:` base, `:2:` ours,
`:3:` theirs; `mergeon_git_query::read_conflict_versions`, que mira
`git ls-files -u -z` para saber cuáles existen) y no de los marcadores: así da
igual el `merge.conflictStyle` y un archivo con líneas que parecen marcadores no
lo confunde. Sin etapas (archivo ya preparado, conflicto ajeno) recurre a los
marcadores del working tree (`merge`, `diff3` y `zdiff3`).

```rust
impl ConflictResolver {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self;
    pub fn set_files(&mut self, files, cx);
    pub fn set_counts(&mut self, pending: usize, resolved: usize, total: usize, cx);
    pub fn set_versions(&mut self, path, versions: Option<Arc<ConflictVersions>>, cx); // barato si no cambió
    pub fn set_load_error(&mut self, error: Option<SharedString>, cx);
    pub fn set_file(&mut self, path, content, cx);           // solo marcadores, sin etapas
    pub fn set_busy(&mut self, busy: bool, cx);
    pub fn reset(&mut self, cx);

    pub fn is_loading(&self) -> bool;
    pub fn document(&self) -> Option<&Arc<MergeDocument>>;
    pub fn editor(&self) -> &Entity<EditorState>;            // el resultado
    pub fn conflict_count(&self) -> usize;
    pub fn resolved_conflicts(&self) -> usize;
    pub fn unresolved_count(&self) -> usize;
    pub fn current_conflict(&self) -> usize;
    pub fn pick(&self, conflict: usize) -> Option<&Pick>;
    pub fn whole_choice(&self) -> Option<WholeChoice>;
    pub fn status_text(&self) -> SharedString;               // "2 of 5 resolved"
    pub fn result_text(&self, cx: &App) -> String;

    pub fn choose(&mut self, conflict, choice: ConflictChoice, window, cx);
    pub fn toggle_line(&mut self, conflict, pane: Pane, offset, window, cx);
    pub fn toggle_side(&mut self, conflict, pane: Pane, window, cx);
    pub fn reset_conflict(&mut self, conflict, window, cx);
    pub fn go_to / next_conflict / previous_conflict(.., window, cx);
    pub fn toggle_base(&mut self, cx);
    pub fn choose_whole(&mut self, choice: WholeChoice, cx);
    pub fn save(&mut self, cx);                               // "Mark as resolved"
}

pub enum ConflictResolverEvent {
    Save { resolution: ConflictResolution, warning: Option<String> },
    SelectFile(String),
}
pub enum ConflictChoice { Current, Incoming, BothCurrentFirst, BothIncomingFirst, Base }
pub enum WholeChoice { Keep(ConflictSide), Delete }
```

Pantalla (archivo de texto con los dos lados):

- Arriba, "Current (ours)" e "Incoming (theirs)" (y "Base" en medio con el
  botón `toggle-base`): listas virtuales (`uniform_list`, filas de 20px, scroll
  horizontal medido sobre la línea más larga). Cada conflicto lleva una fila de
  cabecera con la casilla de bloque (mixta si está a medias), "Conflict n · k
  lines" (clic: lo hace actual) y "Take current"/"Take incoming"/"Take base";
  cada línea en conflicto, su casilla. Las líneas que cambió un solo lado van
  tintadas (entran solas en el resultado).
- La barra del conflicto actual: "Conflict 2 of 5", "Take current", "Take
  incoming", "Take both (current first)", "Take both (incoming first)", "Take
  base" (deshabilitado sin base: añadido por los dos) y "Reset" (vuelven los
  marcadores).
- Abajo, "Result": el `Editor` de gpui-kit (rope, números de línea, deshacer,
  portapapeles, búsqueda). Lo no conflictivo ya viene fusionado; cada conflicto
  sin decidir lleva marcadores `<<<<<<< Current (ours)` / `=======` /
  `>>>>>>> Incoming (theirs)` y un fondo ámbar (verde al decidirse). Elegir un
  lado reemplaza solo ese tramo (con deshacer). Lo escrito a mano se respeta:
  250 ms después de teclear se recoloca cada tramo emparejando las líneas del
  texto anterior y el nuevo (en un hilo de fondo; `conflict_merge::resync`), y
  un conflicto editado cuenta como resuelto cuando ya no tiene marcadores.
- Barra superior: archivo, clase de conflicto (`resolver-kind`), "2 of 5
  resolved" (`resolver-status`), anterior/siguiente (dan la vuelta), "Base" y
  "Mark as resolved".

"Mark as resolved" prepara el texto en un hilo de fondo: finales de línea del
original (CRLF si dominaba en ours), salto final como el original y cuenta de
líneas que empiezan por `<<<<<<<` o `>>>>>>>`. Si quedan conflictos o marcadores
emite `Save` con `warning` y el anfitrión lo confirma antes de escribir.

Un `AA` de texto (añadido por los dos) va al resolvedor de tres vías con la
base vacía (`merge2`: lo común ya sale fusionado; "Take base" apagado).

Casos especiales (`DocumentKind::Whole`, sin editor): un `radio_group`
(`whole_options`, `WholeOption`) con un título y una explicación de qué hizo
cada rama (`whole_text`). Los nombres de los lados salen de
`ConflictVersions::sides` (`ConflictSides`, leídos solo para estos casos: en un
merge, la rama actual y la de `MERGE_MSG`; en un cherry-pick / revert, "commit
abc1234" / "revert of abc1234"; en un rebase, aquello sobre lo que se rebasa y
la rama con el commit que se reaplica; si no se sabe, "current" / "incoming"):

| Código | Opciones |
|---|---|
| `UD` / `DU` (borrado por uno, modificado por el otro) | "Keep the modified file" ("From main · 1.2 KB") / "Delete the file" ("As feature did"). Título "Modified in main, deleted in feature" y "feature (incoming) deleted this file and main (current) changed it…" |
| `DD` (borrado o renombrado por los dos) | "Delete the file" ya elegido, sin confirmación (no queda nada en el working tree) / "Keep the common ancestor" |
| `AU` / `UA` (solo en un lado, tras un renombre) | "Keep the file" / "Delete the file" |
| binario `UU` / `AA` (NUL en los primeros 8000 bytes) | "Keep current (ours)" / "Keep incoming (theirs)" con rama y tamaño ("From main · 9 B") / "Keep base (common ancestor)" si hay base |

`Keep(side)` hace `git checkout-index --stage=N` + `git add` (bytes y modo
exactos); `Delete`, `git rm` (con confirmación salvo en `DD`). "Mark as
resolved" y el contador del banner cuadran con todos (salen de
`conflict_state.pending_files`, que ya contaba los siete códigos).

La lógica pura vive en `src/conflict_merge.rs`: `split_lines` (CRLF fuera de
la línea), `diff_pairs` (patience con Myers acotado), `merge3` (diff3 con el
recorte de `zdiff3`), `merge2` (sin base), `analyze`, `document_from_markers`,
`MergeDocument::chunk_text`/`initial_result`, `Pick`, `resync`,
`finish_result`. Rendimiento: el análisis, la recolocación y el guardado van en
`background_spawn`; el pintado solo recorre las filas visibles y los contadores
están cacheados.

Ids: `conflict-file:<ruta>`, `resolver-file-name`, `resolver-kind`,
`resolver-status`, `conflict-prev`, `conflict-next`, `toggle-base`,
`mark-resolved`, `current-conflict`, `take-current`, `take-incoming`,
`take-both-current-first`, `take-both-incoming-first`, `take-base`,
`reset-conflict`, `conflict-result`; por panel (`ours`/`theirs`/`base`):
`<p>-header-<n>`, `<p>-goto-<n>`, `<p>-take-<n>`, `<p>-block-<n>` (casilla de
bloque), `<p>-line-<línea>` (casilla de línea, número de línea de esa versión
desde 0), `pane-<p>-frame`; en archivos enteros, `whole-choice`.

`ChangesView` lo monta dentro de `DiffPanel::conflict(..)` cuando el archivo
elegido está en conflicto: lee las versiones con `read_conflict_versions` (y
el contenido del working tree sigue en `file_content()`), le pasa
`conflict_state.pending_files` y los contadores, y con `Save` confirma si hay
aviso y llama a `resolve_conflict_with`. El banner de la operación (Continue /
Abort) no es de este crate: lee el mismo `conflict_state` del modelo.

## Diálogos

```rust
pub fn open_stash_diff_dialog(repo_id: &str, stash_ref: &str, window: &mut Window, cx: &mut App) -> Option<DialogId>;
pub fn open_blame_dialog(repo_id: &str, file_path: &str, window: &mut Window, cx: &mut App) -> Option<DialogId>;
```

- **Stash**: `stash_ref` es el SHA del stash; el título es su mensaje si el
  repositorio tiene los stashes cargados, o "Stash". Ancho `max-w-4xl`. Los
  archivos sin seguimiento de un `git stash -u` (en el tercer padre del
  stash, `stash^3`) salen como nuevos (`A`) con su contenido como diff, también
  en un stash que solo tiene eso (antes: "This stash has no file changes").
- **Blame**: título `Blame: <archivo>`, ancho `max-w-5xl`. Lo abre también la
  entrada "Blame" del menú contextual. Es navegable (`BlameViewerEvent`): el
  clic en un bloque cierra el diálogo y lleva al commit
  (`AppModel::show_commit_in_history`); "Blame previous revision" pasa al blame
  del archivo en el padre de ese commit (`AppModel::blame_at_revision`, con la
  ruta de antes de un renombre) y muestra encima "Back" y la revisión
  (`BlameDialog::revision`, `BlameDialog::back`).
- **"File history"** (menú contextual de un archivo, en su grupo tras "Blame /
  Open in editor / Reveal in Explorer"; apagada en un archivo sin
  seguimiento): abre la pestaña de historial del archivo desde `HEAD`
  (`FileTabDescriptor::history(ruta, None)`, ver `view-commits.md`).
- `None` si el repositorio no está abierto.

## Lógica pura (`logic`, reexportada en la raíz)

`conflicted_files`, `staged_files`, `unstaged_files`, `untracked_files`,
`effective_status`, `file_descriptor`, `hunk_action`, `file_menu_entries`
(+ `FileMenuAction`, `FileMenuEntry`), `can_commit`, `amend_is_pushed`,
`amend_message`, `commit_label`, `commit_blocked_reason`, `subject_counter`
(+ `SUBJECT_LIMIT`), `compose_commit_message`, `split_commit_message`,
`is_partially_staged`,
`amend_warning`, `head_commit`, `commit_shortcut_label`, `commit_shortcut_keys`,
`clamp_list_width`, `clamp_local_changes_width`, `commit_template_message`, `BusyAction`, `SidebarSource`, `ChangedFile`
(= `mergeon_gitcmd::FileStatus`).

## Galería y capturas

`examples/changes_gallery.rs`, y `shots.sh` para capturarla en todos los
estados de las referencias.

```bash
cd native
cargo build -p mergeon-view-changes --example changes_gallery
crates/mergeon-view-changes/shots.sh [dark|light|all] [filtro]
# → target-shots/changes-<NNN>-<caso>-<tema>.png, recortadas a la ventana
#   (1440×900), la misma geometría que target-shots/reference/
```

| Variable | Valores |
|---|---|
| `MERGEON_THEME` | `dark` (defecto) · `light` · `system` |
| `MERGEON_CHANGES_MODE` | `list` (defecto) · `full` · `file:<working\|index>:<ruta>` |
| `MERGEON_CHANGES_STATE` | `merge` · `merge-partial` · `merge-ready` · `rebase` · `clean` · `all-staged` · `stage-app` · `pushed` · `empty` · `many` · `files-closed` · `split` · `ignore-spaces` · `slow-diff` |
| `MERGEON_CHANGES_OPEN` | `stash` · `blame:<ruta>` |
| `MERGEON_CHANGES_INPUT` | pasos separados por `;`: `key:` · `type:` · `move:x,y` · `click:x,y` · `dclick:x,y` · `rclick:x,y` · `scroll:x,y,dy` · `wait:<ms>` |
| `MERGEON_CHANGES_WIDTH` / `_HEIGHT` / `_TOP` | tamaño de la ventana y alto del hueco superior (121 lista, 152 pestaña de archivo; con el banner de operación, 175 y 206) |
| `MERGEON_FIXTURE` | directorio ya creado con `make-fixture.sh` |

La galería deja un hueco vacío donde la cáscara pone las pestañas, la barra y
la cabecera de la pestaña, para que las capturas se superpongan con las de
referencia. `508` (todo resuelto) es una pantalla de la vista de commits y no
se captura aquí; su estado (`merge-ready`) sí existe.

## Tests

`cargo test -p mergeon-view-changes`: 41 unitarios y 89 de interfaz
(`tests/ui`, `#[gpui_kit::test]`, ventanas sin pantalla, clics, teclas y
rueda reales, `AppModel` sobre repositorios git de verdad y un
`RecordingUiHost` que responde las confirmaciones).

| Original | Port | Notas |
|---|---|---|
| `ChangesView.test.js` (15) | `tests/ui/changes_view.rs`, `tests/ui/changes_actions.rs` | Todos. "stages the selected file, reuses embedded status, and clears stale selection" comprueba con git real que el archivo pasa al índice y la selección lo sigue (con el mock del original el estado quedaba vacío). "disables the commit form while a commit is running" usa un hook `pre-commit` lento. Los de bloque aplican el parche de verdad y miran `git diff --cached`. |
| `RepositoryChangesSidebar.test.js` (4) | `tests/ui/sidebar.rs` | Todos, más plegado de carpetas, acciones en lote según lo que hay y lista vacía. |
| `RepositoryCommitPanel.test.js` (14) | `tests/ui/commit_panel.rs`, `src/logic.rs` | Todos. "keeps the shortcut out of the button name" es `commit-action` = "Commit" (no hay `aria-keyshortcuts`). Más: un Enter a secas sigue partiendo la línea. |
| `ConflictResolver.test.js` (5) | `tests/ui/conflict_resolver.rs`, `src/conflict_resolver.rs` | Todos, más Undo, los textos de estado y un archivo de 3000 líneas con el conflicto al final. |
| `StashDiffDialog.test.js` (3) | `tests/ui/dialogs.rs` | "does not query git while hidden" no aplica (el diálogo solo existe abierto): se prueba que sin stash no se abre. Más: título con el mensaje del stash y diálogo de blame. |
| — | `tests/ui/changes_actions.rs` | Menú contextual sobre filas de `uniform_list` (copiar ruta, stage, blame, descartar con y sin confirmar, ignorar), "Discard all", resolver un conflicto de punta a punta, divisor (arrastre, máximo, ancho compartido de las pestañas y doble clic), ocultar la lista, y el desplazamiento restaurado y recordado de una pestaña. |
| — | `tests/ui/local_changes.rs` | Disposición de "Local Changes" (caja de commit bajo la lista, diff a la derecha, modo apilado, secciones a la medida de su contenido), cifras `+N −N` (lados, renombre, binario, sin seguimiento con Unicode, relectura del watcher, 2.000 archivos) y plantilla de commit (rellena sin comentarios, vuelve tras el commit, no pisa un borrador, cede ante "Amend", sigue los cambios de la plantilla). |
| — | `tests/ui/conflict_kinds.rs` | Un merge real con los siete códigos (`UU` de texto y binario, `AA` de texto y binario, `UD`, `DU`, `DD`, `AU`, `UA`): cada archivo una vez en "Conflicted files", el contador, y cada uno resuelto desde la vista (con y sin confirmación). Espacio no prepara un conflicto. |
| — | `tests/ui/stash_fixup.rs` | "Stash selected files…" (marcados o solo el del menú), "Fixup into…" (menú, commit `fixup!` real, relectura de la lista), confirmación con un commit publicado, botón apagado sin preparar o con "Amend". |
| — | `tests/ui/changes_view.rs` | Tras un commit sin push el `ahead` de la rama sube (relectura de ramas), el amend parte del mensaje de HEAD y no del último stash, y una vista que la cáscara oculta no carga hasta volver. |

Los botones de `gpui_kit::base` no publican su estado deshabilitado al árbol
de accesibilidad: lo deshabilitado se prueba por comportamiento (el clic no
hace nada).

## Diferencias deliberadas con el original

- **Borradores del resolvedor.** El original guardaba el borrador del archivo
  que se deja bajo la clave del archivo al que se llega
  (`persistCurrentDraft` lee `props.selectedFile`, ya cambiado): al saltar de
  un archivo a otro desde la lista del resolvedor, el segundo mostraba los
  bloques del primero y "Mark as Resolved" habría escrito ese contenido en el
  archivo equivocado. Aquí el borrador se guarda bajo el archivo que se deja y
  solo si tiene algún bloque decidido.
- **El resolvedor ocupa el alto del panel y hace scroll.** En el original medía
  lo que su contenido y `.file-diff` lo recortaba sin scroll: los conflictos
  más allá de la primera pantalla no se alcanzaban. En archivos cortos, la
  columna de archivos llega aquí hasta abajo (en la referencia termina con el
  contenido).
- **"Local Changes" enseña el diff al lado.** En el original (`list-only`) la
  lista iba con la caja de commit y un clic abría la pestaña del archivo; con
  la disposición de Señal el diff está a la derecha y la pestaña se abre con
  doble clic. Tampoco se elige un archivo por defecto.
- **`ChangesMode::Full`** se añade porque el original sigue teniendo ese tercer
  modo (sin `list-only` ni `file-tab`) y sus tests lo usan.
- **El blame usa el repositorio de la vista**, no el activo.
- **El amend parte del commit de HEAD.** El original leía
  `getCommits({ maxCount: 1 })`, que recorre `--all`: con un stash más
  reciente que HEAD (el repositorio de demostración lo tiene) precargaba el
  mensaje del stash y ofrecía enmendarlo. Aquí se lee `HEAD` (`search_commits`
  con `hash: "HEAD"` y `head_commit`).
- **Tras un commit sin push se releen las ramas** desde la propia vista (ver
  `ChangesViewEvent`): en el espacio de trabajo nadie atendía `commit-created`
  y el `ahead` del sidebar se quedaba viejo.
- **Tooltip sobre el menú contextual.** El nombre de un archivo enseña su ruta
  al pasar el cursor (el `title` del original). Si el cursor se queda quieto
  justo en la esquina donde se abre el menú, el tooltip puede salir encima de
  la primera entrada; en cuanto el cursor entra en el menú no aparece.
  `shots.sh` mueve el cursor 4px hacia dentro, como haría quien va a elegir.
- **Área de mensaje.** Se arma en el crate con la receta de `mergeon_ui`
  porque `mergeon_ui::textarea` no baja de 96px y el original mide 3 renglones
  (86px). No se puede estirar con el ratón (el original sí, entre 54 y 120px).

## Defectos del original portados tal cual (decidir si se corrigen)

- **"Discard" del panel de diff descarta sin preguntar** (`discardChanges` no
  llama a `confirm`, aunque el comentario de la plantilla da por hecho un
  diálogo). Solo pregunta "Discard Changes" del menú y "Discard all".
- La primera línea de cada tramo limpio del resolvedor sale con doce espacios
  de sangría y el tramo acaba con una línea en blanco (el `<pre>` de la
  plantilla lleva su propia sangría).
- Tras un commit, "Amend last commit" sigue marcado y el último commit no se
  vuelve a leer (solo se lee al montar la vista).
- Un archivo a medio preparar elegido en "Unstaged" pasa a mostrarse por su
  lado preparado tras cualquier relectura (modo `Full`).

## Lo que el encargo describía y el original no tiene

- No hay **edición manual** en el resolvedor: solo las tres salidas por bloque
  y "Undo".
- Las secciones son tres (**Conflicted Files / Unstaged / Staged**); lo que no
  se sigue va dentro de "Unstaged". No hay lista plana.
- El botón dice **Commit to main / Amend on main / Commit & Push to main /
  Amend & Push on main**.

## Diferencias visuales que quedan

- El botón de commit sale unos 4px más ancho (métricas de la fuente) y el
  `Kbd` de su atajo algo más claro sobre el azul apagado.
- El área de mensaje mide 2px menos que la del original.
- La columna de archivos del resolvedor llega hasta abajo (ver arriba).
- Las barras de desplazamiento son las de gpui-kit, superpuestas.

## Pendiente

Nada de este crate. La integración en la cáscara (`crates/mergeon`) la hace
quien la mantiene, según "Lo que la cáscara debe hacer".
