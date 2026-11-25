# Componentes de `mergeon_ui`

Catálogo del sistema de diseño nativo: qué expone el crate `mergeon-ui`, cómo se
usa cada pieza y a qué componente Vue de `src/components/ui/` equivale. Las
convenciones generales del port están en [`ui-port.md`](ui-port.md).

Todo se importa desde la raíz del crate (`mergeon_ui::button`, `mergeon_ui::Lucide`…)
salvo los tres módulos con varias funciones: `mergeon_ui::dialog`,
`mergeon_ui::toast` y `mergeon_ui::menu`.

## Arranque

```rust
gpui_kit::application()
    .with_assets(mergeon_ui::Assets)          // íconos Lucide (y su versión de trazo 1.8)
    .run(|cx| {
        mergeon_ui::init(cx);                 // gpui-kit + fuentes + tema + capa de Mergeon
        mergeon_ui::theme::apply(ThemePreference::Dark, cx);
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Shell::new(window, cx)))
            .expect("no se pudo abrir la ventana");
    });
```

`mergeon_ui::init` registra en la raíz de cada ventana (`gpui_kit::base::Root`) una
capa propia que aporta:

- **el texto base de la app**: 14px con línea de 20px (`body { font-size: 0.875rem }`),
  heredado por las vistas, los diálogos, los menús y los toasts;
- **la pila de diálogos** (`mergeon_ui::dialog`) y **el anfitrión de toasts**
  (`mergeon_ui::toast`).

No hay nada que montar en las vistas: los helpers de diálogo y toast funcionan en
cualquier ventana abierta con `gpui_kit::open_window`.

## Medidas: rem, texto y radios

- El tema fija `font.size = 16`, que es el **rem de la ventana**. Las utilidades de
  GPUI miden lo mismo que las de Tailwind: `h_8()` = 32px, `gap_2()` = 8px,
  `text_sm()` = 14px, `text_xs()` = 12px.
- El texto sin estilo mide 13px con línea de 18 (Señal; lo pone la capa de la
  raíz). Los componentes de gpui-kit que fijan `text_base()` (p. ej. su `Button`
  mediano) salen a 16px: usar los de `mergeon_ui`.
- GPUI no cambia el alto de línea al cambiar el cuerpo. `MergeonStyled` trae las
  parejas de Tailwind: `.tw_text_xs()` (12/16), `.tw_text_sm()` (14/20),
  `.tw_text_base()` (16/24), `.tw_text_lg()` (18/28) y `.tw_font_medium()`.
- Radios de Señal (0–6px, `radius` del tema = 4): `.radius(Radius::Sm, cx)`, o
  `Radius::Md.px(cx)`.

| `Radius` | px | Dónde |
|---|---|---|
| `Xs`, `Sm` | 2 | pills de ref, badges, checkbox, resaltado por palabra |
| `Md`, `Lg` | 4 | botones, inputs, selects, segmentados |
| `Xl`, `Xxl` | 6 | menús, diálogos, toasts (el máximo) |

- Superficie flotante (`bg-raised`, borde base, `shadow-2`): `.floating_surface(cx)`.
  Relieve de botones (`shadow-1`): `.shadow_control()`. Sombras solo en lo que
  flota; los paneles se separan con filetes de 1px (`border`).
- Colores: `cx.palette().<token>`. Tokens de Señal (`bg_sunken`, `bg_base`,
  `bg_raised`, `bg_inset`, `bg_hover`, `bg_pressed`, `text_1..3`, `border_hair`,
  `border_base`, `border_control`, `border_ink`, `sel_bg`/`sel_fg`/`sel_fg_2`/
  `sel_inactive`/`sel_add`/`sel_del`, `diff_*`, `lanes[0..8]`) y los alias
  heredados de shadcn (`card` = `bg_base`, `background` = `bg_sunken`, `accent` =
  hover, `muted_foreground` = `text_3`...). El naranja señal es la familia
  `primary`: relleno (`primary`, siempre con `primary_foreground` = tinta encima,
  nunca blanco) y texto (`primary_text`). `mergeon_ui::theme::with_alpha(color,
  0.5)` equivale a `bg-color/50`.
