//! Resolvedor de conflictos de tres vías (`ConflictResolver.vue`, ampliado).
//!
//! Lee las tres versiones del índice (`:1:` base, `:2:` actual, `:3:`
//! entrante; ver [`mergeon_git_query::read_conflict_versions`]) y las fusiona
//! línea a línea ([`crate::conflict_merge`]): lo que cambió un solo lado entra
//! solo, y cada conflicto se decide con las casillas por bloque y por línea de
//! los paneles "Current (ours)" e "Incoming (theirs)", con los botones del
//! conflicto actual, o escribiendo en el resultado, que es un editor. La base
//! se enseña como tercer panel plegable. Sin etapas en el índice (archivo ya
//! preparado, conflicto ajeno) los conflictos salen de los marcadores.
//!
//! Los binarios y los archivos borrados por un lado no se fusionan: se elige
//! una versión entera o borrar el archivo.
//!
//! No toca git ni el disco: quien lo aloja le da las versiones y recibe
//! [`ConflictResolverEvent`]. El análisis, la sincronización de lo editado a
//! mano y la preparación del texto al guardar van en hilos de fondo; el
//! pintado solo recorre las filas visibles.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::input::{
    Editor, EditorState, InputEvent, Position, RangeDecoration, RangeDecorationCollection,
    RangeDecorationStyle, Rope, RopeExt as _,
};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ListHorizontalSizingBehavior, ParentElement as _, Render,
    ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task,
    TestSupportExt as _, UniformListScrollHandle, WeakEntity, Window, div, point, px, relative,
    rems, transparent_black, uniform_list,
};
use mergeon_git_ops::{ConflictResolution, ConflictSide};
use mergeon_git_query::{ConflictKind, ConflictSides, ConflictVersions};
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{ActivePaletteExt as _, Lucide};
use mergeon_view_diff::code_font_family;

use crate::conflict_merge::{
    ConflictChoice, DocumentKind, LineTone, MergeDocument, Pane, PaneRow, Pick, analyze,
    count_marker_lines, finish_result, resync,
};

/// Lo que el resolvedor pide a quien lo aloja.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictResolverEvent {
    /// "Mark as resolved". Con `warning`, hay que confirmarlo antes (quedan
    /// conflictos o marcadores en el resultado, o se va a borrar el archivo).
    Save {
        resolution: ConflictResolution,
        warning: Option<String>,
    },
    /// Clic en otro archivo de la lista.
    SelectFile(String),
}

/// Lo elegido en un archivo que se resuelve entero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WholeChoice {
    Keep(ConflictSide),
    Delete,
}

impl WholeChoice {
    fn value(self) -> &'static str {
        match self {
            Self::Keep(ConflictSide::Ours) => "keep-ours",
            Self::Keep(ConflictSide::Theirs) => "keep-theirs",
            Self::Keep(ConflictSide::Base) => "keep-base",
            Self::Delete => "delete",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "keep-ours" => Self::Keep(ConflictSide::Ours),
            "keep-theirs" => Self::Keep(ConflictSide::Theirs),
            "keep-base" => Self::Keep(ConflictSide::Base),
            "delete" => Self::Delete,
            _ => return None,
        })
    }
}

/// Lo que espera tras teclear en el resultado antes de recolocar los tramos.
const SYNC_DELAY: Duration = Duration::from_millis(250);
/// Líneas de contexto por encima del conflicto al saltar a él en el resultado.
const REVEAL_CONTEXT_LINES: u32 = 4;
/// Alto de una fila de los paneles: Geist Mono 12/20.
const ROW_HEIGHT_REMS: f32 = 1.25;

/// El estado de un archivo: su análisis, lo decidido y dónde está cada tramo
/// en el resultado.
struct FileState {
    document: Arc<MergeDocument>,
    /// Las versiones de las que sale `document`.
    versions: Arc<ConflictVersions>,
    picks: Vec<Pick>,
    /// Conflictos editados a mano: `Some(quedan_marcadores)`.
    edited: Vec<Option<bool>>,
    resolved: usize,
    whole: Option<WholeChoice>,
    current: usize,
    /// El texto del editor la última vez que se supo dónde empieza cada tramo.
    synced: Rope,
    /// Línea de inicio de cada tramo en `synced` y, al final, el total.
    starts: Vec<usize>,
    /// El usuario decidió o escribió algo (si no, se puede volver a leer).
    touched: bool,
}

impl FileState {
    fn is_resolved(&self, conflict: usize) -> bool {
        match self.edited.get(conflict).copied().flatten() {
            Some(markers) => !markers,
            None => self
                .picks
                .get(conflict)
                .is_some_and(|pick| *pick != Pick::Unresolved),
        }
    }

    fn recount(&mut self) {
        self.resolved = (0..self.picks.len())
            .filter(|&conflict| self.is_resolved(conflict))
            .count();
    }
}

