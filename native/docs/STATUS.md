# Estado del port a GPUI

Bitácora breve del port (qué está hecho, dónde y qué falta). Se actualiza al
cerrar cada pieza.

## Backend

| Crate | Estado | Notas |
|---|---|---|
| `mergeon-gitcmd` | hecho, 19 tests | Ejecutor `Git`, `OperationContext`, estado estructurado. |
| `mergeon-github` | hecho, 47 tests | `GitHubClient` (device flow, refresh, PAT, repos). Transporte `HttpTransport` + `UreqTransport`. Config por env → `.env` explícito → `option_env!` → default. |
| `mergeon-git-ops` | hecho, 160 tests | 43 operaciones + identidad (con `stash_push_operation` y el `autosquash` de `rebase_action_operation`); 234 escenarios comparados contra el JS real. Editores de rebase como scripts `sh` temporales (sin Node). |
| `mergeon-git-query` | hecho, 198 tests | 27 canales de lectura, paridad verificada contra los handlers JS reales. Además, sin canal del original: `get_working_tree_stats` (cifras `+/−` de los cambios locales), `get_commit_template` (`commit.template`), `get_fixup_candidates` (destinos de "Fixup into…"), `get_file_history` / `get_file_revision_diff` ("File history": `git log --follow -z`, paginado releyendo desde el principio porque `--skip` rompe `--follow`) y `blame_at_revision` (con `previous`/`filename` del porcelain). |
| `mergeon-runtime` | hecho, 77 tests | `RepositoryMutationGate` (dedupe, cancelación, timeout, eventos) y `RepositoryStatusWatcher` (crate `notify`, debounce, foco, huella de HEAD). |
| `mergeon-store` | hecho, 84 tests | `Store` sobre `settings.json` (importa recientes/sesión/ajustes/destacadas del `config.json` de Electron; las cuentas de GitHub no se importan). `SecretCipher` inyectable (falta la implementación con llavero del sistema). Corrige a propósito un bug del original: al guardar una cuenta refrescada gana el token nuevo. |

| `mergeon-updater` | hecho, 142 tests | `AppUpdater` (manifiestos `latest*.yml` de electron-builder, semver, descarga con sha512 y reanudación, instaladores NSIS/AppImage/zip de macOS tras trait). Sin probar en Windows/macOS reales. macOS queda en solo-aviso por defecto. |
| `mergeon-backend` | hecho, 109 tests | `Backend` = `window.api`: `git()` (81 canales, mutaciones por la cola con `Mutation<T>`), `storage()`, `github()`, `shell()`, `updater()` (trait `UpdaterBridge`, falta enchufar `AppUpdater`), `BackendEvent`. `KeychainCipher` (llavero del sistema + AES-GCM). Tabla canal→método en `src/channels.rs`. |

| `mergeon-model` | hecho, 254 tests | Lógica pura del renderer: diff, patch, file_tree, format, workspace_tabs, notify, dialogs, commit_view_utils (grafo), repositories (`Repository`, `RepositoriesStore`), operations_log, keyboard_shortcuts, anchos. Usa tipos espejo (`operation_types`, `repository_read_model`, `RawCommit`, `RecentRepository`) que la capa de app convierte desde runtime/git-ops/git-query/store. |

## Empaquetado y CI

| Pieza | Estado | Notas |
|---|---|---|
| `native/packaging/` + `ci.yml` / `release.yml` | Linux hecho y verificado; Windows y macOS escritos sin probar en su sistema | AppImage y `.deb` (glibc 2.35, en contenedor), instalador NSIS, `Mergeon.app` + zip, manifiestos `latest*.yml` comprobados con el parser del actualizador. Ver `docs/packaging.md` (incluye lo pendiente). |

## Interfaz

| Pieza | Estado | Notas |
|---|---|---|
| `mergeon-ui` tema/fuentes/íconos | hecho | Tema «Señal» (claro y oscuro) generado con `tools/gen-theme.py`; Geist + Geist Mono embebidas; `Lucide` = catálogo completo (trazo 1,5 con terminal recto). La paridad visual con Electron de `native/e2e/` deja de cumplirse a propósito. |
| `mergeon-ui` componentes base | hecho, 58 tests | Los 18 componentes de `src/components/ui/*`; catálogo en `docs/ui-components.md`; galería `examples/gallery.rs` comparada contra la referencia Electron (`target-shots/reference-ui/`). |
| Capturas de referencia de la app Electron | retirado (2026-11-12) | Servían para la paridad visual; se eliminaron junto con la app Electron (`reference-shots.mjs` y `reference-ui-shots.mjs` siguen en el historial de git). |

Ver `docs/ui-port.md` (convenciones) cuando exista.

## Defectos heredados de la app Electron

Se portaron tal cual y después se corrigieron en los crates de backend (2026-10-02).
Cada corrección tiene un test con git real que fallaba con el comportamiento
anterior; los tests que fijaban el defecto se reescribieron con un comentario
que lo explica.

- **Corregido** — `checkMergeConflicts` nunca detectaba conflictos. Ahora decide
  por código de salida de `merge-tree --write-tree --name-only -z` (`Git::exec`):
  0 es limpio, 1 con un árbol en stdout son conflictos, y lo demás es error de
  lectura (un repo que ya no existe ya no sale como `hasConflicts: true`). Las
  rutas salen del bloque de rutas, no de los mensajes "Auto-merging…"/"CONFLICT…".
- **Corregido** — `getCommitFiles` / `getStashFiles`: `--name-status` y `--numstat`
  con `-z`. Renombres y copias con sus +/− reales y rutas no ASCII literales.
- **Corregido** — `getTagDetails` de un tag inexistente falla con
  `Failed to get tag details: Tag '<nombre>' not found.`
- **Corregido** — `getRepositorySummary` en un repo sin commits devuelve el
  resumen con `commitCount: 0` y fechas `null`.