- Selección de fila «en tinta»: fondo `sel_bg`, texto `sel_fg` (metadatos
  `sel_fg_2`, ±n `sel_add`/`sel_del`) y una marca de 2px en `primary` como hijo
  absoluto a la izquierda (no un borde, para no mover el contenido).
- Tipografía: Geist (UI) y Geist Mono (`cx.theme().mono_font_family`: hashes,
  ramas, cifras, código), embebidas en `assets/fonts` con su licencia OFL.

## Catálogo

| Vue (`src/components/ui/`) | `mergeon_ui` | Base |
|---|---|---|
| `button/Button.vue` | `button(id)` → `Button` | propio sobre `gpui_kit::base::Button` |
| `input/Input.vue` | `input(&state)` → `TextField` | `Input` de gpui-kit, superficie y foco propios |
| `textarea/Textarea.vue` | `textarea(&state)` → `TextField` | `Textarea` de gpui-kit |
| `checkbox/Checkbox.vue` | `checkbox(id)` → `Checkbox` | propio sobre `gpui_kit::base::Checkbox` |
| `radio-group/RadioGroup.vue` | `radio_group(id)` → `RadioGroup` | propio sobre `gpui_kit::base::Radio` |
| `select/Select.vue` | `select(id)` → `Select` | disparador propio + `PopupMenu` de gpui-kit |
| `tabs/Tabs.vue` | `tabs(id)` → `Tabs` | propio sobre `gpui_kit::base::{Tabs, Button}` |
| `badge/Badge.vue` + `lib/ui/statusBadge.js` | `badge(label)`, `status_badge(status)` → `Badge` | propio |
| `alert/Alert.vue` | `alert(variant)` → `Alert` | propio |
| `kbd/Kbd.vue` | `kbd(label)` → `Kbd` | propio |
| `spinner/Spinner.vue` | `spinner()` → `Spinner` | propio (SVG del original) |
| `empty-state/EmptyState.vue` | `empty_state(title)` → `EmptyState` | propio |
| `icon/AppIcon.vue` | `icon(Lucide::X)` → `AppIcon` | `Icon` de gpui-kit con trazo 1.8 |
| `context-menu/ContextMenu.vue` | `menu::MenuItem`, `MenuTriggerExt`, `menu::build` | `PopupMenu` / `ContextMenu` de gpui-kit |
| `dialog/AppDialog.vue` | `dialog::AppDialog` | propio sobre `gpui_kit::base::Dialog` |
| `dialogs/GlobalDialogs.vue` + `lib/ui/dialogs.js` | `dialog::confirm`, `confirm_with_option`, `prompt` | `AppDialog` |
| `toast/ToasterHost.vue` + `lib/ui/notify.js` | `toast::success/info/warning/error/show` | propio sobre `gpui_kit::base::ToastManager` |
| `tooltip/HoverCard.vue` | `hover_card(id)` → `HoverCard`, `tooltip(text)` | `HoverCard` / `Tooltip` de gpui-kit |

Los componentes con valor son **controlados**, como el `v-model` del original: se
les pasa el valor actual y avisan del nuevo con `on_change`; la vista lo guarda y
llama a `cx.notify()`. Solo los campos de texto necesitan estado propio
(`InputState` / `TextareaState`, que guarda la vista).

### Button

`variant`: `Default` (primario) · `Secondary` · `Outline` · `Ghost` · `Link` ·
`Destructive` · `DestructiveOutline`. `size`: `Xs` (h-6) · `Sm` (h-7) · `Default`
(h-8) · `Lg` (h-9). Sin etiqueta ni hijos el botón es cuadrado (`icon-xs` … `icon-lg`).