/// Resolvedor de conflictos de un archivo. Es una entidad: guarda lo decidido
/// en cada archivo mientras se salta de uno a otro.
///
/// ```ignore
/// let resolver = cx.new(|cx| ConflictResolver::new(window, cx));
/// cx.subscribe(&resolver, |this, _, event: &ConflictResolverEvent, cx| match event {
///     ConflictResolverEvent::Save { resolution, warning } => this.resolve(resolution, warning, cx),
///     ConflictResolverEvent::SelectFile(path) => this.select_conflict_file(path, cx),
/// });
///
/// // cuando cambian los datos (barato si no cambiaron):
/// resolver.update(cx, |resolver, cx| {
///     resolver.set_files(pending_files, cx);
///     resolver.set_counts(pending, resolved, total, cx);
///     resolver.set_versions(path, versions, cx);
///     resolver.set_busy(resolving, cx);
/// });
/// ```
pub struct ConflictResolver {
    files: Vec<SharedString>,
    selected_file: SharedString,
    pending_count: usize,
    resolved_count: usize,
    total_count: usize,
    busy: bool,
    /// Enseñar la lista de archivos en conflicto (el anfitrión la quita si ya
    /// enseña la suya).
    show_files: bool,
    /// La operación en curso es un rebase: `:2:` es aquello sobre lo que se
    /// rebasa ("ONTO") y `:3:` el commit propio que se reaplica ("YOUR COMMIT").
    rebase: bool,
    /// Calculando el texto a guardar.
    saving: bool,
    versions: Option<Arc<ConflictVersions>>,
    /// El contenido que llegó por [`Self::set_file`] (sin etapas).
    legacy_content: SharedString,
    load_error: Option<SharedString>,
    loading: bool,
    load_generation: u64,
    file: Option<FileState>,
    /// Lo decidido en los archivos que no están a la vista, con su texto.
    drafts: HashMap<String, (FileState, Rope)>,
    /// Texto que el editor debe tomar en cuanto haya ventana.
    pending_text: Option<String>,
    editor: Entity<EditorState>,
    decorations: Option<RangeDecorationCollection>,
    edit_generation: u64,
    dirty: bool,
    show_base: bool,
    ours_scroll: UniformListScrollHandle,
    theirs_scroll: UniformListScrollHandle,
    base_scroll: UniformListScrollHandle,
    /// La lista de archivos en conflicto (virtual: un merge grande puede
    /// dejar cientos).
    files_scroll: UniformListScrollHandle,
    _load_task: Option<Task<()>>,
    _sync_task: Option<Task<()>>,
    _save_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ConflictResolverEvent> for ConflictResolver {}

impl ConflictResolver {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .line_number(true)
                .soft_wrap(false)
                .folding(false)
        });
        let subscription = cx.subscribe(&editor, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.schedule_sync(cx);
            }
        });
        Self {
            files: Vec::new(),
            selected_file: SharedString::default(),
            pending_count: 0,
            resolved_count: 0,
            total_count: 0,
            busy: false,
            show_files: true,
            rebase: false,
            saving: false,
            versions: None,
            legacy_content: SharedString::default(),
            load_error: None,
            loading: false,
            load_generation: 0,
            file: None,
            drafts: HashMap::new(),
            pending_text: None,
            editor,
            decorations: None,
            edit_generation: 0,
            dirty: false,
            show_base: false,
            ours_scroll: UniformListScrollHandle::new(),
            theirs_scroll: UniformListScrollHandle::new(),
            base_scroll: UniformListScrollHandle::new(),
            files_scroll: UniformListScrollHandle::new(),
            _load_task: None,
            _sync_task: None,
            _save_task: None,
            _subscriptions: vec![subscription],
        }
    }

    // ------------------------------------------------------------------
    // Datos
    // ------------------------------------------------------------------

    /// Los archivos que siguen en conflicto.
    pub fn set_files(
        &mut self,
        files: impl IntoIterator<Item = impl Into<SharedString>>,
        cx: &mut Context<Self>,
    ) {
        let files: Vec<SharedString> = files.into_iter().map(Into::into).collect();
        if files != self.files {
            self.files = files;
            cx.notify();
        }
    }

    /// Archivos pendientes, resueltos y totales de la operación.
    pub fn set_counts(
        &mut self,
        pending_count: usize,
        resolved_count: usize,
        total_count: usize,
        cx: &mut Context<Self>,
    ) {
        let counts = (pending_count, resolved_count, total_count);
        if counts != (self.pending_count, self.resolved_count, self.total_count) {
            (self.pending_count, self.resolved_count, self.total_count) = counts;
            cx.notify();
        }
    }

    /// Se está guardando la resolución.
    pub fn set_busy(&mut self, busy: bool, cx: &mut Context<Self>) {
        if self.busy != busy {
            self.busy = busy;
            cx.notify();
        }
    }

    /// Enseñar (o no) la lista de archivos en conflicto del propio
    /// resolvedor.
    /// La operación en curso es un rebase (cambia el nombre de los paneles).
    pub fn set_rebase(&mut self, rebase: bool, cx: &mut Context<Self>) {
        if self.rebase != rebase {
            self.rebase = rebase;
            cx.notify();
        }
    }

    /// Los títulos de los paneles de las dos versiones, con quién es cada
    /// lado (ver [`pane_title`]).
    pub fn pane_titles(&self) -> Option<(String, String)> {
        let sides = &self.file.as_ref()?.versions.sides;
        Some((
            pane_title(Pane::Ours, sides, self.rebase),
            pane_title(Pane::Theirs, sides, self.rebase),
        ))
    }

    pub fn set_show_files(&mut self, show_files: bool, cx: &mut Context<Self>) {
        if self.show_files != show_files {
            self.show_files = show_files;
            cx.notify();
        }
    }

    /// El error al leer las versiones, si lo hubo.
    pub fn set_load_error(&mut self, error: Option<SharedString>, cx: &mut Context<Self>) {
        if self.load_error != error {
            self.load_error = error;
            cx.notify();
        }
    }

    /// El archivo elegido y sus versiones (`None` mientras se leen). Barato si
    /// no cambió nada: se llama en cada pintado del anfitrión.
    ///
    /// Al cambiar de archivo se guarda lo decidido en el que se deja y se
    /// recupera lo del que llega. Unas versiones nuevas del mismo archivo
    /// solo se vuelven a analizar si el usuario no ha tocado nada.
    pub fn set_versions(
        &mut self,
        selected_file: impl Into<SharedString>,
        versions: Option<Arc<ConflictVersions>>,
        cx: &mut Context<Self>,
    ) {
        let selected_file: SharedString = selected_file.into().trim().to_string().into();
        if selected_file != self.selected_file {
            self.leave_file(cx);
            self.selected_file = selected_file.clone();
            self.versions = None;
            // El elegido queda a la vista en la lista (virtual) de archivos.
            if let Some(index) = self.files.iter().position(|path| *path == selected_file) {
                self.files_scroll
                    .scroll_to_item(index, ScrollStrategy::Nearest);
            }
            if let Some((file, text)) = self.drafts.remove(selected_file.as_ref()) {
                self.versions = Some(file.versions.clone());
                self.install(file, text.to_string(), cx);
            }
            cx.notify();
        }

        let Some(versions) = versions else {
            return;
        };
        let unchanged = self
            .versions
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &versions) || **current == *versions);
        if unchanged {
            self.versions = Some(versions);
            return;
        }
        self.versions = Some(versions.clone());
        if self.file.as_ref().is_some_and(|file| file.touched) {
            return;
        }
        self.analyze(versions, cx);
    }

    /// El archivo y su contenido con marcadores, sin etapas del índice (lo
    /// que pasaba el anfitrión antes de leer las versiones).
    pub fn set_file(
        &mut self,
        selected_file: impl Into<SharedString>,
        content: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let selected_file: SharedString = selected_file.into();
        let content: SharedString = content.into();
        if selected_file == self.selected_file && content == self.legacy_content {
            return;
        }
        self.legacy_content = content.clone();
        let versions = ConflictVersions {
            path: selected_file.to_string(),
            working: Some(content.to_string()),
            ..ConflictVersions::default()
        };
        self.set_versions(selected_file, Some(Arc::new(versions)), cx);
    }

    /// Vuelve al estado inicial: sin archivo y sin borradores.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        let pristine = self.selected_file.is_empty()
            && self.file.is_none()
            && self.files.is_empty()
            && self.drafts.is_empty()
            && self.versions.is_none();
        if pristine {
            return;
        }
        self.files.clear();
        self.selected_file = SharedString::default();
        self.legacy_content = SharedString::default();
        self.versions = None;
        self.file = None;
        self.drafts.clear();
        self.busy = false;
        self.saving = false;
        self.loading = false;
        self.load_generation += 1;
        self._load_task = None;
        self.pending_text = Some(String::new());
        cx.notify();
    }

    /// Guarda lo decidido en el archivo que se deja, si se decidió algo.
    fn leave_file(&mut self, cx: &mut Context<Self>) {
        self.flush_sync(cx);
        self.load_generation += 1;
        self._load_task = None;
        self.loading = false;
        if let Some(file) = self.file.take()
            && file.touched
        {
            let text = self.editor.read(cx).text().clone();
            self.drafts
                .insert(self.selected_file.to_string(), (file, text));
        }
    }

    /// Analiza `versions` en un hilo de fondo y lo instala al terminar.
    fn analyze(&mut self, versions: Arc<ConflictVersions>, cx: &mut Context<Self>) {
        self.load_generation += 1;
        let generation = self.load_generation;
        self.loading = true;
        cx.notify();
        let work = cx.background_spawn({
            let versions = versions.clone();
            async move {
                let document = analyze(&versions);
                let (text, starts) = document.initial_result();
                let rope = Rope::from(text.as_str());
                (document, text, starts, rope)
            }
        });
        self._load_task = Some(cx.spawn(async move |this, cx| {
            let (document, text, starts, synced) = work.await;
            this.update(cx, |this, cx| {
                if generation != this.load_generation {
                    return;
                }
                this.loading = false;
                let conflicts = document.conflict_count();
                let whole = default_whole(&document);
                let file = FileState {
                    document: Arc::new(document),
                    versions,
                    picks: vec![Pick::Unresolved; conflicts],
                    edited: vec![None; conflicts],
                    resolved: 0,
                    whole,
                    current: 0,
                    synced,
                    starts,
                    touched: false,
                };
                this.install(file, text, cx);
            })
            .ok();
        }));
    }

    fn install(&mut self, mut file: FileState, text: String, cx: &mut Context<Self>) {
        file.recount();
        file.current = file
            .current
            .min(file.document.conflict_count().saturating_sub(1));
        self.file = Some(file);
        self.dirty = false;
        self.edit_generation += 1;
        self._sync_task = None;
        self.pending_text = Some(text);
        self.reveal_current(None, cx);
        cx.notify();
    }

    /// Pasa al editor el texto pendiente (necesita la ventana).
    fn apply_pending_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.pending_text.take() else {
            return;
        };
        self.editor
            .update(cx, |editor, cx| editor.set_value(text, window, cx));
        if let Some(file) = &mut self.file {
            // Lo que tiene ahora el editor es lo sincronizado.
            file.synced = self.editor.read(cx).text().clone();
        }
        self.refresh_decorations(cx);
    }

    // ------------------------------------------------------------------
    // Lectura
    // ------------------------------------------------------------------

    pub fn files(&self) -> &[SharedString] {
        &self.files
    }

    pub fn selected_file(&self) -> &SharedString {
        &self.selected_file
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// Se están leyendo o analizando las versiones.
    pub fn is_loading(&self) -> bool {
        self.loading || (self.file.is_none() && self.load_error.is_none())
    }

    /// El análisis del archivo a la vista.
    pub fn document(&self) -> Option<&Arc<MergeDocument>> {
        self.file.as_ref().map(|file| &file.document)
    }

    /// El editor del resultado.
    pub fn editor(&self) -> &Entity<EditorState> {
        &self.editor
    }

    /// Conflictos del archivo (los tramos que hay que decidir).
    pub fn conflict_count(&self) -> usize {
        self.file
            .as_ref()
            .map_or(0, |file| file.document.conflict_count())
    }

    /// Conflictos ya decididos (con un botón, con las casillas o a mano).
    pub fn resolved_conflicts(&self) -> usize {
        self.file.as_ref().map_or(0, |file| file.resolved)
    }

    /// Conflictos del archivo todavía sin decidir.
    pub fn unresolved_count(&self) -> usize {
        self.conflict_count() - self.resolved_conflicts()
    }

    pub fn has_unresolved(&self) -> bool {
        self.unresolved_count() > 0
    }

    /// El conflicto que señalan la navegación y los botones.
    pub fn current_conflict(&self) -> usize {
        self.file.as_ref().map_or(0, |file| file.current)
    }

    /// Lo decidido en el conflicto `conflict`.
    pub fn pick(&self, conflict: usize) -> Option<&Pick> {
        self.file.as_ref()?.picks.get(conflict)
    }

    pub fn is_resolved(&self, conflict: usize) -> bool {
        self.file
            .as_ref()
            .is_some_and(|file| file.is_resolved(conflict))
    }

    /// Lo elegido en un archivo que se resuelve entero.
    pub fn whole_choice(&self) -> Option<WholeChoice> {
        self.file.as_ref()?.whole
    }

    /// Las opciones del archivo actual si se resuelve entero (vacío si se
    /// fusiona por líneas o aún no está leído).
    pub fn whole_options(&self) -> Vec<WholeOption> {
        self.file
            .as_ref()
            .filter(|file| file.document.kind == DocumentKind::Whole)
            .map(|file| whole_options(&file.document, &file.versions.sides))
            .unwrap_or_default()
    }

    /// Título y explicación del archivo actual si se resuelve entero.
    pub fn whole_text(&self) -> Option<(String, String)> {
        self.file
            .as_ref()
            .filter(|file| file.document.kind == DocumentKind::Whole)
            .map(|file| whole_text(&file.document, &file.versions.sides))
    }

    pub fn is_base_shown(&self) -> bool {
        self.show_base
    }

    /// El texto del resultado tal como está en el editor (o el que va a tomar).
    pub fn result_text(&self, cx: &App) -> String {
        match &self.pending_text {
            Some(text) => text.clone(),
            None => self.editor.read(cx).text().to_string(),
        }
    }

    /// El texto de la barra: "2 of 5 resolved", o qué falta elegir.
    pub fn status_text(&self) -> SharedString {
        let Some(file) = &self.file else {
            return if self.load_error.is_some() {
                "Could not read the conflict".into()
            } else {
                "Loading…".into()
            };
        };
        match file.document.kind {
            DocumentKind::Whole => match file.whole {
                None => "Choose a version to keep".into(),
                Some(_) => "Ready to mark as resolved".into(),
            },
            DocumentKind::Text if file.document.conflict_count() == 0 => "No conflicts left".into(),
            DocumentKind::Text => format!(
                "{} of {} resolved",
                file.resolved,
                file.document.conflict_count()
            )
            .into(),
        }
    }

    // ------------------------------------------------------------------
    // Acciones
    // ------------------------------------------------------------------

    /// Un botón de conflicto: "Take current", "Take incoming", los dos en un
    /// orden u otro, o la base.
    pub fn choose(
        &mut self,
        conflict: usize,
        choice: ConflictChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = &self.file else {
            return;
        };
        if choice == ConflictChoice::Base && !file.document.has_base {
            return;
        }
        let document = file.document.clone();
        let pick = choice.pick(
            document.side_len(conflict, Pane::Ours),
            document.side_len(conflict, Pane::Theirs),
        );
        self.set_pick(conflict, pick, window, cx);
    }

    /// La casilla de una línea de un lado de un conflicto.
    pub fn toggle_line(
        &mut self,
        conflict: usize,
        pane: Pane,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = &self.file else {
            return;
        };
        let (ours, theirs) = (
            file.document.side_len(conflict, Pane::Ours),
            file.document.side_len(conflict, Pane::Theirs),
        );
        let Some(pick) = file.picks.get(conflict).cloned() else {
            return;
        };
        let pick = pick.toggle_line(pane, offset, ours, theirs);
        self.set_pick(conflict, pick, window, cx);
    }

    /// La casilla de bloque de un lado de un conflicto.
    pub fn toggle_side(
        &mut self,
        conflict: usize,
        pane: Pane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = &self.file else {
            return;
        };
        let (ours, theirs) = (
            file.document.side_len(conflict, Pane::Ours),
            file.document.side_len(conflict, Pane::Theirs),
        );
        let Some(pick) = file.picks.get(conflict).cloned() else {
            return;
        };
        let pick = pick.toggle_side(pane, ours, theirs);
        self.set_pick(conflict, pick, window, cx);
    }

    /// "Reset": el conflicto vuelve a sus marcadores.
    pub fn reset_conflict(&mut self, conflict: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.set_pick(conflict, Pick::Unresolved, window, cx);
    }

    fn set_pick(
        &mut self,
        conflict: usize,
        pick: Pick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        self.apply_pending_text(window, cx);
        self.flush_sync(cx);
        let Some(file) = &mut self.file else {
            return;
        };
        let Some(&chunk) = file.document.conflicts.get(conflict) else {
            return;
        };
        let text = file.document.chunk_text(chunk, &pick);
        file.picks[conflict] = pick;
        file.edited[conflict] = None;
        file.current = conflict;
        file.touched = true;
        file.recount();

        let rope = self.editor.read(cx).text().clone();
        let start = rope.line_start_offset(file.starts[chunk]);
        let end = rope.line_start_offset(file.starts[chunk + 1]);
        let old_lines = rope.slice(start..end).to_string().matches('\n').count();
        let new_lines = text.matches('\n').count();
        self.editor.update(cx, |editor, cx| {
            editor.set_selected_range(start..end, cx);
            editor.replace(text, window, cx);
        });
        for start in &mut file.starts[chunk + 1..] {
            *start = (*start + new_lines).saturating_sub(old_lines);
        }
        file.synced = self.editor.read(cx).text().clone();
        self.edit_generation += 1;
        self.dirty = false;
        self.refresh_decorations(cx);
        cx.notify();
    }

    /// Lleva los paneles (y, si `window`, el resultado) al conflicto actual.
    fn reveal_current(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        let Some(file) = &self.file else {
            return;
        };
        let conflict = file.current;
        for (pane, scroll) in [
            (Pane::Ours, &self.ours_scroll),
            (Pane::Theirs, &self.theirs_scroll),
            (Pane::Base, &self.base_scroll),
        ] {
            if let Some(&row) = file.document.pane(pane).conflict_rows.get(conflict) {
                scroll.scroll_to_item(row, ScrollStrategy::Center);
            }
        }
        let Some(window) = window else {
            return;
        };
        let Some(&chunk) = file.document.conflicts.get(conflict) else {
            return;
        };
        let line = file.starts[chunk] as u32;
        self.editor.update(cx, |editor, cx| {
            editor.set_cursor_position(Position::new(line, 0), window, cx);
            // El conflicto arriba, con unas líneas de contexto (sin esto el
            // cursor queda en el borde de abajo).
            if let Some(line_height) = editor.line_height() {
                let top = line.saturating_sub(REVEAL_CONTEXT_LINES) as f32;
                editor.set_scroll_offset(point(px(0.), -(line_height * top)), cx);
            }
        });
    }

    /// Salta al conflicto `conflict`.
    pub fn go_to(&mut self, conflict: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_pending_text(window, cx);
        self.flush_sync(cx);
        let Some(file) = &mut self.file else {
            return;
        };
        if conflict >= file.document.conflict_count() {
            return;
        }
        file.current = conflict;
        self.reveal_current(Some(window), cx);
        cx.notify();
    }

    /// "Next conflict" (da la vuelta al final).
    pub fn next_conflict(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.conflict_count();
        if count > 0 {
            self.go_to((self.current_conflict() + 1) % count, window, cx);
        }
    }

    /// "Previous conflict" (da la vuelta al principio).
    pub fn previous_conflict(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.conflict_count();
        if count > 0 {
            self.go_to((self.current_conflict() + count - 1) % count, window, cx);
        }
    }

    /// Enseña u oculta el panel de la base.
    pub fn toggle_base(&mut self, cx: &mut Context<Self>) {
        self.show_base = !self.show_base;
        self.reveal_current(None, cx);
        cx.notify();
    }

    /// En un archivo que se resuelve entero: con qué versión quedarse.
    pub fn choose_whole(&mut self, choice: WholeChoice, cx: &mut Context<Self>) {
        if let Some(file) = &mut self.file
            && file.document.kind == DocumentKind::Whole
        {
            file.whole = Some(choice);
            file.touched = true;
            cx.notify();
        }
    }

    /// "Mark as resolved". En texto, el resultado se prepara en un hilo de
    /// fondo (finales de línea, salto final y marcadores que queden) antes
    /// de emitir [`ConflictResolverEvent::Save`].
    pub fn save(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.saving {
            return;
        }
        let Some(file) = &self.file else {
            return;
        };
        let path = file.document.path.clone();
        if file.document.kind == DocumentKind::Whole {
            let Some(choice) = file.whole else {
                return;
            };
            // Lo que borraron los dos ya no está en el working tree: no hay
            // nada que confirmar.
            let gone = file.document.conflict_kind == ConflictKind::BothDeleted;
            let (resolution, warning) = match choice {
                WholeChoice::Keep(side) => (ConflictResolution::Keep(side), None),
                WholeChoice::Delete => (
                    ConflictResolution::Delete,
                    (!gone).then(|| {
                        format!("{path} will be deleted and the conflict marked as resolved.")
                    }),
                ),
            };
            cx.emit(ConflictResolverEvent::Save {
                resolution,
                warning,
            });
            return;
        }

        self.flush_sync(cx);
        let Some(file) = &self.file else {
            return;
        };
        let unresolved = file.document.conflict_count() - file.resolved;
        let (ending, final_newline) = (file.document.line_ending, file.document.final_newline);
        let pending = self.pending_text.clone();
        let rope = self.editor.read(cx).text().clone();
        self.saving = true;
        cx.notify();
        let work = cx.background_spawn(async move {
            let text = pending.unwrap_or_else(|| rope.to_string());
            (
                finish_result(&text, ending, final_newline),
                count_marker_lines(&text),
            )
        });
        self._save_task = Some(cx.spawn(async move |this, cx| {
            let (content, markers) = work.await;
            this.update(cx, |this, cx| {
                this.saving = false;
                cx.notify();
                let warning = save_warning(&path, unresolved, markers);
                cx.emit(ConflictResolverEvent::Save {
                    resolution: ConflictResolution::Content(content),
                    warning,
                });
            })
            .ok();
        }));
    }

    /// Clic en un archivo de la lista.
    pub fn select_file(&mut self, path: &str, cx: &mut Context<Self>) {
        cx.emit(ConflictResolverEvent::SelectFile(path.to_string()));
    }

    // ------------------------------------------------------------------
    // Resultado editado a mano
    // ------------------------------------------------------------------

    /// El editor cambió: dentro de un momento se recoloca cada tramo en el
    /// texto nuevo (en un hilo de fondo).
    fn schedule_sync(&mut self, cx: &mut Context<Self>) {
        let Some(file) = &self.file else {
            return;
        };
        if self.pending_text.is_some() {
            return;
        }
        // Comparar textos es lineal: eso (y todo lo demás) va al hilo de fondo.
        let new = self.editor.read(cx).text().clone();
        self.edit_generation += 1;
        self.dirty = true;
        let generation = self.edit_generation;
        let old = file.synced.clone();
        let starts = file.starts.clone();
        let executor = cx.background_executor().clone();
        self._sync_task = Some(cx.spawn(async move |this, cx| {
            executor.timer(SYNC_DELAY).await;
            let target = new.clone();
            let sync = executor
                .spawn(async move {
                    (old != target).then(|| resync(&old.to_string(), &target.to_string(), &starts))
                })
                .await;
            this.update(cx, |this, cx| {
                if generation != this.edit_generation {
                    return;
                }
                match sync {
                    Some(sync) => this.apply_sync(new, sync, cx),
                    // Era el cambio que hizo el propio resolvedor.
                    None => this.dirty = false,
                }
            })
            .ok();
        }));
    }

    /// Si hay ediciones sin recolocar, se recolocan ya (antes de tocar un
    /// tramo o de guardar).
    fn flush_sync(&mut self, cx: &mut Context<Self>) {
        if !self.dirty {
            return;
        }
        let Some(file) = &self.file else {
            return;
        };
        let new = self.editor.read(cx).text().clone();
        self._sync_task = None;
        if new == file.synced {
            self.dirty = false;
            return;
        }
        let sync = resync(&file.synced.to_string(), &new.to_string(), &file.starts);
        self.apply_sync(new, sync, cx);
    }

    fn apply_sync(
        &mut self,
        text: Rope,
        sync: crate::conflict_merge::Resync,
        cx: &mut Context<Self>,
    ) {
        self.dirty = false;
        let Some(file) = &mut self.file else {
            return;
        };
        for (conflict, &chunk) in file.document.conflicts.iter().enumerate() {
            if sync.touched[chunk] {
                file.edited[conflict] = Some(sync.markers[chunk]);
            }
        }
        if sync.touched.iter().any(|touched| *touched) {
            file.touched = true;
        }
        file.starts = sync.starts;
        file.synced = text;
        file.recount();
        self.refresh_decorations(cx);
        cx.notify();
    }

    /// El fondo de cada conflicto en el resultado: ámbar si sigue sin
    /// decidir, verde si ya está.
    fn refresh_decorations(&mut self, cx: &mut Context<Self>) {
        let Some(file) = &self.file else {
            return;
        };
        let palette = *cx.palette();
        let rope = self.editor.read(cx).text().clone();
        let decorations: Vec<RangeDecoration> = file
            .document
            .conflicts
            .iter()
            .enumerate()
            .filter_map(|(conflict, &chunk)| {
                let start = rope.line_start_offset(file.starts[chunk]);
                let end = rope.line_start_offset(file.starts[chunk + 1]);
                (end > start).then(|| {
                    let color = if file.is_resolved(conflict) {
                        with_alpha(palette.success, 0.12)
                    } else {
                        with_alpha(palette.warning, 0.16)
                    };
                    RangeDecoration::new(start..end)
                        .with_style(RangeDecorationStyle::Fill)
                        .with_color(color)
                })
            })
            .collect();
        if let Some(collection) = &self.decorations {
            collection.set(decorations, cx);
            return;
        }
        let collection = self.editor.update(cx, |editor, cx| {
            editor.create_range_decorations_collection(decorations, cx)
        });
        self.decorations = Some(collection);
    }

    // ------------------------------------------------------------------
    // Pintado
    // ------------------------------------------------------------------

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let soft_border = with_alpha(palette.border, 0.55);

        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w(px(220.))
            .h_full()
            .border_r_1()
            .border_color(palette.border)
            .bg(palette.secondary)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .gap(rems(0.2))
                    .py(rems(0.85))
                    .px(rems(1.))
                    .border_b_1()
                    .border_color(palette.border)
                    .text_color(palette.secondary_foreground)
                    .child(div().font_weight(FontWeight::BOLD).child("Conflicts"))
                    .child(format!(
                        "{} / {} files resolved",
                        self.resolved_count, self.total_count
                    ))
                    .child(
                        div()
                            .text_color(palette.muted_foreground)
                            .child(format!("{} pending", self.pending_count)),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child({
                        // Solo se construyen las filas a la vista: con cientos
                        // de archivos en conflicto, la lista entera por frame
                        // costaba aunque no se moviera nada.
                        let files = self.files.clone();
                        let selected = self.selected_file.clone();
                        let this = cx.entity().downgrade();
                        uniform_list("conflict-files", files.len(), move |range, _, _| {
                            range
                                .filter_map(|ix| files.get(ix))
                                .map(|path| {
                                    let active = *path == selected;
                                    let target = path.clone();
                                    let this = this.clone();
                                    div()
                                        .id(SharedString::from(format!("conflict-file:{path}")))
                                        .test_support()
                                        .aria_label(path.clone())
                                        .flex()
                                        .flex_shrink_0()
                                        .gap(rems(0.5))
                                        .py(rems(0.6))
                                        .px(rems(0.85))
                                        .border_b_1()
                                        .border_color(soft_border)
                                        .cursor_pointer()
                                        .map(|this| {
                                            if active {
                                                this.bg(palette.warning_soft)
                                                    .text_color(palette.warning_soft_foreground)
                                            } else {
                                                this.text_color(palette.secondary_foreground)
                                                    .hover(|this| this.bg(palette.bg_hover))
                                            }
                                        })
                                        .on_click(move |_, _, cx| {
                                            this.update(cx, |this, cx| {
                                                this.select_file(&target, cx)
                                            })
                                            .ok();
                                        })
                                        .child(div().flex_shrink_0().child(if active {
                                            "●"
                                        } else {
                                            "○"
                                        }))
                                        // Todas las filas miden lo mismo (lo
                                        // pide la lista virtual): una ruta
                                        // larga se recorta por delante, donde
                                        // menos dice.
                                        .child(
                                            div()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis_start()
                                                .child(path.clone()),
                                        )
                                        .into_any_element()
                                })
                                .collect()
                        })
                        .size_full()
                        .track_scroll(&self.files_scroll)
                    })
                    .vertical_scrollbar(&self.files_scroll),
            )
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let text_mode = self
            .file
            .as_ref()
            .is_some_and(|file| file.document.kind == DocumentKind::Text);
        let conflicts = self.conflict_count();
        let can_save = match &self.file {
            None => false,
            Some(file) => match file.document.kind {
                DocumentKind::Whole => file.whole.is_some(),
                DocumentKind::Text => true,
            },
        } && !self.busy
            && !self.saving;
        let status_color = match &self.file {
            None => palette.muted_foreground,
            Some(file) => {
                let done = match file.document.kind {
                    DocumentKind::Whole => file.whole.is_some(),
                    DocumentKind::Text => file.resolved == file.document.conflict_count(),
                };
                if done {
                    palette.success
                } else {
                    palette.warning
                }
            }
        };
        let kind = self.file.as_ref().map(|file| kind_label(&file.document));

        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .gap(rems(1.))
            .py(rems(0.5))
            .px(rems(0.75))
            .bg(palette.secondary)
            .border_b_1()
            .border_color(palette.border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(
                        div()
                            .id("resolver-file-name")
                            .test_support()
                            .aria_label(SharedString::from(format!(
                                "Resolve Conflicts: {}",
                                self.selected_file
                            )))
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(palette.foreground)
                            .child(format!("Resolve Conflicts: {}", self.selected_file)),
                    )
                    .when_some(kind, |this, kind| {
                        this.child(
                            div()
                                .id("resolver-kind")
                                .test_support()
                                .aria_label(kind.clone())
                                .truncate()
                                .text_size(rems(0.6875))
                                .text_color(palette.muted_foreground)
                                .child(kind),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(rems(0.5))
                    .child(
                        div()
                            .id("resolver-status")
                            .test_support()
                            .aria_label(self.status_text())
                            .mr(rems(0.25))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(status_color)
                            .child(self.status_text()),
                    )
                    .when(text_mode, |this| {
                        this.child(
                            mergeon_ui::button("conflict-prev")
                                .icon(Lucide::ChevronUp)
                                .ghost()
                                .sm()
                                .tooltip("Previous conflict")
                                .disabled(conflicts == 0)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.previous_conflict(window, cx)
                                })),
                        )
                        .child(
                            mergeon_ui::button("conflict-next")
                                .icon(Lucide::ChevronDown)
                                .ghost()
                                .sm()
                                .tooltip("Next conflict")
                                .disabled(conflicts == 0)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.next_conflict(window, cx)
                                })),
                        )
                        .child(
                            mergeon_ui::button("toggle-base")
                                .label("Base")
                                .outline()
                                .sm()
                                .selected(self.show_base)
                                .disabled(!self.file.as_ref().is_some_and(|f| f.document.has_base))
                                .tooltip("Show the common ancestor")
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_base(cx))),
                        )
                    })
                    .child(
                        mergeon_ui::button("mark-resolved")
                            .label(if self.busy || self.saving {
                                "Saving…"
                            } else {
                                "Mark as resolved"
                            })
                            .sm()
                            .icon(Lucide::Check)
                            .loading(self.busy || self.saving)
                            .disabled(!can_save)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
            )
    }

    /// Un panel de versión: cabecera y lista virtual de filas.
    fn render_pane(&self, pane: Pane, file: &FileState, cx: &mut Context<Self>) -> AnyElement {
        let palette = *cx.palette();
        let data = file.document.pane(pane);
        let (scroll, id) = match pane {
            Pane::Ours => (&self.ours_scroll, "pane-ours"),
            Pane::Theirs => (&self.theirs_scroll, "pane-theirs"),
            Pane::Base => (&self.base_scroll, "pane-base"),
        };
        let title: SharedString = pane_title(pane, &file.versions.sides, self.rebase).into();
        let this = cx.entity().downgrade();
        let count = data.rows.len();
        let body: AnyElement = if !data.present {
            div()
                .p(rems(1.))
                .text_color(palette.muted_foreground)
                .child(match pane {
                    Pane::Base => "No common ancestor: both sides added the file.",
                    _ => "Deleted on this side.",
                })
                .into_any_element()
        } else {
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .font_family(code_font_family(cx))
                .text_size(rems(0.75))
                .line_height(relative(20. / 12.))
                .child(
                    uniform_list(id, count, move |range, _, cx| {
                        let Some(resolver) = this.upgrade() else {
                            return Vec::new();
                        };
                        resolver.read(cx).render_pane_rows(pane, range, &this, cx)
                    })
                    .track_scroll(scroll)
                    .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                    .with_width_from_item(Some(data.widest_row))
                    .size_full(),
                )
                .vertical_scrollbar(scroll)
                .into_any_element()
        };

        div()
            .id(SharedString::from(format!("{id}-frame")))
            .test_support()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(
                div()
                    .id(SharedString::from(format!("{id}-title")))
                    .test_support()
                    .aria_label(title.clone())
                    // Quién es, dicho en una frase ("main (current)").
                    .tooltip(mergeon_ui::tooltip(side_subject(
                        &file.versions.sides,
                        match pane {
                            Pane::Ours => ConflictSide::Ours,
                            Pane::Theirs => ConflictSide::Theirs,
                            Pane::Base => ConflictSide::Base,
                        },
                    )))
                    .child(section_bar(
                        title,
                        format!("{} lines", data.lines.len()),
                        cx,
                    )),
            )
            .child(body)
            .into_any_element()
    }

    fn render_pane_rows(
        &self,
        pane: Pane,
        range: Range<usize>,
        this: &WeakEntity<Self>,
        cx: &App,
    ) -> Vec<AnyElement> {
        let Some(file) = &self.file else {
            return Vec::new();
        };
        let data = file.document.pane(pane);
        range
            .filter_map(|ix| data.rows.get(ix).map(|row| (ix, *row)))
            .map(|(ix, row)| match row {
                PaneRow::Header { conflict } => self.render_header(pane, conflict, file, this, cx),
                PaneRow::Line { line, tone } => {
                    self.render_line(pane, ix, line, tone, file, this, cx)
                }
            })
            .collect()
    }

    fn render_header(
        &self,
        pane: Pane,
        conflict: usize,
        file: &FileState,
        this: &WeakEntity<Self>,
        cx: &App,
    ) -> AnyElement {
        let palette = *cx.palette();
        let len = file.document.side_len(conflict, pane);
        let pick = &file.picks[conflict];
        let current = conflict == file.current;
        let resolved = file.is_resolved(conflict);
        let (checked, mixed) = pick.side_state(pane, len);
        let prefix = pane_prefix(pane);
        let summary = match len {
            0 => "no lines".to_string(),
            1 => "1 line".to_string(),
            n => format!("{n} lines"),
        };
        let take = match pane {
            Pane::Ours => ConflictChoice::Current,
            Pane::Theirs => ConflictChoice::Incoming,
            Pane::Base => ConflictChoice::Base,
        };
        let take_handle = this.clone();
        let go_handle = this.clone();
        let check_handle = this.clone();

        div()
            .id(SharedString::from(format!("{prefix}-header-{conflict}")))
            .test_support()
            .min_w_full()
            .h(rems(ROW_HEIGHT_REMS))
            .flex()
            .items_center()
            .gap(rems(0.5))
            .pl(rems(0.4))
            .pr(rems(0.5))
            .border_l_2()
            .border_color(if current {
                Hsla::from(palette.warning)
            } else {
                transparent_black()
            })
            .bg(if resolved {
                with_alpha(palette.success, 0.10)
            } else {
                palette.warning_soft.into()
            })
            .text_color(palette.warning_soft_foreground)
            .child(
                div()
                    .w(rems(1.1))
                    .flex_shrink_0()
                    .when(pane != Pane::Base && len > 0, |this| {
                        this.child(
                            mergeon_ui::checkbox(SharedString::from(format!(
                                "{prefix}-block-{conflict}"
                            )))
                            .checked(checked)
                            .mixed(mixed)
                            .on_change(move |_, window, cx| {
                                check_handle
                                    .update(cx, |this, cx| {
                                        this.toggle_side(conflict, pane, window, cx)
                                    })
                                    .ok();
                            }),
                        )
                    }),
            )
            .child(
                div()
                    .id(SharedString::from(format!("{prefix}-goto-{conflict}")))
                    .test_support()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .font_weight(FontWeight::SEMIBOLD)
                    .on_click(move |_, window, cx| {
                        go_handle
                            .update(cx, |this, cx| this.go_to(conflict, window, cx))
                            .ok();
                    })
                    .child(format!("Conflict {} · {summary}", conflict + 1)),
            )
            .when(pane != Pane::Base || file.document.has_base, |this| {
                this.child(
                    mergeon_ui::button(SharedString::from(format!("{prefix}-take-{conflict}")))
                        .label(take.label())
                        .xs()
                        .ghost()
                        .on_click(move |_, window, cx| {
                            take_handle
                                .update(cx, |this, cx| this.choose(conflict, take, window, cx))
                                .ok();
                        }),
                )
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_line(
        &self,
        pane: Pane,
        ix: usize,
        line: usize,
        tone: LineTone,
        file: &FileState,
        this: &WeakEntity<Self>,
        cx: &App,
    ) -> AnyElement {
        let palette = *cx.palette();
        let text = file.document.pane(pane).lines[line].clone();
        let prefix = pane_prefix(pane);
        let (background, checkbox) = match tone {
            LineTone::Plain => (None, None),
            LineTone::Changed => (
                Some(match pane {
                    Pane::Base => with_alpha(palette.muted_foreground, 0.08),
                    _ => palette.diff_add_bg.into(),
                }),
                None,
            ),
            LineTone::Conflict { conflict, offset } => {
                let included = file.picks[conflict].includes(pane, offset);
                let background = if included {
                    palette.diff_add_bg.into()
                } else {
                    with_alpha(palette.warning, 0.10)
                };
                let checkbox = (pane != Pane::Base).then(|| {
                    let handle = this.clone();
                    mergeon_ui::checkbox(SharedString::from(format!("{prefix}-line-{line}")))
                        .checked(included)
                        .on_change(move |_, window, cx| {
                            handle
                                .update(cx, |this, cx| {
                                    this.toggle_line(conflict, pane, offset, window, cx)
                                })
                                .ok();
                        })
                });
                (Some(background), checkbox)
            }
        };

        div()
            .id((prefix, ix))
            .test_support()
            .min_w_full()
            .h(rems(ROW_HEIGHT_REMS))
            .flex()
            .items_center()
            .whitespace_nowrap()
            .when_some(background, |this, background| this.bg(background))
            .child(
                div()
                    .w(rems(1.5))
                    .pl(rems(0.4))
                    .flex_shrink_0()
                    .children(checkbox),
            )
            .child(
                div()
                    .w(rems(3.))
                    .flex_shrink_0()
                    .pr(rems(0.75))
                    .text_right()
                    .text_color(palette.muted_foreground)
                    .child(SharedString::from((line + 1).to_string())),
            )
            .child(div().flex_shrink_0().pr(rems(1.)).child(text))
            .into_any_element()
    }

    /// Los botones del conflicto actual, entre los paneles y el resultado.
    fn render_actions(&self, file: &FileState, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let count = file.document.conflict_count();
        let conflict = file.current;
        let pick = file.picks.get(conflict);
        let (ours, theirs) = (
            file.document.side_len(conflict, Pane::Ours),
            file.document.side_len(conflict, Pane::Theirs),
        );
        div()
            .flex()
            .flex_shrink_0()
            .flex_wrap()
            .items_center()
            .gap(rems(0.4))
            .py(rems(0.4))
            .px(rems(0.75))
            .border_t_1()
            .border_b_1()
            .border_color(palette.border)
            .bg(palette.secondary)
            .child(
                div()
                    .id("current-conflict")
                    .test_support()
                    .aria_label(SharedString::from(format!(
                        "Conflict {} of {count}",
                        conflict + 1
                    )))
                    .mr(rems(0.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.foreground)
                    .child(if count == 0 {
                        "No conflicts: the result merges both sides".to_string()
                    } else {
                        format!("Conflict {} of {count}", conflict + 1)
                    }),
            )
            .when(count > 0, |this| {
                this.children(ConflictChoice::ALL.into_iter().map(|choice| {
                    let selected = pick == Some(&choice.pick(ours, theirs));
                    mergeon_ui::button(choice.id())
                        .label(choice.label())
                        .xs()
                        .outline()
                        .selected(selected)
                        .disabled(
                            self.busy
                                || (choice == ConflictChoice::Base && !file.document.has_base),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let conflict = this.current_conflict();
                            this.choose(conflict, choice, window, cx);
                        }))
                }))
                .child(
                    mergeon_ui::button("reset-conflict")
                        .label("Reset")
                        .xs()
                        .ghost()
                        .disabled(self.busy || pick == Some(&Pick::Unresolved))
                        .on_click(cx.listener(|this, _, window, cx| {
                            let conflict = this.current_conflict();
                            this.reset_conflict(conflict, window, cx);
                        })),
                )
            })
    }

    fn render_result(&self, file: &FileState, cx: &mut Context<Self>) -> impl IntoElement {
        let document = &file.document;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(section_bar(
                "Result",
                format!(
                    "editable · {}{}",
                    document.line_ending.label(),
                    if document.from_markers {
                        " · from conflict markers"
                    } else {
                        ""
                    }
                ),
                cx,
            ))
            .child(
                div()
                    .id("conflict-result")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .child(
                        Editor::new(&self.editor)
                            .bordered(false)
                            .readonly(self.busy)
                            .aria_label("Result")
                            .h_full(),
                    ),
            )
    }

    /// Un archivo que se resuelve entero: qué pasó, las opciones y lo que
    /// haya de texto para mirarlo.
    fn render_whole(&self, file: &FileState, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let document = &file.document;
        let sides = &file.versions.sides;
        let (title, description) = whole_text(document, sides);
        let options = whole_options(document, sides);
        let selected = file.whole.map(WholeChoice::value).unwrap_or_default();
        let handle = cx.entity().downgrade();
        let previews: Vec<Pane> = [Pane::Ours, Pane::Theirs]
            .into_iter()
            .filter(|&pane| !document.binary && document.pane(pane).present)
            .collect();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .gap(rems(0.6))
                    .p(rems(1.))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(palette.foreground)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_color(palette.muted_foreground)
                            .child(description),
                    )
                    .child(
                        mergeon_ui::radio_group("whole-choice")
                            .label("Resolution")
                            .options(options.into_iter().map(|option| {
                                let radio = mergeon_ui::RadioOption::new(
                                    option.choice.value(),
                                    option.label,
                                );
                                if option.detail.is_empty() {
                                    radio
                                } else {
                                    radio.description(option.detail)
                                }
                            }))
                            .selected(selected)
                            .on_change(move |value, _, cx| {
                                if let Some(choice) = WholeChoice::parse(value) {
                                    handle
                                        .update(cx, |this, cx| this.choose_whole(choice, cx))
                                        .ok();
                                }
                            }),
                    ),
            )
            .when(!previews.is_empty(), |this| {
                this.child(
                    div()
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .border_t_1()
                        .border_color(palette.border)
                        .children(
                            previews
                                .into_iter()
                                .map(|pane| self.render_pane(pane, file, cx)),
                        ),
                )
            })
    }

    fn render_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = *cx.palette();
        let Some(file) = &self.file else {
            let (title, description) = match &self.load_error {
                Some(error) => ("Could not read the conflict", error.clone()),
                None => (
                    "Reading the conflict…",
                    SharedString::from("Loading the base, current and incoming versions."),
                ),
            };
            return div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(
                    mergeon_ui::empty_state(title)
                        .description(description)
                        .icon(if self.load_error.is_some() {
                            Lucide::TriangleAlert
                        } else {
                            Lucide::LoaderCircle
                        })
                        .spin_icon(self.load_error.is_none()),
                )
                .into_any_element();
        };
        if file.document.kind == DocumentKind::Whole {
            return self.render_whole(file, cx).into_any_element();
        }
        let show_base = self.show_base && file.document.has_base;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_pane(Pane::Ours, file, cx))
                    .when(show_base, |this| {
                        this.child(div().w(px(1.)).h_full().bg(palette.border))
                            .child(self.render_pane(Pane::Base, file, cx))
                    })
                    .child(div().w(px(1.)).h_full().bg(palette.border))
                    .child(self.render_pane(Pane::Theirs, file, cx)),
            )
            .child(self.render_actions(file, cx))
            .child(self.render_result(file, cx))
            .into_any_element()
    }
}

