# `mergeon-view-home`: inicio, lanzador, GitHub y Settings

Port de `dashboard/Dashboard.vue`, `layout/RepositoryLauncher.vue`,
`github/{GitHubRepositoryPicker,GitHubAccountDialog,GitHubAccountsManager}.vue`,
`layout/{SettingsView,UpdateSettings}.vue` y
`repository/{PreferencesDialog,ShortcutsDialog,AboutDialog}.vue`.

Convenciones del port en [`ui-port.md`](ui-port.md); la capa de datos en
[`app-api.md`](app-api.md).

## Uso desde la cáscara

```rust
mergeon_view_home::init(cx);                                   // caché de avatares
let home = cx.new(|cx| HomeView::new(window, cx));             // sin repositorio activo
let settings = cx.new(|cx| SettingsView::new(window, cx));     // pestaña Settings

// Diálogos que piden los flujos de AppModel:
cx.subscribe_in(&model, window, |_, _, event: &AppEvent, window, cx| {
    if let AppEvent::ShowDialog(request) = event {
        mergeon_view_home::handle_dialog_request(request, window, cx);
    }
});

// "Back" de Settings:
cx.subscribe(&settings, |this, _, event: &SettingsEvent, cx| match event {
    SettingsEvent::Close => { /* `showPreferences = false` */ }
});
```

Todas las vistas leen `AppModel::global(cx)`; no reciben datos por parámetro.

## API pública

| Elemento | Qué es |
|---|---|
| `init(cx)` | Crea la caché de avatares. No registra acciones ni atajos (los globales son de la cáscara, vía `AppModel::handle_shortcut`). |
| `handle_dialog_request(&DialogRequest, window, cx) -> bool` | Abre `Launcher`, `GitHubAccount`, `GitHubRepositoryPicker` o `CloneFromUrl`; `false` si la petición es de otro crate. |
| `HomeView::new(window, cx)` | El inicio. Lectores: `selected_repo_path()`, `active_summary_tab()`, `is_group_expanded(name)`, `is_refreshing_github_account()`. Acciones: `select_repository(path, cx)`, `set_summary_tab(tab, cx)`, `toggle_repository_group(name, cx)`. |
| `recent_entries(&AppModel)`, `repository_groups(&[RepositoryEntry])`, `stat_rows(&RepositorySummary)` | Los `computed` del inicio, como funciones puras. |
| `SettingsView::new(window, cx)` | Settings completo. `section()`, `set_section(section, window, cx)`. Emite `SettingsEvent`. |
| `SettingsSection` | `Appearance`, `Git`, `Accounts`, `Help`; `key()`, `parse(key)`, `label()`. |
| `PreferencesPanel::new(section, window, cx)` | Formulario de identidad, auto-fetch y tema (`PreferencesSection::{All, Appearance, Git}`). |
| `GitHubAccountsManager::new(embedded, cx)` | Lista de cuentas. Emite `GitHubAccountsEvent`. |
| `about_panel(cx)`, `update_settings(cx)`, `shortcuts_panel(cx)` | Las tres piezas de Help / About, sin estado. |
| `update_status_text`, `update_action_label`, `update_action_disabled` | Los textos de `UpdateSettings.vue` sobre `UpdaterState`. |
| `open_repository_launcher(window, cx) -> DialogId` | "Open repository": "Open Local" (selector del sistema), "Clone from GitHub", "Clone from URL" y un campo para escribir o pegar la ruta (`RepositoryPathField`: valida con `AppModel::check_typed_repository_path`, Enter o "Open" abre). |
| `open_clone_from_url_dialog(window, cx) -> Entity<CloneFromUrlDialog>` | "Clone from URL": URL (HTTPS, SSH, `git@host:org/repo.git`, `file://` o ruta), carpeta donde clonar (se escribe o "Browse…") y nombre de la carpeta nueva (sale de la URL hasta que se edita; vaciarlo vuelve a seguirla). "Clone" valida (`mergeon_app::clone_url`), comprueba el destino en disco y llama a `AppModel::clone_repository` (mismo progreso, cola, credenciales HTTPS y avisos que el de GitHub); al terminar lo abre con `handle_repository_cloned_from` y se cierra (aunque el diálogo ya se haya cerrado, el clon se abre). Errores bajo cada campo (`clone-url-error`, `clone-url-folder-error`, `clone-url-name-error`) y el de git (`clone-url-clone-error`), sin las credenciales de la URL. Lectores: `is_cloning()`, `errors()`, `clone_path(cx)`. |
| `RepositoryPathField::new(on_open, window, cx)` | El campo de ruta con su error bajo él (`repository-path-error`). |
| `open_repository_path_dialog(description, window, cx, on_open, on_cancel) -> DialogId` | "Open a local repository": el campo de ruta como diálogo. La cáscara lo ofrece si el selector de carpetas falla o, en Linux, si a los 2 s no apareció (sin portal GPUI espera para siempre); el selector no se cancela y, si al final elige, cierra este diálogo. |
| `open_credentials_dialog(request, window, cx, on_answer) -> Entity<CredentialsDialog>` | "Sign in to <host>": usuario, contraseña/token enmascarado, "Remember with Git's credential helper" (solo con helper configurado) y aviso si las anteriores se rechazaron. Lo abre `ShellUiHost::ask_credentials`. |
| `open_github_account_dialog(window, cx) -> Entity<GitHubAccountDialog>` | "Connect GitHub Account". |
| `open_github_repository_picker(window, cx) -> Entity<GitHubRepositoryPicker>` | "Clone From GitHub". |
| `open_github_accounts_dialog`, `open_preferences_dialog`, `open_shortcuts_dialog`, `open_about_dialog` | Los mismos paneles como diálogo (la app original solo los monta incrustados). |
| `set_avatar_fetcher(fetcher, cx)`, `AvatarCache` | Sustituye la descarga de avatares (tests, galería). |
| `markdown::{render_readme, sanitize_markdown_url}` y sus tipos (`Block`, `Inline`, `Run`, `Marks`, `Image`, `ListItem`, `Align`) | El README del inicio ya interpretado (ver «README» abajo). |
| `util::{parent_folder_name, format_repository_size, join_local_path, is_permanent_auth_error, is_transient_auth_error, locale_compare}` | Funciones puras de los componentes. |