```rust
mergeon_ui::button("fetch")
    .outline()                       // o .variant(ButtonVariant::Outline)
    .sm()                            // o .size(ButtonSize::Sm)
    .icon(Lucide::Download)
    .label("Fetch")
    .on_click(cx.listener(|this, _, window, cx| this.fetch(window, cx)))

mergeon_ui::button("refresh").ghost().icon(Lucide::RefreshCw).tooltip("Refresh")
mergeon_ui::button("push").loading(self.pushing).label("Push")
mergeon_ui::button("commit").full_width(true).label("Commit to main")
mergeon_ui::button("delete").destructive().disabled(!self.can_delete).label("Delete")
```

Más: `.trailing_icon(..)`, `.selected(bool)`, `.child(..)` (contenido libre),
estilos de GPUI encima (`.w(..)`, `.ml_auto()`). `ButtonVariant::parse("danger-soft")`
y `ButtonSize::parse("md")` traducen los nombres que acepta `Button.vue`.

Equivalencias de Vue: `variant="primary"` → `Default`, `tertiary` → `Ghost`,
`danger` → `Destructive`, `danger-soft` → `DestructiveOutline`; `icon-only` es
automático; `:loading` → `.loading(true)`; `fullWidth` → `.full_width(true)`.

### Input y Textarea

```rust
// en `new`:
let name = cx.new(|cx| InputState::new(window, cx).placeholder("Branch name"));
let message = cx.new(|cx| TextareaState::new(window, cx).placeholder("Commit message"));

// en `render`:
mergeon_ui::input(&self.name)
mergeon_ui::input(&self.filter)
    .prefix(mergeon_ui::icon(Lucide::Search).tone(IconTone::Muted))
    .cleanable(true)
mergeon_ui::input(&self.token).mask_toggle()
mergeon_ui::textarea(&self.message).rows(6)       // 4 filas por defecto, mínimo 96px
mergeon_ui::input(&self.remote).disabled(true)    // también .readonly(true)

// leer / escribir / escuchar:
let value = self.name.read(cx).value();
self.name.update(cx, |state, cx| state.set_value("main", window, cx));
self._subscriptions.push(cx.subscribe_in(&self.name, window, |this, _, event: &InputEvent, _, cx| {
    if matches!(event, InputEvent::Change) { /* v-model */ }
}));
```

`InputState`, `TextareaState` e `InputEvent` se reexportan desde `mergeon_ui`. El
campo ocupa todo el ancho de su contenedor (como `fullWidth` en Vue); el ancho se
limita con estilos sobre el propio `TextField` (`.w(rems(20.))`).

### Checkbox

```rust
mergeon_ui::checkbox("fetch-all")
    .label("Fetch all remotes")
    .checked(self.fetch_all)
    .on_change(cx.listener(|this, checked: &bool, _, cx| {
        this.fetch_all = *checked;
        cx.notify();
    }))
```

`.disabled(bool)`; `.child(..)` para una etiqueta que no sea texto plano.

### RadioGroup

```rust
mergeon_ui::radio_group("merge-mode")
    .label("Integration mode")                     // aria-label
    .option(RadioOption::new("merge", "Merge").description("Create a merge commit"))
    .option(RadioOption::new("rebase", "Rebase"))
    .option(RadioOption::new("ff", "Fast-forward only").disabled(true))
    .selected(self.mode.clone())
    .on_change(cx.listener(|this, value: &SharedString, _, cx| {
        this.mode = value.clone();
        cx.notify();
    }))
```

`.options(["a", "b"])` acepta cadenas (valor = etiqueta). El grupo es una sola
parada de tabulación; dentro se navega con las flechas, que saltan las opciones
deshabilitadas.

### Select

```rust
mergeon_ui::select("remote")
    .items(["origin", "upstream"])                 // o SelectOption::new(value, label)
    .value(self.remote.clone())                    // sin coincidencia → placeholder
    .placeholder("Select…")
    .on_change(cx.listener(|this, value: &SharedString, _, cx| {
        this.remote = value.clone();
        cx.notify();
    }))
```