/// Cabecera de un panel o del resultado, como las secciones de la lista de
/// cambios (Señal): etiqueta en mono, mayúsculas y apagada, y un dato a la
/// derecha.
fn section_bar(title: impl Into<SharedString>, detail: String, cx: &App) -> impl IntoElement {
    let palette = *cx.palette();
    let title: SharedString = title.into();
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .min_h(px(30.))
        .pl(px(14.))
        .pr(px(10.))
        .border_b_1()
        .border_color(palette.border_hair)
        .bg(palette.bg_inset)
        .font_family(code_font_family(cx))
        .text_size(px(10.5))
        .line_height(relative(1.4))
        .text_color(palette.text_3)
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .truncate()
                .child(title.to_uppercase()),
        )
        .child(div().flex_shrink_0().child(detail))
}

/// Prefijo de los ids de un panel.
fn pane_prefix(pane: Pane) -> &'static str {
    match pane {
        Pane::Base => "base",
        Pane::Ours => "ours",
        Pane::Theirs => "theirs",
    }
}

/// La clase de conflicto, para la barra.
fn kind_label(document: &MergeDocument) -> SharedString {
    let kind = match document.conflict_kind {
        ConflictKind::BothModified => "Modified on both sides",
        ConflictKind::BothAdded => "Added on both sides",
        ConflictKind::DeletedByUs => "Deleted in current, modified in incoming",
        ConflictKind::DeletedByThem => "Modified in current, deleted in incoming",
        ConflictKind::AddedByUs => "Only in current",
        ConflictKind::AddedByThem => "Only in incoming",
        ConflictKind::BothDeleted => "Deleted on both sides",
        ConflictKind::Unstaged => "From conflict markers",
    };
    if document.binary {
        format!("Binary · {kind}").into()
    } else {
        kind.into()
    }
}

