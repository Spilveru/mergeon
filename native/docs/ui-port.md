# Port de la interfaz de Mergeon a GPUI

La interfaz Vue (`src/`) es la **especificación**: mismas pantallas, mismos
textos, mismas acciones, misma estética. No se modifica; se porta.

Stack: [GPUI Kit](https://gpui-kit.com) 0.7 (`gpui-kit`, que reexporta GPUI y
`gpui_kit::component`). Referencias locales, **leerlas antes de escribir UI**:

- `native/target-ref/gpui-kit/skills/gpui-kit/SKILL.md` y sus `references/`
  (uso, convenciones, guías de código, GPUI: entidades, async, foco, tests).
- `native/target-ref/gpui-kit/crates/component/src/` (fuente de cada componente)
  y `native/target-ref/gpui-kit/crates/story/src/` (un ejemplo por componente).
- Nunca inventar una API: buscar la firma real en esas fuentes.

## Crates

| Crate | Contenido |
|---|---|
| `mergeon-ui` | Sistema de diseño: tema, paleta, íconos, componentes compartidos. Sin lógica de app. |
| `mergeon-app` | Capa de aplicación: `AppModel` (estado global), servicios de backend, acciones. Sin render. |
| `mergeon-view-*` | Una familia de pantallas por crate. Dependen de `mergeon-ui`, `mergeon-app`, `mergeon-model`. |
| `mergeon` | Binario: ventana, cáscara (rail, pestañas, toolbar) y composición de las vistas. |

Cada crate de vistas lo escribe un agente distinto a la vez: **tocar solo el
crate propio**. Lo que falte en `mergeon-ui` / `mergeon-app` se resuelve dentro
del crate propio y se anota en el informe final.