`.full_width(false)` para un select en línea (ancho con `.w(..)`); `.disabled(true)`.
La lista se abre con clic, Enter o Espacio y se recorre con las flechas.

### Tabs

```rust
mergeon_ui::tabs("commit-tabs")
    .item(TabItem::new("changes", "Changes"))
    .item(TabItem::new("tree", "Repo tree").icon(Lucide::FolderTree))
    .item(TabItem::new("stats", "Stats").disabled(true))
    .selected(self.tab.clone())
    .on_change(cx.listener(|this, key: &SharedString, _, cx| {
        this.tab = key.clone();
        cx.notify();
    }))
```

`.full_width(true)` reparte el ancho entre las pestañas; por defecto el control
mide lo que su contenido.

### Badge y estado de archivo

`variant`: `Default` · `Primary` · `Secondary` · `Outline` · `Info` · `Success` ·
`Warning` · `Destructive`. `size`: `Sm` · `Md` · `Lg`.

```rust
mergeon_ui::badge("3").variant(BadgeVariant::Info)
mergeon_ui::badge("Private").variant(BadgeVariant::Secondary).icon(Lucide::Lock)
mergeon_ui::badge("Active")                                   // default
mergeon_ui::badge("12").variant(BadgeVariant::Success).solid(true).size(BadgeSize::Sm)

// Estado de un archivo en cualquier lista (M, A, D, R, C, ?, ??, U):
mergeon_ui::status_badge(&file.status)
```

`status_badge` es la única receta con la que se pinta un estado: `M` ámbar, `A` y
sin seguir verdes con `+`, `D` rojo, `R`/`C` neutros, `U` rojo tenue con `!`.
`status_badge_style(status)` y `status_label(status)` devuelven la receta suelta.
Equivalencias de Vue: `variant="accent"` → `Primary`, `danger` → `Destructive`,
`shape="square"` → `.square()`.

### Alert

```rust
mergeon_ui::alert(AlertVariant::Warning)
    .title("Merge conflicts expected")
    .description("Integrating these branches will conflict in 2 file(s).")
    .action(mergeon_ui::button("resolve").outline().xs().label("Resolve"))
```

`variant`: `Default` · `Info` · `Success` · `Warning` · `Error` (`status="accent"`
→ `Info`, `danger` → `Error`). `.icon(..)` cambia el ícono, `.no_icon()` lo quita,
`.child(..)` añade contenido bajo el título.

### Kbd, Spinner, EmptyState, AppIcon

```rust
mergeon_ui::kbd("Ctrl+K")

mergeon_ui::spinner()                                         // 16px, color del texto
mergeon_ui::spinner().size(SpinnerSize::Lg).color(SpinnerColor::Primary)

mergeon_ui::empty_state("No repository selected")
    .description("Choose an open or recent repository from the sidebar to see its summary.")
    .icon(Lucide::FolderGit2)                                 // .spin_icon(true) para cargas
    .child(mergeon_ui::button("open").label("Open repository"))
// .rich_description(elemento) para una descripción con formato (un hash en mono).
// El ícono va en un marco de 44px con marcas de esquina naranjas.

mergeon_ui::icon(Lucide::GitBranch)                           // 16px, text-foreground
mergeon_ui::icon(Lucide::GitBranch).size(px(18.)).tone(IconTone::Muted)
mergeon_ui::icon(Lucide::LoaderCircle).spin(true)
```

Tonos de `AppIcon`: `Default` · `Muted` · `Active` (primario) · `Danger` ·
`Success` · `Current` (hereda el color del texto). `mergeon_ui::icon` dibuja con
el trazo de 1.8 del original; los íconos que van dentro de `Button`, `Badge`,
`Alert` y `Tabs` usan el trazo normal de Lucide (2), igual que en Vue.
`AppIcon::from_icon(Icon::default().data(SVG))` envuelve un SVG propio.

