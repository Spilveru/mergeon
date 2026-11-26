# Changelog

Los cambios notables de Mergeon se documentan en este archivo. El formato sigue
[Keep a Changelog](https://keepachangelog.com/es/1.1.0/) de forma simplificada.

## [Sin publicar]

## [0.15.0] — 2026-11-26

### Añadido

- **Atajos de teclado personalizables (#13):** nueva sección **Settings → Shortcuts** (accesible también con `Ctrl+/` / `⌘/`) con búsqueda, grabación de combinaciones al hacer clic (con soporte de flechas, teclas de función y puntuación), detección de conflictos en línea y por banner, desvinculación y restauración individual o global. Los cambios se aplican en vivo sin reiniciar y se reflejan en la paleta de comandos, tooltips y ayuda.
- **Ejecución directa de Fetch / Pull / Push sin diálogo (#13):** nueva opción en **Settings → Git** (*"Run Fetch, Pull, and Push immediately without dialogs"*) para ejecutar *Fetch*, *Pull* y *Push* al instante con los valores por defecto desde atajos o la barra de herramientas, con respaldo automático al diálogo si no existe un valor seguro (sin remotos, sin upstream o `HEAD` desanclado). `Shift+clic` en la barra o los comandos `… (options)` de la paleta siguen abriendo el diálogo.
- **Workspaces / Grupos de repositorios (#14):** agrupa repositorios en workspaces con nombre para que la barra de pestañas y la navegación por teclado (`Ctrl+Tab`, `Ctrl+1..9`) muestren solo las pestañas del grupo activo, conservando intacto el estado de cada repositorio al cambiar. Divulgación progresiva (sin ruido visual si no creas grupos), selector compacto en la barra de título, submenú **Workspaces ▸** en el clic derecho de pestañas y de filas de repositorios en **Home**, sección `Workspaces` en Home y comandos en la paleta (`Switch / Create / Rename / Delete workspace`).

## [0.14.0] — 2026-11-22

### Añadido

- **Local Changes:** selección múltiple dentro de una sección (`Shift+clic`, `Ctrl/Cmd+clic`, `Ctrl/Cmd+A`) con acciones y resumen por lote (`Stage/Unstage/Discard N files`), casilla en carpetas para preparar/quitar todo el subárbol, toggle Árbol / Lista plana (con carpetas de un solo hijo compactadas en una fila) y renombres `old → new`.
- **Pestañas de documentos:** pestaña de vista previa explícita en cursiva (se reutiliza con un clic desde Commits o paleta; doble clic, arrastrar o "Keep open" la fijan), menú contextual con "Keep open", "Close others", "Close to the right" y "Close all", listado desplegable `▾`, reordenado arrastrando y atajos `Alt+←/→`.
- **Toolbar:** el botón de la rama actual ahora es interactivo y abre directamente el selector rápido de ramas.

### Cambiado

- **Interfaz más limpia y compacta:**
  - El sidebar queda fijo en todas las vistas (Commits, Local Changes y documentos), con las filas "Local Changes" (con contador) y "All Commits" en su parte superior; la barra de documentos (32 px) solo aparece cuando hay documentos abiertos.
  - Se elimina el rail lateral izquierdo (+48 px horizontales): `+` pasa a la barra de pestañas, Operation Log y Settings al extremo derecho de la cabecera, y "Reload application" queda solo en la paleta.
  - Toolbar de 40 px y barra de estado sin datos duplicados (repo, rama y ahead/behind ya no se repiten en varios sitios; ahead/behind queda solo en Pull/Push).
  - En pestañas de repos y documentos, `×` aparece solo en la activa o al pasar el ratón; punto neutro para cambios locales y color reservado para conflictos/operaciones; `Ctrl+W` cierra documentos y ya no cierra el repo accidentalmente en vistas fijas (`Ctrl+Shift+W` cierra el repo); al cerrar un documento se activa la pestaña vecina; archivos con igual nombre se desambiguan por carpeta; cabecera de documento en migas (`src › view › file.rs`).
- **Local Changes y Conflictos:**
  - Al preparar o descartar con `Space`, casilla o `D`, la selección avanza al siguiente archivo de la misma sección.
  - En archivos parcialmente preparados solo se resalta la fila de la sección activa; la cabecera del diff incluye segmentado clicable `Unstaged | Staged`, `status_badge` con color, cifras `+N −N` y elimina el relleno doble.
  - La columna de Local Changes arranca en 400 px (apila por debajo de 900 px) y recuerda su ancho; pistas de teclado solo cuando no hay cambios staged y cerrables con `×`; caja de commit compacta con contador en dos tramos (50 / 72), conteo de archivos staged y cuerpo adaptable; en Split se muestra aviso cuando no aplica el staging por bloque/línea.
  - La sección de conflictos usa ajuste flexible con barra de progreso y `ConflictResolver` muestra el nombre real de rama/commit en sus cabeceras.
- **Rendimiento:**
  - `Sidebar`, `PullRequestsSection`, `SidebarResizer`, `CommitsView` y `HomeView` pintan sin leer `AppModel` en `render()`, eliminando los últimos repintados de ventana completa en la pantalla principal.
  - `update_repository` compara solo el campo modificado sin clonar el `Repository` entero en cada refresco; la paginación del historial deduplica con `HashSet`, comparte filas por `Arc` y usa `Cow<'static, str>` en los colores del grafo.
  - El watcher de archivos respeta `.gitignore`, `.git/info/exclude` y `core.excludesFile` (conservando vigilancia sobre archivos versionados dentro de carpetas ignoradas), evitando agotar `inotify` con `target/`, `.venv/` o `build/`.
  - `DiffViewer` omite tree-sitter en líneas individuales de más de 16 KB para evitar bloqueos en archivos minificados.

### Corregido

- **Local Changes:** el botón `"Discard"` de la cabecera del diff ahora cambia a `"Unstage file"` al visualizar el lado Staged (antes ejecutaba `git checkout` sobre cambios Unstaged fuera de vista) y pide confirmación de forma consistente en Unstaged.

## [0.13.2] — 2026-11-21

### Cambiado

- Optimización general de rendimiento en todas las vistas:
  - `AppShell`, `RepositoryWorkspaceView`, `ChangesView`, `HistoricalFileTab`, `OperationStatusSlot`, `ToolbarView`, `StatusBarView`, `PullRequestIndicator`, `HomeView` y diálogos de GitHub pintan desde snapshots filtrados sin leer `AppModel` en `render`, evitando repintados innecesarios por avisos o pestañas ajenas.
  - `DiffViewer` en modo Unified ya no calcula filas ni marcas de Split por adelantado (las construye bajo demanda, en segundo plano para diffs grandes), y `FileContentViewer` indexa archivos de más de 64 KB fuera del hilo de interfaz.
  - `CommitHistoryList` cachea el layout de badges y evita clonar commits por fila; `CommitsView` comprueba el estado de Checkout en `O(1)` sin construir el menú completo en cada frame.
  - Lista de archivos en conflicto de `ConflictResolver` y selector de repositorios de GitHub (`GitHubRepositoryPicker`) virtualizados con `uniform_list`.

## [0.13.1] — 2026-11-21

### Añadido

- "Clone from URL" en el lanzador, la paleta de comandos y con Ctrl+Shift+O: clonar pegando cualquier URL (HTTPS, SSH, `git@host:org/repo.git`, `file://` o una ruta local), con carpeta destino y nombre editables; usa el mismo progreso y reintento con credenciales que el clonado de GitHub y abre el repositorio en una pestaña. Las credenciales incluidas en una URL se ocultan en avisos, errores y registros.
- Diffs de SVG: conmutador "Image | Text" para ver también el diff del XML.

### Cambiado

- Activar o desactivar "Whole file" o "Ignore spaces" mantiene a la vista la línea que se estaba leyendo.
- Mientras se decodifica una imagen grande se ve "Decoding image…"; las imágenes demasiado grandes para previsualizar siguen mostrando sus medidas.
- Barra lateral más limpia y rápida: botones de Tags, Stashes, Submodules y Worktrees aparecen al pasar el ratón (sin cortar nombres ni mensajes), cabeceras uniformes en las 8 secciones, filtro y búsqueda con `×`, foco y `Esc`, resultados de búsqueda a altura completa, selector de ramas en "New worktree" ("Existing branch"), menos repintados y lecturas de estado de worktrees acotadas.

### Corregido

- Arrastrar una rama sobre otra para hacer rebase ahora rebasa la rama destino elegida (no `HEAD`) y se puede deshacer igual que el merge con rama destino.
- El menú contextual de ramas locales y remotas muestra claramente `"Rebase '<actual>' onto '<rama>'..."` y deshabilita Merge/Rebase con `HEAD` desanclado.
- Push desde el sidebar respeta el nombre real de la rama remota cuando difiere del local (y remotos con `/` en el nombre), y refresca Remotes; Pull y Add Remote ahora aparecen en el Operations Log.
- Las acciones remotas de tags ya no asumen `"origin"` (usan el remoto del repositorio o se ocultan si no hay remotos).
- Al cambiar de repositorio se cierran los diálogos del sidebar y se limpian scroll/worktrees/filtros; navegar a refs o traer PRs ya no actúa sobre otro repositorio ni deja tooltips bloqueados tras clic derecho.
- Durante un merge o rebase en curso se deshabilitan las acciones inline de Tags, Stashes y limpieza de ramas; las ramas destacadas ya borradas no intentan checkout; cambiar filtros de búsqueda ya no dispara búsquedas duplicadas ni oculta resultados al fallar "Load more".
- Al resolver el último conflicto, el aviso "Resolved …" tapaba los botones Continue / Abort del banner.

## [0.13.0] — 2026-11-19

### Añadido

- Deshacer y rehacer operaciones de git (Ctrl+Z / Ctrl+Y y botones en la toolbar): commit, amend, fixup, merge, rebase, pull, reset, cherry-pick, revert, checkout, crear/renombrar/borrar ramas y tags, y stash. Si el repositorio cambió por fuera, la pila se vacía con un aviso; si lo deshecho ya está en el remoto, pide confirmación (y nunca hace push); al deshacer un commit su mensaje vuelve a la caja de commit. En un campo de texto, Ctrl+Z / Ctrl+Y siguen deshaciendo el texto.
- Los descartes de cambios se pueden recuperar con Ctrl+Z (también archivos sin seguimiento y binarios).
- Todos los tipos de conflicto en el resolvedor: añadido por ambos lados, borrado por un lado y modificado por el otro, borrado por ambos y binarios, cada uno con su forma de resolverlo.
- Diff: "Whole file" para ver el archivo completo (si el archivo supera el tope de 2 MB cae automáticamente a solo cambios con aviso), marcas de cambios junto a la barra de desplazamiento (un clic salta al cambio) e imágenes antes y después (PNG, JPG, GIF, WebP, BMP, ICO y SVG).
- Historial: una rama local y su remota en el mismo commit se ven como una sola etiqueta con nube; "+N" muestra todas las ramas al pasar el ratón; doble clic en una etiqueta hace checkout.
- Usuario y contraseña (o token) para remotos HTTPS que no son de GitHub (GitLab, Bitbucket, Azure DevOps…): se piden cuando hacen falta y se reintenta la operación (con aviso si el remoto usa `http://` sin TLS). Nunca van en los argumentos de los procesos, ni en el entorno de los hooks, ni en los registros; se pueden guardar en el gestor de credenciales de git.
- El lanzador "Open repository" acepta una ruta escrita o pegada.
- Notificación del sistema cuando una operación larga termina con la ventana en segundo plano (se puede apagar en Ajustes).
- Guías de sangría en los árboles de archivos.

### Cambiado

- El detalle del commit muestra solo Checkout y ⋯ (Cherry-pick está en ⋯); las ramas que no caben se leen completas al pasar el ratón. Se quitó Unified/Split de ahí: el diff se abre en su pestaña, que tiene su propio Split.
- S / U / D funcionan también con Shift.
- "Open in editor" sin editor configurado abre con la app del sistema y ofrece "Choose editor…".
- El token de la cuenta de GitHub solo se envía a github.com.

### Corregido

- Remotos, tags y stashes creados o borrados desde la terminal no aparecían hasta reiniciar.
- "Open Local" en Linux sin selector de carpetas del sistema (xdg-desktop-portal) se quedaba esperando sin hacer nada.
- "Open in editor" y "Show in folder" podían quedarse esperando para siempre en Linux.
- Un stash con solo archivos sin seguimiento decía que no tenía cambios.
- Cambiar o quitar la plantilla de commit no se notaba hasta reiniciar.
- Las cabeceras del historial se desfasaban al arrastrar el divisor, y con el historial muy estrecho las etiquetas de rama se montaban sobre el hash.
- "Amend last commit" podía proponer el mensaje de un commit anterior tras crear, enmendar o deshacer commits.
- Reset (Hard) con cambios sin commitear ahora avisa en la confirmación (y al deshacer) de que esos cambios locales no se recuperan.
- En Windows, la ruta destino al clonar repositorios ya no mezcla separadores `/` y `\`.

Algunas ideas de esta versión vienen de [gitcraft](https://github.com/Vancete/gitcraft) (MIT OR Apache-2.0); ver `native/THIRD-PARTY-NOTICES.md`.

## [0.12.0] — 2026-11-15

### Añadido

- "Open in editor": abre el repositorio, un archivo o una línea en el editor elegido (detecta VS Code, Cursor, Zed, Sublime, VSCodium, JetBrains y Notepad++; también el programa por defecto o un comando propio con `{path}` `{line}` `{column}`). En los menús de archivo, la cabecera de las pestañas, la toolbar y con Ctrl+Shift+E.
- "Discard hunk" y selección de líneas sueltas en el diff (clic en el medianil, arrastre o Shift+clic) para preparar, quitar o descartar solo esas líneas (S / U / D).
- Local Changes con la disposición del diseño: lista y caja de commit a la izquierda, diff del archivo a la derecha, y cifras +/− con mini barras por archivo. Un clic muestra el diff; el doble clic abre la pestaña.
- Plantilla de commit (`commit.template`).
- Reset Soft en el menú del commit.
- "Copy GitHub link" para commits y archivos.
- Paleta de comandos (Ctrl+Shift+P / ⌘⇧P): comandos, ramas (`@`), repositorios (`#`) y archivos (`/`) con búsqueda difusa. **Pull pasa a Ctrl+Alt+P / ⌘⌥P.**
- Buscar dentro del diff y del visor de archivos (Ctrl+F con el foco en el visor), con mayúsculas y expresiones regulares; F7 / Shift+F7 saltan entre bloques de cambios.
- Comparar ramas o revisiones con diff navegable: modos "Since diverged" y "Direct", commits de cada lado y archivos con cifras. Desde el menú de una rama, "Browse changes" o un PR.
- Búsqueda de commits con filtros por ruta, rama, autor, fechas y contenido (`path:`, `branch:`, `author:`, `after:`, `before:`, `code:`, `regex:`), con chips y panel "Filters".
- Historial de un archivo siguiendo renombres, y blame navegable (clic al commit, "Blame previous revision", copiar hash).
- Stash selectivo: elegir archivos, incluir sin seguimiento, conservar lo preparado o guardar solo lo preparado; "Stash selected files…" en Local Changes.
- "Fixup into…" junto al botón de commit, "Create fixup for this commit" y "Autosquash fixups" en el menú del commit.
- Resolvedor de conflictos de 3 vías: paneles Current / Base / Incoming, elección por bloque o por línea y resultado editable antes de "Mark as resolved".
- Selección múltiple en el historial (Ctrl/⌘+clic, Shift+clic, Shift+flechas): cherry-pick y revert en lote, comparar dos commits y copiar SHAs.
- Worktrees en la barra lateral: crear (rama nueva o existente), abrir en pestaña, quitar, bloquear y limpiar; "Check out in new worktree…" y aviso cuando una rama está ocupada en otro worktree.
- Pull requests de GitHub en la barra lateral, con checks, revisiones, checkout del PR (también de forks) y comparación; el PR de la rama actual aparece en la barra de estado.
- Progreso real en fetch, pull, push y clone (fase, porcentaje, velocidad) con botón "Cancel".
- "Clean up merged branches…": borra en lote las ramas ya integradas (también las de squash probable o con upstream desaparecido) y, si se pide, también en el remoto.
- Menú contextual en los stashes de la barra lateral (inspeccionar, aplicar, sacar, crear rama, descartar).

### Cambiado

- La barra lateral solo dibuja las filas visibles: con 5.000 ramas locales, 5.000 remotas y 10.000 tags pasa de ~5 s a ~1,3 ms por frame. Reflog, registro de operaciones y rebase interactivo también son listas virtuales.
- Los diffs grandes se analizan en segundo plano (elegir uno de 20.000 líneas ya no congela la ventana) y no se cargan enteros en memoria (un parche de 21 MB pasa de ~80 MB a ~8 MB).
- Abrir un repositorio repinta la ventana muchas menos veces y la sesión se restaura en paralelo (con 8 repos, el activo está listo en ~1 s en vez de ~1,7 s).
- Comparar ramas lista como mucho 200 commits por lado y dice cuántos más hay.

### Corregido

- Con `color.ui=always` (u otras opciones como `diff.noprefix` o `diff.external`) en la configuración de git, los diffs, el historial y las ramas se leían mal.
- El resolvedor de conflictos mezclaba la versión base de `diff3`/`zdiff3` en el lado "Current"; ahora la muestra aparte.
- Tras una operación que git rechaza (por ejemplo, un cherry-pick que pisaría cambios locales), la fila de cambios sin commitear y el contador de Local Changes desaparecían hasta reiniciar.
- Los contadores de Tags, Stashes y Submodules marcaban 0 mientras la sección estaba plegada.
- "Fixup into…" seguía disponible durante un merge o rebase en curso.
- Un `git add` o `git reset` hecho fuera de la app (en la terminal) no se reflejaba en Local Changes hasta reiniciar.
- El diff de un archivo renombrado en un commit salía como un archivo nuevo entero; ahora dice de dónde viene (y un parche sin líneas, como un cambio de permisos o un binario, explica qué cambió en vez de quedar en blanco).
- Los stashes con un mensaje largo empujaban sus botones fuera de la barra lateral.
- Al escribir el asunto del commit el panel encogía y el botón Commit se movía justo antes de pulsarlo.
- Al elegir "Custom command…" como editor, el primer clic en el campo se perdía; ahora el cursor va directo al campo.
- En Linux, si el selector de carpetas del sistema falla, "Open Local" pide la ruta a mano en vez de no hacer nada.

## [0.11.1] — 2026-11-15

### Cambiado

- El README del resumen se interpreta como Markdown real: HTML habitual (párrafos y bloques centrados, enlaces, imágenes, `<details>`), badges en fila, tablas, listas anidadas y de tareas, citas y código resaltado. Los READMEs grandes ya no se traban (se interpretan una vez y se pintan como lista virtual).
- El mensaje del commit en el detalle se pliega a 3 líneas (los `Co-Authored-By:` van aparte) y al desplegarse crece como mucho al 40 % con scroll propio: la lista de archivos siempre conserva su espacio.
- Toolbar: acciones a la izquierda, buscador centrado y el repositorio y la rama a su derecha.
- Help / About ocupa el ancho disponible y la chuleta de atajos va en dos columnas.
- "Reload application" pide confirmación.
- La lista de archivos de un commit ya no marca el primero por defecto.

## [0.11.0] — 2026-11-13

### Añadido

- Barra de estado: repositorio, rama con ↑↓ y upstream, hora del último fetch, posición en el historial y versión de git.
- Toolbar: Fetch | Pull | Push en grupo con contadores (por traer y por enviar), rama con ↑↓ y buscador con Ctrl+K (⌘K) que abre la búsqueda de commits.
- Historial: fila "N uncommitted changes · M staged" encima de HEAD (lleva a Local Changes) e iniciales del autor.
- Detalle del commit: hash partido con copiar, botones Checkout, Cherry-pick y ⋯, autor con correo, padre clicable, cifras grandes de archivos y líneas, y mini barras de añadido/quitado por archivo.
- Local Changes: casillas por archivo y por sección, "Stage hunk" / "Unstage hunk" en cada bloque, asunto con contador /72 y cuerpo aparte, "Sign (SSH/GPG)" (`git commit -S`, recordado por repositorio), botón "Commit to <rama>" y atajos Espacio (preparar/quitar), D (descartar) y flechas en la lista.
- Menú contextual de commits con cabecera (hash · asunto) y atajos de una letra (B, T, C, R) con el menú abierto.
- Avisos de progreso para fetch, pull, push y clone, y aviso de pull con conflictos con "Resolve" y "Abort merge".
- Estado vacío de Local Changes con el commit de HEAD y "Open terminal" / "Open folder".

### Cambiado

- La fila seleccionada en el tema claro es grafito en vez de negro.
- El diff ocupa todo el ancho del panel aunque las líneas sean cortas.
- "Changed Files" de un commit es una lista virtual: con miles de archivos ya no se traba (un frame con 5.000 archivos pasa de 1,5 s a ~2 ms).

### Corregido

- En Windows, las pestañas de la barra superior (y su ×) no respondían al clic.
- En Windows, "Show in folder" abría otra carpeta con rutas con `/`.
- La cabecera "GRAPH" del historial se montaba sobre "MESSAGE" con un solo carril.
- Las marcas de esquina de los estados vacíos no se veían.



Primera versión de la app nativa (Rust + GPUI) publicada como "latest".

### Añadido

- Nueva estética "Señal": blanco y tinta con un único acento naranja, tipografías Geist y Geist Mono, esquinas casi rectas y líneas finas. Historial en columnas (grafo, mensaje, autor, fecha, hash), fila seleccionada en tinta, etiquetas distintas para HEAD, ramas locales, remotas y tags, y grafo en escalones. Las instalaciones nuevas arrancan en tema claro.
- Pestañas de repositorio: la tira se desplaza con la rueda, siempre muestra la activa y tiene un menú "▾" con todos los repos; menú contextual (cerrar, cerrar otras, cerrar a la derecha, copiar ruta, mostrar en carpeta), cierre con clic central, reordenar arrastrando e indicador de operación en curso.
- Atajos: Ctrl+Tab / Ctrl+Shift+Tab (repo siguiente / anterior), Ctrl+1…9 (repo N), Ctrl+W (cierra lo que se ve), Ctrl+Shift+W (cierra el repo) y Ctrl+Shift+T (reabre el último repo cerrado).
- Al cerrar un repo con borrador de commit u operación en curso se pide confirmación; el borrador se conserva al cerrar y al reiniciar.
- Los repos con el mismo nombre se distinguen por su carpeta y los avisos de operaciones dicen de qué repo son.

### Cambiado

- Commits y Local Changes pasan a Alt+1 / Alt+2 (Ctrl+1…9 queda para los repos).
- Al abrir un repo se selecciona el commit de HEAD, no la primera fila del historial (que podía ser un stash).
- Mergeon pasa a ser la app nativa en Rust + GPUI (antes publicada como prerelease con tags `native-vX.Y.Z`). Los releases vuelven a usar tags `vX.Y.Z` y se publican como "latest".
- CI y releases corren en máquinas de instabox; Windows y macOS siguen en GitHub, y la compilación de comprobación de esas plataformas solo corre en pull requests o a mano.

### Corregido

- Los atajos de teclado no hacían nada en ninguna plataforma.
- Cerrar una pestaña a la izquierda de la activa cambiaba el repo activo.
- Al volver a un repo que cambió mientras su pestaña estaba oculta, el historial seguía mostrando datos viejos.
- Con muchos repos abiertos, las últimas pestañas (incluida la activa) quedaban fuera de alcance.
- El scroll reescribía la configuración en disco en cada paso de la rueda.
- El tooltip de una pestaña podía aparecer encima de su menú contextual.

### Eliminado

- La app Electron + Vue (`src/`, `electron/`, `e2e/`, configuración de npm) y sus workflows. Las instalaciones de Electron (v0.9.0 y anteriores) hay que reemplazarlas a mano por el instalador nativo.

## [0.9.0] — 2026-09-14

### Añadido

- La barra lateral de ramas, remotos y tags se puede ensanchar arrastrando su borde. Doble clic devuelve el ancho inicial y con el divisor enfocado las flechas lo ajustan. El ancho se guarda y es el mismo en todas las pestañas de repositorio.

### Cambiado

- En el historial, las ramas y tags van antes del mensaje del commit: son lo que identifica la fila.
- Cuando una rama local y su copia remota están en el mismo commit se muestra un solo badge con una nube por remoto (`main ☁`) en vez de repetir `main` y `origin/main`; el nombre completo del remoto está en el tooltip. Un remoto sin rama local en el commit sigue mostrándose entero.
- Los nombres de rama ya no se recortan con puntos suspensivos. Si en una fila no caben todos, los que sobran se resumen en un chip `+N` cuyo tooltip lista los nombres completos. El mensaje del commit conserva siempre un mínimo de ancho.

## [0.8.4] — 2026-09-14

### Corregido

- Los instaladores de 0.8.1, 0.8.2 y 0.8.3 no llegaron a publicarse: dos tests fallaban solo en el runner de Windows (rutas con barra invertida y un repositorio vacío que `git count-objects` mide en 0 KiB). Esta versión es la primera que entrega todo lo anotado en esas tres.

## [0.8.3] — 2026-09-14

### Cambiado

- En Linux los cambios del working tree llegan por eventos del sistema de archivos (chokidar) en vez de consultar `git status` cada dos segundos. Los directorios `node_modules`, `out`, `dist`, `release` y el interior de `.git` (salvo lo que mueve HEAD) no se vigilan, así un monorepo no agota los watches de inotify; si aun así no se puede vigilar, se vuelve al sondeo de antes. Windows y macOS siguen con el `fs.watch` recursivo nativo.

## [0.8.2] — 2026-09-14

### Cambiado

- El visor de diff (unificado y dividido) y el visor de archivos solo montan las líneas que caben en pantalla y colorean la sintaxis de esas líneas al pasar por ellas, en vez de todo el archivo de golpe. Un diff de miles de líneas abre al instante y deja de congelar la interfaz.

## [0.8.1] — 2026-09-14

### Cambiado

- El tamaño del repositorio del panel de resumen se obtiene de `git count-objects` (objetos sueltos y packs de `.git`) en vez de recorrer todo el working tree archivo por archivo. En repositorios grandes el resumen deja de tardar segundos y de cargar el disco.

## [0.8.0] — 2026-09-14

### Cambiado

- Mergeon pasa de Electron 28 a Electron 44 (Chromium 152, Node 24): arranque y consumo de memoria del motor más recientes, y dos años de parches de seguridad que la 28 ya no recibía.
- Se actualizan las herramientas de build y pruebas (electron-vite 5, Vite 7, electron-builder 26, Vitest 4, Playwright 1.63) y la integración continua pasa a Node 22.

### Corregido

- simple-git 3.36 rechazaba variables del entorno del usuario como `EDITOR`, `PAGER`, `SSH_ASKPASS` o `GIT_SSH_COMMAND` y dejaba a la app sin poder ejecutar git; se habilitan de forma explícita porque ese entorno es el del propio usuario.

## [0.7.10] — 2026-09-14

### Corregido

- Las notificaciones ya no dejan de aparecer si el reloj del sistema retrocede, por ejemplo tras un ajuste horario o al despertar el equipo de la suspensión.
- Un mismo error vuelve a avisarse si reaparece más tarde; antes quedaba silenciado durante el resto de la sesión.

## [0.7.8] — 2026-09-11

### Corregido

- Los reintentos automáticos de una operación fallida ya no acumulan notificaciones idénticas mientras el aviso anterior siga visible.
- Un error al comparar ramas muestra un solo aviso, con el detalle completo disponible.

## [0.7.7] — 2026-09-11

### Cambiado

- En la pestaña del diff de un archivo, el divisor entre la lista de archivos y el diff se puede arrastrar. El ancho es el mismo en todas las pestañas de diff y se guarda al soltar; doble clic sobre el divisor lo devuelve al ancho inicial, y con el divisor enfocado las flechas lo ajustan.

### Corregido

- Abrir un archivo con doble clic desde una pestaña de diff dejaba la pestaña nueva en blanco hasta cambiar de pestaña y volver.

## [0.7.6] — 2026-09-11

### Cambiado

- La lista de archivos de un commit es plana: nombre del archivo con su carpeta en gris al lado, sin árbol. Igual en la pestaña de diff de un commit.
- «Repo tree» pasa de sub-pestaña del inspector a botón que abre la estructura del repositorio en ese commit en una pestaña propia.

### Corregido

- Al cambiar de archivo dentro de una pestaña, la lista ya no se recarga ni pierde el scroll: la pestaña conserva su panel y los archivos del commit se piden una sola vez.

## [0.7.5] — 2026-09-11

### Cambiado

- Dentro de una pestaña de archivo, un clic cambia el archivo de esa misma pestaña; el doble clic o el menú contextual («Open here» / «Open in new tab») abren otra.
- La pestaña de un archivo de un commit muestra la misma lista que el detalle del commit: árbol, insignia de estado coloreada y cifras de cambio.
- Listas de archivos más densas: filas más bajas y sin línea separadora, en commits y en Local Changes.
- Linux: «Open terminal» respeta la variable `TERMINAL` y arranca cualquier terminal en la carpeta del repositorio aunque no acepte flag de directorio.

### Corregido

- Windows: «Open terminal» no abría nada. Ahora abre Windows Terminal en la carpeta del repositorio y, si no está instalado, una consola `cmd` visible.

## [0.7.4] — 2026-09-10

### Cambiado

- Las notificaciones muestran un resumen de dos o tres líneas; el texto completo se abre desde «Details».
- Un solo aviso por flujo: commit con push, merge con push, clonación y push de etiquetas ya no emiten notificaciones intermedias ni repetidas.
- Los avisos de actualización se reemplazan en sitio en lugar de acumularse.
- Toda notificación se cierra sola en ocho segundos como máximo.

### Corregido

- Windows: la app volvía a arrancar sin ventana visible desde 0.7.3, porque Electron no emite `ready-to-show` con la barra de título integrada. La ventana se muestra también al terminar la carga, y su fondo inicial sigue el tema.
- El botón de cierre ya no queda tapado por «Download», «Restart» o «View releases».
- Un mismo error capturado por varias capas ya no genera dos notificaciones.
- Se eliminó el prefijo técnico de IPC de los mensajes de error.

## [0.7.3] — 2026-09-10

### Cambiado

- Avatares de GitHub permitidos por la política de contenido, con icono de respaldo cuando la imagen no carga.
- Separación entre las tarjetas de cuentas y el botón Add Account en Configuración.
- Clone From GitHub limita la altura del listado y mantiene visibles el destino y las acciones.
- La carpeta de destino puede elegirse antes del repositorio; la ruta se actualiza al cambiar la selección.
- Windows integra sus controles nativos de ventana en la fila de pestañas, con colores adaptados al tema.

## [0.7.2] — 2026-09-09

### Cambiado

- Barra global siempre visible con Home fijo, repositorios y una única pestaña de Configuración.
- Commits y Local Changes son pestañas fijas independientes por repositorio; Local Changes incluye un contador de archivos cambiados.
- El panel de ramas queda dentro de Commits y las acciones Git ocupan todo el ancho bajo las pestañas de proyectos.
- Cerrar un archivo devuelve a su vista de origen y cerrar el último repositorio activa Home.
- La sesión guardada se adapta a las nuevas pestañas y conserva los archivos abiertos. Los borradores se mantienen al alternar entre Home, repositorios y Configuración.

## [0.7.1] — 2026-09-09

### Cambiado

- El panel lateral del repositorio y el listado del inicio ocupan toda la altura, junto a la franja de iconos.
- Las pestañas de proyectos aparecen arriba, seguidas por las acciones Git y las pestañas de contenido.
- Configuración ocupa todo el espacio a la derecha de la franja de iconos y conserva el contexto del repositorio al volver.
- La versión y las acciones de actualización se muestran en Ayuda / Acerca de; el engranaje indica cuando hay una actualización disponible.

### Corregido

- El inicio deja de mostrar la barra Git, sus botones desactivados y el mensaje «No repository opened».
- La barra de acciones se adapta al ancho del área del repositorio en ventanas pequeñas.

## [0.7.0] — 2026-09-09

### Añadido

- Franja lateral con accesos para abrir o clonar repositorios, recargar la aplicación, consultar el registro de operaciones y abrir Configuración.
- Vista central de Configuración con apariencia, identidad Git, auto-fetch, cuentas, ayuda, atajos y actualizaciones.
- Pestañas de archivos independientes por repositorio, con una pestaña fija Commits que conserva el historial y Local Changes. Cada archivo identifica si se revisa el working tree, el index o una versión histórica.
- Restauración de pestañas, versión seleccionada y posición de lectura después de recargar o reiniciar la aplicación.

### Cambiado

- Los diffs y el contenido de Repo tree se abren en pestañas permanentes. Abrir de nuevo el mismo archivo y versión activa la pestaña existente.
- Las operaciones Git conservan sus accesos en la toolbar; Reflog tiene un acceso propio. El menú superior personalizado se integra en la interfaz y macOS conserva su menú convencional del sistema.
- Local Changes conserva el borrador de commit al alternar entre pestañas, repositorios y Configuración; en ventanas pequeñas el formulario aparece debajo de los archivos.

### Corregido

- Los archivos con cambios preparados y sin preparar se pueden revisar por separado sin mezclar ambos diffs.
- Las respuestas tardías de Git no sustituyen los detalles o el árbol de otro commit al cambiar rápidamente de selección.

## [0.6.7] — 2026-09-07

### Corregido

- Las ramas locales con barra en el nombre (`feature/login`, `fix/crash`) ya no se tratan como remotas. El historial pedía las refs sin `--decorate=full`, así que git las entregaba con el nombre corto y el cliente tenía que adivinar el tipo con "si tiene barra es remota". Por eso salían con icono de nube en el historial, el doble click ofrecía crear un tracking de una rama que ya era tuya, y el marcado de "ya está en tu upstream" podía equivocarse. Ahora el tipo se lee de la ruta real de cada ref.
- Doble click sobre el commit de una rama remota cuya rama local ya tienes pero está atrasada: te lleva a tu rama local, en vez de caer al diálogo de HEAD desanclado.

## [0.6.6] — 2026-09-07

### Corregido

- El doble click sobre un commit que solo tiene una rama remota ya no plantea dos caminos. Antes ofrecía elegir entre crear la rama local que trackea la remota y dejar HEAD desanclado; ahora abre directamente el mismo diálogo de tracking que sale al doble click sobre esa rama en el sidebar. El checkout desanclado queda reservado a los commits que no tienen ninguna rama encima.

## [0.6.5] — 2026-09-07

### Corregido

- El doble click sobre un commit ya no abre un diálogo cuando no hay nada que decidir. Si el commit es la punta de una rama local que ya tienes y el árbol de trabajo está limpio, se cambia a esa rama directamente. El diálogo queda para los casos con una decisión real: cambios sin commitear (mantener / stash / descartar), varias ramas locales en el mismo commit, crear la rama local que trackea una remota, o desanclar HEAD en un commit sin ramas.

## [0.6.4] — 2026-09-07

### Corregido

- El diálogo de confirmación ya no aparece detrás del diálogo que lo pidió. Al confirmar un merge o un rebase desde el diálogo de acciones de rama, el confirm quedaba tapado (y oscurecido) por él: todos los diálogos compartían el mismo nivel de apilamiento y ganaba el que se hubiera montado más tarde. Ahora confirm y prompt viven en una capa por encima.
- Doble click (o "Checkout" del menú contextual) sobre una rama de origin que ya tienes en local te cambia a esa rama local, en vez de negarse con un aviso y mandarte a buscarla en la lista de arriba.

### Cambiado

- El doble click sobre un commit del historial deja de asumir siempre "checkout desasociado". Ahora mira los refs del commit: si es la punta de una rama local te ofrece cambiarte a ella, si solo tiene una rama remota te ofrece crear la local que la trackea, y si hay varias candidatas puedes elegir. El HEAD desanclado sigue disponible, con su aviso.
- El checkout desde el historial pasa por el mismo diálogo de cambios locales que el del sidebar (mantener, guardar en stash y reaplicar, o descartar). Antes git rechazaba el cambio y solo salía un toast de error.

## [0.6.3] — 2026-09-04

### Corregido

- El historial de commits ya no se rehace en cada fetch o pull. Antes cualquier fetch —incluido el auto-fetch periódico— vaciaba la lista, la sustituía por el spinner y la reconstruía desde la primera página: se perdían el scroll, las páginas ya cargadas por scroll infinito y el panel de detalle. Ahora:
  - Fetch y pull informan si algún ref se movió de verdad; si no cambió nada, no se marca mutación ni se recarga.
  - El refresco relee la ventana completa que el usuario tenía cargada, en vez de encogerla a 200 commits.
  - Las filas que no cambiaron conservan su identidad, así que solo se repinta lo que de verdad cambió, y el spinner a pantalla completa queda reservado para la primera carga de un repositorio.
  - La posición del scroll se ancla al refrescar: si estabas leyendo commits viejos, la lista ya no salta a la punta cuando llegan commits nuevos.
  - Fetch y pull no recargan el panel de detalle del commit seleccionado (su contenido es inmutable).

## [0.6.2] — 2026-08-04

### Corregido

- Linux: al abrir un repositorio ya no aparece el diálogo "A JavaScript error occurred in the main process" (write EPIPE). El watcher de archivos va directo a polling en Linux (fs.watch recursivo no existe en el Node de Electron 28) y stdout/stderr quedan blindados contra pipes cerrados en apps empaquetadas.

### Cambiado

- Sidebars más densos: las filas y headers de sección del sidebar del repositorio y del árbol de repos del dashboard bajan de 32px a 26px de alto, con estados vacíos y paddings ajustados al mismo ritmo.

## [0.6.1] — 2026-08-04

### Añadido

- Doble click en una rama remota abre un diálogo de confirmación que explica que se creará una rama local trackeando la remota (estilo Fork), con las mismas opciones para los cambios locales.
- Doble click en un commit del historial pide confirmación y hace checkout (detached HEAD).

### Corregido

- Fetch y pull ahora refrescan las ramas del remoto y los tags en el sidebar; antes las refs nuevas de origin no aparecían hasta un refresh manual. Push refresca las ramas del remoto tras publicar una rama nueva.

## [0.6.0] — 2026-08-03

### Añadido

- Atajos de teclado: ⌘1/⌘2 cambian de vista, ⌘F abre la búsqueda de commits, ⌘⇧L el panel de operaciones, ⌘⇧R el reflog, ⌘, las preferencias y ⌘/ la hoja de atajos (Help > Keyboard Shortcuts). Las flechas ↑/↓ navegan el historial. En Windows/Linux, Ctrl en lugar de ⌘.
- Auto-fetch periódico: fetch silencioso en segundo plano (intervalo configurable, por defecto cada 10 minutos) mientras la ventana tiene foco, y al recuperar el foco si el último quedó atrás. Los resultados van al panel de operaciones, sin toasts.
- Diálogo de preferencias (File > Preferences…): identidad git (nombre/email, global o por repositorio, con herencia visible), auto-fetch y tema.
- Vista de reflog (Repository > Reflog…): las últimas posiciones de HEAD con acciones para crear una rama en una entrada o hacer hard-reset a ella — la red de seguridad tras un reset o rebase que salió mal.
- Diálogo About con la versión y enlace a los releases.

### Corregido

- `reset` valida el modo contra una lista cerrada (soft/mixed/hard/keep/merge) en vez de interpolarlo directo al comando.
- El item About del menú no abría nada; ahora sí.

## [0.5.0] — 2026-08-03

### Añadido

- Rebase interactivo completo: todo-list editable desde el context menu del historial (reordenar con ↑/↓, pick/reword/squash/fixup/drop, mensajes de reword inline). El banner de rebase muestra el progreso ("3/7") y los editores de git van por archivo con `ELECTRON_RUN_AS_NODE`, sin límites de línea de comandos ni escapado de shell.
- Panel de operaciones: historial de la sesión con fases, duración, outcome y el mensaje completo de cada operación git (los toasts truncan y desaparecen). Botón con punto rojo de errores sin revisar en la toolbar.
- "Load more" real en el historial: páginas de 200 commits al hacer scroll, con dedupe si el historial mutó entre páginas.

### Corregido

- `git blame` de archivos grandes (>1 MB) ya no congela el proceso principal: se anuncia el tamaño en vez de calcularse. Los diffs (working tree, commit, stash, untracked) tienen cap de 2 MB con estado "Diff too large".
- El spinner "Loading commits" nunca se mostraba; el visor de blame gana un guard de carrera al cambiar de archivo.

## [0.4.1] — 2026-08-03

### Añadido

- Navegación por refs: click simple en una rama (local, starred o bajo un remoto) o en un tag lleva el historial a su commit; el doble click sigue haciendo checkout. El HEAD conserva su marca y la rama que se está viendo queda señalada en el sidebar.
- El historial auto-scrollea a la fila seleccionada (también desde la búsqueda).

### Corregido

- Si el commit pedido cae fuera de la ventana cargada, se reintenta con una ventana mayor y se avisa en vez de seleccionar otro commit en silencio.

### Cambiado

- "Toggle Developer Tools" ya no aparece en el menú View del build empaquetado.

## [0.4.0] — 2026-08-02

### Añadido

- Gestión completa de remotos desde el sidebar: editar URL, renombrar y eliminar (antes solo se podía añadir).
- Búsqueda en el historial completo hecha por git: por mensaje, autor (`author:nombre`) o hash, sin el límite anterior de 200 commits.
- Diff de stash: click en un stash abre sus archivos y parches antes de aplicarlo o borrarlo.
- Submódulos reales en el sidebar: listado con estado (ok, sin inicializar, desincronizado, conflicto), init/update individual o de todos, y apertura como pestaña del workspace.
- Suite E2E de flujos git reales contra un remoto bare local (fetch, pull, push, conflictos con resolución y abort, stash, ramas), corriendo también en CI.

### Corregido

- El drop de rebase interactivo podía eliminar otro commit si un mensaje citaba el hash corto del commit a borrar.
- Los flujos que hacían pop de `stash@{0}` a ciegas ahora apuntan al stash exacto que crearon (el índice se corre cuando aparecen stashes intermedios), y pop/drop desde el sidebar volvieron a funcionar.
- El push posterior a un merge ya no asume que el remoto se llama `origin`.
- `git pull` pasa la estrategia elegida (`--no-rebase`/`--rebase`) de forma explícita: git moderno abortaba los pull divergentes con "Need to specify how to reconcile divergent branches".
- Las secciones expandidas del sidebar (tags, stashes, submódulos) ya no quedan vacías al abrir o restaurar un repo hasta refrescar a mano.
- Errores que antes morían en la consola ahora avisan: búsqueda de commits fallida, persistencia de tema/preferencias/layout, y rollbacks de stash fallidos.

### Cambiado

- Toda la interfaz quedó unificada en inglés (los toasts del updater y del sidebar estaban en español).
- CSS compartido para las secciones del sidebar y los diálogos de fetch/push/pull, corrigiendo alturas divergentes entre secciones.
- Endurecimiento: Content-Security-Policy en el renderer y allowlist http/https para abrir enlaces externos.

## [0.3.0] — 2026-07-28

- Staging por bloques (hunks) desde el visor de diffs, commit con amend y caja de commit integrada en la vista de cambios.
- Publicar tags en el remoto y borrarlos del remoto.
- Detalle de commit rehecho: una sola vista, árbol de archivos unificado con la vista de cambios y la misma insignia de estado en ambas listas.
- Preferencias de diff (unified/split, ignorar espacios) compartidas y persistidas.
- Indicador del commit en el que está parado el working tree.
- Correcciones del visor split y del updater (errores resumidos, sin comprobar feeds inexistentes).

## [0.2.1] — 2026-07-26

- Icono de la aplicación actualizado.

## [0.2.0] — 2026-07-26

- La aplicación pasa a llamarse **Mergeon**, con icono propio.
- Autenticación GitHub por device flow directo contra github.com: se elimina el backend propio y no se guardan secretos.
- Sistema de diseño T3 Code aplicado a toda la app; grafo de commits rediseñado.
- Listado de repositorios paginado, con repos de colaborador incluidos vía OAuth App.

## [0.1.x] — 2026-06 a 2026-07

- Base del producto: apertura de repos, historial con grafo, working tree, operaciones git principales (fetch/pull/push/checkout/merge/rebase/reset/revert/cherry-pick), stash, tags, conflictos con continue/abort/skip.
- Cola de mutaciones git serializada por repo con cancelación de operaciones.
- Auto-updater contra feed generic y pipeline de release multiplataforma en GitHub Actions.