- **Corregido** — `searchCommits`: el hash va tras `--end-of-options`.
- **Corregido** — `rebaseAction`: el editor de secuencia busca la línea por
  cualquier abreviatura del hash completo (resuelto antes con `rev-parse`), así
  que `drop` funciona con cualquier `core.abbrev`; si el commit no está en el
  todo-list (otra rama) el rebase se cancela y la acción vuelve `blocked`
  ("Commit <hash> is not part of the current branch history.") en vez de
  anunciar éxito. `squash` arranca un commit antes (o con `--root`) y acepta el
  mensaje combinado sin editor; squashear la raíz se bloquea. `drop`/`reword`
  sobre la raíz usan `--root`.
- **Corregido** — `fetch` ejecuta `git fetch <remoto>`; "Fetch all" trae cada remoto.
- **Corregido** — `push` lee el upstream de la rama (`for-each-ref
  %(upstream)`): `-u` y `UPSTREAM_CREATED` solo si no tenía.
- **Corregido** — `drop` con hashes de más de 7 caracteres (ver `rebaseAction`).
- **Corregido** — `sequenceAction "continue"` decide por código de salida:
  con conflictos sin resolver vuelve `blocked` / `CONFLICTS_REMAIN`.
- **Corregido, falta enchufarlo** — `deleteRemoteBranch` con token: nuevas
  `mergeon_git_ops::delete_remote_branch_with_auth_operation` y
  `GitApi::delete_remote_branch_with_auth(repo, remoto, rama, token)`. Las
  funciones de antes siguen igual (sin token); `mergeon-app`
  (`git_client.rs::delete_remote_branch`) tiene que pasar a la nueva con
  `git_auth_token()`.
- **Corregido en el backend, falta en la vista** — "Interactive rebase from
  here…" con un stash: el rango de `CommitsView::open_interactive_rebase` son
  todas las filas por encima del commit, y `git log --all` pone encima de HEAD
  el merge de `refs/stash` (y commits de otras ramas). Nueva
  `mergeon_model::commit_view_utils::interactive_rebase_range(commits, hash)`
  que sigue los padres desde HEAD y devuelve el rango o el error con el mismo
  aviso (`InteractiveRebaseRangeError::message`); `mergeon-view-commits` debe
  usarla. La otra mitad ("Amend last commit" precargaba el mensaje del stash)
  se resolvió en la vista: `mergeon-view-changes` pide el commit de `HEAD` con
  `search_commits` y lo toma con `logic::head_commit()`. Un test de
  `mergeon-git-query` cubre que, con un stash, esa búsqueda devuelve HEAD.
- **Corregido** — Branches durante un rebase: `get_branches` ignora las líneas
  `(no branch, …)` de `git branch`. Un repo sin commits (o una rama huérfana)
  devuelve su rama en `current` (`symbolic-ref HEAD`), así que la interfaz ya no
  cae en "Detached · HEAD"; `normalize_current_branch_from_status` no necesitaba
  cambios (`git status` ya nombraba la rama).
- Sin corregir — sin empaquetar, "Version" en About muestra la versión de
  Electron: no aplica a la app nativa.

Otros cambios de la misma tanda: `mergeon_model::collate` es público
(`locale_compare`), y `mergeon-updater` usa `option_env!("APP_UPDATE_FEED_URL")`
como feed de respaldo (`BUILT_IN_UPDATE_FEED_URL`, `resolve_update_feed_url_or`,
`UpdaterConfig::from_env_with_built_in_feed`).

## Vistas

| Crate | Estado |
|---|---|
| `mergeon-app` | hecho, 151 tests (ver `docs/app-api.md`) |
| `mergeon-view-diff` | hecho, 75 tests (`docs/view-diff.md`): `DiffViewer`, `DiffPanel`, `FileContentViewer`, `BlameViewer`, `FileTreeRow` (con las barritas de `file_stats`), resaltado tree-sitter |
| `mergeon-view-sidebar` | hecho, 77 tests (`docs/view-sidebar.md`); integrado en la cáscara |
| `mergeon-view-home` | hecho, 80 tests (`docs/view-home.md`); integrado en la cáscara |
| `mergeon-view-toolbar` | hecho, 83 tests (`docs/view-toolbar.md`); ya integrado en la cáscara |
| `mergeon-view-commits` | hecho, 122 tests (`docs/view-commits.md`); integrado en la cáscara |
| `mergeon-view-changes` | hecho, 130 tests (`docs/view-changes.md`); integrado en la cáscara; relee ramas tras el commit; "Local Changes" con la disposición de Señal (lista + caja de commit a la izquierda, diff a la derecha), cifras `+/−` por archivo y plantilla de commit; resolvedor de conflictos de tres vías (etapas del índice, paneles ours/theirs/base con casillas por bloque y línea, resultado editable, binarios y borrado/modificación; 2026-11-15) |
| cáscara (`crates/mergeon`) | hecha: rail, pestañas, workspace, atajos, `UiHost`, todas las vistas montadas (sin marcadores) |
| empaquetado y CI nativo | Linux construido y verificado (AppImage 20,8 MB, deb 15,1 MB, `native/dist/`); NSIS probado bajo wine con binario de relleno; macOS solo estructura; workflows `ci.yml` / `release.yml` (`docs/packaging.md`) |

Notas de entorno: swap temporal `/swapfile-mergeon` (8 GB; quitar con
`sudo swapoff /swapfile-mergeon && sudo rm /swapfile-mergeon`). El trabajo va en
la rama `native-gpui`.

## Punto de corte (2026-10-02, segundo límite de uso)

- Integrado en la cáscara y funcionando: rail, pestañas, workspace, toolbar, banner de operación, diálogos de fetch/pull/push/rama/log/reflog. Se prueba con
  `MERGEON_USER_DATA_DIR=<dir> native/tools/shot.sh <png> 6 -- <repo>`.