/// El nombre corto de un lado ("main") o, si no se sabe, su papel
/// ("current" / "incoming").
fn side_short(sides: &ConflictSides, side: ConflictSide) -> String {
    match side {
        ConflictSide::Ours => sides.ours.clone().unwrap_or_else(|| "current".into()),
        ConflictSide::Theirs => sides.theirs.clone().unwrap_or_else(|| "incoming".into()),
        ConflictSide::Base => "the common ancestor".into(),
    }
}

/// La cabecera del panel de una versión: el papel del lado y quién es.
/// En un merge (o cherry-pick, revert) `CURRENT · main` e `INCOMING · feature`;
/// en un rebase `ONTO · main` (sobre lo que se rebasa) y `YOUR COMMIT ·
/// a1b2c3d` (el commit propio que se reaplica). Sin nombre, el papel y el
/// lado de siempre (`Current (ours)`), como [`side_subject`].
pub(crate) fn pane_title(pane: Pane, sides: &ConflictSides, rebase: bool) -> String {
    match pane {
        Pane::Base => "Base".into(),
        Pane::Ours => {
            let role = if rebase { "Onto" } else { "Current" };
            match &sides.ours {
                Some(name) => format!("{role} · {name}"),
                None if rebase => "Onto (ours)".into(),
                None => "Current (ours)".into(),
            }
        }
        Pane::Theirs => {
            if rebase {
                // `feature (a1b2c3d)` o `commit a1b2c3d`: el hash del commit.
                let commit = sides.theirs.as_deref().map(|name| {
                    name.rsplit_once('(')
                        .and_then(|(_, rest)| rest.strip_suffix(')'))
                        .or_else(|| name.strip_prefix("commit "))
                        .unwrap_or(name)
                        .to_string()
                });
                match commit {
                    Some(commit) => format!("Your commit · {commit}"),
                    None => "Your commit (theirs)".into(),
                }
            } else {
                match &sides.theirs {
                    Some(name) => format!("Incoming · {name}"),
                    None => "Incoming (theirs)".into(),
                }
            }
        }
    }
}

