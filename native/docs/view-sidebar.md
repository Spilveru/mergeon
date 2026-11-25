# `mergeon-view-sidebar`

Sidebar del repositorio activo: navegación, filtro, secciones (Starred, Branches,
Remotes, Tags, Stashes, Submodules), búsqueda de commits, menús contextuales y los
diálogos que abre. Convenciones generales en [`ui-port.md`](ui-port.md).

| Original (`src/components/`) | Aquí |
|---|---|
| `layout/Sidebar.vue` | `Sidebar` (`sidebar.rs`, `actions.rs`, `flows.rs`, `menus.rs`, `logic.rs`) |
| `layout/SidebarNavigation.vue`, `SidebarCommitSearchPanel.vue` | `sidebar.rs` (`render_navigation`, `render_search_panel`) |
| `layout/Sidebar{Branch,Remote,Tag,Stash,Submodule}Section.vue`, `sidebar-section.css` | `render_sections.rs`, `section.rs` |
| `repository/CheckoutBranchDialog.vue` | `dialogs::open_checkout_branch_dialog` |
| `repository/BranchActionDialog.vue` | `dialogs::open_branch_action_dialog`, `open_branch_comparison_dialog` |
| `repository/AddRemoteDialog.vue` | `dialogs::open_add_remote_dialog` |
| `repository/CreateBranchDialog.vue` | `dialogs::open_create_branch_dialog` |
| `<AppDialog>` de detalles de tag (en `Sidebar.vue`) | `dialogs::open_tag_details_dialog` |
| `.sidebar-resizer` de `RepositoryWorkspace.vue` + `useSidebarWidth` | `SidebarResizer` |
| `repository/StashDiffDialog.vue` | no se porta aquí: `SidebarEvent::OpenStashDiff` |

## Montaje

```rust
use mergeon_view_sidebar::{Sidebar, SidebarEvent, SidebarResizer};

mergeon_view_sidebar::init(cx);                       // no registra nada hoy; por uniformidad

// Una sola instancia: trabaja siempre sobre `AppModel::active_repository()`.
let sidebar = cx.new(|cx| Sidebar::new(window, cx));
cx.subscribe(&sidebar, |this, _, event: &SidebarEvent, cx| match event {
    SidebarEvent::CommitSelectionRequested { repo_id, request } => { /* historial */ }
    SidebarEvent::OpenStashDiff { repo_path, stash } => { /* diálogo de stash */ }
})
.detach();

// En `render`, dentro de la fila que comparte con la vista de commits:
div().flex().child(sidebar.clone()).child(SidebarResizer::new(&sidebar)).child(commits)
```

- Orden de las secciones: Starred, Branches, Pull requests, Remotes, Worktrees,
  Tags, Stashes, Submodules. Cada una guarda su plegado en
  `settings.sidebarSections` (las nuevas, `pullRequests` y `worktrees`, como
  claves que el store conserva sin conocerlas).
- `Sidebar::new` usa `AppModel::global(cx)`, lee `settings.sidebarSections`
  (`load_sidebar_sections`) y el ancho guardado (`load_sidebar_width`), y carga las
  secciones que arrancan desplegadas. Tags, stashes y submódulos se leen siempre
  (`load_expanded_sections`), aunque estén plegados: su cabecera muestra cuántos
  hay. Las filas de stash tienen menú contextual (`menus::stash_menu`).
- El sidebar se da a sí mismo el ancho de `AppModel::sidebar_width()` y ocupa todo el
  alto de su contenedor. El scroll es suyo.
- En la app Vue cada repositorio monta su propio sidebar (`v-if` del activo). Aquí hay
  uno: al cambiar el repositorio activo vacía el filtro y la búsqueda, vuelve al panel
  del repositorio y cierra Branch Actions, que es lo que se veía al remontarse.
- Confirmaciones, prompts y avisos van por `AppModel` (`confirm`, `confirm_with_option`,
  `prompt`, `notify_*`): los pinta el `UiHost` de la cáscara. Los diálogos propios se
  abren sobre `mergeon_ui::dialog` en la ventana del sidebar.

### `SidebarResizer`