## Compilar y ver

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cd native
cargo check -p <crate>                 # usa native/target, compartido (GPUI ya está compilado ahí)
cargo clippy -p <crate> --all-targets -- -D warnings
cargo fmt -p <crate>
cargo test -p <crate>
```

- **No** usar otro `CARGO_TARGET_DIR` para crates de UI: recompilar GPUI tarda
  15 minutos y varios GB. Si `cargo` dice "Blocking waiting for file lock", es
  otro agente compilando: esperar.
- No correr `cargo build`/`check` sin `-p` (compilaría crates ajenos a medio escribir).
- La máquina no tiene GPU ni pantalla. Para ver una ventana:
  `native/tools/shot.sh <salida.png> [segundos] [-- args]` lanza
  `native/target/debug/mergeon` (o el binario de `MERGEON_BIN`) en un display
  virtual y guarda una captura; `native/tools/in-x.sh <comando>` corre cualquier
  comando en ese display. Las capturas van en `native/target-shots/`.
- Referencia visual de la app actual: `native/target-shots/reference/*.png`
  (claro y oscuro) e índice en `native/target-shots/reference/README.md`.
  Repo de demostración para capturas: `native/tools/make-fixture.sh <dir>`.

## Estética

- **Componentes: usar siempre los de `mergeon_ui`** (`button`, `input`, `textarea`,
  `checkbox`, `radio_group`, `select`, `tabs`, `badge`, `status_badge`, `alert`,
  `kbd`, `spinner`, `empty_state`, `icon`, `hover_card`, `tooltip`, y los módulos
  `dialog`, `toast`, `menu`, `style`). Catálogo con ejemplos en
  `native/docs/ui-components.md`; galería viva en
  `crates/mergeon-ui/examples/gallery.rs`. Los componentes equivalentes de
  gpui-kit (su `Button`, `Dialog`, `Notification`...) tienen otras medidas: no usarlos
  directamente salvo los que `mergeon_ui` no cubre (`resizable`, `Scrollbar`,
  `v_virtual_list`, `Tree`, `TitleBar`...).
- El rem de la ventana es 16px, así que las utilidades de GPUI miden lo mismo que
  las de Tailwind (`h_8()` = 32px, `text_sm()` = 14px). El texto base de 13px
  (Señal) lo fija `mergeon_ui::init`.
- La app arranca con `.with_assets(mergeon_ui::Assets)` y `mergeon_ui::init(cx)`.
- GPUI no tiene `backdrop-filter`: donde el original desenfoca el fondo se usa el
  color opaco equivalente.

- Tema: `mergeon_ui::init(cx)` registra las fuentes (Geist y Geist Mono) y el
  tema «Señal» claro/oscuro que genera `native/tools/gen-theme.py` (fuente de
  verdad: los tokens de Señal, no `globals.css`; la app nativa ya no sigue la
  estética de Electron).
- Colores **siempre** del tema: `cx.theme().<token>` (gpui-kit) o
  `cx.palette().<token>` (`mergeon_ui::ActivePaletteExt`: `card`, `sidebar_*`,
  `primary_soft`, `success_soft`, `floating_border`...). Nada de colores literales
  salvo los que el CSS original también fija literales (colores de carril del
  grafo, etc.).
- Íconos renombrados en el catálogo Lucide embebido respecto de `lucide-vue-next`:
  `History` → `Lucide::RotateCcwClock`, `FileCode2` → `Lucide::FileCodeCorner`. Si
  falta otro, buscar su nombre actual en
  `native/target-ref/gpui-kit/crates/assets/assets/icons/`.
- Íconos: catálogo Lucide completo, `Icon::new(mergeon_ui::Lucide::GitBranch)`.
  Mismo ícono que usa el componente Vue (`lucide-vue-next`), mismo tamaño.
- Medidas: traducir las clases Tailwind del componente Vue a su equivalente
  GPUI (`h-8` → `.h_8()`, `gap-2` → `.gap_2()`, `text-sm` → `.text_sm()`,
  radios con `Radius` de Señal, 0–6px). Base de texto 13px. 1 unidad Tailwind = 4px.
- Texto de interfaz en inglés, copiado literal del componente Vue.

## Convenciones de código

- Identificadores en inglés, comentarios en español solo donde expliquen un porqué.
- Vistas con estado: `struct` + `impl Render`, creadas con `cx.new`. Piezas sin
  estado: `#[derive(IntoElement)]` + `impl RenderOnce`.
- El estado de la app vive en `mergeon-app` (`AppModel`). Las vistas lo
  observan (`cx.observe`) y llaman a sus métodos; no guardan copias de datos del
  repo salvo estado puramente visual (selección, scroll, pestaña activa).
- Git y disco **nunca** en el hilo de UI: `cx.background_spawn` / los métodos
  async de `mergeon-app`.
- IDs de elementos derivados del dominio (hash, ruta, nombre de rama), no índices.
- Listas largas virtualizadas (`uniform_list`, `v_virtual_list`).
- Sin `unwrap()` sobre datos externos.
- Tests: `#[gpui_kit::test]` para interacción (ver `references/gpui/test.md`);
  portar los tests de componente de vitest que verifiquen comportamiento.

## Encargo común de los crates de vistas (`mergeon-view-*`)

- **Leer antes**: este documento, `docs/ui-components.md` (componentes de
  `mergeon_ui`), `docs/app-api.md` (cómo obtener `AppModel`, observarlo, llamar
  acciones, eventos, `UiHost`), `docs/app-layer.md`, y de gpui-kit
  (`native/target-ref/gpui-kit/skills/gpui-kit/`): `SKILL.md`,
  `references/coding-guides.md`, `references/gpui/{entity,event,async,context,layout-style,focus-handle,action,element-id,test}.md`.
- **Fidelidad**: cada componente Vue asignado se porta completo — plantilla,
  script y `<style scoped>` —: mismas secciones, textos, íconos, atajos, menús
  contextuales, estados vacío/cargando/error, validaciones y confirmaciones, y
  las mismas llamadas (ahora a `AppModel`). Leer también sus tests de vitest
  (`*.test.js` junto al componente) y portarlos como `#[gpui_kit::test]`.
- **Datos y acciones** siempre vía `mergeon_app::AppModel`. Si falta algo ahí,
  resolverlo dentro del crate propio (p. ej. llamando al `Backend` con
  `cx.background_spawn`) y anotarlo en el informe; no editar `mergeon-app`.
- **Límites entre vistas**: lo que pertenece a otro crate no se implementa: se
  emite un evento propio (`impl EventEmitter<...>`) o se usa un evento/acción de
  `AppModel`, y se anota en el informe para que la cáscara lo conecte.
- **API del crate**: `pub fn init(cx: &mut App)` (acciones/atajos propios, si los
  hay) y una o más vistas (`Entity`) con constructor `new(..., window, cx)`.
  Diálogos como funciones `open_*(…, window, cx)` sobre `mergeon_ui::dialog`.
- **Galería**: `examples/<vista>_gallery.rs` — una ventana que arranca un
  `AppModel` real sobre el repo de demostración (`native/tools/make-fixture.sh`,
  en un directorio temporal) y muestra la vista sola, con variables de entorno
  para elegir tema (`MERGEON_THEME`) y estado (qué sección, qué diálogo abierto,
  qué menú). Capturar con `MERGEON_BIN=native/target/debug/examples/<…>
  native/tools/shot.sh native/target-shots/<vista>-<caso>-<tema>.png 5` y
  comparar con `native/target-shots/reference/` (índice en su `README.md`) hasta
  que coincidan a ojo en claro y oscuro.
- **Cierre**: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate>
  --all-targets -- -D warnings`, `cargo fmt -p <crate>`, `cargo test -p <crate>`
  limpios; `native/docs/view-<nombre>.md` con la API pública y los eventos que
  emite; informe final con API, tests portados/omitidos, diferencias visuales no
  cerradas, pendientes para la cáscara y rutas de capturas.
