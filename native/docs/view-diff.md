# `mergeon-view-diff`: visores de diff, de archivo y de blame

Port de `FileDiffViewer.vue`, `FileContentViewer.vue`, `BlameViewer.vue`,
`FileTreeRow.vue`, `RepositoryDiffPanel.vue` y `src/lib/highlight.js`. Son los
visores que comparten la vista de commits, la de cambios locales, las pestañas
de archivo y el diálogo de un stash.

El crate **no depende de `mergeon-app`**: cada pieza recibe datos y avisa con
eventos o callbacks. Pedir el diff, el archivo o el blame (y decidir qué acción
de bloque toca) es cosa de la vista que lo aloja.

```toml
mergeon-view-diff.workspace = true
```

```rust
mergeon_ui::init(cx);
mergeon_view_diff::init(cx);   // teclas de los visores: Ctrl+F, F7 / Shift+F7, Alt+↓ / Alt+↑
```

## Qué hay

| Vue | Rust | Tipo |
|---|---|---|
| `FileDiffViewer.vue` | `DiffViewer` + `DiffViewerEvent` | `Entity` (`impl Render`) |
| `RepositoryDiffPanel.vue` | `DiffPanel` | `RenderOnce`, controlado, sobre un `Entity<DiffViewer>` |
| `FileContentViewer.vue` | `FileContentViewer` + `FileContentViewerEvent` | `Entity` |
| `BlameViewer.vue` | `BlameViewer` (`BlameState`, `BlameLine`) | `Entity` |
| `FileTreeRow.vue` | `FileTreeRow`, `file_stats` → `FileStats` (con barritas `change_bars`) | `RenderOnce` |
| `lib/highlight.js` | módulo `highlight` | funciones puras |
| `useDiffPreferences` (`'unified'`/`'split'`) | `DiffViewMode` (reexportado de `mergeon_model`) | enum |

Los tres visores son entidades porque guardan estado entre frames (texto
analizado, desplazamiento, selección, caché de coloreado). Se crean **una vez**
(`cx.new(|cx| DiffViewer::new(window, cx))`), se guardan en la vista, se les dan
los datos con sus `set_*` cuando cambian y se pintan como hijo
(`.child(self.viewer.clone())`). Todos los `set_*` comparan antes de hacer nada:
llamarlos con lo que ya había no cuesta.

Los tres **ocupan todo el espacio de su contenedor** (`size_full`) y hacen su
propio scroll: el contenedor tiene que tener alto definido (`flex_1().min_h_0()`
dentro de una columna, o un `h(..)` fijo).

## `DiffViewer`

```rust
pub struct DiffViewer;                       // impl Render, Focusable, EventEmitter<DiffViewerEvent>

impl DiffViewer {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self;

    // Datos (las props del original)
    pub fn set_diff(&mut self, diff: impl Into<SharedString>, file_path: impl Into<SharedString>, cx: &mut Context<Self>);
    pub fn set_view_mode(&mut self, mode: DiffViewMode, cx: &mut Context<Self>);
    pub fn set_hunk_action(&mut self, action: Option<HunkAction>, cx: &mut Context<Self>);
    pub fn set_scroll_position(&mut self, position: ScrollPosition, cx: &mut Context<Self>);

    // Lectura
    pub fn diff(&self) -> &SharedString;
    pub fn file_path(&self) -> &SharedString;
    pub fn view_mode(&self) -> DiffViewMode;
    pub fn hunk_action(&self) -> Option<HunkAction>;
    pub fn language(&self) -> Option<highlight::Language>;
    pub fn is_empty(&self) -> bool;                              // sin diff: "No changes to display"
    pub fn is_parsing(&self) -> bool;                            // un diff grande analizándose en segundo plano
    pub fn lines(&self) -> impl ExactSizeIterator<Item = &mergeon_model::diff::DiffLine>;
    pub fn split_rows(&mut self) -> Arc<Vec<mergeon_model::diff::SplitRow>>; // Arc: la búsqueda la usa en segundo plano
    pub fn hunk_count(&self) -> usize;
    pub fn gutter_digits(&self) -> usize;                        // cifras del medianil (mín. 2)
    pub fn row_count(&mut self) -> usize;                        // filas de la vista actual
    pub fn painted_rows(&self) -> Range<usize>;                  // filas dibujadas en el último frame
    pub fn content_height(&self) -> Option<Pixels>;              // alto total, tras el primer frame
    pub fn line_segments(&self, index: usize) -> Vec<highlight::LineSegment>;
    pub fn scroll_position(&self) -> ScrollPosition;
    pub fn selected_text(&self) -> Option<String>;

    // Acciones
    pub fn select_all(&mut self, cx: &mut Context<Self>);
    pub fn apply_hunk(&mut self, index: usize, cx: &mut Context<Self>);   // botón principal del bloque
    pub fn apply_hunk_action(&mut self, index: usize, action: PatchAction, cx: &mut Context<Self>);

    // Líneas sueltas (solo vista unificada con acción de bloque)
    pub fn patch_actions(&self) -> &'static [PatchAction];      // lo que se ofrece
    pub fn selects_lines(&self) -> bool;
    pub fn select_lines(&mut self, rows: RangeInclusive<usize>, cx: &mut Context<Self>);
    pub fn clear_line_selection(&mut self, cx: &mut Context<Self>);
    pub fn selected_lines(&self) -> Vec<usize>;                 // altas y bajas elegidas, índices de lines()
    pub fn apply_lines(&mut self, action: PatchAction, cx: &mut Context<Self>);
}

// "Whole file" y marcas de cambios
impl DiffViewer {
    pub fn set_hunk_buttons(&mut self, show: bool, cx: &mut Context<Self>);   // sin "Stage/Discard hunk"; las líneas siguen
    pub fn has_hunk_buttons(&self) -> bool;
    pub fn change_markers(&mut self) -> Rc<Vec<ChangeMarker>>;                 // las de la vista actual
    pub fn jump_to_marker(&mut self, fraction: f32, cx: &mut Context<Self>) -> Option<usize>;
    pub fn line_anchor(&mut self) -> Option<LineAnchor>;                      // primera línea visible (lado nuevo) y su altura
    pub fn scroll_to_line(&mut self, anchor: &LineAnchor, cx: &mut Context<Self>); // vuelve a ella en otro diff del archivo
}
#[non_exhaustive]
pub struct LineAnchor { pub file_path: SharedString, pub line: u32, pub offset: f32, pub left: f32 }
pub struct ChangeMarker { pub row: usize, pub top: f32, pub bottom: f32, pub additions: bool, pub deletions: bool }

pub enum DiffViewerEvent {
    ApplyHunk(HunkRequest),          // @apply-hunk (también "Discard hunk": confirmar antes)
    ApplyLines(LinesRequest),        // "Stage lines" / "Unstage lines" / "Discard lines" o S / U / D
    Scrolled(ScrollPosition),        // scroll de .diff-viewer
}

#[non_exhaustive]
pub struct HunkRequest {
    pub index: usize,                // posición del bloque `@@`, desde 0
    pub action: PatchAction,
    pub patch: Option<String>,       // mergeon_model::patch::extract_hunk_patch(diff, index)
}

#[non_exhaustive]
pub struct LinesRequest {
    pub action: PatchAction,
    pub lines: Vec<usize>,           // índices de lines()
    pub patch: Option<String>,       // extract_lines_patch(diff, &lines, action.direction())
}

/// Lado del diff a la vista: sin preparar (`Stage`) o preparado (`Unstage`).
pub enum HunkAction { Stage, Unstage }
impl HunkAction {
    pub fn parse(name: &str) -> Option<Self>;   // "stage" | "unstage"
    pub fn as_str(self) -> &'static str;
    pub fn label(self) -> &'static str;         // "Stage hunk" / "Unstage hunk"
    pub fn tooltip(self) -> &'static str;       // "Stage only this hunk" / "Unstage only this hunk"
    pub fn is_reverse(self) -> bool;            // Unstage → git apply --reverse
    pub fn primary(self) -> PatchAction;
    pub fn patch_actions(self) -> &'static [PatchAction];   // Stage → [Discard, Stage]; Unstage → [Unstage]
}

/// Lo que se le pide a git con un parche.
pub enum PatchAction { Stage, Unstage, Discard }
impl PatchAction {
    pub fn hunk_label(self) / hunk_tooltip(self) / lines_label(self) / lines_tooltip(self) -> &'static str;
    pub fn key(self) -> &'static str;           // "s" / "u" / "d"
    pub fn is_reverse(self) -> bool;            // Unstage y Discard
    pub fn touches_worktree(self) -> bool;      // Discard: git apply --reverse sin --cached
    pub fn direction(self) -> PatchDirection;   // Forward (Stage) / Reverse
}

#[non_exhaustive]
pub struct ScrollPosition { pub top: f32, pub left: f32 }   // píxeles; ScrollPosition::new(top, left)
```