- A medias (se pidió a cada agente cerrar y dejar su `docs/view-*.md` con sección "Pendiente"): `mergeon-view-sidebar`, `mergeon-view-home`, `mergeon-view-commits`, `mergeon-view-changes`, empaquetado/CI (`native/packaging/`, `docs/packaging.md`).
- Falta: terminar esas vistas, enchufarlas en `crates/mergeon/src/views.rs` (hoy marcadores) y conectar sus eventos (sidebar → commits, stash diff, blame), prueba de punta a punta de la app completa, adoptar en vistas/app las dos correcciones de defectos que quedan por enchufar (ver arriba), verificación en Windows y macOS.

## Estado al 2026-10-02 (fin de la migración)

- Todas las pantallas de la app Electron están portadas e integradas en `native/target/debug/mergeon`.
- Verificación del workspace: `cargo fmt --check` ok, `cargo clippy --workspace --all-targets -D warnings` limpio, `cargo test --workspace` 1757 tests sin fallos.
- Probado de punta a punta con clics reales (`native/tools/drive.sh`) sobre el repo de `native/tools/make-fixture.sh`: historial, sidebar, menús, fetch, stage all + commit, pestañas de archivo, Settings, tema claro, Home.
- Los 13 defectos heredados están corregidos (ver arriba).

Pendiente fuera de esta máquina:
- Compilar y probar en Windows y macOS (todo el código específico está escrito, nada se ejecutó).
- Ventana en negro con Mesa 22/23 (Ubuntu 22.04, Debian 12) sin GPU: probar con GPU real.
- Hecho (2026-11-12): la app Electron se eliminó del repositorio y los releases (`vX.Y.Z`) salen como "latest".
- Subido a `origin/native-gpui`. Primer release nativo `native-v0.9.0` (prerelease) publicado por `native-release.yml` en runners de GitHub: AppImage 22,8 MB, .deb 16,5 MB, `Mergeon-Setup-0.9.0.exe` 12,1 MB y los `latest*.yml`. El instalador de Windows se construyó pero no se ha probado en un Windows real.

## Verificación e2e contra Electron (2026-10-02)

> Histórico: el arnés `native/e2e/` y la app Electron se eliminaron el 2026-11-12;
> siguen en el historial de git (commit anterior a la eliminación).

Arnés en `native/e2e/` (ver su README): el recorrido de referencia completo
(18 sesiones, 182 pantallas × 2 temas) grabado en Electron y reproducido en la
nativa con clics reales de X, comparando capturas y estado final de git.

Resultado final: 364 de 364 capturas reproducidas (incluidos los 4 tooltips
por tema), **0 diferencias de estado de git** en los repos de las 18 sesiones,
0 notas. Diferencia de píxeles (8 % de tolerancia de color): mediana 1,4 %,
máximo 5,6 %; lo que queda son horas relativas, antialias y el desenfoque del
fondo de los diálogos.

Corregido a raíz de la comparación:
- Al abrir un repo se elige la primera fila del historial, no HEAD (en Electron
  el repo recién agregado aún no tiene rama cuando carga el historial).
  *Revertido el 2026-11-13*: ahora se elige HEAD (la primera fila podía ser un
  stash).
- Fondo de diálogos: GPUI no tiene `backdrop-filter`; el fondo se tapa más para
  que el contenido detrás pierda el mismo contraste que con `blur(4px)`.
- Toasts: el título recortado a 3 líneas termina en `…`.
- Resaltado HTML/JSX: los nombres de etiqueta van sin color, como en Electron
  (el resaltado aplanado deja `hljs-name` suelto y la regla `.hljs-tag
  .hljs-name` no lo alcanza).
- GPUI partía las líneas antes de cada `/` («Merge feature» / «/search into
  main»), lo que además cambiaba alturas (la tarjeta Merge de Branch Actions
  quedaba más baja y un clic caía fuera). Parche en `vendor/gpui-pre`
  (`[patch.crates-io]`, ver `vendor/gpui-pre/MERGEON.md`).
- Barras de scroll siempre visibles cuando hay desborde, como
  `::-webkit-scrollbar` (antes solo al pasar el cursor).

Divergencia deliberada: "Interactive rebase from here…" con un stash funciona
(Electron avisa «The range contains a merge commit» por la fila del stash; ver
defectos heredados).

## Sistema de pestañas (2026-11-12)

Correcciones y mejoras de la revisión de pestañas. Cada defecto corregido tiene
un test que fallaba antes (comprobado revirtiendo el arreglo).

- **Corregido** — Cerrar una pestaña a la izquierda de la activa cambiaba el
  repositorio activo (`RepositoriesStore::remove_repository` no movía el
  índice; heredado del original). Al cerrar la activa se pasa a la de la
  derecha, o a la de la izquierda si era la última.
- **Corregido** — Historial obsoleto al volver a una pestaña cuyo repositorio
  cambió estando oculta. Causa: el repo oculto no tiene watcher y al volver solo
  se emite `hydrate-opened-repository`, que `CommitsView` trata a propósito como
  no-mutación del historial; si la rama no cambia de nombre nada recarga. Ahora
  `mergeon-app` (`tabs.rs`) guarda al ocultar un repo la huella de sus refs
  (`mergeon_runtime::get_refs_fingerprint`: `HEAD`, `packed-refs` y `refs/**`
  por mtime y tamaño, sin lanzar git) y al volver, si cambió, registra
  `external-head-change` (recarga como haría el watcher). Si no cambió no recarga.
- **Corregido** — Ningún atajo de teclado hacía nada, en ninguna plataforma (la
  revisión lo atribuyó a Xvfb). La acción global llega con la ventana prestada
  por GPUI (`App::update_window_erased` la saca de `cx.windows` mientras
  despacha la tecla); `shortcuts::run` hacía `window.update` para mirar las
  guardas, eso devolvía "window not found" y el `unwrap_or(false)` descartaba
  el atajo en silencio. `cx.active_window()` sí estaba bien en Xvfb (trazas con
  `RUST_LOG=mergeon=trace`, que ahora registran cada pulsación y su acción).
  Ahora las guardas se miran en `cx.defer`. Test con ventana y
  `simulate_keystrokes` en `crates/mergeon/src/shortcuts.rs`.