Divisor de 4px (con 3px más de margen para acertarle) que va justo después del
sidebar. Arrastrar llama a `start/drag/stop_sidebar_resize`, las flechas izquierda y
derecha a `nudge_sidebar_width(±16)` y el doble clic a `reset_sidebar_width`. El origen
del arrastre es el borde izquierdo del sidebar; el ancho del contenedor es lo que queda
de ventana a su derecha, salvo que se fije con `Sidebar::set_container_width`.

La cáscara puede usar este divisor o el suyo: el ancho vive en `AppModel` y el sidebar
solo lo aplica.

## Sección "Pull requests" (`pull_requests.rs`)

Propia, sin original. `PullRequestsSection` es una entidad que crea el sidebar
(`sidebar.pull_requests_section()`) y pinta entre Branches y Remotes. Los datos
los mantiene `AppModel` (ver `app-api.md`, "Pull requests y checks de
GitHub"); la sección pide `watch_pull_requests` y solo repinta si cambia lo que
enseña (`generation` del estado, repositorio, rama o plegado).

- Oculta sin remoto de github.com, con GitHub Enterprise o si la cuenta no ve
  el repositorio. Sin cuenta: "Sign in to GitHub to see pull requests" (abre
  Settings → Accounts).
- Cabecera `PULL REQUESTS` con el número de PRs abiertos y, al pasar, el botón
  de refresco. Plegada o desplegada se guarda en
  `settings.sidebarSections.pullRequests` (por defecto desplegada).
- Filas virtuales de 42px (`VirtualRows`): ícono de PR (borrador aparte),
  `#n`, título, estado de checks con ícono y color (✓/✗/● y `passed/total`);
  debajo, monograma del autor (como la columna Author del historial; no se
  descargan avatares), autor, rama, `Draft` y la revisión (✓ aprobado, cambios
  pedidos, revisión pendiente). Tooltip con todo. El PR de la rama actual va
  seleccionado sin foco.
- Clic: abrir en el navegador. Menú: "Open in browser", "Checkout PR branch"
  (trae la rama con `fetch_pull_request` y luego `CheckoutRemoteBranch` o
  `CheckoutBranch("pr-<n>")`, con sus diálogos de siempre), "Copy link" y,
  solo si la rama es del propio repositorio, "Compare with current branch"
  (trae la rama y abre la comparación navegable con
  `CompareWithCurrent("<remoto>/<rama>")`, como el menú de una rama remota).
- La paleta tiene "Checkout pull request…" (los PRs ya leídos → "Checkout PR
  branch") y "Open pull request of current branch".
- Fallos discretos bajo la cabecera (`pull-requests-error`), sin toasts.
- `menu(número, cx)` describe el menú como datos (`PullRequestMenuEntry`) y
  `run_command(PullRequestCommand, window, cx)` lo ejecuta.

Ids: `section-header-pull-requests`, `pull-requests-refresh`,
`pull-requests-sign-in`, `pull-requests-error`, `pull-request:<n>`,
`pull-request-checks:<n>`, `pull-request-review:<n>`.

## Eventos (`SidebarEvent`)

| Evento | Cuándo | Quién lo atiende |
|---|---|---|
| `CommitSelectionRequested { repo_id, request }` | Clic en una rama local o remota, en un tag o en un resultado de la búsqueda (`requestCommitFocus`). `request` es un `CommitSelectionRequest { hash, ref_name, requested_at }`. Antes de emitirlo el sidebar ya llamó a `handle_change_view(Commits)`. | La vista de commits: recargar con `preferredCommitHash = hash` y `ensureCommitVisible` (el `watch` de `commitSelectionRequest.requestedAt` de `CommitView.vue`). |
| `OpenStashDiff { repo_path, stash }` | Clic en un stash. | El crate del visor de diffs (`StashDiffDialog.vue`). |
| `OpenComparison(ComparisonRequest { repo_id, repo_path, base, head, mode })` | "Compare with '<rama actual>'..." en el menú de una rama local o remota (base = rama actual o `HEAD` si está desanclado), y "Browse changes" de Branch Comparison (base = destino, head = origen). `mode` es `CompareMode::SinceDivergence` por defecto. | La cáscara: `mergeon_view_commits::open_compare_view(&request, window, cx)`, y con su `CompareViewEvent::ShowCommit` cierra el diálogo y selecciona el commit en el historial. |

`commitSelectionRequest` vivía en el objeto del repositorio del store; `AppModel` no lo
tiene, así que lo guarda el sidebar, por repositorio:

- `sidebar.commit_selection_request(repo_id) -> Option<&CommitSelectionRequest>`: la
  última petición (para quien se monte después del evento).
- `sidebar.viewed_ref(repo_id) -> Option<&str>`: la ref que se marca como "la estás
  viendo" (`viewedRefName`).
- `sidebar.clear_viewed_ref(repo_id, cx)`: **lo llama la vista de commits** cuando el
  usuario elige otro commit a mano (`handleHistorySelect`); retira la marca sin tocar el
  resto de la petición.

Del modelo escucha `AppEvent::FocusCommitSearch` (`mod+f`): muestra la búsqueda y
enfoca su campo.

## API de `Sidebar`

| Qué | Método |
|---|---|
| Panel a la vista | `panel_mode()`, `is_search_panel_visible(cx)`, `show_repository_panel(cx)`, `show_search_panel(window, cx)` |
| Filtro de ramas | `filter_input()`, `filter_text(cx)`, `set_filter_text(text, window, cx)` |
| Búsqueda de commits | `search_input()`, `set_search_query(query, window, cx)`, `search_results()`, `is_search_loading()`, `select_search_result(full_hash, cx)`, `search_error()`, `search_options()`, `search_has_more()`, `load_more_search_results(cx)` |
| Filtros de la búsqueda | `is_search_filters_open()`, `toggle_search_filters(cx)`, `search_filter_fields(cx)`, `set_search_filters(SearchFilterFields, window, cx)`, `set_search_date_preset(DatePreset, cx)`, `set_search_code_regex(bool, cx)`, `clear_search_filter(SearchFilterKind, window, cx)` |
| Secciones | `is_section_expanded(Section, cx)`, `toggle_section(Section, cx)` |
| Refresco | `refresh(cx)`, `is_refreshing(cx)` |
| Ver una ref en el historial | `view_branch(name, cx)`, `view_remote_branch(remote, branch, cx)`, `view_tag(tag, cx)` |
| Órdenes | `run_command(SidebarCommand, window, cx)`, `drop_branch(origen, destino, window, cx)` |
| Menús (como datos) | `local_branch_menu(name, cx)`, `remote_branch_menu(remote, branch, cx)`, `tag_menu(tag, cx)`, `remote_menu(name, cx)` → `Vec<MenuEntry>` |
| Diálogos abiertos | `checkout_dialog()`, `branch_action_dialog()`, `create_branch_dialog()`, `add_remote_dialog()`, `tag_details_dialog()` |
| Divisor | `set_container_width(Option<Pixels>, cx)` |

`SidebarCommand` es la lista cerrada de lo que el sidebar sabe hacer (checkout,
fast-forward, pull, push, Branch Actions, nueva rama, nuevo tag, upstream, renombrar,
borrar, copiar nombre, destacar; lo mismo para ramas remotas; tags; remotos; stashes;
submódulos). Cada entrada de menú, botón de fila y doble clic es una de esas órdenes,
así que un atajo o un menú de la cáscara puede lanzar la misma con `run_command`.

`MenuEntry` describe una entrada (`label()`, `is_disabled()`, `is_danger()`,
`command()`) o un separador; `menus::{local_branch_menu, remote_branch_menu, tag_menu,
remote_menu}` las calculan a partir de un `OpenRepository` sin necesidad de vista.

### Búsqueda de commits con filtros

El cuadro admite la sintaxis rápida (documentada en el tooltip del `?` junto al
título, `logic::SEARCH_SYNTAX_HELP`): `author:`, `path:`, `branch:`, `after:`,
`before:` (fechas de git o relativas `7d`, `2w`, `3m`, `1y`; una fecha sola,
`2024-03-10`, cuenta el día entero en los dos extremos: git la leería con la hora
actual), `code:` (texto que el
commit añade o quita, `-S`) y `regex:` (líneas cambiadas que casan, `-G`); los
valores con espacios van entre comillas (`path:"my docs"`). "Filters" despliega los
campos equivalentes (Path, Branch, Date con Any / 7 days / 30 days / Custom y sus
After / Before, Code contains con la casilla "Regular expression"). Si un filtro está
en los dos sitios gana el cuadro (`logic::merge_search_filters`). Los filtros
activos salen como chips bajo el cuadro; su × lo quita del cuadro y de los campos.
Con algún filtro, un hex suelto es texto (no el atajo del commit exacto). La búsqueda
pide páginas de 100 (`--max-count`); "Load 100 more" pide la siguiente (`--skip`) y
la añade a la lista virtual sin moverla. Los fallos (una rama que no existe) se
dicen en el panel (`commit-search-error`), no con un aviso: suelen ser un filtro a
medio escribir.

`logic` tiene lo puro: `parse_commit_search_query`, `merge_search_filters`,
`search_filter_chips`, `remove_search_filters`, `relative_date`, `parse_upstream_reference`,
`branch_tracking_reference`, `normalize_remote_url`, `build_pull_request_url`,
`detached_head_label`, `filtered_branches`.

### Qué relee cada orden

Como en el original, cada flujo aplica su resultado
(`apply_repository_operation_result`) y relee solo lo que pudo cambiar
(`sync_repository_read_model`). Los que el original protegía con `runBranchOperation`
usan `run_repo_operation_single_flight(repo, <operación>)` con el mismo nombre:
`checkout`, `create-branch`, `create-tag`, `setup-tracking`, `rename-branch`,
`delete-branch`, `delete-remote-branch`, `fast-forward-branch`, `pull`, `push`,
`add-remote`, `set-remote-url`, `rename-remote`, `remove-remote`, `submodule-update`,
`sidebar-refresh`.

## Diálogos (`mergeon_view_sidebar::dialogs`)

Cada `open_*` crea una entidad con el estado del diálogo, la abre sobre
`mergeon_ui::dialog` y la devuelve; quien la abre se suscribe a sus eventos **y la
conserva**: un diálogo cerrado no retiene su entidad.

| Función | Entidad y eventos |
|---|---|
| `open_checkout_branch_dialog(CheckoutBranchOptions, window, cx)` | `CheckoutBranchDialog` → `Submit { local_changes_mode, target }` / `Close`. Las opciones son las props del original (`target_label`, `has_local_changes`, `remote_branch`, `target_options`, `title`, `description`): sirve también a la vista de commits (checkout de un commit, elección entre varias ramas). |
| `open_create_branch_dialog(CreateBranchOptions, window, cx)` | `CreateBranchDialog` → `Submit { branch_name, checkout_after_create, local_changes_mode }` / `Close`. |
| `open_add_remote_dialog(nombre, url, window, cx)` | `AddRemoteDialog` → `TestConnection { name, url }` / `Submit { name, url }` / `Close`. Solo recoge datos; el progreso se le devuelve con `set_testing_connection`, `set_connection_result`, `set_submitting`, y se cierra con `close` / `dismiss`. |
| `open_branch_action_dialog(&model, repo_path, origen, destino, Option<BranchAction>, window, cx)` | `BranchActionDialog` → `ActionCompleted(BranchActionCompleted { result, mutation_operation })` / `Close`. Hace él mismo el merge (con push opcional), el rebase y la comparación. |
| `open_branch_comparison_dialog(origen, destino, BranchComparison, window, cx)` | Sin entidad: muestra una comparación ya calculada. Abierto desde Branch Actions, su pie añade "Browse changes": cierra los dos diálogos y `BranchActionDialog` emite `BrowseComparison { base, head }` (el sidebar lo pasa como `SidebarEvent::OpenComparison`). |
| `open_tag_details_dialog(&model, repo_path, tag, window, cx)` | `TagDetailsDialog` (`details()`, `is_loading()`); se cierra solo si la lectura falla. |
| `open_cleanup_branches_dialog(&model, repo_id, repo_path, window, cx)` | `CleanupBranchesDialog` (`candidates()`, `selected_names()`, `request()`, `set_base`, `set_selected`, `toggle_all`, `set_delete_remote`, `set_force_unmerged`, `submit`). Sin eventos: hace él mismo el borrado (`flows::cleanup_branches`). |

### Clean up merged branches

`SidebarCommand::CleanupMergedBranches`: entrada "Clean up merged branches…" del
menú de la cabecera Branches (clic derecho; `branches_section_menu(cx)` /
`menus::branches_section_menu(repo)`) y botón de escoba que aparece al pasar
el cursor por la cabecera. También en la paleta ("Clean up merged
branches…", `Command::Sidebar` → `run_command(SidebarCommand::CleanupMergedBranches, …)`).

El diálogo pide `AppModel::get_merged_branches` (fuera del hilo de UI) y lista,
virtualizadas, las ramas locales integradas en la base: por defecto la rama de
`origin/HEAD` (la local si existe), si no `main` o `master`; el select "Merged
into" permite elegir otra (ramas locales y la remota por defecto). Nunca lista
la actual, la base, las extraídas en otro worktree ni las protegidas (`main`,
`master`, `develop`, `dev`, `trunk`, `staging`, `production`, `release/*`…,
`mergeon_git_query::is_protected_branch`). Cada fila: casilla, nombre, último
commit (hash, asunto, fecha relativa) y etiquetas:

- integradas por merge o fast-forward (`git for-each-ref --merged`): marcadas;
- "probably merged (squash)": lo que la rama cambió desde su base común ya
  está igual en la base (comparando árboles; solo en las 100 no integradas más
  recientes); sin marcar, se borran con `-D` si se marcan;
- "N unmerged commits" + "gone": commits sin integrar y upstream borrado en el
  remoto; solo se pueden marcar con "Allow deleting branches with unmerged
  commits" (se borran con `-D`, con un aviso en el diálogo).

"Also delete remote branches" (apagada) borra también la rama remota de las
que aún la tienen. "Delete N branches" cierra el diálogo y lanza una mutación
`cleanup-branches` por la cola (las no forzadas se vuelven a comprobar contra la
base justo antes de borrarlas): aviso con el resultado ("Deleted 7 branches.",
o los fallos uno por línea), entrada en el log de operaciones (con el `was
<hash>` de cada rama para recuperarla) y relectura de ramas, remotos y
destacadas (las borradas dejan de estar destacadas).

## Galería y capturas

```bash
cd native
cargo run -p mergeon-view-sidebar --example sidebar_gallery
crates/mergeon-view-sidebar/shots.sh [dark|light|all] [regex de casos]
```

La galería abre un `AppModel` real (backend de `mergeon_app::testing::TestBackend`:
ajustes en memoria, sin vigilar el disco) sobre el repositorio de
`tools/make-fixture.sh`, creado en un temporal, y coloca el sidebar donde lo tiene la
app (ventana de 1440×900, a la derecha del rail y bajo las barras) para comparar con
`target-shots/reference/` sin recolocar.

| Variable | Valores |
|---|---|
| `MERGEON_THEME` | `dark` (defecto) · `light` · `system` |
| `MERGEON_SIDEBAR_FIXTURE` | `demo` (defecto) · `detached` · `behind` (↑2 ↓2) · `submodules` · `empty` |
| `MERGEON_SIDEBAR_REPO` | ruta de un repositorio propio en lugar de un fixture |
| `MERGEON_SIDEBAR_STARRED` | ramas destacadas, separadas por comas |
| `MERGEON_SIDEBAR_SECTIONS` | `all` (defecto) · `none` · `default` · lista (`starred,branches,...`) |
| `MERGEON_SIDEBAR_STEPS` | pasos separados por `;`, uno cada 400 ms: `filter:<t>`, `search:<t>`, `panel:search`, `toggle:<sección>`, `refresh`, `view:<rama>`, órdenes (`checkout:<rama>`, `branch-action:merge:<rama>`, `add-remote`, `tag-details:<tag>`, `delete-branch:<rama>`...) y entrada simulada (`click:x,y`, `dblclick:x,y`, `rclick:x,y`, `move:x,y`, `key:<tecla>`, `type:<texto>`, `wait:<ms>`). La lista completa está en la cabecera de `examples/sidebar_gallery.rs`. |

`shots.sh` deja en `native/target-shots/sidebar-<caso>-<tema>.png` los 46 casos, que
cubren las referencias `200`, `201`, `220`–`228`, `231`–`235`, `409`, `410`, `413`–`425`,
`433`–`444`, `446`–`448`, `601`, `604`–`606` y `612`.

## Worktrees

Sección "Worktrees" (`worktrees.rs`), entre Remotes y Tags. Si está desplegada se
guarda en `settings.sidebarSections.worktrees` (una clave que el store conserva
sin conocerla, así que `mergeon-store` no cambia).

- **Lista**: `AppModel::get_worktrees` (`git worktree list --porcelain -z`, con
  respaldo sin `-z` para git < 2.36) en segundo plano al abrir o cambiar de
  repositorio, con cada `MutationRecorded` del repositorio activo y con el botón
  de refresco; una lectura en vuelo como mucho (las peticiones de en medio se
  funden en una). Se lee aunque la sección esté plegada: es barata (solo mira
  `.git/worktrees`) y marca las ramas ocupadas.
- **Cambios**: con la sección desplegada, `AppModel::worktree_has_changes` por
  worktree (máx. 50), uno tras otro: `git status --porcelain -z` que se corta en
  el primer byte. Un punto ámbar (`worktree-dirty:<ruta>`) y el tooltip lo dicen.
- **Filas** (`VirtualRows`, `worktree:<ruta>`): el actual con el cuadrado
  naranja; carpeta, rama o "detached at <sha>" en mono; candado si está
  bloqueado; tachado y carpeta roja si su carpeta ya no existe (prunable).
  Doble clic o el botón `worktree-open:<ruta>` → "Open in new tab"
  (`AppModel::add_repository` con la ruta, como un submódulo).
- **Menú** (`Sidebar::worktree_menu`, `worktrees::worktree_menu`): Open in new tab,
  Copy path, Lock / Unlock, Remove worktree… (no en el principal ni en el actual;
  bloqueado hay que desbloquearlo antes) o Prune si su carpeta ya no está.
- **Cabecera**: `new-worktree` (+) abre «New Worktree»; `prune-worktrees` aparece
  si hay alguno podable.
- **«New Worktree»** (`dialogs::open_new_worktree_dialog`): "New branch" (nombre
  y "From", la rama actual por defecto) o "Existing branch"; carpeta sugerida
  `../<repo>-<rama>` que sigue a la rama hasta que se edita; "Open it in a new
  tab". Validación pura (`validate_new_worktree`, `branch_name_problem`): nombre
  válido, rama nueva que no exista, rama existente que exista y no esté en otro
  worktree, carpeta no vacía; que la carpeta no exista lo comprueba git.
- **Quitar**: `confirm_with_option` con la ruta, que la rama se queda y, si se
  sabe que tiene cambios, que se pierden; la casilla "Also discard its
  uncommitted changes (--force)" es la única vía a `--force`. Sin ella, git se
  niega y el aviso lo explica (`Code::WorktreeHasChanges`).
- **Ramas**: el menú de una rama local trae "Check out in new worktree..."
  (abre el diálogo en "Existing branch"). Una rama desprotegida en otro worktree
  lleva un ícono de carpeta (`branch-worktree:<rama>`, tooltip con la ruta); su
  "Checkout" pasa a "Open its worktree (<carpeta>)", y el doble clic pregunta
  "'x' is open in another worktree" ofreciendo abrir ese worktree en vez de
  dejar que git falle. Si aun así llega a git (desde otra vista), git-ops lo
  traduce a `Code::BranchInOtherWorktree` con el mismo texto.
- Mutaciones (cola, toasts y registro como las demás): `worktree-add`,
  `worktree-remove`, `worktree-lock`, `worktree-unlock`, `worktree-prune`
  (`mergeon_git_ops::*_worktree_operation`, `prune_worktrees_operation`).
- API: `worktrees()`, `worktree_has_changes(ruta)`, `branch_worktree(rama)`,
  `new_worktree_dialog()`, `worktree_menu(ruta, cx)`; órdenes
  `SidebarCommand::{NewWorktree, CheckoutInNewWorktree, OpenWorktree,
  RemoveWorktree, LockWorktree, UnlockWorktree, CopyWorktreePath, PruneWorktrees}`.
- Tests: `tests/worktrees.rs` (7, git real: listar principal / con rama /
  desanclado y bloqueado / prunable con estado y marcas, crear con rama nueva y
  abrir la pestaña, crear con rama existente desde su menú, quitar limpio, con
  cambios sin y con `--force`, abrir con doble clic, checkout de una rama
  ocupada, bloquear / desbloquear / podar) y unitarios del diálogo y del menú.

## Tests

- `src/logic.rs`, `src/menus.rs`, `src/dialogs/*`: lógica pura y política de los menús
  (20 tests).
- `tests/sidebar.rs`: el sidebar real en una ventana sin pantalla, con git real (52),
  incluido un repositorio de miles de refs (solo se construyen las filas a la
  vista; los datos no se recopian si no cambian).
- `tests/dialogs.rs`: `CheckoutBranchDialog` y «Create Branch» (7).
- `tests/pull_requests.rs`: la sección de PRs con GitHub guionizado (5): lista,
  clic que abre el navegador y plegado guardado; "Sign in…" sin cuenta; oculta
  fuera de GitHub; checkout de un PR de un fork (`pr-2`) desde el menú y de uno
  del propio repositorio (diálogo "Track Remote Branch").
  `Harness::mount_with(TestBackend, cx)` monta el sidebar sobre un backend ya
  preparado.

Las fixtures están en `tests/support/mod.rs` (`TestRepo`, `Harness`). Los elementos se
buscan por id de dominio: `branch:<nombre>`, `starred:<nombre>`, `star:<nombre>`,
`unstar:<nombre>`, `remote:<nombre>`, `remote-branch:<remoto>/<rama>`, `tag:<tag>`,
`checkout-tag:<tag>`, `delete-tag:<tag>`, `stash:<hash>`,
`stash-{branch,apply,pop,drop}:<hash>`, `submodule:<ruta>`,
`submodule-{update,open}:<ruta>`, `section-header-<sección>`, `add-remote`,
`create-tag`, `update-all-submodules`, `sidebar-show-repository`,
`sidebar-show-search`, `sidebar-refresh`, `sidebar-filter`, `commit-search-input`,
`commit-result:<hash>`, `detached-head`, `sidebar-resizer`, `commit-search-help`,
`commit-search-filters`, `commit-search-fields`, `search-field-{path,branch,after,before,code}`,
`tab-{any,7d,30d,custom}`, `search-code-regex`, `commit-search-chips`,
`search-chip:<path|branch|date|code>`, `search-chip-clear:<…>`, `commit-search-error`,
`commit-search-load-more`, `branch-comparison-browse`.

## Diferencias con el original

- **Repositorio de cada flujo.** `applyOperationResult` y `refreshSections` leían
  `activeRepo` en cada paso: cambiar de pestaña a mitad de una operación aplicaba el
  resultado al repositorio equivocado. Aquí cada flujo trabaja sobre el repositorio con
  el que empezó.
- **Doble clic.** El primer clic navega al tip de la rama y el segundo hace checkout
  (en el original el doble clic disparaba dos navegaciones y el checkout).
- **`mod+f`** además de mostrar la búsqueda enfoca su campo.
- **Diálogos que no se dejan cerrar.** `mergeon_ui::dialog::AppDialog` no puede vetar su
  cierre. Branch Actions con una acción en curso y «Remote» mientras añade el remoto se
  vuelven a mostrar si se descartan con Escape o la X (un parpadeo), que es lo más
  parecido al `if (isBusy) return` del original.
- **Arrastre de ramas.** La fila arrastrada se atenúa mientras dura y la de destino se
  marca con un borde discontinuo (el `outline` del original, 1px más adentro).
- **Filas virtuales.** Las secciones comparten un único scroll con la navegación,
  el filtro, las cabeceras y los textos vacíos, como en el original. Las filas de
  cada sección (destacadas, ramas, remotos y sus ramas, tags, stashes, submódulos)
  son tramos de alto fijo (`virtual_rows.rs`): ocupan su alto en el layout pero
  solo se construyen las que caen en la parte visible del scroll. Las ramas y
  remotos miden 26px y las filas con botones (tags, stashes, submódulos) 1.75rem;
  cada grupo de Remotes deja 0.075rem debajo. Los resultados de la búsqueda son
  una `list()` virtual con su `max-height` de 320px.
- **Datos copiados una vez.** El sidebar no clona el repositorio en cada frame: lo
  copia (con los nombres en minúsculas para el filtro, qué ramas están
  destacadas y las filas de Remotes) cuando el modelo notifica y sus datos
  cambiaron de verdad, y filtra las ramas solo si cambian los datos o el texto
  del filtro. `data_builds()` y `filter_builds()` (ocultos) lo cuentan para los
  tests.
- **Tooltips.** Son los de `mergeon_ui` (siguen al cursor). Tras un clic derecho las
  filas no muestran el suyo hasta que el puntero se mueve, para que no tape el menú.

### Diferencias visuales no cerradas

- **Fracciones de píxel.** GPUI redondea cada medida a píxeles enteros; Chromium
  acumula las fracciones (`0.075rem`, `0.2rem`...). Las filas quedan a ±1px de la
  referencia; `section::content` reparte el píxel sobrante para que no se acumule.
- **Corte de línea en `/`.** GPUI parte el texto también tras `/`: en la tarjeta Merge
  de Branch Actions, «Merge feature/search into main.» se corta en `feature` /
  `/search` en vez de dejar el nombre de la rama entero.
- **Cabecera del diálogo.** `AppDialog` reserva más margen a la derecha del título para
  la X, así que la descripción de Branch Actions corta la línea una palabra antes.
- **Flecha del título de Branch Actions.** `→` sale de la fuente de reserva y se ve más
  gruesa que en Chromium.
- **Íconos.** El trazo de GPUI es algo más grueso (ver `ui-components.md`): los
  chevrons de 12px de las cabeceras se ven más marcados.
- **Barra de scroll.** La de gpui-kit, superpuesta; el original usa la nativa.

## Pendientes para la cáscara

- Instalar el `UiHost` real (confirm, prompt y avisos): sin él los flujos que preguntan
  no hacen nada.
- Conectar `CommitSelectionRequested` y `clear_viewed_ref` con la vista de commits, y
  `OpenStashDiff` con el diálogo de stash.
- Mantener viva la entidad `Sidebar` mientras haya repositorio activo (guarda las
  peticiones de selección por repositorio).
- `CreateBranchDialog` está portado aquí porque lo abre el sidebar; el flujo «New
  Branch» de la barra (`AppEvent::ShowDialog(DialogRequest::CreateBranch)`) puede
  reutilizar `dialogs::open_create_branch_dialog`.

## Pendiente

El port está completo respecto de lo que hace `Sidebar.vue`; lo que sigue es lo que
queda sin cubrir o sin revisar.

- **Componentes.** `StashDiffDialog.vue` no se porta aquí (evento `OpenStashDiff`). No
  hay árbol de ramas por carpetas ni «fetch» por remoto: el original tampoco los tiene
  (la lista de ramas es plana y el menú de un remoto solo ofrece Edit URL, Rename y
  Remove).
- **Menús.** Portados los cuatro (rama local, rama remota, tag, remoto). «Interactively
  Rebase on…» queda siempre deshabilitada, como en el original.
- **Diálogos.** Portados los cinco. El aviso de squash de Branch Actions sale sin fondo
  y con el texto normal porque el original usa utilidades (`bg-warning-soft`,
  `text-warning-soft-foreground`) que su tema no define: se portó lo que se ve.
- **Tests sin cubrir.** Flujos escritos y sin test de ejecución: merge con «Push after
  merge» (incluido el prompt de `origin` cuando falta el remoto), `PushTag`,
  `DeleteRemoteTag`, «New Tag...» sobre una rama remota, `PullRemoteBranch`,
  `OpenPullRequest`, actualizar submódulos y abrir uno inicializado como pestaña, las
  flechas del divisor, y que Branch Actions / «Remote» reaparezcan si se descartan con
  una operación en curso. Los dos casos de `Sidebar.test.js` sobre entradas
  deshabilitadas del menú se portaron como tests de `menus.rs` (sobre la descripción
  del menú), no pulsando la entrada.
- **Capturas.** Las 92 (`sidebar-*-{dark,light}.png`) están generadas. Se compararon
  lado a lado con la referencia, en oscuro: sidebar completo, plegado, filtro, rama
  vista, hover de stash, búsqueda (vacía, resultados), los menús de rama local, rama
  actual y rama remota, checkout, Branch Actions (merge, confirmación), comparación,
  añadir remoto, detalles de tag, ↑↓, HEAD desanclado y submódulos; en claro: sidebar
  completo, track remote, squash, comparación y repositorio vacío. El resto (prompts y
  confirms, que pinta `mergeon_ui`; menús de tag y de remoto; hover de tag; tooltip;
  Create Branch; rebase/compare/drag-drop de Branch Actions; y las variantes claras de
  lo anterior) está capturado pero no revisado contra la referencia.
