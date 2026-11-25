# Capa de aplicación

Entre el backend (crates sin UI) y las vistas (GPUI) hay dos capas. Juntas
reemplazan a `electron/preload.js` + `electron/ipc/*` y a los stores y
composables de Vue (`src/stores`, `src/composables`, el `<script>` de `App.vue`).

```
vistas (mergeon-view-*, mergeon)
        │  observan y llaman
        ▼
mergeon-app      AppModel (Entity de GPUI): estado global + flujos async   ← src/stores, src/composables, App.vue
        │
        ▼
mergeon-backend  Backend: equivalente síncrono de `window.api`            ← electron/preload.js, electron/ipc/*
        │
        ▼
mergeon-git-ops · mergeon-git-query · mergeon-runtime · mergeon-store · mergeon-github · mergeon-updater · mergeon-gitcmd
```

## `mergeon-backend` (sin GPUI)

Un `Backend` (`Send + Sync`, se comparte con `Arc`) que expone **un método por
canal IPC** de `electron/preload.js`, con el mismo nombre en snake_case, los
mismos argumentos y la misma forma de resultado:

- `backend.git()` — los 79 canales `git:*`. Las lecturas delegan en
  `mergeon-git-query` y devuelven `Result`. Las mutaciones pasan por la cola de
  `mergeon-runtime` exactamente como lo hace `registerRepoMutationHandler` en
  `electron/ipc/git-handlers.js` (misma `operation`, misma clave de dedupe, mismo
  timeout local/red, misma política de cancelación) y devuelven siempre el
  resultado estructurado de `mergeon-git-ops`, nunca `Err`. Aquí viven también los
  handlers que mezclan lógica propia con una operación (clone, initRepository,
  addToIgnore, setupTracking, watchStatus/unwatchStatus/setStatusWatchWindowFocus,
  cancelOperation, getOperationQueue...).
- `backend.storage()` — `mergeon_store::Store`.
- `backend.github()` — `mergeon_github::GitHubClient`.
- `backend.shell()` — `shell:*` (`electron/ipc/shell-handlers.js`): abrir URL,
  abrir ruta, mostrar en el explorador, abrir terminal; nativo por plataforma.
- `backend.updater()` — `mergeon_updater::AppUpdater`.
- Eventos hacia arriba (`git:status-changed`, `git:operation-event`,
  `updater:event`): un único `BackendEvent` entregado por un callback
  `Fn(BackendEvent) + Send + Sync` que se fija al construir. La capa de arriba lo
  reenvía al hilo de UI.

Todo es síncrono y bloqueante; lo llama `mergeon-app` desde hilos de fondo.
`dialog:openDirectory` y `app:*` (minimizar, salir, recargar, tema de la barra de
título) no van aquí: son de la ventana y los resuelve la capa GPUI.

## `mergeon-app` (GPUI, sin render)

- `AppModel`: una `Entity` de GPUI con el estado que hoy tienen los stores de
  Pinia (`repositories`, `github`, `operationsLog`, `updater`) y los ajustes.
  `AppModel::init(backend, cx)` la crea y la registra; `AppModel::global(cx)`
  devuelve el `Entity<AppModel>`. Las vistas la observan con `cx.observe` y leen
  con `model.read(cx)`.
- Los repositorios abiertos son `mergeon_model::Repository` (las transformaciones
  puras ya están ahí). `AppModel` añade lo que persiste o llama al backend.
- Cada acción del store, cada función de `useGit` y cada flujo de `App.vue` /
  composables es un método de `AppModel` (o de un sub-objeto suyo) con el mismo
  nombre en snake_case. Los que hacen trabajo async devuelven `Task<T>`:
  lanzan el backend con `cx.background_spawn`, y al volver aplican el resultado
  al modelo y hacen `cx.notify()`. El orden de pasos, los `sync` posteriores y
  los textos de notificación son los del original.
- Lo que en Vue es UI global se pide a través de un trait inyectado, `UiHost`:
  notificaciones (`notify.success/error/warning/info` con sus opciones),
  `confirm`, `prompt` y el selector de directorio. La cáscara instala la
  implementación real; los tests usan una que registra lo pedido y responde lo
  que el test indique. Así los flujos quedan lineales con `await`.
- Lo que una vista necesita saber además de "algo cambió" se emite como evento
  (`cx.emit(AppEvent::...)`, `AppModel: EventEmitter<AppEvent>`): mutación
  registrada en un repo (para recargar historial), petición de abrir Settings en
  una sección, petición de foco en la búsqueda de commits, etc.
- Los eventos del backend llegan por un canal y se aplican en el hilo de UI
  (estado → modelo, operación → log y cola, updater → estado del updater).
- Estado puramente visual (qué diálogo está abierto, selección, scroll) no va
  aquí: es de cada vista.

Tests: `#[gpui_kit::test]` con repos git reales en temporales y un `UiHost` de
prueba; se portan los tests de vitest de stores y composables.