- **Corregido** — La tira de pestañas de repo no se desplazaba: la barra de
  título de gpui-kit no encoge (`flex_shrink_0`) y la tira le imponía el ancho
  de todas sus pestañas; además `occlude` en cada pestaña tapaba la rueda. Ahora
  `w_0` + `flex_1`, `block_mouse_except_scroll`, la activa se trae a la vista
  (también al ensancharse por el punto o el spinner) y "▾" lista todas.
- **Corregido** — El scroll de una pestaña de archivo, el commit seleccionado y
  el borrador del commit ya no reescriben `settings.json` ni notifican a
  `AppModel` en cada cambio: `update_repository_workspace_quietly` agrupa el
  guardado 750 ms después del último (o antes, con cualquier guardado
  inmediato, al cambiar de pestaña o al salir).
- **Corregido** — Punto de cambios no determinista tras restaurar: mientras
  `is_restoring_workspace()` la cáscara no monta el espacio de trabajo de los
  repos que pasan por activos. Un repo restaurado y no visitado no muestra
  punto (no se sabe).
- Nombres: si dos repos abiertos se llaman igual, la pestaña añade la carpeta
  padre mínima que los distingue (`distinguishing_parent_suffixes`: `fx1`,
  `a/x`), con la ruta completa en el tooltip. Con más de un repo abierto los
  avisos de operaciones empiezan por `demo (fx2): `.
- Espacio de trabajo: "Commits" y "Local Changes" quedan fijas; solo las de
  archivo se desplazan y la activa se trae a la vista.
- Cerrar un repo con borrador de commit u operación en curso pide
  confirmación. El borrador se guarda en la sesión (`commitDraft` de
  `RepositoryWorkspace`) y vuelve al reabrir el repo o la app.
- Pestaña de repo: menú contextual (Close, Close others, Close to the right,
  Copy path, Show in folder), clic central para cerrar, spinner mientras hay
  una operación en curso o en cola, y arrastrar para reordenar. Pestaña de
  archivo: clic central y menú (Close, Close others, Close to the right).

Atajos (`mergeon_model::keyboard_shortcuts`):

| Atajo | Acción |
|---|---|
| `Ctrl+Tab` / `Ctrl+Shift+Tab` (también `Ctrl+PgDn` / `Ctrl+PgUp`) | Pestaña de repo siguiente / anterior, dando la vuelta; desde Home o Settings, la primera / la última |
| `Ctrl+1`…`Ctrl+8` | Pestaña de repo N; `Ctrl+9`, la última (como en los navegadores) |
| `Alt+1` / `Alt+2` | Commits / Local Changes (antes `Ctrl+1` / `Ctrl+2`, que ahora son pestañas de repo) |
| `Ctrl+W` | Cierra lo que se ve: la pestaña de archivo activa; si la activa es Commits o Local Changes, el repositorio (con confirmación si procede); con Settings a la vista, Settings; en Home, nada |
| `Ctrl+Shift+W` | Cierra el repositorio activo aunque haya un archivo abierto |
| `Ctrl+Shift+T` | Reabre el último repositorio cerrado, con sus pestañas y su borrador |

En macOS `Ctrl` es `Cmd` salvo en `Ctrl+Tab`, que es Control también allí.

Sin hacer: filtro en la lista "▾", lista "▾" de pestañas de archivo, "fijar"
pestañas de vista previa, expulsión LRU de vistas ocultas, punto de cambios de
repos no visitados por sondeo en segundo plano.

## Rendimiento: avisos del modelo, cachés y arranque (2026-11-15)

GPUI invalida la ventana entera cuando avisa una entidad que alguna vista leyó
al pintar, y aquí todas leen `AppModel`: cada `cx.notify()` del modelo es un
repintado completo. Cambios (guía en `docs/app-api.md`, "Cuándo avisa el modelo"):

- Relecturas: `update_repository` solo avisa si el repositorio cambió; la
  relectura agrupa sus pasos con `notify_soon` (un aviso por vuelta del
  executor); las lecturas en vuelo (`run_shared`) ya no avisan salvo
  `run_single_flight` (spinner de la barra); `record_mutation` solo emite
  `MutationRecorded` (el sidebar recarga sus secciones con el evento).
  Hidratar un repo: 14 → 4 avisos. Un `status-watch` sin cambios: 4 → 0 avisos,
  y la barra y la barra de estado pasan de 4 repintados a 0.
- Divisores: `drag_sidebar_resize` / `drag_file_list_resize` no avisan (quien
  arrastra repinta); el aviso y el guardado, al soltar.
- Observadores: toolbar, barra de estado, cáscara y espacio de trabajo comparan
  lo que pintan antes de `cx.notify()`. El reloj de "fetched N min ago" solo
  repinta si cambia el texto (antes, cada 30 s).
- Cachés con tope: avatares (200, LRU), resúmenes del inicio (32, LRU; al cerrar
  un repo que no es reciente se olvida el suyo), anchos de refs del historial
  (4096), gitdirs de `mergeon-gitcmd` (256), última sincronización con el remoto
  de la barra de estado (solo repos abiertos).
- Arranque: tras el tema y los recientes, la sesión se restaura mientras GitHub
  (refresco de tokens por red), los ajustes y el actualizador se leen en
  paralelo; los repos de la sesión se inspeccionan a la vez y se abren en la
  misma vuelta. Se sigue hidratando solo el activo (los demás, al visitarlos).
  Trazas con `RUST_LOG=mergeon::startup=info`. Medido en el VPS (render por
  software; mediana de 5): con 8 repos, pestañas 1450 → 671 ms, repo activo
  hidratado 1700 → 1033 ms, primera página del historial 1580 → 821 ms; con 1
  repo, 853 → 729 ms y 691 → 646 ms. Primer frame igual (~210 ms).

## Comparar ramas y filtros de búsqueda (2026-11-15)