Qué hace:

- **Vista unificada**: medianil doble (viejo / nuevo) fijo al desplazar en
  horizontal, cabeceras de bloque, altas y bajas con su fondo, lo que cambió
  dentro de la línea resaltado, notas de git (`\ No newline…`) en cursiva.
  Las filas miden lo que la línea más larga, no lo que el visor (igual que el
  original).
- **Vista partida**: dos mitades con su número; las líneas largas se parten en
  renglones (no hay scroll horizontal).
- **Acción por bloque** (`set_hunk_action(Some(..))`, solo vista unificada):
  en lo no preparado, "Discard hunk" y "Stage hunk"; en lo preparado, solo
  "Unstage hunk". Botones siempre visibles y alineados a la derecha de cada
  cabecera `@@` (Señal `.hunk .hb`: 20px, texto de 11,5px). Ids:
  `("hunk-action", índice)` el principal y `("hunk-discard", índice)` el de
  descartar. El clic emite `ApplyHunk` con la `PatchAction` y el parche ya
  recortado; confirmar el descarte es cosa de quien aloja el visor.
- **Líneas sueltas** (misma condición): clic en el medianil elige una línea,
  arrastrar por él elige un tramo, Mayús+clic extiende y un clic en el
  medianil de una cabecera elige su bloque entero. Cuentan solo las altas y
  bajas. Las elegidas llevan el medianil en tinta (`sel-bg`), los números en
  `sel-add` / `sel-del` y la marca naranja de 2px; sobre la primera que se
  vea aparece la barra "Discard lines" / "Stage lines" (o "Unstage lines"),
  ids `lines-discard`, `lines-stage`, `lines-unstage`. Con el foco en el
  visor, `S` / `U` / `D` hacen lo mismo (solo las que se ofrecen; también en
mayúscula, con Shift). Un clic en
  el código, `Esc` o un diff nuevo sueltan la selección. Emite `ApplyLines`
  con el parche de `mergeon_model::patch::extract_lines_patch` (cabeceras
  `@@` recalculadas; al preparar, las bajas no elegidas pasan a contexto y las
  altas no elegidas se omiten; al quitar o descartar, al revés).
- **Coloreado** por la extensión de `file_path`, solo de las filas visibles y
  guardado por línea.
- **Selección y copia**: arrastrar selecciona (doble clic una palabra, triple
  la línea, Mayús+clic extiende); `Ctrl/Cmd+C` copia, `Ctrl/Cmd+A` selecciona
  todo, `Esc` suelta. Se copia el texto original (con sus tabuladores), una
  línea por fila y sin los signos `+`/`-`. En la vista partida la selección se
  queda en el lado donde empezó. Los números de línea no se seleccionan.
- **Teclado** con el foco en el visor: flechas, `PageUp`/`PageDown`,
  `Home`/`End`.
- **Virtualización**: solo se construyen las filas de la ventana visible
  (`painted_rows()`), sea cual sea el tamaño del diff.