Un `open_*` llamado con su diálogo ya abierto devuelve el que hay.

## Eventos

| Evento | Quién lo emite | Qué debe hacer quien escucha |
|---|---|---|
| `SettingsEvent::Close` | "Back" de `SettingsView` | Ocultar Settings sin cerrar la pestaña (`showPreferences = false`). `AppModel` no tiene método para eso: ver Pendiente. |
| `GitHubAccountsEvent::AddAccount` | `GitHubAccountsManager` | `SettingsView` y `open_github_accounts_dialog` ya lo traducen a `AppModel::add_account_from_settings`. |
| `GitHubAccountsEvent::Close` | "Close" del gestor como diálogo | Nada: el diálogo ya se cerró. |

`HomeView` no emite eventos: todo lo que el `Dashboard` emitía tiene método
en `AppModel`.

## Qué llama cada vista

| Acción | `AppModel` |
|---|---|
| Fila del inicio (clic / doble clic), "Open Repository" | `load_repository_summary`, `handle_open_repository` |
| "Remove from Recent", "Clear recent" | `remove_recent_repository` + `clear_repository_summary`; `confirm` + `clear_recent_repositories` + `retain_repository_summaries` |
| GitHub del inicio: "Refresh", "Install App", "Manage" / "Reconnect" | `refresh_account`, `open_external`, `open_settings("accounts")` |
| Lanzador | `open_local_repository`, `open_github_picker`, `show_clone_from_url`, `check_typed_repository_path` + `open_repository_path` |
| Clone from URL | `default_clone_folder`, `pick_directory`, `check_clone_destination`, `git_auth_token`, `clone_repository`, `handle_repository_cloned_from` |
| Alta de cuenta | `reset_login`, `start_login`, `login_with_token`, `copy_user_code`, `open_verification_page`, `open_install_page`, `cancel_login`, `handle_account_added`, `close_account_dialog` |
| Selector de repositorios | `list_all_repositories`, `cancel_repository_requests`, `refresh_account`, `pick_directory`, `clone_repository`, `handle_repository_cloned`, `open_external` |
| Cuentas | `set_active_account`, `confirm` + `remove_account`, `add_account_from_settings` |
| Preferencias | `get_identity`, `set_identity`, `save_auto_fetch_settings`, `set_desktop_notifications_enabled` (Notifications), `set_dark`; editor externo (Settings → Git): `detect_editors`, `save_external_editor` |
| Help / About | `open_external`, `activate_update`, `request_minimize`, `request_quit` |

## Galería y capturas

```bash
cd native
cargo build -p mergeon-view-home --example home_gallery
crates/mergeon-view-home/shots.sh [dark|light|all] [filtro]   # p. ej. `all '^02'`
```