- Comparación navegable de dos ramas o revisiones (`mergeon_git_query::compare_revisions`,
  `get_comparison_files`, `get_comparison_diff`; vista `CompareView` en
  `mergeon-view-commits`): A y B fijados a su SHA, modos "Since diverged" (`A...B`,
  por defecto) y "Direct" (`A B`), commits de cada lado (200 por lado), archivos con
  cifras y barritas (1000 como mucho, "N more"), diff por archivo con renombres. Se
  abre con "Compare with '<rama actual>'..." del menú de una rama y con "Browse
  changes" de Branch Comparison, y desde la selección múltiple del historial
  ("Compare these two", `open_compare_view`).
- Búsqueda de commits con filtros de ruta, rama, fechas (presets y personalizadas) y
  código (`-S` / `-G`), sintaxis rápida en el cuadro (`path:`, `branch:`, `after:`,
  `before:`, `code:`, `regex:`), chips y "Load more" (`skip`). Ver `view-sidebar.md`.

## Stash selectivo y fixups dirigidos (2026-11-15)

- **Stash con opciones**: `mergeon_git_ops::stash_push_operation(StashOptions
  { message, paths, include_untracked, keep_index, staged_only })` (rutas
  literales; con git 2.26+ por stdin con `--pathspec-from-file`; un `--staged`
  que git deja a medias se deshace sin dejar stash; renombres y borrados
  preparados por ruta se rechazan explicando por qué). Diálogo propio en la
  barra (resumen, casillas, combinaciones imposibles apagadas, `--staged`
  solo con git 2.35+) y "Stash selected files…" en el menú de Local Changes.
- **Fixups**: `CommitOptions::fixup` (`commit --fixup=<sha>`), "Fixup into…"
  en la caja de commit (`mergeon_git_query::get_fixup_candidates`: 30 últimos
  commits, publicados aparte y con confirmación) y, en el menú de un commit,
  "Create fixup for this commit" y "Autosquash fixups"
  (`rebase_action(.., "autosquash", ..)`: `rebase -i --autosquash
  --autostash` sin editor; un conflicto se queda en el rebase de siempre).

## Paleta de comandos y búsqueda en los visores (2026-11-15)

- **Paleta de comandos** (`Ctrl+Shift+P` / `⌘⇧P`, también escribiendo en un
  campo): diálogo arriba y centrado (`AppDialog::top_anchored().flush_body()`)
  con campo, lista virtual (`uniform_list`, 34px por fila: ícono, nombre con
  las letras encontradas en naranja, contexto atenuado y atajo en `kbd`; la
  elegida en tinta con la marca naranja) y pie con las pistas. Busca comandos,
  ramas locales y remotas del repositorio activo, repositorios (pestañas
  abiertas y recientes) y archivos (`git ls-files -z`, en segundo plano y
  guardado por repositorio en `crates/mergeon/src/file_index.rs`; una mutación
  lo marca viejo y se relee al abrir la paleta). Prefijos: `>` comandos, `@`
  ramas, `#` repositorios, `/` archivos; sin prefijo, todo mezclado y
  priorizado (comando > repositorio > rama > archivo, lo deshabilitado al
  final). Búsqueda difusa en `mergeon_model::fuzzy` (letras en orden, con
  premio a seguidas, inicio de palabra, camelCase y nombre de archivo) y orden
  en `mergeon_model::command_palette` (`rank_with_files`: los archivos no se
  copian); filtra al cambiar la consulta, fuera del render, en segundo plano
  con más de 3.000 candidatos, y como mucho 500 resultados.
- Registro único en `crates/mergeon/src/commands.rs` (`COMMANDS`: id,
  etiqueta, grupo, ícono, disponibilidad, ejecutar): casi todo es un
  `ShortcutAction` (lo mismo que su atajo) y el resto el método del modelo que
  usa la barra (stash, terminal, carpeta) o los ajustes (tema). Lo que necesita
  repositorio sale deshabilitado sin él, y lo que lo cambia, también con una
  operación de secuencia a medias. "Switch to branch…" usa el checkout del
  sidebar (`SidebarCommand::CheckoutBranch` / `CheckoutRemoteBranch`, con su
  diálogo si hay cambios); "Go to file…" abre la pestaña de árbol del archivo
  en HEAD (un archivo nuevo aún sin commit da el error de lectura de la
  pestaña). La acción va al repositorio activo al abrir la paleta
  (`PaletteTarget`); si entretanto se cerró, se avisa.
- **Pull pasa a `Ctrl+Alt+P` / `⌘⌥P`** (antes `mod+shift+p`, ahora la paleta).
- **Selectores** (integración ronda 1): "File history…", "Compare with current
  branch…" y "Fixup into…" son `Command::Pick(Picker)`: la cáscara reabre la
  paleta como selector sobre el mismo repositorio (`AppShell::open_picker`,
  `CommandPalette::with_options` con otro texto vacío y `FileIntent`): un
  archivo → su pestaña de historial desde `HEAD`; una rama local o remota →
  `SidebarCommand::CompareWithCurrent`; un commit de la caja de commit →
  `ChangesView::commit_fixup`. "Stash selected files…" abre el diálogo de
  stash con lo marcado (preparado) en Local Changes; él y "Fixup into…" se
  apagan sin nada marcado (`CommandContext::has_staged`). "Stash changes…"
  abre el diálogo con opciones del stash selectivo.
- **Buscar en el diff y en el visor de archivos** (`mergeon-view-diff`, ver
  `docs/view-diff.md`): `Ctrl+F` con el foco en el visor abre la barra
  (campo, "3 / 17", ↑ ↓, Aa, .*, ×), resalta las coincidencias y la actual,
  Enter / Shift+Enter navegan y desplazan hasta ella, Esc cierra. F7 /
  Shift+F7 (o Alt+↓ / Alt+↑) saltan entre bloques de cambios. Regla: `Ctrl+F`
  es "Search commits" salvo con el foco en un visor (contexto de teclado
  `CodeViewer`, más profundo que el global).