- **Análisis en segundo plano**: un diff de más de 64 KiB se analiza
  (líneas, cambios dentro de la línea, columnas y las filas de la vista
  partida, para que cambiar de vista tampoco las calcule en el hilo de la
  interfaz) fuera del hilo de la interfaz. `set_diff` vuelve en el acto; mientras
  tanto `is_parsing()` es `true`, `diff()` / `file_path()` ya dicen lo pedido,
  `lines()` sigue con el diff que se ve, y ese diff anterior se sigue pintando
  sin acciones de bloque ni de líneas. Si el análisis pasa de 30 ms se atenúa
  y aparece un spinner pequeño arriba a la derecha (`diff-parsing`); si acaba
  antes, no hay estado intermedio. Un `set_diff` posterior cancela el
  análisis pendiente: uno viejo nunca pisa al nuevo. Un
  `set_scroll_position` durante el análisis se aplica cuando aparece el diff.
  Los diffs pequeños se analizan en el acto, como siempre.
- **Cachés**: los altos de las filas (en la vista partida, los cortes de cada
  fila) se calculan una vez por diff, vista, tipografía y ancho, no por frame
  ni al cambiar el alto del visor; el texto ya formado (`shape_line`) de
  celdas y números se guarda entre frames (clave: texto, estilos, cuerpo y
  ancho de corte; dos generaciones de 4096 formas, lo descartado se suelta en
  segundo plano), así que volver por lo ya visto no lo forma de nuevo. Esto
  último vale también para el visor de archivos y el blame.

- **Marcas de cambios** (`change_markers`, Señal): una tira de 12px a la
  derecha de la barra de desplazamiento (`change-markers`) con una marca por
  bloque de altas y bajas, a su altura proporcional (fila / filas de la vista):
  verde (`diff-add-sign`) si solo añade, roja (`diff-del-sign`) si solo quita,
  las dos mitades si hace ambas cosas; alto mínimo 4px. Un clic lleva al bloque
  más cercano (como F7: arriba con dos líneas de contexto). Las posiciones se
  calculan al analizar el diff (en segundo plano si es grande; las de la vista
  partida junto con sus filas) y se agrupan a 1024 posiciones: pintar recorre
  como mucho 1024 marcas, nunca las líneas. Sin cambios (renombre, binario) no
  hay tira.
- **"Whole file"** (`set_hunk_buttons(false)`): con el diff del archivo entero
  (`--unified=999999999`, un solo bloque) las cabeceras no llevan "Stage hunk" /
  "Discard hunk" (serían el archivo entero; descartar el bloque lo descartaría
  todo de un clic) y `apply_hunk` no hace nada. Las líneas sueltas siguen:
  `extract_lines_patch` recalcula la cabecera y git aplica ese parche igual
  (test con git real en `mergeon_model::patch`). gitcraft hace lo mismo con sus
  parciales.

`set_diff` con un diff distinto vuelve el desplazamiento al principio y suelta
la selección; para restaurar el de una pestaña, llamar a `set_scroll_position`
**después** (se ajusta a los límites en el primer frame; si el diff se está
analizando, se guarda y se aplica cuando aparece).

Al alternar "Whole file" el desplazamiento en píxeles no sirve (el diff por
bloques y el del archivo entero tienen alturas distintas): quien aloja el
visor toma `line_anchor()` antes de pedir el otro diff (la primera línea
visible de la versión nueva —o la siguiente que exista si arriba hay una
cabecera o líneas borradas— y a qué distancia del borde está) y, cuando llega,
llama a `scroll_to_line` **en lugar de** restaurar el desplazamiento guardado.
La línea queda a la misma altura; si el diff por bloques no la tiene, se va a
la de número más cercano (a igual distancia, la anterior). Solo actúa si el
diff es del mismo archivo; con un diff analizándose se aplica al aparecer.
Lo hacen `ChangesView`, `CommitChangesTab` (detalle de un commit y Compare) y
`HistoricalFileTabView` (pestaña de un commit y de historial).

### Uso directo (detalle de un commit, pestaña de archivo, diálogo de stash)

```rust
struct CommitDiff {
    viewer: Entity<DiffViewer>,
    _subscriptions: Vec<Subscription>,
}

impl CommitDiff {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let viewer = cx.new(|cx| DiffViewer::new(window, cx));
        let _subscriptions = vec![cx.subscribe(&viewer, |this, _, event: &DiffViewerEvent, cx| {
            if let DiffViewerEvent::Scrolled(position) = event {
                this.remember_scroll(*position, cx);     // @view-scroll de la pestaña
            }
        })];
        Self { viewer, _subscriptions }
    }

    /// Cuando llega el diff (o cambian las preferencias):
    fn show(&mut self, diff: String, path: String, mode: DiffViewMode, cx: &mut Context<Self>) {
        self.viewer.update(cx, |viewer, cx| {
            viewer.set_diff(diff, path, cx);
            viewer.set_view_mode(mode, cx);
        });
    }
}

// render:
div().flex_1().min_h_0().child(self.viewer.clone())
```

Los estados que el original pintaba **fuera** del visor siguen siendo de quien
lo aloja: "Loading diff...", "Diff too large to display here (2.9 KB)"
(`mergeon_model::format::format_size`), el botón "Retry"… El visor solo sabe
decir "No changes to display" cuando el diff está vacío. Un diff sin bloques
(un binario: `Binary files a/x and b/x differ`) deja el visor vacío, como en el
original.

Para una caja que se ajuste al contenido con un tope (el `max-height: 420px`
del diálogo de stash): `h(viewer.read(cx).content_height().unwrap_or(px(420.)).min(px(420.)))`.

## Buscar en el visor y saltar entre cambios

`DiffViewer` y `FileContentViewer` tienen una barra de búsqueda (módulo
`find`) y el diff, navegación por bloques de cambios. Las teclas las registra
`mergeon_view_diff::init` en el contexto de teclado `CodeViewer`
(`VIEWER_KEY_CONTEXT`), que es el del visor entero (lienzo y barra):

| Tecla | Acción (`mergeon_model::keyboard_shortcuts::VIEWER_SHORTCUTS`) |
|---|---|
| `mod+f` | `FindInViewer`: abre la barra (o vuelve a su campo, con todo seleccionado); si hay una selección de una línea, la usa como consulta |
| Enter / Shift+Enter (en la barra) | Coincidencia siguiente / anterior, dando la vuelta; el visor se desplaza hasta ella (centrada si no se veía; también en horizontal) |
| Esc (en la barra o en el visor) | Cierra la barra y devuelve el foco al visor |
| `f7` / `alt+down` | `NextChange`: siguiente bloque de altas y bajas (solo el diff), arriba con dos líneas de contexto; da la vuelta |
| `shift+f7` / `alt+up` | `PreviousChange` |

