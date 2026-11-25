# API de `mergeon-app` para quien escribe vistas

`mergeon-app` es la capa entre el backend y las vistas (ver `app-layer.md`).
Expone una entidad de GPUI, `AppModel`, con el estado que tenían los stores de
Pinia y un método por cada acción de store, función de `useGit` y flujo de
`App.vue`. No pinta nada.

> Estado: completo y probado. `check --all-targets`, `clippy -D warnings`,
> `fmt` y `cargo test -p mergeon-app` (151 tests + 3 doctests, también con
> varias semillas del planificador) pasan. Ver [Tests](#tests).

## Arranque (lo hace la cáscara)

```rust
use mergeon_app::{AppModel, backend_with_events};
use mergeon_backend::BackendOptions;

let (backend, events) = backend_with_events(BackendOptions::new())?; // eventos ya encaminados
let model = AppModel::init(backend, events, cx);                     // registra el global
model.update(cx, |model, cx| model.set_ui_host(host, cx));           // avisos, confirm, prompt
model.update(cx, |model, cx| model.start(cx)).detach();              // el onMounted de App.vue
```

- `backend_with_events` fija el callback del `Backend` a un canal;
  `AppModel::init` consume el receptor y aplica cada evento en el hilo de UI
  (`handle_status_changed`, `handle_operation_event`, eventos del updater). Si
  la cáscara construye el `Backend` por su cuenta, `event_channel()` da el
  callback para `BackendOptions::on_event` y el receptor.
- Para el actualizador: `BackendOptions::updater(Arc::new(AppUpdaterBridge::new(app_updater)))`.
- La cáscara llama además a `set_window_focused(bool)` cuando la ventana gana
  o pierde el foco (watcher y auto-fetch) y a `shutdown(cx)` al salir.

## Leer y observar

```rust
let model = AppModel::global(cx);            // Entity<AppModel>
let branches = model.read(cx).active_repository().map(|repo| repo.branches.clone());

cx.observe(&model, |_this, _model, cx| cx.notify()).detach();          // "algo cambió"
cx.subscribe(&model, |this, _model, event: &AppEvent, cx| { /* ... */ }).detach();
```

Cuándo avisa el modelo (rendimiento): cada aviso de `AppModel` hace que la
ventana entera se vuelva a pintar (GPUI invalida la ventana de cualquier vista
que leyera el modelo al pintar), así que el modelo avisa poco:

- `update_repository` y los `set_repository_*` solo avisan si el repositorio
  cambió. Las relecturas (`sync_repository_read_model`, hidratar, el watcher)
  agrupan sus pasos con `notify_soon`: un aviso por vuelta del executor.
- `record_mutation` no avisa a `observe`: quien dependa de una mutación
  escucha `AppEvent::MutationRecorded` (el sidebar recarga sus secciones así).
  Un `status-watch` que no encuentra nada nuevo no avisa.
- Las lecturas en vuelo (`run_shared`) solo avisan si alguna vista pinta que
  están en vuelo (`run_single_flight`: el spinner de la barra).
- `drag_sidebar_resize` / `drag_file_list_resize` no avisan: devuelven si el
  ancho cambió y quien arrastra repinta (`window.refresh()` o su `cx.notify()`);
  el aviso llega al soltar.
- Un observador que hace algo más que `cx.notify()` debería comparar lo que lee
  antes de avisar (como `ToolbarView`, `StatusBarView` o la cáscara).

Lo que se lee (todo `&self`, sin esperas):

| Qué | Método |
|---|---|
| Repos abiertos, activo, recientes, sesión | `open_repositories()`, `active_repository()`, `active_repository_index()`, `repository(id o índice)`, `recent_repositories()`, `workspace_session()`, `repositories()` (el `RepositoriesStore` con sus getters `active_repository_*`) |
| Un repo (`OpenRepository` = `mergeon_model::Repository<StashEntry, Submodule>`) | `status`, `branches`, `remotes`, `tags`, `stashes`, `submodules`, `starred_branches`, `changes_count`, `current_branch`, `operation_state`, `conflict_state`, `mutation_queue`, `repo_lifecycle_state`, `sidebar_loading`, `sidebar_loaded`, `last_mutation_at/operation` |
| Pestañas de un repo | `repository_workspace(path)`, `commit_draft(path)` |
| Limpieza de ramas | `get_merged_branches(path, base, cx)` → `MergedBranchesReport` (candidatas `Merged` / `Squashed` / `Unmerged` con upstream borrado, base por defecto, bases posibles); `cleanup_branches(path, CleanupBranchesOptions, cx)`: una mutación `cleanup-branches` por la cola (aviso "Deleted 7 branches.", entrada en el log); con `delete_remote` autentica con la cuenta activa |
| Pestañas de repo (`tabs.rs`) | `repository_tab_labels()` / `repository_label(path)` (nombre + carpeta que distingue homónimos), `message_for_repository(path, msg)`, `repository_close_warnings(id)`, `closed_repositories()`, `is_restoring_workspace()`, `has_pending_workspace_persist()` |
| Barra de herramientas | `busy_toolbar_operation()`, `is_single_flight_running(key)`, `busy_sequence_action()`, `active_repo_mutation_queue()`, `active_branch_entry()`, `default_remote_name()`, `default_remote_branch_name()`, `active_repo_has_local_changes()` |
| Navegación | `active_view()`, `show_preferences()`, `settings_tab_open()`, `settings_section()`, `visited_repositories()` |
| Ajustes | `is_dark()`, `auto_fetch_enabled()`, `auto_fetch_interval_minutes()`, `diff_preferences()`, `sidebar_width()`, `file_list_width()`, `history_width()`, `sidebar_sections()` |
| GitHub | `github_accounts()`, `active_github_account()`, `active_github_installation()`, `has_github_accounts()`, `get_git_auth_context()`, `account_requires_installation(id)`, `account_requires_reconnect(id)`, `auth_service_available()`, `github_auth_flow()`, `github_repositories_loading()` / `_error()` |
| Pull requests (`pull_requests.rs`) | `pull_requests(repo_id)` / `active_pull_requests()` (`RepoPullRequests`: `availability`, `remote`, `pulls`, `current`/`current_for(rama)`, `loading`, `error`, `rate_limited_until`, `generation`), `active_branch_pull_request()`, `is_watching_pull_requests()` |
| Log de operaciones | `operations_log()` |
| Updater | `updater_state()` (con `is_visible()`, `is_ready()`, `is_busy()`...) |
| Resúmenes del inicio | `summary_cache()`, `repository_summary(path)`, `loading_summary_path()` |
| `useGit` | `git_is_loading()`, `git_error()` |

## Llamar acciones y esperar un `Task`

Los métodos con trabajo de fondo devuelven `Task<T>`. Git y disco corren en
hilos de fondo (`cx.background_spawn`); el resultado se aplica al modelo en el
hilo de UI.

```rust
// Sin esperar el resultado:
model.update(cx, |model, cx| model.stage(&repo_path, "README.md", cx)).detach();

// Esperándolo:
cx.spawn(async move |_this, cx| {
    let result = model.update(cx, |model, cx| model.stage(&repo_path, "README.md", cx)).await;
    if result.success() { /* ... */ }
}).detach();
```

- **Las acciones no se cancelan al soltar su `Task`**: una mutación o un flujo
  llega al final aunque nadie lo espere (como una promesa). Las lecturas
  (`get_*`, `load_commit_history`...) sí se abandonan al soltar el `Task`.
- Las mutaciones devuelven el resultado estructurado del backend
  (`Mutation<MutationResult>` = `GitMutation`, `Mutation<OperationResult>` =
  `GitOperation`...), nunca un error, y ya muestran su aviso
  (`notifyOperationResult`). Dos peticiones idénticas en vuelo comparten una
  sola llamada y un solo aviso.
- Las lecturas devuelven `QueryResult<T>`; las que en `useGit` avisaban del
  fallo lo siguen haciendo.
- Como en el original, `useGit` no toca el estado: tras una mutación la vista
  aplica el resultado y pide la relectura dirigida:

```rust
model.update(cx, |model, cx| {
    model.apply_repository_operation_result(repo_id.as_str(), &result, cx);
    model.sync_repository_read_model(
        repo_id.as_str(),
        SyncRequest::new().status(true).changes_count(true).operation("commit").result(result),
        cx,
    )
}).await;
```

  `SyncRequest` son las claves del objeto `requested` (`status`, `branches`,
  `remotes`, `tags`, `stashes`, `submodules`, `changes_count`,
  `starred_branches`, `result`, `operation`). `StructuredOutcome` (lo
  implementan todos los resultados) da `to_operation_result()` y
  `succeeded_or_noop()`.
- Los flujos de `App.vue` ya hacen ambas cosas: `handle_fetch_submit`,
  `handle_pull_submit`, `handle_push_submit`, `handle_stash` (abre el
  diálogo: `AppEvent::ShowDialog(DialogRequest::Stash { paths })`; con
  `handle_stash_paths(paths)` solo esas rutas) y `handle_stash_submit(StashRequest)`
  (`git stash push` con opciones por la cola de mutaciones y relectura de los
  stashes),
  `handle_create_branch_submit`, `handle_sequence_action`, `run_fetch`...
- `run_single_flight(key, cx, |model, cx| -> Task<()>)` y
  `run_repo_operation_single_flight(repo_id, operation, cx, ...)` son el
  `useSingleFlight` de la aplicación.

## Pull requests y checks de GitHub

`pull_requests.rs` (propio, sin original): PRs abiertos del repositorio en
github.com, el PR de la rama actual y, de cada uno, checks del último commit
(`check-runs` + `status`) y revisión. Solo consulta.

- `watch_pull_requests(cx)`: lo llaman las vistas que lo pintan (sección del
  sidebar e indicador de la barra de estado); es idempotente y, si una recarga
  de la app lo detuvo, lo vuelven a pedir al observar el modelo. Se observa a
  sí mismo y refresca el repositorio activo al cambiar de repositorio (si lo
  que hay tiene más de 5 min), de rama o upstream, de cuenta (se olvida todo),
  al terminar un fetch/pull/push (incluso sin refs nuevas) y cada 5 min solo
  con la ventana enfocada (`set_window_focused`).
- `refresh_pull_requests(repo_id, force, cx)`: en segundo plano; sin `force` no
  repite antes de 15 s; con uno en vuelo encadena otro. Lee los remotos
  (`upstream` > remoto del upstream de la rama > `origin` > el primero de
  GitHub), la lista (≤ 100), el PR de la rama (`head=dueño:rama`) y los
  detalles de los 30 PRs más recientes (4 hilos). Peticiones condicionales con
  `ETag` (una `ResponseCache` por cuenta: un 304 no gasta cupo).
- Avisa (`cx.notify()`) solo si cambia lo que se pinta (`generation` sube).
  Nunca muestra toasts: los fallos quedan en `error` (red: "Can't reach GitHub
  right now."; 401: reconectar la cuenta; cupo agotado: "GitHub rate limit
  reached. Retrying at HH:MM." y no se pregunta hasta `rate_limited_until`).
  Un 404/403 deja `availability = NoAccess` (sección oculta); sin remoto de
  github.com (o GitHub Enterprise), `NotGitHub`; sin cuenta, `SignedOut`.
- `fetch_pull_request(repo_id, número, cx) -> Task<Option<PullRequestCheckout>>`:
  trae la rama del PR por la cola de mutaciones (como `fetch`, con aviso y
  log; `GitApi::fetch_pull_request`): la rama del propio repositorio se
  actualiza en `refs/remotes/<remoto>/<rama>` y la de un fork se trae con
  `pull/<n>/head:pr-<n>`. Relee ramas y remotos y devuelve a qué hacer
  checkout (`RemoteBranch` / `LocalBranch`); el checkout lo hace la vista con
  su flujo de siempre.
- El token va solo en la cabecera `Authorization` (nunca en URLs ni en el log;
  `FetchPullRequestOptions` lo oculta en su `Debug`).

## Deshacer y rehacer

`undo.rs` (propio, sin original). Una pila por repositorio (por ruta), en el
modelo, de 50 entradas como mucho. Las mutaciones que se pueden deshacer la
alimentan solas: su canal del backend lee HEAD, la rama y los refs que toca
antes y después de correr, en su mismo turno de la cola
(`Mutation::capture`, `MutationConfig::capture(RefProbe)`), y
`invoke_structured_operation` lo convierte en una entrada
(`plan_undo(UndoIntent, RefCapture, resultado)`: pura, con tests sin git).

| Método | Qué hace |
|---|---|
| `undo(cx)` / `redo(cx) -> Task<bool>` | Deshace / rehace lo último del repositorio activo (atajos `mod+z`, `mod+shift+z`, `mod+y`; `ShortcutAction::Undo` / `Redo`; botones de la barra; paleta). |
| `run_undo(repo_path, redo, cx) -> Task<bool>` | Lo mismo para un repositorio: pide confirmación si quitaría commits que ya están en el upstream (haría falta force push; nunca hace push), corre la mutación `undo` / `redo` por la cola (sale en el log de operaciones), avisa ("Undid commit 'Fix login'.") y relee estado, ramas, tags y stashes (`MutationRecorded` con `undo` / `redo` recarga el historial). Con una operación a medias (merge, rebase…) no hace nada y lo dice. |
| `undo_action_label(repo_path, redo)` / `active_undo_label(redo)` | "Undo commit 'Fix login'", "Undo merge of feature/x", "Undo delete branch 'foo'"…; `None` sin nada. |
| `can_undo(redo)` | Hay algo, no hay otro en curso ni operación a medias (repositorio activo). |
| `is_undo_running(repo_path)`, `undo_history(repo_path, redo)` | En curso; etiquetas de la pila (de la más reciente a la más vieja). |
| `clear_undo_history(repo_path, cx)` | Olvida la pila (y borra las copias de descartes). |

Qué entra y cómo se deshace (pasos `mergeon_backend::undo::UndoStep`):

| Operación | Deshacer | Rehacer |
|---|---|---|
| commit, amend, fixup | `reset --soft` al HEAD anterior (los cambios vuelven al índice) | `reset --soft` |
| merge, rebase (también interactivo, reword/drop/squash/autosquash), cherry-pick y revert (uno o en lote), pull, fast-forward | `reset --keep` (se niega antes de pisar cambios locales: "Commit, stash or discard them first.") | `reset --keep` |
| uno de esos detenido por conflictos y cerrado con Continue | la operación entera, a donde estaba HEAD antes de empezar | |
| reset `soft` / `mixed` / `hard` | `--soft` / `--mixed` / `--keep` de vuelta (lo que `hard` borró del working tree no vuelve) | igual hacia delante |
| checkout de rama, commit o tag | checkout de vuelta, sin `-f` | checkout |
| checkout de rama remota | checkout de vuelta y borra la rama creada (si sigue igual) | la recrea con su upstream |
| crear / renombrar / borrar rama | borrar / renombrar al revés / recrear con su hash y su upstream | al revés |
| crear / borrar tag | `update-ref`; un anotado vuelve con su mismo objeto tag (mensaje, autor, firma) mientras `gc` no lo pode (≥ 2 semanas) | al revés |
| stash push / pop / apply / drop | pop (`--index`) / quita lo aplicado y `stash store` / quita lo aplicado / `stash store` | de nuevo |
| descartar archivo, "Discard all", bloque o líneas | restaura la copia guardada antes | vuelve a descartar si nada cambió |

No entran (ni invalidan la pila): push, fetch, borrar ramas o tags remotos,
limpieza de ramas, worktrees, submódulos, stage/unstage, resolver
conflictos. Dentro de `run_checkout` / `submit_create_branch`, el stash y el
pop que rodean al checkout no entran: solo el checkout (y el descarte, si se
eligió descartar).

Antes de dar un paso se comprueba que el repositorio sigue como lo dejó la
operación (HEAD y rama, el ref con el mismo valor, el stash en la lista…). Si
no (un commit o un checkout en la terminal), `UNDO_STALE`: la pila del
repositorio se vacía y se avisa ("…The repository changed outside Mergeon,
so its undo history was cleared."). `UNDO_BLOCKED` (cambios locales,
archivos que volvieron) deja la entrada donde estaba.

**Descartes.** `discard_changes`, `discard_all` y `apply_patch` con
`worktree` guardan antes una copia (`discard_*_backed_up` del backend, en el
mismo turno de la cola) en `<carpeta de settings.json>/undo-backups/<sesión>/<n>/`
(sin archivo de ajustes, en el temporal): el parche índice → working tree
(`git diff --binary`: también binarios) y una copia byte a byte de lo que no
tiene seguimiento (archivos, enlaces y carpetas). No se usa `git stash create`
+ `store`: no guarda lo que no tiene seguimiento sin tocar el working tree y
el stash aparecería en la lista. Un bloque o unas líneas no necesitan copia:
el propio parche lo es. Deshacer aplica el parche (`git apply --check` antes:
si el archivo cambió, no toca nada) y restaura las copias solo donde ya no
hay nada. Si la copia pasaría de 50 MiB (`DISCARD_BACKUP_LIMIT`, estimado
antes), se pregunta "Discard without undo?"; si se cancela, el descarte
vuelve como `CANCELLED` sin tocar nada. Las copias se borran al salir de la
pila y al cerrar la app (`shutdown`); las de sesiones de más de un día, al
arrancar.

## Eventos (`AppEvent`)

| Evento | Cuándo |
|---|---|
| `MutationRecorded { repo_id, operation }` | Cambió `lastMutationAt` (`recordMutation` o `applyRepositoryOperationResult`): recargar historial. `operation` es el `lastMutationOperation` (`commit`, `fetch`, `status-watch`, `external-head-change`, `hydrate-opened-repository`...). |
| `ActiveRepositoryChanged { repo_id }` | Cambió la pestaña activa. |
| `OpenSettings { section }` | `openSettings(section)`. |
| `FocusCommitSearch` | `mod+f` (`sidebarSearchRequest`). |
| `ViewChanged { view }` | `handleChangeView`. |
| `WorkspaceTabReplaced { repo_path, from, to }` | `onReplace` de `createFileOpener`: el panel de `from` pasa a ser el de `to`. |
| `CommitSelectionRequested { repo_id, hash }` | `show_commit_in_history` (sin original): la pestaña Commits ya está a la vista; la `CommitsView` de ese repositorio elige el commit y lo trae a la vista. |
| `ShowDialog(DialogRequest)` | Un flujo pide abrir un diálogo: `Launcher`, `GitHubAccount`, `GitHubRepositoryPicker`, `Fetch`, `Pull`, `Push`, `CreateBranch { base_reference, base_label, has_local_changes }`, `OperationsLog`, `Reflog`. |
| `ThemeChanged { dark }` | `applyTheme`: la cáscara aplica el tema (y la barra de título). |
| `Window(WindowRequest)` | `Minimize`, `Quit`, `ToggleDevTools`, `Reloaded`. |

Del backend llega además `BackendEvent::OperationProgress` (canal
`git:operation-progress`), que el modelo aplica al aviso de progreso de su
operación sin emitir ningún `AppEvent`.

Qué diálogo está abierto, la selección y el scroll son estado de cada vista.

## `UiHost`

Lo que en Vue era interfaz global. La cáscara implementa el trait y lo instala
con `set_ui_host`:

```rust
pub trait UiHost: 'static {
    fn notify(&self, toast: Toast, cx: &mut App);            // ya recortado y deduplicado
    fn show_progress(&self, toast: ProgressToast, cx: &mut App); // por defecto, nada
    fn dismiss_toast(&self, id: &ToastId, cx: &mut App);     // por defecto, nada
    fn visible_toast_ids(&self, cx: &App) -> Vec<ToastId>;   // por defecto, ninguno
    fn confirm(&self, options: ConfirmOptions, cx: &mut App) -> Answer<ConfirmValue>;
    fn confirm_with_option(&self, options: ConfirmOptions, cx: &mut App) -> Answer<ConfirmAnswer>;
    fn prompt(&self, options: PromptOptions, cx: &mut App) -> Answer<PromptAnswer>;
    fn pick_directory(&self, cx: &mut App) -> Answer<Option<PathBuf>>;
    // Por defecto nadie contesta (cuenta como cancelado).
    fn ask_credentials(&self, request: CredentialsRequest, cx: &mut App)
        -> Answer<Option<CredentialsAnswer>>;
}
```

`Answer<T>` es un `futures::channel::oneshot::Receiver<T>`; soltar el emisor
cuenta como cerrar el diálogo sin elegir. Las llamadas llegan con el modelo
tomado: la implementación no debe leer `AppModel` en la propia llamada.

Las vistas piden lo mismo a través del modelo: `notify_success/info/warning/error`,
`notify_error_with(title, &error, cx)`, `report_persistence_error`,
`confirm(options, cx) -> Task<bool>`, `confirm_with_option`, `prompt`,
`pick_directory`. Un aviso con botón propio trae
`ToastActionKind::Custom(clave)`; al pulsarlo, `run_toast_action(clave, cx)`.

Fetch, pull, push y clone (no silenciosos) muestran mientras corren un
`ProgressToast` ("Fetch origin…") con el mismo id que el aviso de su resultado,
que lo reemplaza; si no hay resultado que anunciar, se retira con
`dismiss_toast`. El aviso nace con la barra indeterminada; en cuanto la cola
arranca la operación gana el botón "Cancel" (`handle_cancel_operation`), y el
progreso real de git (`--progress`, ver abajo) lo reescribe en su sitio:
barra con el avance de la fase en curso, "45 %" junto al título y la fase en
la descripción ("Receiving objects 450/1,000 · 2.00 MiB/s", "Remote: Counting
objects 55/458"). Una fase sin porcentaje (enumerar objetos, repos locales
pequeños) vuelve a la barra indeterminada.

Progreso (`progress.rs`): el runner lee stderr en streaming cuando el `Git`
lleva `with_progress()` (fetch, pull, push y clone pasan `--progress`), separa
las líneas de progreso del resto (el stderr del resultado y los mensajes de
error quedan como sin `--progress`) y las publica en el `OperationContext`. La
cola las limita a ~10 por segundo por operación (cada cambio de fase pasa) y
las emite como `BackendEvent::OperationProgress`, siempre antes del evento final
de la operación. `handle_operation_progress` solo llama a `UiHost::show_progress`
si cambia lo que se ve (porcentaje entero, texto o botón) y **no avisa** a las
vistas del modelo: el progreso no repinta la ventana, solo el toast.
`active_progress_toasts()` lista los avisos abiertos (tests). Un pull detenido por conflictos avisa "Pull stopped: N
conflicts" con los archivos en `description` y dos botones: "Resolve" (lleva a
Local Changes de ese repositorio) y "Abort merge"/"Abort rebase" (el
`handle_sequence_action("abort")` de siempre, con su confirmación).

### Credenciales HTTPS (`credentials.rs`)

Fetch, pull, push (desde sus diálogos) y `clone_repository` pasan por
`AppModel::with_http_credentials`: si git falla porque un remoto HTTP(S) pide
usuario y contraseña (`credentials_prompt_origin(&result)`: "could not read
Username…", "Authentication failed…", 401/403; nunca SSH ni github.com con una
cuenta conectada, que sigue usando su token), se pide con
`UiHost::ask_credentials(CredentialsRequest { origin, host, operation, username,
refused, can_remember })` y se repite la operación (hasta 3 veces). Mientras
tanto el aviso de error del primer intento no se muestra; si el usuario cancela,
se muestra entonces. Las credenciales quedan en memoria por origen
(`https://host[:puerto]`) ligadas a la pestaña (o al destino del clone); al
cerrar la última pestaña que las usó se borran (`release_http_credentials`).
`http_credential_origins()` lista las de la sesión. Con "Remember"
(`can_remember` solo si hay `credential.helper` para ese origen) se guardan
además con `git credential approve` cuando la operación ya funcionó. El
auto-fetch no pregunta, pero usa las que haya. Cómo llegan a git:
`mergeon_gitcmd::credentials` (helper en línea por `-c` que lee variables de
entorno del proceso hijo; nada en argv ni en el log de operaciones). El token de
GitHub va ahora solo a github.com (`http.https://github.com/.extraHeader`).

### Notificaciones del sistema (`desktop_notifications.rs`)

Un fetch, pull, push o clone no silencioso que dura ≥ 5 s
(`desktop_notifications::LONG_OPERATION`) y termina con la ventana sin foco
publica `App::show_system_notification` ("Push origin/main finished",
"Pull origin/main failed" + el motivo). `desktop_notifications_enabled()` /
`set_desktop_notifications_enabled(bool, cx)` (clave `desktopNotifications` de
los ajustes; encendido si no está). La cáscara fija la identidad de la app
(`set_app_identity("io.github.spilveru.Mergeon", "Mergeon")`), sin la cual GPUI no publica.
Regla pura: `desktop_notifications::should_notify_desktop`.

### Remotos y tags cambiados fuera de la app

El watcher avisa con motivos propios: `fs-watch-config` / `poll-config`
(`.git/config`) y `fs-watch-refs` / `poll-refs` (`refs/tags/**`,
`refs/remotes/**`, también `packed-refs`) y `fs-watch-stash` / `poll-stash`
(`refs/stash` y su reflog `logs/refs/stash`, que cambia con cada `git stash
push`/`drop`). `handle_status_changed` los manda a
`queue_metadata_refresh(MetadataRefresh { refs, config, stash })` (agrupado
250 ms) y `refresh_active_repository_metadata` relee lo que toque: remotos
(config o refs), tags y ramas (refs), stashes (stash); con refs o stash registra
la mutación `external-refs-change` para que el historial pinte las etiquetas. Si cambió la lista de remotos, la sección de pull
requests vuelve a mirar (`refresh_pull_requests(force)`).

### Ruta escrita a mano (`local_paths.rs`)

`check_typed_repository_path(text, cx) -> Task<Result<PathBuf, TypedPathError>>`
valida en segundo plano una ruta escrita o pegada (quita comillas, expande `~`,
debe existir, ser carpeta y estar en un repositorio git) y devuelve la raíz.
`TypedPathError::message()` es el texto bajo el campo. `expand_home` vive aquí.

### Clonar desde una URL (`clone_url.rs`)

"Clone from URL" (lanzador, paleta, `mod+shift+o`): `show_clone_from_url(cx)`
emite `ShowDialog(DialogRequest::CloneFromUrl)`. Funciones puras:
`parse_clone_url(text, windows) -> Result<String, CloneUrlError>` (HTTPS/HTTP,
`ssh://`, `git://`, `file://`, `[usuario@]host:ruta` y rutas locales; sin
espacios ni `-` inicial; `CloneUrlError::message()` es el texto bajo el campo),
`repository_name_from_url` (el nombre que daría `git clone`),
`validate_folder_name`, `is_github_url`. `check_clone_destination(target, cx)`
comprueba en segundo plano que exista la carpeta y que el destino no exista o
sea una carpeta vacía. `default_clone_folder()` propone la carpeta del reciente
más nuevo. El clon es `clone_repository` (el mismo del selector de GitHub, con
el token de la cuenta activa, que solo viaja a github.com) y al terminar
`handle_repository_cloned_from(path, is_github, cx)` lo abre
(`handle_repository_cloned` es esta con `is_github = true`). Las credenciales
que traiga la URL (`https://user:token@host`) nunca llegan a avisos, título de
progreso, registro de operaciones ni log: `mergeon_gitcmd::credentials::redact_url_credentials`.

Resolver el último conflicto (el resultado deja `conflict_state.ready_to_continue`)
no muestra "Resolved …" y retira el de antes (mismo id): el banner de la operación
ya lo dice y el aviso tapaba sus botones Continue / Abort.

## Vista mínima: lista ramas y hace checkout

Este ejemplo es el doctest de `crates/mergeon-app/src/lib.rs` ("Una vista
mínima"): se compila con `cargo test -p mergeon-app --doc`.

```rust
use gpui_kit::component::button::Button;
use gpui_kit::prelude::*;
use gpui_kit::{Context, Entity, SharedString, Window, div};
use mergeon_app::{
    AppModel, CheckoutRequest, CheckoutTarget, FlowStatus, LocalChangesMode, SyncRequest,
};

pub struct BranchList {
    model: Entity<AppModel>,
}

impl BranchList {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        // Volver a pintar cuando cambie el modelo.
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        Self { model }
    }

    fn checkout(&mut self, branch: String, cx: &mut Context<Self>) {
        let model = self.model.clone();
        let Some(repo_id) = model.read(cx).active_repository().map(|repo| repo.id.clone())
        else {
            return;
        };

        cx.spawn(async move |_, cx| {
            // El flujo aplica cada resultado al repositorio y muestra sus avisos.
            let outcome = model
                .update(cx, |model, cx| {
                    model.run_checkout(
                        repo_id.as_str(),
                        CheckoutRequest {
                            label: branch.clone(),
                            target: CheckoutTarget::Reference(branch),
                            local_changes_mode: LocalChangesMode::Keep,
                        },
                        cx,
                    )
                })
                .await;
            if outcome.status == FlowStatus::Aborted {
                return;
            }

            // Qué releer lo decide la vista, como en Sidebar.vue.
            let needs_status = outcome
                .result
                .as_ref()
                .is_none_or(|result| result.status().is_none());
            let mut request = SyncRequest::new()
                .branches(true)
                .status(needs_status)
                .changes_count(needs_status)
                .stashes(outcome.stashed_changes)
                .operation("checkout");
            if let Some(result) = outcome.result {
                request = request.result(result);
            }
            model
                .update(cx, |model, cx| {
                    model.sync_repository_read_model(repo_id.as_str(), request, cx)
                })
                .await;
        })
        .detach();
    }
}

impl Render for BranchList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let branches = self
            .model
            .read(cx)
            .active_repository()
            .map(|repo| repo.branches.clone())
            .unwrap_or_default();

        div()
            .flex()
            .flex_col()
            .gap_1()
            .children(branches.into_iter().map(|branch| {
                let name = branch.name.clone();
                Button::new(SharedString::from(format!("checkout-{name}")))
                    .label(branch.name)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.checkout(name.clone(), cx);
                    }))
            }))
    }
}
```

Las ramas llegan al modelo con la hidratación al abrir el repositorio; para
pedirlas desde una vista que las necesite antes:
`ensure_repository_sections_loaded(repo_id, SyncRequest::new().branches(true), cx)`.

## Tests de vistas

`mergeon_app::testing`: `RecordingUiHost` (registra avisos y preguntas, responde
lo programado: `answer_confirm`, `answer_prompt`, `answer_directory`,
`hold_answers`...), `TestBackend` (store en memoria, watcher silencioso,
`RecordingLauncher`, `FakeGitHubTransport`). El test debe llamar a
`cx.executor().allow_parking()`: los eventos del backend llegan desde otros
hilos y el planificador de tests de GPUI los rechaza si no.

### Añadidos sin canal en el original (selección múltiple y worktrees)

| Método | Qué hace |
|---|---|
| `cherry_pick_commits(repo, &[sha], cx)` | `git cherry-pick a b c` (del más viejo al más nuevo); operación `cherry-pick`. |
| `revert_commits(repo, &[sha], single_commit, cx)` | `git revert --no-edit …` (del más nuevo al más viejo) o, con `single_commit`, `--no-commit` y un único commit; operación `revert`. |
| `get_worktrees(repo, cx)` | `Vec<Worktree>` (ruta, HEAD, rama, principal, actual, bare, detached, locked, prunable). Sin avisar. |
| `worktree_has_changes(ruta, cx)` | `bool`, lectura barata. Sin avisar. |
| `add_worktree(repo, AddWorktreeOptions, cx)` | `worktree-add` (rama existente o nueva desde un punto). |
| `remove_worktree(repo, ruta, force, cx)` | `worktree-remove`. |
| `set_worktree_locked(repo, ruta, lock, motivo, cx)` | `worktree-lock` / `worktree-unlock`. |
| `prune_worktrees(repo, cx)` | `worktree-prune`. |

## Tabla función original → método

`useGit.js` → `git_client` (mismo nombre en snake_case): `inspect_repository`,
`init_repository`, `is_valid_repository`, `get_commits`, `search_commits`,
`resolve_ref`, `get_identity`, `set_identity`, `get_reflog`, `get_commit_files`,
`apply_patch`, `get_tree_entries`, `get_file_at_commit`, `get_diff`,
`get_branches`, `get_remotes`, `add_remote`, `remove_remote`, `rename_remote`,
`set_remote_url`, `test_remote_connection`, `clone_repository`, `get_status`,
`get_operation_state`, `get_tags`, `get_tag_details`, `check_merge_conflicts`,
`get_stashes`, `get_stash_files`, `get_stash_diff`, `get_submodules`,
`update_submodules`, `create_tag`, `delete_tag`, `push_tag`,
`delete_remote_tag`, `checkout_tag`, `fetch`, `pull`, `push`, `checkout`,
`checkout_remote_branch`, `create_branch`, `rename_branch`, `delete_branch`,
`delete_remote_branch`, `fast_forward_branch`, `merge`, `rebase`,
`compare_branches`, `stash`, `apply_stash`, `pop_stash`, `stash_branch`,
`drop_stash`, `has_uncommitted_changes`, `setup_tracking`, `reset`, `revert`,
`cherry_pick`, `stage`, `stage_all`, `unstage`, `unstage_all`,
`discard_changes`, `discard_all`, `commit`, `add_to_ignore`, `blame`,
`read_file`, `read_conflict_versions`, `resolve_conflict`, `resolve_conflict_with`,
`get_commit_diff`, `read_image_pair` (las dos versiones de una imagen:
`ImagePairRequest::{Commit, Range, WorkingTree}`, sin aviso), `rebase_action`,
`interactive_rebase`, `sequence_action`. `silent` es un parámetro (`fetch`,
`push`, `merge`, `commit`, `add_remote`); `authContext` es el `auth_token` de
las opciones (`model.git_auth_token()`).

| Original | Método de `AppModel` |
|---|---|
| `useRepositorySync`: `syncRepositoryReadModel`, `ensureRepositorySectionsLoaded`, `recordMutation` | `sync_repository_read_model`, `ensure_repository_sections_loaded`, `record_mutation` |
| store `repositories`: `loadFromStorage`, `persistWorkspaceSession`, `addRepository`, `removeRepository`, `setActiveRepository`, `updateRepositoryData` | `load_repositories_from_storage`, `persist_workspace_session`, `add_repository`, `remove_repository`, `set_active_repository`, `update_repository(key, cx, \|repo\| ..)` |
| `setRepositoryMutationQueue`, `applyRepositoryOperationEvent`, `setRepositoryStatus`, `applyRepositoryOperationResult`, `setRepositorySidebarLoading`, `setRepository{Branches,Remotes,Tags,Stashes,Submodules}Data`, `setRepositoryStarredBranches`, `invalidateRepositorySidebarData/Sections`, `invalidateRepositoryState`, `updateRepositoryBranch`, `removeRecentRepository`, `clearRecentRepositories` | los mismos nombres en snake_case |
| `getRepositoryWorkspace`, `saveRepositoryWorkspace` | `get_repository_workspace` / `repository_workspace`, `save_repository_workspace` |
| `RepositoryWorkspace.vue`: `activate`, `openFile`, `close`, `showHome`, `navigateTabs`, `rememberScroll`, `flush`, `state.selectedCommit = ...` | `activate_workspace_tab`, `open_workspace_file`, `close_workspace_tab` (+ `close_active_workspace_tab`), `show_workspace_home`, `navigate_workspace_tabs`, `remember_workspace_scroll`, `flush_workspaces`, `set_workspace_selected_commit`; genérico: `update_repository_workspace` |
| `storage.addStarredBranch/removeStarredBranch` | `add_starred_branch`, `remove_starred_branch` |
| store `operationsLog` | `operations_log()`, `mark_operations_seen`, `clear_operations_log` (lo alimentan los eventos y `apply_repository_operation_result`) |
| `App.vue`: `onMounted`, `reloadApplication` | `start`, `reload_application` |
| `showHomeTab`, `selectRepositoryTab`, `closeSettingsTab`, `openSettings`, `handleChangeView`, `addAccountFromSettings`, `closeAccountDialog`, `handleAccountAdded` | mismos nombres |
| `hydrateOpenedRepository`, `restoreWorkspaceSession`, `syncActiveRepositoryStatus`, `queueActiveRepositoryStatusSync`, `watchActiveRepositoryStatus`, `stopWatchingRepositoryStatus`, `handleStatusChanged`, `handleOperationEvent` | mismos nombres; `mergeStatusWatchReason` / `isHeadStatusWatchReason` son funciones de `shell` |
| `handleNonGitRepositoryOpen`, `openRepositoryPath`, `showRepositoryOptions`, `openLocalRepository`, `openGitHubPicker`, `handleRepositoryCloned`, `handleOpenRepository` | mismos nombres |
| `handleSequenceAction`, `handleCancelOperation`, `handleFetch/Pull/Push`, `promptForOriginRemote`, `runFetch`, `handleFetchSubmit`, `runAutoFetch`, `scheduleAutoFetch`, `handlePullSubmit`, `handlePushSubmit`, `handleStash`, `handleNewBranch`, `handleCreateBranchSubmit`, `handleOpenTerminal`, `handleOpenExplorer` | mismos nombres |
| `useKeyboardShortcuts([...])` | `handle_shortcut(ShortcutAction, cx)`; `request_minimize`, `request_quit` |
| handlers `focus` / `blur` | `set_window_focused(bool, cx)` |
| `useSingleFlight` | `run_single_flight`, `is_single_flight_running`, `run_repo_operation_single_flight` |
| `useCheckoutFlow.runCheckout`, `useCreateBranchFlow.submitCreateBranch` | `run_checkout(repo, CheckoutRequest, cx)`, `submit_create_branch(repo, CreateBranchRequest, cx)` |
| store `github`: `loadFromStorage`, `initialize`, `addAccount`, `refreshAccount`, `removeAccount`, `setActiveAccount`, `setActiveInstallation`, `updateAccount` | `load_github_accounts_from_storage`, `initialize_github`, `add_account`, `refresh_account`, `remove_account`, `set_active_account`, `set_active_installation`, `update_account` |
| `useGitHubAuthFlow`: `startLogin`, `loginWithToken`, `openInstallPage`, `openVerificationPage`, `copyUserCode`, `cancel`, `reset` | `start_login`, `login_with_token`, `open_install_page`, `open_verification_page`, `copy_user_code`, `cancel_login`, `reset_login`; estado en `github_auth_flow()` |
| `useGitHubRepositories`: `listRepositories`, `listAllRepositories`, `searchRepositories`, `cancelRepositoryRequests` | mismos nombres |
| `useTheme`: `loadTheme`, `toggleTheme` | `load_theme`, `toggle_theme`, `set_dark` |
| `useAppSettings`: `loadAppSettings`, `saveAutoFetchSettings` | mismos nombres |
| `useDiffPreferences`: `loadDiffPreferences`, `toggleDiffView`, `toggleIgnoreWhitespace` | mismos nombres; además `toggle_whole_file` ("Whole file", `commitView.wholeFile`, `DiffPreferences::whole_file`; avisa como los otros y quien muestra un diff lo vuelve a pedir con `DiffOptions::whole_file`) |
| `useSidebarWidth`: `load`, `start`, `move`, `stop`, `nudge`, `reset` | `load_sidebar_width`, `start_sidebar_resize`, `drag_sidebar_resize` (sin `cx`: devuelve si hay que repintar), `stop_sidebar_resize`, `nudge_sidebar_width`, `reset_sidebar_width` |
| `useFileListWidth` (+ `save`) | `load_file_list_width`, `start/drag/stop_file_list_resize`, `nudge_file_list_width`, `reset_file_list_width`, `save_file_list_width` |
| `CommitView.vue`: `persistHistoryWidth`; `Sidebar.vue`: `persistSidebarSections` | `load_history_width`, `persist_history_width`; `load_sidebar_sections`, `update_sidebar_sections` |
| store `updater` + `useUpdater`: `apply`, `handleEvent`, `check`, `install`, `activate`, `openReleases` | `apply_updater_state`, `handle_updater_event`, `check_for_updates`, `install_update`, `activate_update`, `open_releases`, `load_updater_state` |
| `useRepositorySummary` | `load_repository_summary`, `clear_repository_summary`, `retain_repository_summaries` |
| `CommitView.vue`: `performLoadCommits`, `performLoadMoreCommits`; búsqueda de la barra lateral | `load_commit_history(path, CommitHistoryRequest, cx)`, `load_more_commit_history`, `search_commit_history` (filas `CommitRow` con grafo y pertenencia) |
| `shell.openExternal`, `shell.showItemInFolder` | `open_external`, `show_item_in_folder` |
| Comparación navegable de dos revisiones (nuevo, sin original) | `compare_revisions(path, base, head, CompareMode, CompareLimits, cx)` (fija A y B a su SHA y trae commits y archivos), `get_comparison_files(path, &revisions, mode, limit, cx)` (otro modo sobre los mismos SHA), `get_comparison_diff(path, &revisions, mode, &ComparedFile, DiffOptions, cx)`. No avisan ni tocan el estado del modelo: la vista presenta el fallo. `search_commits` acepta además `skip`, `path`, `branch`, `since`, `until`, `code` y `code_regex` en `CommitSearchOptions`. |
| "File history" y blame navegable (nuevo, sin original) | `get_file_history(repo, ruta, FileHistoryOptions { revision, skip, max_count }, cx)` (páginas de 100 siguiendo renombres; error como dato), `get_file_revision_diff(repo, hash, ruta, ruta_anterior, DiffOptions, cx)`, `blame_at_revision(repo, revisión, ruta, cx)`, `show_commit_in_history(repo_id, hash, cx) -> bool` (activa el repositorio y Commits, emite `CommitSelectionRequested`) |
| Editor externo (nuevo, sin original) | `external_editor`, `save_external_editor`, `detect_editors`, `open_in_editor(path, line)` (sin editor elegido ni detectado abre con el programa del sistema y lo dice con un aviso; si falla, aviso con el error; los dos con el botón "Choose editor…", clave `CHOOSE_EDITOR_ACTION`, que abre Ajustes → Git; en Linux no se espera a `xdg-open` más de 2 s, `ProcessLauncher::run_for`: sin escritorio ejecuta el programa en primer plano y antes la app se quedaba esperando a que se cerrara, sin ningún aviso), `open_working_tree_file_in_editor(repo, ruta, line)` (avisa si ya no existe), `handle_open_in_editor` (repositorio), `open_active_in_editor` (atajo `mod+shift+e`) |

## Diferencias deliberadas

- Las mutaciones devuelven el tipo del backend (`Mutation<T>`), no un objeto
  suelto; `createIpcErrorResult` y `createSimpleOperationResult` no existen
  porque el backend nunca lanza ni devuelve vacío. El uso posicional antiguo de
  `fetch/pull/push` no es expresable.
- `isLoading` / `error` de `useGit` eran por instancia del composable; aquí hay
  uno para toda la app (`git_is_loading`, `git_error`).
- `add_account`, `add_repository`, `remove_recent_repository`... devuelven
  `anyhow::Result` donde el original dejaba propagar la excepción del IPC.
- Al restaurar la sesión el repositorio activo se hidrata una vez (en el
  original el watcher de `activeRepository.id` podía hidratarlo dos veces).
- `load_theme` no vuelve a guardar el tema que acaba de leer.
- `run_checkout` con modo `discard`: si no se pueden inspeccionar los cambios
  locales, avisa y aborta (el original dejaba escapar la excepción).
- Las rutas de `git:status-changed` y `git:operation-event` se comparan
  normalizadas con la del repositorio (barras de Windows).
- Pestañas de repositorio (propio, no del original): `request_close_repositories(ids)`
  pide confirmación si hay borrador de commit u operación en curso;
  `close_other_repositories`, `close_repositories_to_the_right`,
  `reopen_closed_repository`, `cycle_repository_tab`,
  `select_repository_tab_number`, `close_current_tab` (`mod+w`),
  `close_active_repository`, `move_repository_tab`. Al ocultar un repositorio
  se guarda la huella de sus refs y, si cambió al volver, se registra
  `external-head-change`.
- Estado "de vista" que cambia a menudo (scroll de pestaña, commit
  seleccionado, `set_commit_draft`): `update_repository_workspace_quietly`, sin
  `cx.notify()` y con el guardado agrupado (`WORKSPACE_PERSIST_DELAY`, 750 ms).
- El estado de pestañas de cada repositorio vive solo en la sesión del modelo
  (el original tenía una copia reactiva por componente); `visited` y `slots`
  quedan en la vista.
- `reload_application` reinicia el estado y vuelve a ejecutar `start` (no hay
  ventana web que recargar) y emite `Window(Reloaded)`.
- `listAllRepositories`: `account_login` del fallo sale de la clave
  `accountLogin` de la instalación, que el cliente de GitHub no rellena (igual
  que en el original, donde siempre quedaba vacío).

## Tests

`cargo test -p mergeon-app`: 151 tests en `src/tests/` y en los módulos, sobre
repositorios git reales en directorios temporales, `Store::in_memory`, un
transporte de GitHub guionizado y `RecordingUiHost`.

| Original | Port | Notas |
|---|---|---|
| `useRepositorySync.test.js` | `tests/repository_sync.rs` (5 + 2) | |
| `useGit.test.js` | `tests/git_client.rs` (7 + 4), `tests/git_client_smoke.rs` | "rejects legacy positional usage" no aplica: las opciones son un tipo. |
| `useCreateBranchFlow.test.js` | `tests/create_branch_flow.rs` (5 + 3), `flows::tests` | "notifies when the rollback pop itself fails" no se puede provocar con git real; se cubre el aviso hermano (pop fallido tras crear la rama). `stash@{0}` de respaldo: test unitario de `stash_ref_of`. |
| `useGitHubAuthFlow.test.js` | `tests/github_auth_flow.rs` (8 + 2) | |
| `useGitHubRepositories.test.js` | `tests/github_repositories.rs` (6 + 2) | |
| `stores/github.test.js` | `tests/github_store.rs` (9 + 4), `github::tests` | |
| `useTheme.test.js` | `tests/settings.rs` (3 + 7 del resto de ajustes) | `setTitleBarTheme` es el evento `ThemeChanged`. |
| `stores/repositories.test.js` (lo no puro) | `tests/repositories_store.rs` (3 + 4) | Lo puro está en `mergeon-model`. |
| `App.test.js` (lógica) | `tests/app.rs` (15 + 4) | Se comprueba el efecto de cada relectura (mutación registrada, secciones leídas, estado del repo) en vez de los argumentos de un mock. |
| — | `tests/undo.rs` (15), `undo::tests` (8) | Deshacer / rehacer con git real: commit, amend, fixup, merge (también detenido por conflictos y cerrado con Continue), rebase, acción de rebase, cherry-pick, revert, reset, pull, checkout, ramas, tags anotados, stash push/pop/drop, descartes (archivo sin seguimiento, todo, líneas), cambio externo que vacía la pila, `--keep` que se niega, commits publicados (confirmación), operación a medias y atajos. Los pasos sueltos, en `mergeon-git-ops` (`tests/undo.rs`, 19). |
| — | `tests/full_flow.rs` | abrir → hidratar → stage → commit → crear rama → push a un remoto bare → fetch sin cambios no marca mutación. |
| — | `tests/pull_requests.rs` (8), `pull_requests::tests` (6) | PRs con GitHub guionizado: lista con checks y revisión, sin cuenta (no se llama a GitHub), fuera de GitHub, sin acceso, red caída sin toasts, cupo agotado, `ETag`/304 sin repintar, cambio de rama, fetch de la rama de un fork. |
| — | `tests/checkout_flow.rs`, `tests/shell.rs`, `tests/workspace.rs`, `tests/history.rs`, `tests/summary.rs`, `tests/updater.rs`, `tests/ui_host.rs` | Humo de lo que no tenía test original. |

Cosas que los tests dejaron claras y conviene saber al escribir vistas:

- El primer `git fetch` de un repositorio recién clonado o publicado crea
  `refs/remotes/origin/HEAD`: ese fetch sí mueve refs y registra mutación; los
  siguientes sin novedades, no.
- El auto-fetch periódico lo programa `start` (y cada cambio de sus ajustes).
  Un modelo al que no se le llame `start` no lo tiene.
- `fast_forward_branch` solo adelanta la rama en la que se está ("Switch to
  main before fast-forwarding it.").
- Buscar commits sin ningún criterio devuelve una lista vacía sin ejecutar git.

## Pendiente para las vistas

No es de esta capa: selección y reconciliación del commit seleccionado,
coalescing de `loadCommits`, `visited` / `slots` de las pestañas, límites del
ancho del historial (`clampHistoryWidth`), el estado de los diálogos y el
`UiHost` real de la cáscara (más aplicar `ThemeChanged` y `Window(...)`, y
llamar a `set_window_focused` y `shutdown`).