- Tests: `fuzzy` (7), `command_palette` (5, con 5.000 ramas y 20.000 archivos),
  `text_search` (4), atajos de la paleta y de los visores; en la cáscara, el
  registro entero ejecutado con `TestBackend`, teclado (flechas, Enter, Esc,
  prefijos), checkout por la paleta (directo y con diálogo), "Go to file",
  repositorio de destino, lista virtual con 5.000 ramas y 20.000 archivos,
  regla de `Ctrl+F`, `git ls-files` real; en los visores, abrir, contar,
  navegar, desplazar hasta la coincidencia, Aa / .*, vista partida, archivo,
  F7 y `Ctrl+F` fuera del visor.

## Progreso real de red y limpieza de ramas (2026-11-15)

- **Progreso de fetch, pull, push y clone**: `--progress` + `Git::with_progress`
  (`mergeon-gitcmd/src/progress.rs`: parser tolerante a traducciones y a `\r\n`,
  filtro que deja el stderr como sin `--progress`), publicado por la cola
  (`OperationProgress`, ~10/s, `BackendEvent::OperationProgress`) y pintado en el
  mismo toast sin repintar la ventana: barra por fase, "45 %", fase y velocidad,
  y botón "Cancel". El toast de progreso se reescribe en su sitio (no vuelve a
  entrar ni se reordena). Ver `app-api.md`.
- **Clean up merged branches…** (menú de la cabecera Branches): diálogo con las
  ramas integradas en la rama principal (merge, fast-forward, squash probable,
  upstream "gone"), borrado en lote por la cola (`cleanup-branches`) con aviso y
  log. `get_merged_branches` (git-query), `cleanup_branches_operation` (git-ops).
  Ver `view-sidebar.md`.

## Pull requests y checks de GitHub (2026-11-15)

Solo consulta, con el token de la cuenta activa: sección "Pull requests" en el
sidebar (lista virtual con checks, borrador, revisión y monograma del autor;
clic abre el PR; menú con checkout de su rama, copiar enlace y comparar) e
indicador `#123 · ✓ 5/5 checks` del PR de la rama en la barra de estado.
`mergeon-github` (`pulls.rs`: paginación, `ETag`/304, cupo `X-RateLimit-*`),
`mergeon-app` (`pull_requests.rs`: estado por repositorio, refresco al hacer
fetch, al cambiar de rama y cada 5 min con foco; sin toasts) y
`fetch_pull_request_operation` en `mergeon-git-ops`. Ver `app-api.md`,
`view-sidebar.md` y `view-toolbar.md`. Sin probar contra GitHub real (solo
transporte guionizado y `MERGEON_TEST_GITHUB_FIXTURE`).

## Funciones nuevas

- 2026-11-15 — **Historial de un archivo y blame navegable**: pestaña "File
  history" (`TabSource::History`) desde el menú de un archivo (Local Changes,
  Changed Files, pestaña de archivo, Repo tree, menú de la pestaña) y el botón
  "File history" de la pestaña de un commit; lista virtual paginada que sigue
  renombres, diff de cada revisión con su ruta, flechas, clic en el hash →
  commit en el historial. Blame: clic en un bloque → commit; "Blame previous
  revision" con "Back". Ver `view-commits.md`, `view-diff.md`, `view-changes.md`.
- 2026-11-15 — **Integración ronda 1** (rama `integ-r1`: comparar ramas, stash
  selectivo y fixups, paleta y buscar en el diff, historial de archivo). Menús
  de archivo por grupos: ver/abrir, "File history", "Stash selected files…"
  (Local Changes) y copiar al final; en "Changed Files" y la pestaña de archivo,
  "Open in editor · File history · Copy Path / Copy GitHub link". El doble clic
  en un commit de la comparación va por `AppModel::show_commit_in_history`
  (activa la pestaña Commits, como "File history" y el blame). La paleta suma
  "File history…", "Compare with current branch…", "Stash selected files…" y
  "Fixup into…".
- 2026-11-15 — **Selección múltiple en el historial**: Ctrl/⌘+clic, Shift+clic,
  Shift+flechas y Esc; resumen "N commits selected" con lista compacta virtual y
  totales del rango (`git diff --shortstat`) si es seguido; cherry-pick y revert
  en lote (también en un único commit de revert), "Compare these two" y "Copy
  SHAs". Paran en el primer conflicto; el banner ofrece Skip en secuencias de
  cherry-pick/revert. Ver `view-commits.md`.
- 2026-11-15 — **Worktrees**: sección en el sidebar (estado de cada uno en
  segundo plano), abrir como pestaña, «New Worktree» (rama nueva o existente,
  carpeta sugerida), quitar (con `--force` solo a petición), bloquear,
  desbloquear, podar; "Check out in new worktree..." en el menú de una rama, y
  las ramas ocupadas en otro worktree se marcan y su checkout lo avisa. Ver
  `view-sidebar.md`.
- 2026-11-15 — **Integración ronda 2** (rama `integ-r2`: resolvedor de
  conflictos de 3 vías, progreso real y limpieza de ramas, pull requests y
  checks, selección múltiple y worktrees). Secciones del sidebar en este orden:
  Starred, Branches, Pull requests, Remotes, Worktrees, Tags, Stashes,
  Submodules. "Compare with current branch" de un PR abre la comparación
  navegable (`SidebarCommand::CompareWithCurrent("<remoto>/<rama>")`), como la
  de una rama remota. La paleta suma "Clean up merged branches…", "New
  worktree…" (`Command::Sidebar`, que la cáscara pasa a `Sidebar::run_command`),
  "Open pull request of current branch" (apagado si la rama no tiene PR
  conocido) y "Checkout pull request…" (`Picker::PullRequest`: los PRs abiertos
  ya leídos → "Checkout PR branch" de la sección).