**Regla de `mod+f`**: en toda la app es "Search commits", salvo con el foco
dentro de un visor de diff o de archivo (tras un clic en su código o en su
barra), donde gana la búsqueda del visor: su binding va en un contexto de
teclado más profundo y GPUI lo prueba antes que el global, y su acción para
la propagación. Sin `mergeon_view_diff::init`, `mod+f` sigue siendo global
también dentro del visor.

La barra (`find-bar`, arriba a la derecha): campo (`find-input`), contador
(`find-count`: "3 / 17", "No results", "Invalid regex", "10000+" si hay más
de `MAX_MATCHES`), ↑ (`find-previous`), ↓ (`find-next`), Aa (`find-case`),
.* (`find-regex`) y × (`find-close`). Todas las coincidencias que se ven se
resaltan (`warning` al 38 %) y la actual con más contraste (`primary` al
62 %), como marcas del lienzo detrás del texto.

La búsqueda corre sobre las líneas ya analizadas (las del diff unificado, o
los dos lados de la vista partida; las notas `\ No newline…` no cuentan),
nunca en el render: al cambiar la consulta, las opciones o el documento
(otro diff, otra vista, otro archivo). Se guarda por consulta y generación
del documento; con más de 20.000 filas va en segundo plano y una búsqueda
vieja no pisa a la nueva. La lógica pura (consulta, regex, coincidencias por
fila) está en `mergeon_model::text_search`.

```rust
viewer.open_find(window, cx);          // como mod+f
viewer.find_next(cx); viewer.find_previous(cx);
viewer.close_find(window, cx);
viewer.find_status(cx) -> FindStatus   // open, query, case_sensitive, regex, invalid, searching,
                                       // matches (fila, columna, bytes), truncated, current, counter
diff_viewer.change_starts() -> Rc<Vec<usize>>   // primera fila de cada bloque, vista actual
diff_viewer.next_change(cx) / previous_change(cx) -> Option<usize>
```

## `DiffPanel` (cambios locales)

Componente **controlado**: se construye en cada `render` con el estado actual.
Él mismo pone al día al visor (diff, vista y acción de bloque), así que con el
panel no hace falta llamar a los `set_*` del visor.

```rust
impl DiffPanel {
    pub fn new(viewer: &Entity<DiffViewer>) -> Self;
    pub fn file(self, path: impl Into<SharedString>, status: impl Into<SharedString>) -> Self;  // selectedFile: ruta + working_dir
    pub fn file_opt(self, file: Option<(SharedString, SharedString)>) -> Self;
    pub fn diff(self, diff: impl Into<SharedString>) -> Self;        // fileDiff
    pub fn loading(self, loading: bool) -> Self;                     // loadingDiff
    pub fn too_large(self, size: Option<u64>) -> Self;               // diffTooLarge + diffSize
    pub fn view_mode(self, mode: DiffViewMode) -> Self;              // diffViewMode
    pub fn ignore_whitespace(self, on: bool) -> Self;                // ignoreWhitespace
    pub fn hunk_action(self, action: Option<HunkAction>) -> Self;    // hunkAction
    pub fn discardable(self, discardable: bool) -> Self;             // canDiscard (por defecto true)
    pub fn conflict(self, resolver: impl IntoElement) -> Self;       // isConflict: se pinta esto en lugar del diff
    pub fn on_toggle_view(self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self;
    pub fn on_toggle_ignore_whitespace(self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self;
    pub fn on_discard(self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self;
    pub fn whole_file(self, on: bool) -> Self;                       // "Whole file" encendido; quita los botones de bloque
    pub fn on_toggle_whole_file(self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self;
    pub fn image(self, viewer: Option<Entity<ImageDiffViewer>>) -> Self;   // imagen: este visor en lugar del diff
                                                                           // (un SVG añade "Image | Text")
}

pub fn whole_file_tooltip(on: bool) -> &'static str;

pub fn view_toggle_label(mode: DiffViewMode) -> &'static str;       // "Split" | "Unified"
pub fn ignore_whitespace_tooltip(on: bool) -> &'static str;
pub fn panel_status_label(status: &str) -> &str;                    // "?", "??", "U" → "U"
```

```rust
// en `new`: suscribirse al visor para el staging por bloque
cx.subscribe_in(&diff_viewer, window, |this, _, event: &DiffViewerEvent, window, cx| {
    if let DiffViewerEvent::ApplyHunk(request) = event {
        match &request.patch {
            Some(patch) => this.apply_patch(patch.clone(), request.action.is_reverse(), window, cx),
            None => mergeon_ui::toast::error(window, cx, "Could not build a patch for that block"),
        }
    }
})

// en `render`
DiffPanel::new(&self.diff_viewer)
    .file_opt(selected.map(|file| (file.path.clone().into(), file.working_dir.clone().into())))
    .diff(self.file_diff.clone())
    .loading(self.loading_diff)
    .too_large(self.diff_too_large.then_some(self.diff_size))
    .view_mode(preferences.diff_view_mode)
    .ignore_whitespace(preferences.ignore_whitespace)
    .hunk_action(self.hunk_action())            // None con "Ignore spaces", sin seguir o en conflicto
    .discardable(self.can_discard())
    .on_toggle_view(cx.listener(|this, _, _, cx| this.toggle_diff_view(cx)))
    .on_toggle_ignore_whitespace(cx.listener(|this, _, _, cx| this.toggle_ignore_whitespace(cx)))
    .on_discard(cx.listener(|this, _, window, cx| this.discard_selected(window, cx)))
```

- Sin archivo (`file_opt(None)`) no pinta nada, como el `v-if="selectedFile"`.
- Con "Whole file" y acción de bloque, la nota `diff-note` explica que el
  archivo entero es un bloque y que se eligen líneas en el medianil.
- Ids de sus botones: `toggle-whole-file`, `toggle-ignore-whitespace`, `toggle-diff-view`,
  `discard-selected`. Estados: `diff-loading`, `diff-too-large`, `diff-empty`,
  nota `diff-note`.