### Menús

```rust
use mergeon_ui::menu::{MenuItem, MenuTriggerExt as _};

// Menú contextual (clic derecho) sobre cualquier elemento con id:
div()
    .id(("branch", branch.name.clone()))
    .child(branch.name.clone())
    .context_menu_items(move |_, _| vec![
        MenuItem::new("Checkout").icon(Lucide::GitBranch).on_click(|window, cx| { /* … */ }),
        MenuItem::new("Copy branch name").icon(Lucide::Copy).shortcut("Ctrl+C"),
        MenuItem::new("Rename…").icon(Lucide::Pencil).disabled(true),
        MenuItem::separator(),
        MenuItem::new("Delete branch").icon(Lucide::Trash).danger(),
    ])

// Menú desplegable de un botón:
mergeon_ui::button("more")
    .outline()
    .label("Branch actions")
    .trailing_icon(Lucide::ChevronDown)
    .dropdown_menu_items(|_, _| vec![/* … */])
```

Las entradas se construyen cada vez que el menú se abre. Otras piezas:
`MenuItem::label("Group")` (rótulo), `MenuItem::header("c4e1a07 · fix(cart)")`
(cabecera `.mlab` de Señal: mono de 10px en mayúsculas y apagado),
`MenuItem::submenu("More", items)`, `.checked(true)` (check a la derecha).
Un atajo de una sola letra o cifra (`.shortcut("B")`) además ejecuta la
entrada si se pulsa con el menú abierto (cierra el menú y corre la acción;
deshabilitada no hace nada); los atajos con modificadores solo se muestran. Para un constructor de menú de gpui-kit ya
existente: `menu::build(popup_menu, items, window, cx)`.

Equivalencia con Vue: `{ label, icon, shortcut, danger, disabled, action }` →
`MenuItem::new(label).icon(..).shortcut(..).danger().disabled(..).on_click(..)`;
`{ type: 'separator' }` → `MenuItem::separator()`. La posición (`x`, `y`) y el
`visible` del original los maneja gpui-kit.

### AppDialog

```rust
use mergeon_ui::dialog::{self, AppDialog, DialogWidth};

AppDialog::new()
    .title("Fetch")
    .description("Fetch latest changes from remote repository")
    .width(DialogWidth::Lg)                        // max-w-lg; por defecto Xl
    .body(move |_, _| form.clone())                // una entidad o elementos
    .footer(|_, _| {
        div().flex().gap_2()
            .child(mergeon_ui::button("cancel").ghost().sm().label("Cancel")
                .on_click(|_, window, cx| { dialog::close(window, cx); }))
            .child(mergeon_ui::button("ok").sm().label("Fetch"))
    })
    .on_close(|window, cx| { /* X, Escape o clic fuera */ })
    .open(window, cx);
```

`body`, `footer` y `header` se reconstruyen en cada frame: capturar entidades o
estado, no elementos ya armados. El cuerpo hace scroll solo cuando el diálogo no
cabe en la ventana. `.top_anchored()` lo pone arriba (a 1/8 del alto) en vez
de centrado y `.flush_body()` quita el relleno y el scroll del cuerpo (la paleta
de comandos, con su lista virtual). `.close_on_backdrop(false)`, `.close_button(false)`,
`.on_confirm(|window, cx| -> bool)` (Enter; `true` cierra). Cerrar:
`dialog::close(window, cx)` (el de arriba), `dialog::close_id(window, cx, id)`,
`dialog::close_all(window, cx)`. `AppDialog::into_embedded(window, cx)` es el modo
`embedded` del original.

Anchos (`width="max-w-…"`): `Sm` 24rem · `Md` 28 · `Lg` 32 · `Xl` 36 · `Xl2` 42 ·
`Xl3` 48 · `Xl4` 56 · `Xl5` 64 · `Xl6` 72 · `Rems(n)`.