- 2026-11-15 — **QA final de 0.12.0** (4 agentes). Corregido: un resultado
  fallido sin `status` (git se niega: cambios locales, upstream inexistente…)
  invalidaba el estado y nadie lo releía; ahora
  `AppModel::apply_repository_operation_result` pide una relectura de status y
  `changes_count`. Tags, stashes y submódulos se leen al abrir el repositorio
  aunque estén plegados (su cabecera decía 0). Menú contextual en las filas de
  stash (`menus::stash_menu`). "Fixup into…" se apaga con un merge, rebase,
  cherry-pick o revert a medias (`CommitPanel::operation_in_progress`).
  El watcher trata `.git/index` como cambio del working tree (`git add` /
  `git reset` de la terminal; `index.lock` sigue ignorado). `get_commit_diff`
  añade la ruta de origen de un renombre al pathspec; el visor explica los
  parches sin filas (renombre, modo, binario). Las filas del sidebar
  (`section::item`) llevan `w_full`. La línea del aviso bajo el botón de commit
  se reserva siempre. Elegir "Custom command…" enfoca "Command". Si el selector
  de carpetas falla, `ShellUiHost::pick_directory` pide la ruta con un prompt.
  Pendiente: remotos añadidos o quitados por fuera de la app no aparecen hasta
  un refresco manual; conflictos add/add y delete/modify sin resolvedor propio
  (salen en Unstaged y Staged); sin xdg-desktop-portal el selector de carpetas
  de Linux se queda esperando (GPUI no devuelve error); atajos S/U/D solo en
  minúscula; árbol de Local Changes sin guías de sangría.
- 2026-11-16 — **Detalle del commit más limpio.** Sin Unified | Split (solo
  cambiaba la preferencia global; el diff se abre en su pestaña, que tiene su
  propio Split). Solo Checkout y ⋯ a la vista (Cherry-pick está en ⋯). Las
  refs que no caben llevan tooltip con todos los nombres y nunca bajan de
  56 px; el resto del hash cede primero. El bloque de cifras recorta "Repo
  tree" en vez de montarlo sobre números anchos. Historial con mínimo de
  320 px (las etiquetas de rama se montaban sobre el hash).
- 2026-11-19 — **Deshacer y rehacer operaciones de git** (Ctrl+Z / Ctrl+Y /
  Ctrl+Shift+Z; ⌘Z / ⌘⇧Z). Pila por repositorio en el modelo (50 entradas,
  `undo.rs`): commit/amend/fixup (`reset --soft`), merge, rebase (también
  interactivo y sus acciones), pull, reset, cherry-pick y revert (`--keep`),
  checkout, ramas, tags (un anotado vuelve con su objeto), stash
  push/pop/apply/drop y descartes (copia en `undo-backups/` antes de
  descartar; 50 MiB como mucho, con confirmación por encima). La cola lee
  HEAD y refs antes y después de cada operación (`Mutation::capture`); un
  cambio externo vacía la pila con aviso, y deshacer commits publicados pide
  confirmación. Botones Undo | Redo en la barra, paleta y chuleta. Ver
  `app-api.md` ("Deshacer y rehacer") y `view-toolbar.md`.
- Bloque 3 · t2 (cambios locales): los siete códigos de conflicto (`AA`, `UD`,
  `DU`, `DD`, `AU`, `UA` además de `UU`) salen solo en "Conflicted files" y se
  resuelven desde el resolvedor (`AA` de texto a tres vías con base vacía;
  binarios, borrados y renombres con opciones que dicen qué rama hizo qué);
  Shift+S/U/D; guías de sangría en `FileTreeRow`; stash con archivos sin
  seguimiento (`stash^3`); "Open in editor" sin editor avisa con "Choose
  editor…" (y ya no se cuelga esperando a `xdg-open`); `commit.template` se relee al vaciarse la caja o al volver.
- 2026-11-19 — **Bloque 3 · diff y etiquetas (t3).** "Whole file" en las pestañas
  de diff (commit, historial de archivo, Local Changes, comparar): preferencia
  `commitView.wholeFile`, `DiffOptions::whole_file` (`--unified=999999999`, mismo
  tope de 2 MiB); en Local Changes sin botones de bloque, con líneas sueltas.
  Marcas de cambios junto a la barra del `DiffViewer` (clic para saltar).
  Imágenes antes/después (`ImageDiffViewer`, `read_image_pair`, tope 20 MB).
  Historial: `main` + `origin/main` en una etiqueta con nube, "+N" con todas las
  refs, doble clic en una rama hace checkout; cabeceras de columna sin desfase
  al arrastrar el divisor. Pendiente: marcas en el visor de archivo (no tiene
  cambios que marcar).
- 2026-11-19 — **Infraestructura (bloque 3, t4).** Usuario y contraseña/token
  para remotos HTTPS que no son de GitHub: si fetch, pull, push o clone fallan
  por credenciales, diálogo "Sign in to <host>" y reintento; helper en línea por
  `-c` que lee de un archivo efímero `0600` borrado y puesto a ceros al terminar
  (nada en argv, ni en el entorno de los hooks, ni en logs), en
  memoria por origen mientras siga abierta la pestaña, "Remember" con `git
  credential approve` si hay helper, y aviso si el remoto usa `http://` no local. El token de GitHub va solo a github.com.
  El watcher vigila `.git/config`, `refs/tags/**`, `refs/remotes/**`,
  `refs/stash` y `logs/refs/stash` (`fs-watch-config`, `fs-watch-refs`,
  `fs-watch-stash` y sus `poll-*`): remotos, tags, stashes y PRs se releen sin
  reiniciar. Lanzador con campo para escribir/pegar la ruta (valida,
  expande `~`); en Linux, si el portal no responde en 2 s, se ofrece ese campo
  sin cancelar el selector. Notificación del sistema al terminar un fetch, pull,
  push o clone de ≥ 5 s con la ventana sin foco (Settings → Git →
  Notifications). Ver `app-api.md` y `view-home.md`.