- `conflict(..)` es el hueco del `ConflictResolver`, que no es de este crate:
  con él se pinta solo ese elemento, sin cabecera.
- La regla de cuándo hay acción de bloque (`hunkAction` de `ChangesView.vue`)
  es de la vista de cambios, no del panel.

## `ImageDiffViewer` (imágenes antes y después)

```rust
pub fn is_image_path(path: &str) -> bool;                    // png, jpg/jpeg, gif, webp, bmp, ico, svg
pub fn is_svg_path(path: &str) -> bool;                      // imagen que además tiene diff de texto
pub fn image_text_toggle(viewer: &Entity<ImageDiffViewer>, cx: &App) -> Tabs; // "Image | Text" de un SVG
pub fn image_format_for_path(path: &str) -> Option<ImageFormat>;
pub fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)>; // solo la cabecera (y la raíz de un SVG)
pub struct ImageVersion { pub image: Option<Arc<Image>>, pub size: u64, pub dimensions: Option<(u32, u32)>, pub too_large: bool }
impl ImageVersion { pub fn new(path: &str, bytes: Vec<u8>, size: u64, too_large: bool) -> Self; pub fn caption(&self) -> String; }
pub struct ImageDiff { pub before: Option<ImageVersion>, pub after: Option<ImageVersion> }

impl ImageDiffViewer {          // Entity: cx.new(ImageDiffViewer::new)
    pub fn set_loading(&mut self, cx);  pub fn set_images(&mut self, diff: ImageDiff, cx);
    pub fn set_error(&mut self, message, cx);  pub fn clear(&mut self, cx);
    pub fn images(&self) -> Option<&ImageDiff>;  pub fn is_loading(&self) -> bool;  pub fn error(&self) -> Option<&SharedString>;
    pub fn svg_as_text(&self) -> bool;  pub fn set_svg_as_text(&mut self, as_text: bool, cx);
    pub fn shows_image(&self, path: &str) -> bool;      // imagen, salvo un SVG con "Text" elegido
}
```

Las dos versiones lado a lado ("BEFORE" / "AFTER"; añadida: solo "ADDED";
borrada: solo "DELETED"), cada una con "W × H px · tamaño" y la imagen centrada
sin pasar de su tamaño real (`ObjectFit::ScaleDown`) sobre `bg-inset`. Leer los
bytes es de quien lo aloja, fuera del hilo de la interfaz;
`ImageVersion::new` (medir la cabecera y la huella de los bytes) también va en
segundo plano. GPUI decodifica en segundo plano; el visor quita de la caché de
assets de GPUI las imágenes que deja de mostrar (al cambiar o al cerrarse).
Una versión de más de 20 MB (`MAX_IMAGE_BYTES` de `mergeon-git-query`) no se
carga: "Too large to preview (…)", pero de ella se leen los primeros 64 KB
(`IMAGE_HEADER_BYTES`) y la leyenda sigue diciendo sus medidas. Una que no se
puede decodificar: "Can't display this image". Si decodificar pasa de 200 ms
(`LOADING_DELAY` de GPUI; una foto de varios MB), el recuadro dice "Decoding
image…" con un spinner: para eso el `img` lleva id (sin él GPUI no guarda
estado entre frames y nunca pinta su `with_loading`). Ids: `image-diff`,
`image-before`, `image-after` (etiqueta = la leyenda), `image-before-decoding`,
`image-after-decoding`, `image-loading`, `image-error`, `image-missing`.

Un SVG es imagen y texto: la cabecera del diff lleva el segmentado
"Image | Text" (`image_text_toggle`, ids `tab-svg-image` / `tab-svg-text`),
por defecto "Image". Con "Text" se pinta el diff de texto del XML con sus
interruptores ("Whole file", "Ignore spaces", "Split" y, en Local Changes, las
acciones de bloque). La elección vive en el `ImageDiffViewer` de cada vista
(`svg_as_text`) y se mantiene al pasar a otro archivo de esa vista; quien lo
aloja pide siempre las dos cosas (imágenes y diff) y decide qué pintar con
`shows_image(path)`. Lo usan `DiffPanel` (Local Changes), `CommitChangesTab`
(detalle de commit y Compare) y `HistoricalFileTabView`.

## `FileContentViewer`

```rust
pub struct FileContentViewer;                // impl Render, Focusable, EventEmitter<FileContentViewerEvent>

impl FileContentViewer {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self;
    pub fn set_file(&mut self, path: impl Into<SharedString>, file: Option<FileContent>, cx: &mut Context<Self>);
    pub fn set_loading(&mut self, loading: bool, cx: &mut Context<Self>);
    pub fn set_error(&mut self, error: impl Into<SharedString>, cx: &mut Context<Self>);   // "" lo quita
    pub fn set_header_hidden(&mut self, hidden: bool, cx: &mut Context<Self>);            // hide-header
    pub fn set_scroll_position(&mut self, position: ScrollPosition, cx: &mut Context<Self>);

    pub fn path(&self) -> &SharedString;
    pub fn file(&self) -> Option<&FileContent>;
    pub fn is_loading(&self) -> bool;
    pub fn error(&self) -> &SharedString;
    pub fn is_header_hidden(&self) -> bool;
    pub fn language(&self) -> Option<highlight::Language>;
    pub fn line_count(&self) -> usize;
    pub fn line(&self, index: usize) -> Option<&str>;
    pub fn status_text(&self) -> Option<SharedString>;     // "Binary file, 2.1 KB", "Empty file"…
    pub fn painted_rows(&self) -> Range<usize>;
    pub fn scroll_position(&self) -> ScrollPosition;
    pub fn selected_text(&self) -> Option<String>;
    pub fn select_all(&mut self, cx: &mut Context<Self>);
}

pub enum FileContentViewerEvent { Close, Scrolled(ScrollPosition) }   // @close, @view-scroll

#[non_exhaustive]
pub struct FileContent { pub content: SharedString, pub size: u64, pub binary: bool, pub too_large: bool }
impl FileContent {
    pub fn text(content: impl Into<SharedString>) -> Self;
    pub fn binary(size: u64) -> Self;
    pub fn too_large(size: u64) -> Self;
    pub fn with_size(self, size: u64) -> Self;
}
```