`shots.sh` deja `target-shots/home-<NNN>-<caso>-<tema>.png`, ya recortadas a la
ventana (1440×900) y con el número de la pantalla de `target-shots/reference/`
a la que equivalen. La galería deja el hueco del rail y de la barra de
pestañas para que la vista caiga donde la pone la cáscara. Variables de la
galería (`MERGEON_HOME_STATE`, `MERGEON_HOME_OPEN`, `MERGEON_GALLERY_INPUT`
con pasos `id:<id>`…): cabecera de `examples/home_gallery.rs`.

Casos capturados, en claro y oscuro: `001`–`008`, `010`–`015`, `020`–`026`,
`101`–`115`, más `016` (README con bloque de código, enlace y negrita) y `017`
(repositorios abiertos), que no tienen referencia.

## Tests

`cargo test -p mergeon-view-home`: 86 tests (`src/tests/` y los unitarios de
cada módulo), con el modelo real, repositorios git temporales, GitHub
guionizado y ventanas sin pantalla.

| Original | Aquí |
|---|---|
| `Dashboard.test.js` (8) | `tests/home.rs`: 7 portados y 8 propios. |
| `GitHubRepositoryPicker.test.js` (9) | `tests/github_picker.rs`: los 9 en 8 tests (el de «clona a través de `useGit`» queda dentro de los de clonado) y 5 propios. |
| `GitHubAccountDialog.test.js` (2) | `tests/github_account.rs`: los 2 y 5 propios. |
| `GitHubAccountsManager.test.js` (3) | `tests/github_accounts.rs`: los 3 y 2 propios. |
| `PreferencesDialog.test.js` (4) | `tests/preferences.rs`: los 4 y 3 propios. |
| `UpdateSettings.test.js` (3) | `tests/settings.rs` (interacción) y `help.rs` (textos). |
| sin test original | `tests/launcher.rs`, `tests/settings.rs`, `tests/readme.rs`, `markdown.rs`, `readme.rs`, `util.rs`, `avatar.rs`. |

Cómo se portó lo que no tiene equivalente directo:

- **"Deshabilitado mientras está en vuelo".** Los tests originales dejaban una
  promesa colgada y miraban `disabled`. Aquí el botón base no publica su estado
  deshabilitado y un transporte que bloquea colgaría el planificador de tests,
  así que se comprueba dentro del mismo frame: tras el clic la vista dice que
  está ocupada y un segundo clic no lanza otra petición.
- **Clonado.** No se comprueba el `authContext` (`clone_repository` solo
  recibe el token): el repositorio falso se clona de verdad desde un remoto
  bare local y se comprueba que queda abierto.
- **`win32`.** La ruta con `\` se prueba en `util::join_local_path`; la vista
  usa el separador de la plataforma en la que corre.

Omitido: `Dashboard › reuses the same repository summary request…` (la
deduplicación es de `AppModel::load_repository_summary`, no de la vista).

## Diferencias con el original

- **Estrella de los repositorios.** En esta máquina sale el glifo de texto y
  no el emoji en color, aunque se pide la fuente de emojis de la plataforma.
- **Barra de progreso de la descarga.** Usa `--primary` sobre `--muted`, como
  declara el CSS (`accent-color`). La captura de referencia muestra la barra
  sin tema de Chromium (verde sobre gris).
- **Error del token.** El recuadro dice `Failed to authenticate with token:
  Bad credentials`, sin el prefijo `Error invoking remote method…` del IPC de
  Electron; ocupa una línea menos.
- **Selector de repositorios.** La lista usa la barra de scroll superpuesta
  de gpui-kit (el original reserva 6px). Si el diálogo no cabe en la ventana,
  hace scroll el cuerpo entero en vez de encoger solo la lista
  (`AppDialog` no tiene `bodyClass`).
- **README.** Ya no es el renderizador mínimo del original (ver «README»). La cursiva no se ve:
  Geist se embebe sin itálica (solo Geist Mono la trae). El código en línea va en Geist Mono al
  tamaño del texto (un texto con estilos no cambia de tamaño por tramos) y sin el relleno de la
  pastilla. Las imágenes no se descargan (la app no instala cliente HTTP en GPUI): un logo se ve
  como `[alt]` atenuado y un badge como una pastilla con su texto alternativo.
- **Sin `letter-spacing`.** GPUI no lo tiene: el nombre del repositorio va sin
  el `-0.01em`; el código del device flow lo imita separando los caracteres.
- **Tras un clic no se mueve el foco.** Un campo sigue enfocado al pulsar un
  botón (los botones de `mergeon_ui` no toman el foco con el ratón).
- **Orden alfabético.** `localeCompare` se aproxima sin ICU.
- **Paneles de Settings.** El tema y el auto-fetch se leen del modelo en cada
  frame en vez de copiarse a un borrador al montar.
- Desajustes de 1px por redondeo de alturas de línea.

Corregido respecto del original: la URL de un enlace del README ya no se
escapa dos veces (un `&` llega como `&`).

## README

`markdown::render_readme` interpreta el README con `pulldown-cmark`
(CommonMark + tablas, tachado, listas de tareas y avisos `> [!NOTE]` de GFM)
y traduce el HTML incrustado, en bloques y dentro de un párrafo: `<p>`/`<div>`
(`align="center"`/`right`, `<center>`), `<h1>`–`<h6>`, `<a href>`, `<img>`,
`<br>`, `<b>`/`<strong>`, `<i>`/`<em>`, `<code>`/`<kbd>`, `<s>`/`<del>`,
`<details>`/`<summary>` (siempre desplegado), `<ul>`/`<ol>`/`<li>`,
`<blockquote>`, `<hr>`; el resto de etiquetas se descarta dejando su texto, y
los comentarios y `<script>`/`<style>` desaparecen. Las URL sueltas pasan a ser
enlaces. Los destinos pasan por `sanitize_markdown_url` (http/https/mailto y
rutas relativas; lo demás, `#`) y solo se abren fuera los http/https/mailto,
como antes.