/// Un lado como sujeto de una frase: "main (current)" o "The current branch".
fn side_subject(sides: &ConflictSides, side: ConflictSide) -> String {
    match (side, &sides.ours, &sides.theirs) {
        (ConflictSide::Ours, Some(name), _) => format!("{name} (current)"),
        (ConflictSide::Ours, None, _) => "The current branch".into(),
        (ConflictSide::Theirs, _, Some(name)) => format!("{name} (incoming)"),
        (ConflictSide::Theirs, _, None) => "The incoming changes".into(),
        (ConflictSide::Base, _, _) => "The common ancestor".into(),
    }
}

/// Lo mismo, en medio de la frase.
fn side_object(sides: &ConflictSides, side: ConflictSide) -> String {
    match (side, &sides.ours, &sides.theirs) {
        (ConflictSide::Ours, None, _) => "the current branch".into(),
        (ConflictSide::Theirs, _, None) => "the incoming changes".into(),
        _ => side_subject(sides, side),
    }
}

/// Título y explicación de un archivo que se resuelve entero: qué hizo cada
/// rama con él.
fn whole_text(document: &MergeDocument, sides: &ConflictSides) -> (String, String) {
    let short = |side| side_short(sides, side);
    let deleted_and_modified = |deleted: ConflictSide, modified: ConflictSide| {
        (
            if deleted == ConflictSide::Ours {
                format!(
                    "Deleted in {}, modified in {}",
                    short(deleted),
                    short(modified)
                )
            } else {
                format!(
                    "Modified in {}, deleted in {}",
                    short(modified),
                    short(deleted)
                )
            },
            format!(
                "{} deleted this file and {} changed it. Keep the modified file, or delete it.",
                side_subject(sides, deleted),
                side_object(sides, modified)
            ),
        )
    };
    match document.conflict_kind {
        ConflictKind::DeletedByUs => deleted_and_modified(ConflictSide::Ours, ConflictSide::Theirs),
        ConflictKind::DeletedByThem => {
            deleted_and_modified(ConflictSide::Theirs, ConflictSide::Ours)
        }
        ConflictKind::BothDeleted => (
            "Deleted on both sides".into(),
            format!(
                "{} and {} both deleted or renamed this file, so it is resolved as deleted. Keep the common ancestor only if you still need it.",
                side_subject(sides, ConflictSide::Ours),
                side_object(sides, ConflictSide::Theirs)
            ),
        ),
        ConflictKind::AddedByUs | ConflictKind::AddedByThem if !document.binary => {
            let side = if document.ours.present {
                ConflictSide::Ours
            } else {
                ConflictSide::Theirs
            };
            (
                format!("Only in {}", short(side)),
                format!(
                    "Only {} has this file (usually after a rename on the other side). Keep it or delete it.",
                    side_object(sides, side)
                ),
            )
        }
        _ => (
            "Binary file".into(),
            "Binary files can't be merged line by line. Keep one version as it is.".into(),
        ),
    }
}