```rust
// de `mergeon_git_query::FileAtCommit`
let content = if file.binary {
    FileContent::binary(file.size)
} else if file.too_large {
    FileContent::too_large(file.size)
} else {
    FileContent::text(file.content).with_size(file.size)
};
self.file_viewer.update(cx, |viewer, cx| {
    viewer.set_loading(false, cx);
    viewer.set_error("", cx);
    viewer.set_file(path, Some(content), cx);
});
```

Orden de lo que se pinta, como en el original: cargando (spinner) → error (en
rojo) → "Binary file, {size}" → "Too large to show here, {size}" → las líneas →
"Empty file". La cabecera (ruta recortada por el principio, tamaño y botón
`close-file`) se quita con `set_header_hidden(true)`.

## `BlameViewer`

El original pedía el blame él mismo; aquí lo pide quien lo aloja.

```rust
impl BlameViewer {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self;    // empieza en Loading
    pub fn set_state(&mut self, state: BlameState, cx: &mut Context<Self>);
    pub fn state(&self) -> &BlameState;
    pub fn line_count(&self) -> usize;
    pub fn status_text(&self) -> Option<SharedString>;   // "Loading blame...", "Error: …", "Too large to blame here, …"
    pub fn painted_rows(&self) -> Range<usize>;
    pub fn selected_text(&self) -> Option<String>;
    // Navegación (blame navegable)
    pub fn blocks_in(&self, rows: Range<usize>) -> Vec<(usize, usize)>;   // (primera fila, filas) del mismo commit
    pub fn line(&self, ix: usize) -> Option<&BlameLine>;
    pub fn open_commit_at(&mut self, ix: usize, cx: &mut Context<Self>);       // emite OpenCommit
    pub fn blame_previous_at(&mut self, ix: usize, cx: &mut Context<Self>);    // emite BlamePrevious
}

pub enum BlameState { Loading, Error(SharedString), TooLarge(u64), Lines(Vec<BlameLine>) }

#[non_exhaustive]
pub struct BlameLine {
    pub short_hash: SharedString, pub author: SharedString, pub date: Option<i64>, pub summary: SharedString, pub content: SharedString,
    pub hash: SharedString,                                   // completo (navegación)
    pub previous: Option<(SharedString, SharedString)>,       // `previous` del porcelain: padre y ruta
}
impl BlameLine {
    pub fn new(short_hash, author, date: Option<i64>, summary, content) -> Self;
    pub fn commit(self, hash) -> Self;
    pub fn previous(self, hash, path) -> Self;
    pub fn is_uncommitted(&self) -> bool;   // 0000000 ("Not Committed Yet")
}

pub enum BlameViewerEvent {
    OpenCommit { hash: SharedString },                           // clic en el bloque o "Show commit in history"
    BlamePrevious { hash: SharedString, path: SharedString },   // "Blame previous revision"
}
```

**Blame navegable.** Cada bloque visible (líneas seguidas del mismo commit) lleva
encima de la columna de metadatos un blanco transparente (`blame-commit:<fila>`)
con tooltip (asunto, autor, fecha y hash), puntero, un tinte al pasar el ratón,
clic → `OpenCommit` y menú contextual: "Show commit in history", "Blame previous
revision" (apagado si la línea no tiene `previous`) y "Copy commit hash". Solo se
construyen los de las filas a la vista y se recolocan al desplazarse (`scrolled`
repinta el visor). Las líneas sin commit no tienen blanco. Qué hacer con los
eventos lo decide quien aloja el visor (en la app, el diálogo de blame de
`mergeon-view-changes`).

```rust
// de `mergeon_git_query::BlameResult`
let state = if result.too_large {
    BlameState::TooLarge(result.size)
} else {
    BlameState::Lines(result.lines.into_iter().map(|line| {
        let blame_line = BlameLine::new(line.short_hash, line.author, line.date, line.summary, line.content)
            .commit(line.hash);
        match line.previous_hash.zip(line.previous_path) {
            Some((hash, path)) => blame_line.previous(hash, path),
            None => blame_line,
        }
    }).collect())
};
viewer.update(cx, |viewer, cx| viewer.set_state(state, cx));

// dentro del diálogo "Blame: app.js":
div().h(px(420.)).child(self.blame_viewer.clone())
```

`date` son milisegundos desde época; se pinta con `format_short_date`.

## `FileTreeRow`

Fila de carpeta o de archivo para cualquier listado (cambios locales, archivos
de un commit, stash). No guarda estado: la lista decide qué carpetas están
desplegadas y qué archivo está seleccionado.

Con `depth > 0` pinta una guía de sangría por nivel: una línea vertical de 1px
(`border-hair`; en la fila seleccionada, `sel-fg` al 16%) bajo la flecha de la
carpeta de ese nivel, para que un archivo de la raíz no parezca dentro de la
carpeta de encima. Son `depth` divs por fila pintada (las listas van
virtualizadas): nada se recorre por frame. Ids `("tree-guide", nivel)`.