Un diálogo abierto después queda siempre delante (lo que en Vue resolvía
`stackLevel`), y los toasts quedan por encima de todos.

### confirm y prompt

```rust
use mergeon_ui::dialog::{self, ConfirmOption, ConfirmOptions, ConfirmVariant, PromptOptions};

dialog::confirm(
    window,
    cx,
    ConfirmOptions {
        title: "Delete tag".into(),
        description: "This cannot be undone.".into(),
        confirm_text: "Delete".into(),
        variant: ConfirmVariant::Destructive,
        ..Default::default()                        // title "Are you sure?", "Confirm", "Cancel"
    },
    move |ok, window, cx| if ok { /* … */ },
);

// Confirmar y, de paso, decidir algo secundario (empieza apagado):
dialog::confirm_with_option(
    window,
    cx,
    ConfirmOptions { title: "Delete branch".into(), ..Default::default() },
    ConfirmOption { label: "Also on origin".into(), checked: false },
    move |outcome, window, cx| if outcome.confirmed { /* outcome.checked */ },
);

dialog::prompt(
    window,
    cx,
    PromptOptions {
        title: "Rename branch".into(),
        default_value: branch.name.clone().into(),
        validate: Some(Rc::new(|name: &str| !name.contains(' '))),
        input_error_message: "Branch names cannot contain spaces".into(),
        ..Default::default()                        // textarea: true, rows: 4 para varias líneas
    },
    move |name, window, cx| if let Some(name) = name { /* … */ },
);
```

La respuesta llega una sola vez. Escape y la X responden `dismiss_value` en
`confirm` (por defecto `false`), `confirmed: false` en `confirm_with_option` y
`None` en `prompt`. Enter confirma. El `pattern` (regex) del original es
`validate`, una función.

### Toasts

```rust
use mergeon_ui::toast::{self, ToastKind, ToastOptions};

toast::success(window, cx, "Pushed.");
toast::info(window, cx, "Checking for updates…");
toast::warning(window, cx, "Could not save the sidebar layout.");
toast::error(window, cx, "Could not push to origin");

toast::show(
    window,
    cx,
    ToastKind::Error,
    "Could not push to origin",
    ToastOptions::new()
        .error(&error)                              // texto completo en "Details" + identidad del fallo
        .description("Check your credentials.")
        .action("Retry", |window, cx| { /* … */ })
        .id("push")                                 // reescribe su propio toast
        .duration(Duration::from_secs(6)),
);
```

Variantes de Señal:

```rust
// Operación larga: sin temporizador, sin X ni "Details", barra de 2px.
toast::show(window, cx, ToastKind::Info, "Fetch origin…",
    ToastOptions::new().id("op:fetch").icon(Lucide::ArrowDownToLine)
        .progress(ToastProgress::Indeterminate));   // o Fraction(0.64): "64 %"
// …y el resultado lo reemplaza con el mismo id, o toast::dismiss(window, cx, &"op:fetch".into()).

// Con botones: el primario naranja y el secundario.
toast::show(window, cx, ToastKind::Error, "Pull stopped: 2 conflicts",
    ToastOptions::new().description("src/cart/totals.ts, CartSummary.tsx")
        .action("Resolve", |_, _| {}).cancel("Abort merge", |_, _| {}));
```

Reglas heredadas de `notify.js`:

- el título se recorta a 160 caracteres; el texto completo se abre con "Details",
  que aparece si hay más texto del que cabe, si el resumen pasa de 90 caracteres
  o si es un error. Con una acción propia, "Details" pasa a su lado;
- duración: éxito 4s, info 5s, aviso 7s, error 8s; `.duration(..)` nunca pasa de
  8s. El cursor encima de la lista pausa los temporizadores;
- un aviso idéntico (o del mismo `.error(..)`) no se repite mientras el anterior
  siga en pantalla o dentro de los 2 segundos siguientes; con `.id(..)` no se
  deduplica, se reemplaza.