/// Una opción de un archivo que se resuelve entero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WholeOption {
    pub choice: WholeChoice,
    pub label: String,
    /// Debajo, en gris: de qué rama viene y cuánto ocupa.
    pub detail: String,
}

/// Las opciones de un archivo que se resuelve entero, según la clase de
/// conflicto:
///
/// - borrado por un lado y modificado por el otro (`UD` / `DU`): "Keep the
///   modified file" o "Delete the file";
/// - borrado por los dos (`DD`): "Delete the file" (lo elegido de entrada) o
///   quedarse con el ancestro común;
/// - solo en un lado (`AU` / `UA`): quedárselo o borrarlo;
/// - binario con los dos lados (`UU` / `AA`): una de las versiones entera,
///   con su tamaño.
pub fn whole_options(document: &MergeDocument, sides: &ConflictSides) -> Vec<WholeOption> {
    let size = |side: ConflictSide| {
        let index = match side {
            ConflictSide::Base => 0,
            ConflictSide::Ours => 1,
            ConflictSide::Theirs => 2,
        };
        document.sizes[index].map(format_size).unwrap_or_default()
    };
    let from = |side: ConflictSide| {
        let size = size(side);
        let name = side_short(sides, side);
        if size.is_empty() {
            format!("From {name}")
        } else {
            format!("From {name} · {size}")
        }
    };
    let option = |choice, label: &str, detail: String| WholeOption {
        choice,
        label: label.to_string(),
        detail,
    };
    let delete = |detail: String| option(WholeChoice::Delete, "Delete the file", detail);
    match document.conflict_kind {
        ConflictKind::DeletedByUs | ConflictKind::DeletedByThem => {
            let (kept, deleted) = if document.ours.present {
                (ConflictSide::Ours, ConflictSide::Theirs)
            } else {
                (ConflictSide::Theirs, ConflictSide::Ours)
            };
            vec![
                option(
                    WholeChoice::Keep(kept),
                    "Keep the modified file",
                    from(kept),
                ),
                delete(format!("As {} did", side_short(sides, deleted))),
            ]
        }
        ConflictKind::BothDeleted => vec![
            delete("As both sides did".into()),
            option(
                WholeChoice::Keep(ConflictSide::Base),
                "Keep the common ancestor",
                size(ConflictSide::Base),
            ),
        ],
        ConflictKind::AddedByUs | ConflictKind::AddedByThem => {
            let side = if document.ours.present {
                ConflictSide::Ours
            } else {
                ConflictSide::Theirs
            };
            vec![
                option(WholeChoice::Keep(side), "Keep the file", from(side)),
                delete(String::new()),
            ]
        }
        _ => {
            let mut options = vec![
                option(
                    WholeChoice::Keep(ConflictSide::Ours),
                    "Keep current (ours)",
                    from(ConflictSide::Ours),
                ),
                option(
                    WholeChoice::Keep(ConflictSide::Theirs),
                    "Keep incoming (theirs)",
                    from(ConflictSide::Theirs),
                ),
            ];
            if document.base.present {
                options.push(option(
                    WholeChoice::Keep(ConflictSide::Base),
                    "Keep base (common ancestor)",
                    size(ConflictSide::Base),
                ));
            }
            options
        }
    }
}