```rust
impl FileTreeRow {                            // RenderOnce; además Styled, InteractiveElement, ParentElement
    pub fn folder(id: impl Into<ElementId>, name: impl Into<SharedString>) -> Self;
    pub fn file(id: impl Into<ElementId>, name: impl Into<SharedString>) -> Self;
    pub fn from_row<T>(row: &mergeon_model::file_tree::FileTreeRow<T>) -> Self;   // id = row.key
    pub fn depth(self, depth: usize) -> Self;
    pub fn expanded(self, expanded: bool) -> Self;          // carpetas; por defecto true
    pub fn selected(self, selected: bool) -> Self;
    pub fn dir(self, dir: impl Into<SharedString>) -> Self;     // lista plana: carpeta en gris tras el nombre
    pub fn path(self, path: impl Into<SharedString>) -> Self;   // tooltip del nombre
    pub fn leading(self, element: impl IntoElement) -> Self;    // slot #leading (solo archivos)
    pub fn trailing(self, element: impl IntoElement) -> Self;   // slot #trailing (solo archivos)
    pub fn on_activate(self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self;   // clic
    pub fn on_open_new(self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self;   // doble clic
    pub fn on_context_menu(self, f: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static) -> Self;
}

pub fn file_stats(additions: usize, deletions: usize) -> Option<FileStats>;   // "+12 −3"; None si 0/0

impl FileStats {
    pub fn on_ink(self, on_ink: bool) -> Self;   // fila seleccionada en tinta: sel-add / sel-del
    pub fn bars(self, bars: bool) -> Self;       // + las cinco barritas ▮▮▮▮▮ tras las cifras
    pub fn additions / deletions(&self) -> usize;
    pub fn has_bars(&self) -> bool;
    pub fn labels(&self) -> (Option<String>, Option<String>);   // ("+12", "−3")
}

// Las barritas de Señal: cinco bloques de 6×8 px con 1 px de separación, 6 px
// tras las cifras. Verde (`success`) lo añadido, rojo (`destructive`) lo
// quitado y gris (`border-base`) lo que sobra; sobre tinta, `sel-add`,
// `sel-del` y blanco al 20 %.
pub const CHANGE_BAR_BLOCKS: usize = 5;
pub enum ChangeBlock { Added, Removed, Empty }
pub fn change_bars(additions: usize, deletions: usize) -> [ChangeBlock; 5];
pub fn bar_pattern(blocks: &[ChangeBlock]) -> String;   // "+++−·": etiqueta accesible (id `change-bars`)
```

`change_bars` reparte la proporción como el diseño (`round(a/t·5)` verdes y
`min(5 − verdes, round(r/t·5))` rojos: `+10 −4` → ▮▮▮▮ verde + ▮ rojo, `+38 −2`
→ cinco verdes) cuando cambiaron cinco líneas o más; con menos, cada bloque es
una línea y el resto queda gris (`+1` → un verde y cuatro grises), como el
`diffstat` de GitHub. Lo usa la lista de archivos de un commit
(`mergeon-view-commits`) y lo puede usar cualquier otra lista (Local Changes):

```rust
.trailing(div().children(
    file_stats(file.additions, file.deletions).map(|stats| stats.bars(true).on_ink(selected)),
))
```

```rust
use mergeon_model::file_tree::build_file_tree_rows;
use mergeon_ui::menu::MenuTriggerExt as _;

// Árbol (cambios locales)
let rows = build_file_tree_rows(&files, |file| &file.path, |folder| self.is_expanded(folder));
div().children(rows.iter().map(|row| {
    let tree_row = FileTreeRow::from_row(row).expanded(self.is_expanded(&row.path));
    match &row.file {
        None => {
            let folder = row.path.clone();
            tree_row
                .on_activate(cx.listener(move |this, _, _, cx| this.toggle_folder(&folder, cx)))
                .into_any_element()
        }
        Some(file) => {
            let (select, open, menu) = (file.clone(), file.clone(), file.clone());
            tree_row
                .selected(self.selected_path.as_deref() == Some(&file.path))
                .leading(mergeon_ui::status_badge(&file.status))
                .on_activate(cx.listener(move |this, _, window, cx| this.select_file(&select, window, cx)))
                .on_open_new(cx.listener(move |this, _, window, cx| this.open_file(&open, window, cx)))
                .context_menu_items(move |_, _| file_menu(&menu))
                .into_any_element()
        }
    }
}))

// Lista plana (archivos de un commit): nombre + carpeta en gris + cifras y barritas
FileTreeRow::file(SharedString::from(format!("file:{}", file.path)), name)
    .path(file.path.clone())
    .dir(dir)
    .selected(selected)
    .leading(mergeon_ui::status_badge(&file.status).on_ink(selected))
    .trailing(div().children(
        file_stats(file.additions, file.deletions).map(|stats| stats.bars(true).on_ink(selected)),
    ))
```

Como en el original, un doble clic llama dos veces a `on_activate` y después a
`on_open_new`. Las carpetas no tienen fondo de hover ni cursor de mano.

## `highlight` (`src/lib/highlight.js`)

```rust
pub enum Language { JavaScript, TypeScript, Tsx, Html, Xml, Css, Scss, Less, Json, Markdown, Bash, Python, /* … */ }
impl Language {
    pub fn name(self) -> &'static str;              // el nombre del original: "javascript", "xml", "ini"…
    pub fn from_name(name: &str) -> Option<Self>;
}
pub fn language_for_path(path: &str) -> Option<Language>;      // languageForPath

pub enum TokenKind { Keyword, String, Number, Title, Variable, Comment, Deletion, Addition }
impl TokenKind { pub fn hljs_class(self) -> &'static str; pub fn is_italic(self) -> bool; }

pub type Token = (Range<usize>, TokenKind);
pub fn highlight_tokens(code: &str, language: Option<Language>) -> Vec<Token>;
pub fn highlight_segments(code: &str, language: Option<Language>) -> Vec<SyntaxSegment>;          // highlightSegments
pub fn apply_inline_range(segments: &[SyntaxSegment], range: Option<InlineRange>) -> Vec<LineSegment>;   // applyInlineRange
pub fn build_line_segments(line: &DiffLine, language: Option<Language>) -> Vec<LineSegment>;      // buildLineSegments

pub struct SyntaxColors { pub keyword: Hsla, pub string: Hsla, /* … */ }     // SyntaxColors::new(cx), .color(kind)
pub fn code_font_family(cx: &App) -> SharedString;   // (raíz del crate) la primera de `--font-mono` instalada
```

El mapa extensión / nombre de archivo → lenguaje es el del original. El análisis
lo hace tree-sitter (el resaltador de gpui-kit) **línea a línea**, como el
original, y las capturas se agrupan en las ocho clases de color del CSS de
`FileDiffViewer.vue`. Los colores salen del tema activo (`SyntaxColors::new`),
en claro y en oscuro.

Para colorear código fuera de los visores (p. ej. una vista previa): 
`highlight_tokens` da los rangos, `SyntaxColors::color(kind)` el color y
`code_font_family(cx)` la fuente.

## Galería y capturas

```bash
cd native
cargo run -p mergeon-view-diff --example diff_gallery
MERGEON_THEME=light MERGEON_DIFF_CASE=staging cargo run -p mergeon-view-diff --example diff_gallery

MERGEON_DIFF_CASE=split MERGEON_BIN=target/debug/examples/diff_gallery \
  tools/shot.sh target-shots/diff-split-dark.png 4
```