`toast::dismiss(window, cx, &id)`, `toast::visible_titles(window, cx)`,
`toast::report_persistence_error(window, cx, "Could not save settings", &error)`,
`toast::truncate_notification(text)`.

### HoverCard y tooltip

```rust
mergeon_ui::hover_card("author")
    .align(HoverCardAlign::Start)                  // Start · Center (defecto) · End
    .trigger(div().child("Ada Lovelace"))
    .content(|_, _| div().p_2().child("ada@example.com"))

div().id("refresh").tooltip(mergeon_ui::tooltip("Refresh"))
div().id("palette").tooltip(mergeon_ui::tooltip_with_shortcut("Command palette", "Ctrl+K"))
mergeon_ui::button("refresh").ghost().icon(Lucide::RefreshCw).tooltip("Refresh")
```

`.open_delay(..)` / `.close_delay(..)` (120ms / 150ms como en el original),
`.disabled(true)`. El tooltip de texto sustituye al `title` nativo del original.

## Galería y capturas

```bash
cd native
cargo run -p mergeon-ui --example gallery
MERGEON_THEME=light MERGEON_GALLERY_PAGE=forms cargo run -p mergeon-ui --example gallery
```

| Variable | Valores |
|---|---|
| `MERGEON_THEME` | `dark` (defecto) · `light` · `system` |
| `MERGEON_GALLERY_PAGE` | `controls` (defecto) · `forms` · `feedback` · `overlays` |
| `MERGEON_GALLERY_OPEN` | lista separada por comas: `dialog`, `dialog-long`, `stacked`, `confirm`, `confirm-destructive`, `confirm-option`, `prompt`, `prompt-error`, `prompt-textarea`, `toasts`, `toast-success`, `toast-warning`, `toast-error`, `toast-action`, `details` |
| `MERGEON_GALLERY_INPUT` | pasos separados por `;`, uno cada 300ms: `key:enter`, `focus:8`, `move:x,y`, `click:x,y`, `rclick:x,y` (coordenadas de la ventana) |

- `native/tools/ui-gallery-shots.sh [dark|light|all] [pages|overlays|states|all]`
  captura la galería nativa en `native/target-shots/ui-gallery-<tema>-*.png`
  (páginas, diálogos y toasts abiertos, y estados con interacción: menú
  contextual, desplegable, select abierto, hover card, tooltip, foco).
- (Hasta el 2026-11-12, `native/tools/reference-ui-shots.mjs` generaba las mismas
  páginas con el CSS real de la app Electron; se eliminó junto con ella.)

## Diferencias conocidas con el original

- **Sin desenfoque de fondo.** GPUI no tiene `backdrop-filter`: el backdrop de los
  diálogos vela el contenido con el color del original pero no lo desenfoca, y la
  superficie del diálogo es `--background` opaco en vez del 80 % translúcido.
- **Sombra de menús y listas de select.** Usan la superficie de popover de gpui-kit
  (anillo de 1px + sombra corta) en lugar de `--shadow-floating`. Diálogos, toasts
  y hover cards sí llevan las sombras del original.
- **Menús.** El separador mide 2px y va de borde a borde (1px con margen en el
  original), el hover de una fila tiene radio 8 en vez de 6 y la fila destructiva
  usa el mismo fondo de hover que las demás (`destructive/8` en el original).
- **Campos de texto.** El placeholder usa `--muted-foreground` pleno (72 % en el
  original), el texto del área lleva 10px de margen lateral (11) y el campo
  deshabilitado se atenúa con la receta de gpui-kit, no con `opacity-64`.
- **Tooltip.** Sigue al cursor (el `title` nativo del original lo hacía el sistema).
- **Texto e íconos.** El rasterizado de GPUI deja los trazos algo más gruesos y el
  texto una fracción de píxel más ancho que Chromium.
- **Toasts.** Entran con un deslizamiento corto y fundido; sonner los desliza
  desde fuera de la pantalla.