/// Lo elegido de entrada: un archivo que borraron los dos ya está borrado.
fn default_whole(document: &MergeDocument) -> Option<WholeChoice> {
    (document.kind == DocumentKind::Whole && document.conflict_kind == ConflictKind::BothDeleted)
        .then_some(WholeChoice::Delete)
}

fn format_size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

/// El aviso que hay que confirmar antes de marcar el archivo como resuelto,
/// si queda algo por decidir.
fn save_warning(path: &str, unresolved: usize, markers: usize) -> Option<String> {
    match (unresolved, markers) {
        (0, 0) => None,
        (0, markers) => Some(format!(
            "The result still has {markers} line(s) that look like conflict markers (<<<<<<< or >>>>>>>). Mark {path} as resolved anyway?"
        )),
        (unresolved, _) => Some(format!(
            "{unresolved} conflict(s) in {path} are still unresolved and keep their conflict markers. Mark it as resolved anyway?"
        )),
    }
}

impl Render for ConflictResolver {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.apply_pending_text(window, cx);
        let palette = *cx.palette();

        div()
            .id("conflict-resolver")
            .test_support()
            .flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .bg(palette.card)
            .text_color(palette.foreground)
            .text_size(rems(0.8125))
            .when(self.show_files, |this| this.child(self.render_sidebar(cx)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(self.render_toolbar(cx))
                    .child(self.render_body(cx)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pane_titles_say_who_each_side_is() {
        let merge = ConflictSides {
            ours: Some("main".into()),
            theirs: Some("feature".into()),
        };
        assert_eq!(pane_title(Pane::Ours, &merge, false), "Current · main");
        assert_eq!(
            pane_title(Pane::Theirs, &merge, false),
            "Incoming · feature"
        );
        assert_eq!(pane_title(Pane::Base, &merge, false), "Base");

        let rebase = ConflictSides {
            ours: Some("main".into()),
            theirs: Some("feature (a1b2c3d)".into()),
        };
        assert_eq!(pane_title(Pane::Ours, &rebase, true), "Onto · main");
        assert_eq!(
            pane_title(Pane::Theirs, &rebase, true),
            "Your commit · a1b2c3d"
        );
        let detached = ConflictSides {
            ours: None,
            theirs: Some("commit a1b2c3d".into()),
        };
        assert_eq!(
            pane_title(Pane::Theirs, &detached, true),
            "Your commit · a1b2c3d"
        );
        assert_eq!(pane_title(Pane::Ours, &detached, true), "Onto (ours)");

        // Sin nombres, los de siempre.
        let unknown = ConflictSides::default();
        assert_eq!(pane_title(Pane::Ours, &unknown, false), "Current (ours)");
        assert_eq!(
            pane_title(Pane::Theirs, &unknown, false),
            "Incoming (theirs)"
        );
    }

    #[test]
    fn the_save_warning_says_what_is_left() {
        assert_eq!(save_warning("a.txt", 0, 0), None);
        assert_eq!(
            save_warning("a.txt", 0, 2).as_deref(),
            Some(
                "The result still has 2 line(s) that look like conflict markers (<<<<<<< or >>>>>>>). Mark a.txt as resolved anyway?"
            )
        );
        assert!(
            save_warning("a.txt", 1, 2)
                .unwrap()
                .starts_with("1 conflict(s) in a.txt are still unresolved")
        );
    }

    #[test]
    fn whole_choices_round_trip_through_their_values() {
        for choice in [
            WholeChoice::Keep(ConflictSide::Ours),
            WholeChoice::Keep(ConflictSide::Theirs),
            WholeChoice::Keep(ConflictSide::Base),
            WholeChoice::Delete,
        ] {
            assert_eq!(WholeChoice::parse(choice.value()), Some(choice));
        }
        assert_eq!(format_size(2048), "2.0 KB");
    }
}