- 2026-11-19 — **Windows (T5) y QA final bloque 3 (171–173).** Suite de Windows
  en verde en CI (normalización de rutas sin `\\?\`, separadores de destino en
  `mergeon-view-home::util::join_local_path`, CRLF). Hallazgos de QA corregidos:
  orden de registro `init_bindings` (`shortcuts::init` antes que
  `mergeon_ui::init`) para que `Ctrl+Z`/`Ctrl+Y` deshagan texto en los campos
  sin perder los atajos sin foco; `ChangesView::last_commit` sigue a `HEAD` y
  deshacer un commit devuelve su mensaje a la caja vacía; credenciales HTTP por
  archivo efímero `0600` (sin llegar a `env` de hooks) y cancelación limpia;
  "Whole file" cae a solo cambios con aviso si supera 2 MiB; botones `↶ ↷`
  anclados a la izquierda de Fetch/Pull/Push con tooltips explicativos; aviso
  al hacer Reset (Hard) con cambios locales; reconciliación inmediata de la pila
  de deshacer tras cambios externos de HEAD; refs remotas ordenadas (`origin`
  primero) y checkout deduplicado entre remotos; SVG limitado a su tamaño
  lógico; diálogos de descarte actualizados (ya son recuperables).
- Diff (QA bloque 3, severidad baja): conmutador "Image | Text" para SVG (diff del XML con sus interruptores), "Whole file" conserva la línea visible (`DiffViewer::line_anchor`/`scroll_to_line`), "Decoding image…" mientras GPUI decodifica una imagen grande (el `img` necesitaba id) y medidas de las imágenes "Too large to preview" (se leen 64 KB de cabecera).
- 2026-11-19 — **Clone from URL y aviso del último conflicto.** Lanzador, paleta y `mod+shift+o`: clonar pegando cualquier URL (HTTPS, SSH, `git@host:…`, `file://`, ruta) con carpeta y nombre editables, sobre `clone_repository` (progreso, credenciales, avisos) y abriéndolo en pestaña; credenciales de la URL enmascaradas en avisos y logs. Resolver el último conflicto ya no muestra un "Resolved …" encima de Continue / Abort del banner. Ver `app-api.md` y `view-home.md`.
- 2026-11-21 — **0.13.1: pulido de sidebar, bugs git y "Ignore spaces".** `RebaseOptions::target_branch` (arrastrar rama sobre rama no activa rebasa la rama destino y se deshace junto con merge); etiquetas `"Rebase '<actual>' onto '<rama>'..."` y `HEAD` desanclado; `PushBranch` con upstream renombrado y remotos con `/`; Pull y Add Remote en Operations Log; tags remotos sin `"origin"` fijo; cierre de diálogos y guarda de repo activo en PRs/navegación; tooltips tras clic derecho; acciones inline de Tags/Stashes/Submodules/Worktrees en hover y cabeceras uniformes; filtro/búsqueda con `×`/`Esc` sin duplicados y selector de ramas en `NewWorktreeDialog`; `Rc<[RemoteRow]>`, filtro de repintados en `model_changed` y lecturas de worktrees acotadas; `"Ignore spaces"` también conserva `line_anchor`.
- 2026-11-21 — **0.13.2: optimización de vistas y CI/release.** `AppShell`, `RepositoryWorkspaceView`, `ChangesView`, `HistoricalFileTab`, `OperationStatusSlot`, `ToolbarView`, `StatusBarView`, `PullRequestIndicator`, `HomeView` y diálogos de GitHub/Fetch/Stash pintan desde snapshots `PartialEq` sin leer `AppModel` en `render` (evitando `record_entities_accessed` en GPUI). `CommitsView::render` usa `is_checkout_disabled` `O(1)`; `CommitHistoryList` cachea `badge_layout` (tope 2048) y evita clones por fila; `DiffViewer` calcula filas Split bajo demanda (en segundo plano > 64 KB); `FileContentViewer` indexa líneas en segundo plano > 64 KB; `ConflictResolver` (lista de archivos) y `GitHubRepositoryPicker` virtualizados con `uniform_list`. 2558 tests verdes. CI/Release: `lld-link` + sin debuginfo en Windows CI, build check separado solo en macOS y sin `codegen-units = 1` en `[profile.release]`.
- 2026-11-22 — **0.14.0: rendimiento integral, Local Changes UX y rediseño de cabecera/pestañas.**
  - **Rendimiento (Bloque A):** `Sidebar`, `PullRequestsSection`, `SidebarResizer`, `CommitsView` y `HomeView` sin lecturas de `AppModel` en `render()` (+ tests `render_count`); `update_repository` con detección por campo (`tracked`) sin clonar `Repository`; watcher con `ignore` respetando `.gitignore` + `git ls-files -c -i --exclude-standard`; historial con `HashSet`, `Arc<Vec<CommitRow>>` y `GraphColor(Cow<'static, str>)`; tree-sitter omitido en líneas > 16 KB.
  - **Local Changes (Bloque B):** avance de selección tras stage/unstage/discard, selección múltiple (`Shift`/`Ctrl+clic`/`Ctrl+A`) + resumen/lotes + casillas en carpetas, claridad Unstaged/Staged + segmentado clicable + arreglo del botón `"Discard"` en Staged (`"Unstage file"`), ancho por defecto 400 px persistido (`commitView.localChangesWidth`), pistas cerrables, caja de commit compacta con contador 50/72, carpetas compactas + renombres `old → new` + toggle Árbol/Lista, `fit_section` en conflictos y nombres reales de rama/commit en `ConflictResolver`.
  - **Estética y pestañas (Bloque C):** sidebar fijo con "Local Changes" / "All Commits" arriba y barra de documentos de 32 px oculta cuando no hay archivos; `FileTab.preview` en cursiva, activar vecina al cerrar, desambiguación por carpeta, menú "Keep open / Close all", `Alt+←/→` y migas en cabecera; rail lateral eliminado (+48 px), toolbar de 40 px con selector de rama clicable, sin repeticiones de repo/rama/ahead-behind, `Ctrl+W` seguro en vistas fijas y `×` en hover/activa. Suite completa del workspace verde (`fmt`, `clippy -D warnings`, todos los crates).