| Variable | Valores |
|---|---|
| `MERGEON_THEME` | `dark` (defecto) · `light` · `system` |
| `MERGEON_DIFF_CASE` | `unified` (defecto) · `split` · `whitespace` · `whitespace-split` · `staging` · `staging-split` · `long-lines` · `staged` · `untracked` · `deleted` · `ignore-spaces` · `loading` · `too-large` · `empty` · `file` · `file-header` · `binary` · `file-too-large` · `file-empty` · `file-loading` · `file-error` · `blame` · `blame-loading` · `blame-error` · `blame-too-large` · `tree` · `large` · `large-split` · `large-file` |
| `MERGEON_DIFF_INPUT` | pasos separados por `;`, uno cada 300ms: `move:x,y`, `click:x,y`, `dclick:x,y`, `drag:x1,y1,x2,y2`, `scroll:x,y,dx,dy`, `key:<tecla>` |
| `MERGEON_DIFF_BENCH` | `1`: 120 pasos de scroll y milisegundos por paso en stderr |

Los datos van embebidos (los diffs del repo de `tools/make-fixture.sh`): el
crate no usa `AppModel`, así que la galería tampoco.

## Eventos y huecos para quien aloja los visores

| Origen | Evento / callback | Qué hace la vista |
|---|---|---|
| `DiffViewer` | `DiffViewerEvent::ApplyHunk(HunkRequest)` / `ApplyLines(LinesRequest)` | `apply_patch(repo, patch, ApplyPatchOptions { reverse: action.is_reverse(), worktree: action.touches_worktree(), file_path })`, confirmando antes si es `Discard`; sin `patch`, avisar "Could not build a patch for that block" |
| `DiffViewer` | `DiffViewerEvent::Scrolled(ScrollPosition)` | guardar el scroll de la pestaña (`view-scroll`) |
| `FileContentViewer` | `FileContentViewerEvent::Close` | cerrar el archivo abierto en "Repo tree" |
| `FileContentViewer` | `FileContentViewerEvent::Scrolled(..)` | guardar el scroll de la pestaña |
| `DiffPanel` | `on_toggle_view`, `on_toggle_ignore_whitespace`, `on_discard` | `toggle_diff_view`, `toggle_ignore_whitespace` (+ recargar el diff), confirmar y descartar |
| `DiffPanel` | `.conflict(elemento)` | montar el resolvedor de conflictos (otro crate) |
| `FileTreeRow` | `on_activate`, `on_open_new`, menú contextual | seleccionar / plegar, abrir en pestaña nueva, menú del archivo |

No hay nada que conectar en la cáscara: el crate no emite eventos globales ni
registra acciones.

## Tests

`cargo test -p mergeon-view-diff`: 84 tests (36 de unidad y 48 de interacción).

- `src/highlight.rs`: `highlight.test.js` completo (elegir lenguaje, colorear
  una línea, fusionar color y cambio, lo que pinta el visor) más la tabla de
  extensiones entera y el agrupado de tokens por lenguaje.
- `tests/viewers.rs` (`#[gpui_kit::test]`, ventana sin pantalla, clics, rueda y
  teclas reales): `FileDiffViewer.test.js` completo salvo el caso que leía el
  CSS (`does not let the split gutter grow with the pane`, que vigilaba una
  regla `flex` y aquí no tiene equivalente: el medianil es geometría del
  lienzo), más staging por bloque con el parche de `mergeon_model::patch`,
  panel (`RepositoryDiffPanel`), contenido de archivo, blame, fila de árbol,
  virtualización con 20.000 líneas, scroll, selección y copia.
- `src/canvas.rs`, `src/text.rs`, `src/typography.rs`: geometría de filas,
  selección, scroll, tabuladores, corte de renglones y mezcla de colores.

## Diferencias con el original

Las que afectan a quien usa la API:

- **Hay "Discard hunk" y acciones por línea**, que el original no tenía (solo
  "Stage hunk" / "Unstage hunk").
- **`BlameViewer` no pide el blame**: recibe `BlameState`.
- **`DiffPanel` no monta el `ConflictResolver`**: lo recibe por `.conflict(..)`.

De aspecto y comportamiento:

- La nota de "Ignore spaces" del panel va **debajo** del visor. En el original
  quedaba fuera de la vista, a la derecha (un `p` hermano del visor dentro de un
  flex en fila), y provocaba un scroll horizontal en el panel.
- El contenido de archivo va **coloreado**. En el original las reglas `.hljs-*`
  vivían en el `<style scoped>` de `FileDiffViewer.vue` y no alcanzaban a
  `FileContentViewer.vue`, que por eso salía sin color pese a calcularlo (ver
  `reference/*-214-repo-tree-file-content.png`).
- El coloreado es de tree-sitter, no de highlight.js: los grupos de color son
  los mismos, pero algún token puede caer en otro grupo (sobre todo fuera de
  JS/TS, CSS, JSON, YAML, Markdown y HTML, que son los que se ajustaron contra
  las capturas). Kotlin, Swift, C#, ini/toml y Dockerfile usan un reconocedor de
  línea propio (gpui-kit 0.7 no trae consultas de resaltado para ellos).
- La primera línea que se colorea de cada lenguaje compila sus consultas de
  tree-sitter (entre 1 y 100 ms según el lenguaje, una vez por proceso).
- Las barras de scroll son las de gpui-kit, superpuestas al contenido; las del
  original ocupaban sus 10px.
- En la vista partida la selección se queda en un lado (en el navegador
  recorría los dos, intercalados). El texto se parte en renglones por espacios
  y, si una palabra no cabe, por donde toque; Chromium conoce además otros
  puntos de corte (tras un guion, etc.).
- El `title` con el asunto del commit sobre la columna de metadatos del blame
  no está: el lienzo no tiene tooltips por zona.
- El botón de bloque se enseña al pasar por su cabecera o al enfocarlo con el
  teclado, sin la transición de opacidad de 120 ms.
- Los finales de línea de Windows (`\r`) no se pintan ni se copian.