Imágenes: un párrafo hecho solo de imágenes es `Block::Images` (una fila con
`flex-wrap`); cada imagen es un badge (URL de shields.io, `badge`, CI…, o
imagen pequeña dentro de un enlace) que se pinta como pastilla `Outline` con
su `alt`, o una imagen grande que se pinta `[alt]` atenuado. Dentro de una
frase, lo mismo como tramos de texto.

Rendimiento: `HomeView` guarda un `readme::ReadmeDocument` por hash del texto
(se interpreta una vez) y lo pinta con `list()` de GPUI (alturas variables):
cada frame construye solo los bloques visibles; una lista de primer nivel va
en una fila por elemento. Cada párrafo es un único `StyledText` con estilos
por tramo (antes, un `div` por palabra). Con un README de 3.000 líneas, en la
ventana de test (sin GPU ni texto real): 406,6 ms por frame antes y 1,4–1,5 ms
después; la interpretación cuesta 5 ms y solo se hace una vez. El resaltado de
los bloques de código es el del diff (`mergeon_view_diff::highlight`), se
calcula la primera vez que se pinta cada bloque y se guarda en el documento.

## Pendiente

**Para la cáscara**

- Instalar el `UiHost` real (avisos, `confirm`, `prompt`, selector de
  carpetas). Sus diálogos deben abrirse con `cx.defer`: las peticiones llegan
  con la ventana tomada. `examples/home_gallery.rs` tiene uno mínimo.
- Repartir `AppEvent::ShowDialog` con `handle_dialog_request`.
- Atender `SettingsEvent::Close`. Falta en `mergeon-app` un método para
  `showPreferences = false` (hoy solo lo hacen `show_home_tab`,
  `select_repository_tab` y `close_settings_tab`, con otros efectos).
- Aplicar `AppEvent::ThemeChanged` (`mergeon_ui::theme::apply`).
- Los avatares se descargan con `mergeon_github::UreqTransport` en un hilo de
  fondo, no con el cliente HTTP de GPUI: no hay nada que configurar.

**De otros crates** (resuelto aquí por dentro)

- `mergeon_ui::input` no acepta `id`: los tests escriben en los campos por su
  estado y la galería hace clic por coordenadas.
- El `RadioGroup` de `mergeon_ui` deja el anillo de foco tras un clic de ratón.
- `Lucide::Github` del catálogo de gpui-kit es la marca rellena; este crate
  embebe el SVG de Lucide (`parts::github_icon`).
- `mergeon_model::collate::locale_compare` es privado: `util::locale_compare`
  es una aproximación propia.

**Sin cerrar**

- Capturas revisadas a ojo una por una: buena parte de las oscuras del inicio,
  de los diálogos y de Settings, y una sola clara (`021`). El resto solo pasó
  la comparación automática contra la referencia (entre 0,45 % y 5,9 % de
  píxeles distintos en la zona de la vista). Faltan por mirar `004`, `011`,
  `014`, `015`, `017`, `020`, `022`, `023`, `025`, `101`–`105`, `107`, `111`,
  `113`–`115` y casi todas las claras.
- El emoji de la estrella (ver Diferencias).
- El alta por device flow y la descarga de avatares solo se han ejercitado
  contra el GitHub guionizado, nunca contra github.com.
- Los diálogos `open_preferences_dialog`, `open_shortcuts_dialog`,
  `open_about_dialog` y `open_github_accounts_dialog` compilan y el de cuentas
  tiene test, pero no hay captura de ninguno.
