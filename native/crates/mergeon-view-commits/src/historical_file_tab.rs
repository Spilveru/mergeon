//! Pestaña de archivo de un commit o del árbol
//! (`src/components/repository/HistoricalFileTab.vue`): la lista de archivos
//! del commit (o el árbol del repositorio en ese commit) a la izquierda,
//! plegable, y el diff del archivo (o su contenido) a la derecha, con el
//! scroll de la pestaña recordado.
//!
//! Con fuente `history` ("File history", sin equivalente en el original) la
//! izquierda es la lista de revisiones del archivo siguiendo sus renombres y
//! la derecha el diff del archivo en la revisión elegida.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::prelude::*;
use gpui_kit::{
    App, ClipboardItem, Context, Entity, FocusHandle, KeyDownEvent, MouseButton, Pixels, Point,
    SharedString, Subscription, Task, Window, canvas, div, px, relative,
};
use mergeon_app::{AppEvent, AppModel};
use mergeon_backend::{DiffOptions, FileAtCommit, FileChange, FileRevision};
use mergeon_model::diff_preferences::DiffPreferences;
use mergeon_model::format::format_size;
use mergeon_model::resizable_width::ResizableWidth;
use mergeon_model::workspace_tabs::{FileTab, FileTabDescriptor, Scroll, TabSource};
use mergeon_ui::menu::MenuItem;
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{ActivePaletteExt as _, Lucide};
use mergeon_view_diff::{
    DiffViewMode, DiffViewer, DiffViewerEvent, FileContentViewer, FileContentViewerEvent,
    ImageDiffViewer, LineAnchor, ScrollPosition, is_image_path,
};

use crate::changes_tab::{CommitChangesTab, CommitChangesTabEvent};
use crate::file_history_list::{FileHistoryList, FileHistoryListEvent, FileHistorySource};
use crate::monogram::monogram;
use crate::resizer::{RESIZER_WIDTH, drag_tracker};
use crate::tree_tab::{CommitTreeTab, CommitTreeTabEvent, file_content};

/// Lo que se carga para una pestaña: `[repo, id de la pestaña, ignoreWhitespace,
/// wholeFile]`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DataKey {
    repo_path: String,
    tab_id: String,
    ignore_whitespace: bool,
    whole_file: bool,
}

/// Lo del `AppModel` de lo que depende la pestaña (y lo que pinta): el
/// modelo avisa por cualquier cosa (un toast, el progreso de una operación,
/// otro repositorio u otra pestaña) y, si nada de esto cambió, la pestaña no
/// hace nada ni se repinta.
#[derive(Debug, Clone, PartialEq)]
struct ModelSnapshot {
    active: bool,
    repo_path: Option<String>,
    /// La pestaña sin su scroll: lo guarda la propia pestaña al desplazarse
    /// y no cambia nada de lo que se pinta.
    tab: Option<FileTab>,
    preferences: DiffPreferences,
    file_list_width: ResizableWidth,
}

/// Lo que se pasó por última vez a la lista (o al árbol) de la izquierda.
#[derive(Debug, Clone, PartialEq)]
struct ListsKey {
    repo_path: String,
    path: String,
    source: TabSource,
    commit_hash: String,
    /// Cambia cada vez que llegan los archivos del commit.
    files_generation: u64,
    loading_files: bool,
}

/// Las entradas del menú contextual de un archivo de la lista, sin
/// separadores: abrir ("Open here", "Open in new tab", "Open in editor"), su
/// historial ("File history") y copiar ("Copy Path", más "Copy GitHub link"
/// si el repositorio tiene un remoto de GitHub).
pub fn file_menu_labels() -> [&'static str; 5] {
    [
        "Open here",
        "Open in new tab",
        "Open in editor",
        "File history",
        "Copy Path",
    ]
}

/// Pestaña de archivo histórico. La crea la cáscara para cada pestaña de
/// fuente `commit` o `tree`.
pub struct HistoricalFileTabView {
    model: Entity<AppModel>,
    repo_id: String,
    tab_id: String,
    /// `filesOpen`: la lista lateral a la vista (la conmuta la cabecera de la
    /// pestaña, que es de la cáscara).
    files_open: bool,

    changes: Entity<CommitChangesTab>,
    tree: Entity<CommitTreeTab>,
    history: Entity<FileHistoryList>,
    diff_viewer: Entity<DiffViewer>,
    file_viewer: Entity<FileContentViewer>,
    /// Antes y después de una imagen (en lugar de su diff).
    image_viewer: Entity<ImageDiffViewer>,
    /// `(commit, ruta)` de las versiones que tiene el visor de imágenes: las
    /// preferencias del diff no cambian la imagen.
    image_key: Option<(String, String)>,
    resizer_focus: FocusHandle,

    files: Vec<FileChange>,
    diff: String,
    content: Option<FileAtCommit>,
    loading: bool,
    error: String,
    too_large: bool,
    whole_file_fell_back: bool,
    size: u64,
    /// La línea que se leía al pulsar "Whole file": el diff que llega
    /// después vuelve a ella en lugar de al desplazamiento en píxeles
    /// guardado, que en el otro diff es otro sitio.
    line_anchor: Option<LineAnchor>,

    request: u64,
    loaded_key: Option<DataKey>,
    /// Lo último que se pidió: el modelo avisa muchas veces y la misma carga
    /// no se repite mientras está en vuelo (ni tras fallar: para eso está
    /// "Retry").
    requested_key: Option<DataKey>,
    /// Los archivos del commit no cambian al pasar de un archivo a otro de la
    /// misma pestaña: se piden una vez por commit, y así la lista no parpadea.
    files_commit: String,
    /// Commit de la pestaña, el último que se vio en la sesión.
    commit_hash: String,
    was_active: bool,
    _load: Option<Task<()>>,

    /// Pestaña de historial: la revisión elegida en la lista, y qué diff
    /// (`[hash, ignoreWhitespace, wholeFile]`) se pidió para ella.
    revision: Option<FileRevision>,
    revision_key: Option<(String, bool, bool)>,
    /// El diff de la revisión trae bloques (`@@`): un renombre puro o un
    /// cambio de modo no los traen y el visor quedaría en blanco.
    revision_has_hunks: bool,
    /// Un commit movió HEAD con la pestaña oculta: al volver se relee.
    history_stale: bool,

    root_left: Pixels,
    root_width: f64,
    /// Lo que se vio del modelo en el último aviso.
    snapshot: Option<ModelSnapshot>,
    /// Cambia cada vez que se reemplazan `files`.
    files_generation: u64,
    lists_key: Option<ListsKey>,
    /// Veces que se pintó (tests).
    renders: usize,
    _subscriptions: Vec<Subscription>,
}

impl HistoricalFileTabView {
    pub fn new(
        repo_id: impl Into<String>,
        tab_id: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let model = AppModel::global(cx);
        let changes = cx.new(|cx| {
            let mut tab = CommitChangesTab::new(window, cx);
            tab.set_external_files(true, cx);
            tab
        });
        let tree = cx.new(|cx| {
            let mut tree = CommitTreeTab::new(window, cx);
            tree.set_external_files(true, cx);
            tree
        });
        let history = cx.new(|cx| FileHistoryList::new(window, cx));
        let diff_viewer = cx.new(|cx| DiffViewer::new(window, cx));
        let file_viewer = cx.new(|cx| {
            let mut viewer = FileContentViewer::new(window, cx);
            viewer.set_header_hidden(true, cx);
            viewer
        });

        let view = cx.weak_entity();
        changes.update(cx, |tab, _| {
            let view = view.clone();
            tab.set_file_context_menu(move |file, _, cx| {
                let descriptor = view.upgrade().map(|view| {
                    let view = view.read(cx);
                    (
                        view.commit_descriptor(&file.path),
                        view.github_web_url(cx).is_some(),
                    )
                });
                descriptor
                    .map(|(descriptor, on_github)| file_menu(&view, descriptor, on_github))
                    .unwrap_or_default()
            });
        });
        tree.update(cx, |tree, _| {
            let view = view.clone();
            tree.set_file_context_menu(move |descriptor, _, cx| {
                let on_github = view
                    .upgrade()
                    .is_some_and(|view| view.read(cx).github_web_url(cx).is_some());
                file_menu(&view, descriptor.clone(), on_github)
            });
        });

        let subscriptions = vec![
            cx.observe(&model, |this, _, cx| this.model_changed(cx)),
            cx.subscribe(&model, |this, _, event: &AppEvent, cx| {
                // Al reutilizar la pestaña con otro archivo el panel sigue
                // montado: pasa a ser el de la pestaña nueva.
                if let AppEvent::WorkspaceTabReplaced {
                    repo_path,
                    from,
                    to,
                } = event
                    && *from == this.tab_id
                    && this.repo_path(cx).as_deref() == Some(repo_path.as_str())
                {
                    this.tab_id = to.clone();
                    this.model_changed(cx);
                }
                // La historia desde HEAD sigue a la rama: un commit nuevo (o
                // un checkout) la cambia.
                if let AppEvent::MutationRecorded { repo_id, operation } = event
                    && *repo_id == this.repo_id
                    // Lo mismo que relee el historial principal (no el
                    // vigilante del working tree).
                    && crate::commits_view::is_history_mutation(operation.as_deref())
                    && this.tab(cx).is_some_and(|tab| {
                        tab.source == TabSource::History && tab.commit_hash.is_empty()
                    })
                {
                    if this.is_active(cx) {
                        this.history.update(cx, |list, cx| list.reload(cx));
                    } else {
                        this.history_stale = true;
                    }
                }
            }),
            cx.subscribe(
                &history,
                |this, _, event: &FileHistoryListEvent, cx| match event {
                    FileHistoryListEvent::Select(revision) => {
                        this.revision = Some(revision.clone());
                        this.load_revision_diff(cx);
                    }
                    FileHistoryListEvent::ShowCommit(hash) => this.show_commit(hash, cx),
                    FileHistoryListEvent::ViewFileAt(revision) => this.open_in_new_tab(
                        FileTabDescriptor::tree(revision.path.clone(), &revision.hash),
                        cx,
                    ),
                },
            ),
            cx.subscribe(
                &changes,
                |this, _, event: &CommitChangesTabEvent, cx| match event {
                    CommitChangesTabEvent::SelectFile(file) => {
                        let descriptor = this.commit_descriptor(&file.path);
                        this.open_here(descriptor, cx);
                    }
                    CommitChangesTabEvent::OpenFile(file) => {
                        let descriptor = this.commit_descriptor(&file.path);
                        this.open_in_new_tab(descriptor, cx);
                    }
                    CommitChangesTabEvent::ToggleIgnoreWhitespace
                    | CommitChangesTabEvent::ToggleWholeFile => {}
                },
            ),
            cx.subscribe(
                &tree,
                |this, _, event: &CommitTreeTabEvent, cx| match event {
                    CommitTreeTabEvent::OpenFile(descriptor) => {
                        this.open_here(descriptor.clone(), cx)
                    }
                    CommitTreeTabEvent::OpenFileNew(descriptor) => {
                        this.open_in_new_tab(descriptor.clone(), cx)
                    }
                },
            ),
            cx.subscribe(&diff_viewer, |this, _, event: &DiffViewerEvent, cx| {
                if let DiffViewerEvent::Scrolled(position) = event {
                    this.remember_scroll(*position, cx);
                }
            }),
            cx.subscribe(
                &file_viewer,
                |this, _, event: &FileContentViewerEvent, cx| match event {
                    FileContentViewerEvent::Scrolled(position) => {
                        this.remember_scroll(*position, cx)
                    }
                    FileContentViewerEvent::Close => this.close(cx),
                },
            ),
        ];

        let mut this = Self {
            model,
            repo_id: repo_id.into(),
            tab_id: tab_id.into(),
            files_open: true,
            changes,
            tree,
            history,
            diff_viewer,
            file_viewer,
            image_viewer: cx.new(ImageDiffViewer::new),
            image_key: None,
            resizer_focus: cx.focus_handle().tab_stop(true),
            files: Vec::new(),
            diff: String::new(),
            content: None,
            loading: false,
            error: String::new(),
            too_large: false,
            whole_file_fell_back: false,
            size: 0,
            line_anchor: None,
            request: 0,
            loaded_key: None,
            requested_key: None,
            files_commit: String::new(),
            commit_hash: String::new(),
            was_active: false,
            _load: None,
            revision: None,
            revision_key: None,
            revision_has_hunks: false,
            history_stale: false,
            root_left: Pixels::ZERO,
            root_width: 0.,
            snapshot: None,
            files_generation: 0,
            lists_key: None,
            renders: 0,
            _subscriptions: subscriptions,
        };
        this.model.update(cx, |model, cx| {
            model.load_diff_preferences(false, cx).detach();
            model.load_file_list_width(cx).detach();
        });
        this.model_changed(cx);
        this
    }

    // ------------------------------------------------------------------
    // API
    // ------------------------------------------------------------------

    /// `filesOpen` de `RepositoryWorkspace.vue`: la lista de archivos a la
    /// vista. La pestaña de árbol sin archivo elegido la enseña siempre.
    pub fn set_files_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.files_open != open {
            self.files_open = open;
            cx.notify();
        }
    }

    pub fn tab_id(&self) -> &str {
        &self.tab_id
    }

    /// La pestaña tal como está en la sesión, si sigue abierta.
    pub fn tab(&self, cx: &App) -> Option<FileTab> {
        let path = self.repo_path(cx)?;
        self.model
            .read(cx)
            .repository_workspace(&path)
            .tabs
            .into_iter()
            .find(|tab| tab.id == self.tab_id)
    }

    /// La pestaña es la que se está viendo.
    pub fn is_active(&self, cx: &App) -> bool {
        let model = self.model.read(cx);
        let Some(path) = self.repo_path(cx) else {
            return false;
        };
        !model.show_preferences()
            && model
                .active_repository()
                .is_some_and(|repo| repo.id == self.repo_id)
            && model.repository_workspace(&path).active_tab == self.tab_id
    }

    /// `showFiles`.
    pub fn shows_files(&self, cx: &App) -> bool {
        self.shows_files_for(self.tab(cx).as_ref())
    }

    fn shows_files_for(&self, tab: Option<&FileTab>) -> bool {
        // La lista de revisiones es la mitad de la pestaña de historial.
        self.files_open
            || tab.is_none_or(|tab| tab.path.is_empty() || tab.source == TabSource::History)
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// Veces que se pintó (tests).
    #[doc(hidden)]
    pub fn render_count(&self) -> usize {
        self.renders
    }

    pub fn error(&self) -> &str {
        &self.error
    }

    pub fn diff(&self) -> &str {
        &self.diff
    }

    /// Archivos del commit de la pestaña.
    pub fn files(&self) -> &[FileChange] {
        &self.files
    }

    pub fn diff_viewer(&self) -> &Entity<DiffViewer> {
        &self.diff_viewer
    }

    pub fn file_viewer(&self) -> &Entity<FileContentViewer> {
        &self.file_viewer
    }

    pub fn changes_list(&self) -> &Entity<CommitChangesTab> {
        &self.changes
    }

    pub fn tree(&self) -> &Entity<CommitTreeTab> {
        &self.tree
    }

    /// La lista de revisiones de la pestaña de historial.
    pub fn history(&self) -> &Entity<FileHistoryList> {
        &self.history
    }

    /// La revisión cuyo diff se muestra (pestaña de historial).
    pub fn selected_revision(&self) -> Option<&FileRevision> {
        self.revision.as_ref()
    }

    /// El texto de estado del panel de la derecha, si lo hay.
    pub fn status_text(&self, cx: &App) -> Option<SharedString> {
        let tab = self.tab(cx)?;
        if tab.source == TabSource::History && self.revision.is_none() {
            return Some(self.history_status(cx));
        }
        if tab.source == TabSource::Tree {
            return tab
                .path
                .is_empty()
                .then(|| "Pick a file on the left to see it as it was at this commit.".into());
        }
        if self.loading {
            Some("Loading diff…".into())
        } else if !self.error.is_empty() {
            Some(self.error.clone().into())
        } else if self.too_large {
            Some(
                format!(
                    "Diff too large to display (over {})",
                    format_size(self.size as f64)
                )
                .into(),
            )
        } else if self.diff.is_empty() {
            Some("No changes to display".into())
        } else {
            None
        }
    }

    /// `load`: vuelve a pedir el diff (o el archivo) de la pestaña. Es lo que
    /// hace "Retry".
    pub fn load(&mut self, cx: &mut Context<Self>) {
        let (Some(tab), Some(key)) = (self.tab(cx), self.data_key(cx)) else {
            return;
        };
        if tab.source == TabSource::History {
            self.requested_key = Some(key);
            if self.revision.is_some() {
                self.load_revision_diff(cx);
            } else {
                self.history.update(cx, |list, cx| list.reload(cx));
            }
            return;
        }
        self.request += 1;
        self.requested_key = Some(key.clone());
        let version = self.request;
        let repo_path = key.repo_path.clone();
        self.loading = true;
        self.error.clear();
        self.sync_viewers(cx);
        cx.notify();

        let reuse_files = self.files_commit == tab.commit_hash;
        let files = (!reuse_files).then(|| {
            self.model.update(cx, |model, cx| {
                model.get_commit_files(&repo_path, &tab.commit_hash, cx)
            })
        });
        if tab.source == TabSource::Commit {
            self.load_image(&repo_path, &tab.commit_hash, &tab.path, None, cx);
        }
        enum Read {
            Diff(Task<mergeon_backend::QueryResult<mergeon_backend::DiffPayload>>),
            File(Task<mergeon_backend::QueryResult<FileAtCommit>>),
            Nothing,
        }
        let read = match tab.source {
            TabSource::Tree if !tab.path.is_empty() => {
                Read::File(self.model.update(cx, |model, cx| {
                    model.get_file_at_commit(&repo_path, &tab.commit_hash, &tab.path, cx)
                }))
            }
            TabSource::Tree => Read::Nothing,
            _ => Read::Diff(self.model.update(cx, |model, cx| {
                model.get_commit_diff(
                    &repo_path,
                    &tab.commit_hash,
                    &tab.path,
                    DiffOptions {
                        ignore_whitespace: key.ignore_whitespace,
                        whole_file: key.whole_file,
                    },
                    cx,
                )
            })),
        };

        self._load = Some(cx.spawn(async move |this, cx| {
            let files = match files {
                Some(files) => Some(files.await),
                None => None,
            };
            let failure = |error: &mergeon_backend::QueryError| {
                if error.message().is_empty() {
                    "This file or commit is no longer available.".to_string()
                } else {
                    error.message().to_string()
                }
            };
            let mut error = None;
            let files = match files {
                Some(Ok(files)) => Some(files),
                Some(Err(failed)) => {
                    error = Some(failure(&failed));
                    None
                }
                None => None,
            };
            let (diff, content) = match read {
                Read::Diff(read) => match read.await {
                    Ok(payload) => (Some(payload), None),
                    Err(failed) => {
                        error.get_or_insert(failure(&failed));
                        (None, None)
                    }
                },
                Read::File(read) => match read.await {
                    Ok(file) => (None, Some(file)),
                    Err(failed) => {
                        error.get_or_insert(failure(&failed));
                        (None, None)
                    }
                },
                Read::Nothing => (None, None),
            };

            let _ = this.update(cx, |this, cx| {
                if version != this.request {
                    return;
                }
                match error {
                    Some(error) => this.error = error,
                    None => {
                        this.loaded_key = Some(key);
                        if let Some(files) = files {
                            this.files = files;
                            this.files_generation += 1;
                        }
                        this.files_commit = tab.commit_hash.clone();
                        if tab.source == TabSource::Tree {
                            this.content = content;
                            this.whole_file_fell_back = false;
                        } else {
                            let payload = diff.unwrap_or_default();
                            this.diff = payload.diff;
                            this.too_large = payload.too_large;
                            this.whole_file_fell_back = payload.whole_file_fell_back;
                            this.size = payload.size;
                        }
                    }
                }
                this.loading = false;
                this.sync_viewers(cx);
                if !this.scroll_to_anchor(cx) {
                    this.restore_scroll(cx);
                }
                cx.notify();
            });
        }));
    }

    /// El visor de las dos versiones de una imagen (si el archivo lo es).
    pub fn image_viewer(&self) -> &Entity<ImageDiffViewer> {
        &self.image_viewer
    }

    // ------------------------------------------------------------------
    // Interno
    // ------------------------------------------------------------------

    fn repo_path(&self, cx: &App) -> Option<String> {
        self.model
            .read(cx)
            .repository(self.repo_id.as_str())
            .map(|repo| repo.path.clone())
    }

    fn data_key(&self, cx: &App) -> Option<DataKey> {
        Some(DataKey {
            repo_path: self.repo_path(cx)?,
            tab_id: self.tab_id.clone(),
            ignore_whitespace: self.model.read(cx).diff_preferences().ignore_whitespace,
            whole_file: self.model.read(cx).diff_preferences().whole_file,
        })
    }

    /// Si el archivo es una imagen, sus dos versiones (en el padre y en el
    /// commit) para verlas lado a lado. Una vez por commit y ruta.
    fn load_image(
        &mut self,
        repo_path: &str,
        commit_hash: &str,
        path: &str,
        previous_path: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !is_image_path(path) {
            if self.image_key.take().is_some() {
                self.image_viewer.update(cx, |viewer, cx| viewer.clear(cx));
            }
            return;
        }
        let key = (commit_hash.to_string(), path.to_string());
        if self.image_key.as_ref() == Some(&key) {
            return;
        }
        self.image_key = Some(key.clone());
        let request = mergeon_backend::ImagePairRequest::Commit {
            hash: commit_hash.to_string(),
            path: path.to_string(),
            previous_path,
        };
        let task = crate::images::load_image_pair(
            &self.model,
            &self.image_viewer,
            repo_path,
            path,
            request,
            cx,
        );
        cx.spawn(async move |this, cx| {
            let images = task.await;
            let _ = this.update(cx, |this, cx| {
                // Otra imagen pedida mientras tanto: esta ya no.
                if this.image_key.as_ref() == Some(&key) {
                    crate::images::show(&this.image_viewer, images, cx);
                }
            });
        })
        .detach();
    }

    /// `commitDescriptor(file)`.
    fn commit_descriptor(&self, path: &str) -> FileTabDescriptor {
        FileTabDescriptor::commit(path, &self.commit_hash)
    }

    /// Lo que la pestaña lee del modelo, tal como está ahora.
    fn model_snapshot(&self, cx: &App) -> ModelSnapshot {
        let model = self.model.read(cx);
        let repo_path = model
            .repository(self.repo_id.as_str())
            .map(|repo| repo.path.clone());
        let (active, tab) = match &repo_path {
            Some(path) => {
                let workspace = model.repository_workspace(path);
                let active = !model.show_preferences()
                    && model
                        .active_repository()
                        .is_some_and(|repo| repo.id == self.repo_id)
                    && workspace.active_tab == self.tab_id;
                let tab = workspace
                    .tabs
                    .into_iter()
                    .find(|tab| tab.id == self.tab_id)
                    .map(|tab| FileTab {
                        scroll: Scroll::default(),
                        ..tab
                    });
                (active, tab)
            }
            None => (false, None),
        };
        ModelSnapshot {
            active,
            repo_path,
            tab,
            preferences: *model.diff_preferences(),
            file_list_width: *model.file_list_width(),
        }
    }

    /// El `watch(() => [dataKey(), props.active], …)` del original.
    fn model_changed(&mut self, cx: &mut Context<Self>) {
        // Nada de lo que la pestaña usa cambió (un toast, el progreso de una
        // operación, otro repositorio u otra pestaña): ni se sincroniza ni se
        // repinta.
        let snapshot = self.model_snapshot(cx);
        if self.snapshot.as_ref() == Some(&snapshot) {
            return;
        }
        let active = snapshot.active;
        self.snapshot = Some(snapshot);

        let became_active = active && !self.was_active;
        self.was_active = active;

        self.sync_lists(cx);
        let mode = self.model.read(cx).diff_preferences().diff_view_mode;
        self.diff_viewer
            .update(cx, |viewer, cx| viewer.set_view_mode(mode, cx));

        if active
            && self
                .tab(cx)
                .is_some_and(|tab| tab.source == TabSource::History)
        {
            self.sync_history(cx);
        } else if active {
            let key = self.data_key(cx);
            if key != self.requested_key {
                self.load(cx);
            } else if became_active && key == self.loaded_key {
                self.restore_scroll(cx);
            }
        }
        cx.notify();
    }

    /// La lista (o el árbol) de la izquierda, con los archivos del commit.
    /// Solo se les pasan (sin copiarlos) si cambiaron los archivos, la
    /// pestaña, el archivo elegido o si se están cargando.
    fn sync_lists(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.tab(cx) else {
            return;
        };
        self.commit_hash = tab.commit_hash.clone();
        let repo_path = self.repo_path(cx).unwrap_or_default();
        let loading_files = self.loading && self.files.is_empty();
        let key = ListsKey {
            repo_path,
            path: tab.path,
            source: tab.source,
            commit_hash: tab.commit_hash,
            files_generation: self.files_generation,
            loading_files,
        };
        if self.lists_key.as_ref() == Some(&key) {
            return;
        }
        let files = &self.files;
        self.changes.update(cx, |list, cx| {
            list.set_loading_details(loading_files, cx);
            list.set_commit_files(files, cx);
            list.set_selected_path(Some(&key.path), cx);
        });
        if key.source == TabSource::Tree {
            self.tree.update(cx, |tree, cx| {
                tree.set_source(&key.repo_path, &key.commit_hash, cx);
                tree.set_commit_files(files, cx);
            });
        }
        self.lists_key = Some(key);
    }

    fn sync_viewers(&mut self, cx: &mut Context<Self>) {
        self.sync_lists(cx);
        let Some(tab) = self.tab(cx) else {
            return;
        };
        if tab.source == TabSource::Tree {
            let content = self.content.as_ref().map(file_content);
            let (loading, error, path) = (self.loading, self.error.clone(), tab.path.clone());
            self.file_viewer.update(cx, |viewer, cx| {
                viewer.set_loading(loading, cx);
                viewer.set_error(error, cx);
                viewer.set_file(path, content, cx);
            });
        } else {
            // En el historial, la ruta que tenía el archivo en la revisión.
            let path = match (&self.revision, tab.source) {
                (Some(revision), TabSource::History) => revision.path.clone(),
                _ => tab.path.clone(),
            };
            let diff = self.diff.clone();
            self.diff_viewer
                .update(cx, |viewer, cx| viewer.set_diff(diff, path, cx));
        }
    }

    /// Pestaña de historial a la vista: la lista apunta al archivo de la
    /// pestaña y el diff es el de la revisión elegida.
    fn sync_history(&mut self, cx: &mut Context<Self>) {
        let (Some(tab), Some(repo_path)) = (self.tab(cx), self.repo_path(cx)) else {
            return;
        };
        let source = FileHistorySource {
            repo_path,
            path: tab.path.clone(),
            revision: (!tab.commit_hash.is_empty()).then(|| tab.commit_hash.clone()),
        };
        let stale = std::mem::take(&mut self.history_stale);
        self.history.update(cx, |list, cx| {
            if list.source() == Some(&source) {
                if stale {
                    list.reload(cx);
                }
            } else {
                list.set_source(source, cx);
            }
        });
        // "Ignore spaces" o "Whole file" cambió: el diff de la misma revisión,
        // otra vez.
        if let Some(revision) = &self.revision {
            let preferences = *self.model.read(cx).diff_preferences();
            let key = (
                revision.hash.clone(),
                preferences.ignore_whitespace,
                preferences.whole_file,
            );
            if self.revision_key.as_ref() != Some(&key) {
                self.load_revision_diff(cx);
            }
        }
    }

    /// El diff del archivo en la revisión elegida, con la ruta que tenía en
    /// ella (y la anterior si esa revisión lo renombró).
    fn load_revision_diff(&mut self, cx: &mut Context<Self>) {
        let (Some(revision), Some(repo_path)) = (self.revision.clone(), self.repo_path(cx)) else {
            return;
        };
        let preferences = *self.model.read(cx).diff_preferences();
        let (ignore_whitespace, whole_file) =
            (preferences.ignore_whitespace, preferences.whole_file);
        self.revision_key = Some((revision.hash.clone(), ignore_whitespace, whole_file));
        self.request += 1;
        let version = self.request;
        self.loading = true;
        self.error.clear();
        cx.notify();

        let previous = revision
            .previous_path
            .clone()
            .filter(|_| revision.is_rename());
        self.load_image(
            &repo_path,
            &revision.hash,
            &revision.path,
            Some(previous.clone().unwrap_or_else(|| revision.path.clone())),
            cx,
        );
        let read = self.model.update(cx, |model, cx| {
            model.get_file_revision_diff(
                &repo_path,
                &revision.hash,
                &revision.path,
                previous.as_deref(),
                DiffOptions {
                    ignore_whitespace,
                    whole_file,
                },
                cx,
            )
        });
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |this, cx| {
                if version != this.request {
                    return;
                }
                match result {
                    Ok(payload) => {
                        this.revision_has_hunks =
                            payload.diff.starts_with("@@") || payload.diff.contains("\n@@");
                        this.diff = payload.diff;
                        this.too_large = payload.too_large;
                        this.whole_file_fell_back = payload.whole_file_fell_back;
                        this.size = payload.size;
                    }
                    Err(error) => this.error = error.message().to_string(),
                }
                this.loading = false;
                this.sync_viewers(cx);
                if !this.scroll_to_anchor(cx) {
                    this.diff_viewer.update(cx, |viewer, cx| {
                        viewer.set_scroll_position(ScrollPosition::new(0., 0.), cx)
                    });
                }
                cx.notify();
            });
        }));
    }

    /// Lo que dice la derecha del historial sin revisión elegida.
    fn history_status(&self, cx: &App) -> SharedString {
        let list = self.history.read(cx);
        if let Some(error) = list.error() {
            error.to_string().into()
        } else if list.is_loading() {
            "Loading history…".into()
        } else if list.rows().is_empty() {
            "No commit has touched this file.".into()
        } else {
            "Pick a revision on the left to see what it changed.".into()
        }
    }

    /// Clic en el hash de una revisión: el commit en el historial principal.
    pub fn show_commit(&mut self, hash: &str, cx: &mut Context<Self>) {
        let repo_id = self.repo_id.clone();
        self.model.update(cx, |model, cx| {
            model.show_commit_in_history(&repo_id, hash, cx);
        });
    }

    /// "File history" de un archivo: pestaña nueva con su historial desde el
    /// commit del descriptor (o desde HEAD si no trae).
    pub fn open_file_history(&mut self, descriptor: &FileTabDescriptor, cx: &mut Context<Self>) {
        let commit = descriptor
            .commit_hash
            .as_deref()
            .filter(|hash| !hash.is_empty())
            .unwrap_or(&self.commit_hash)
            .to_string();
        let commit = (!commit.is_empty()).then_some(commit);
        self.open_in_new_tab(
            FileTabDescriptor::history(descriptor.path.clone(), commit.as_deref()),
            cx,
        );
    }

    /// Tras "Whole file", el diff nuevo vuelve a la línea que se leía. Dice
    /// si lo hizo (si no, el desplazamiento es el de siempre).
    fn scroll_to_anchor(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(anchor) = self.line_anchor.take() else {
            return false;
        };
        self.diff_viewer
            .update(cx, |viewer, cx| viewer.scroll_to_line(&anchor, cx));
        true
    }

    /// `restoreScroll`: la posición guardada de la pestaña.
    fn restore_scroll(&mut self, cx: &mut Context<Self>) {
        if !self.is_active(cx) {
            return;
        }
        let Some(tab) = self.tab(cx) else {
            return;
        };
        // El historial cambia de diff con cada revisión: no hay un scroll
        // de la pestaña que recordar.
        if tab.source == TabSource::History {
            return;
        }
        let position = ScrollPosition::new(tab.scroll.top as f32, tab.scroll.left as f32);
        if tab.source == TabSource::Tree {
            self.file_viewer
                .update(cx, |viewer, cx| viewer.set_scroll_position(position, cx));
        } else {
            self.diff_viewer
                .update(cx, |viewer, cx| viewer.set_scroll_position(position, cx));
        }
    }

    /// `@view-scroll`: solo la pestaña activa recuerda su scroll.
    fn remember_scroll(&mut self, position: ScrollPosition, cx: &mut Context<Self>) {
        if !self.is_active(cx)
            || self.loading
            || self
                .tab(cx)
                .is_some_and(|tab| tab.source == TabSource::History)
        {
            return;
        }
        let Some(path) = self.repo_path(cx) else {
            return;
        };
        let tab_id = self.tab_id.clone();
        self.model.update(cx, |model, cx| {
            model.remember_workspace_scroll(
                &path,
                &tab_id,
                Scroll {
                    top: f64::from(position.top),
                    left: f64::from(position.left),
                },
                cx,
            );
        });
    }

    /// Clic: este mismo archivo cambia en la pestaña actual.
    pub fn open_here(&mut self, descriptor: FileTabDescriptor, cx: &mut Context<Self>) {
        self.open(descriptor, true, cx);
    }

    /// Doble clic o menú: otra pestaña.
    pub fn open_in_new_tab(&mut self, descriptor: FileTabDescriptor, cx: &mut Context<Self>) {
        self.open(descriptor, false, cx);
    }

    fn open(&mut self, descriptor: FileTabDescriptor, replace: bool, cx: &mut Context<Self>) {
        let Some(path) = self.repo_path(cx) else {
            return;
        };
        self.model.update(cx, |model, cx| {
            model.open_workspace_file(&path, &descriptor, replace, cx);
        });
    }

    /// `emit('close')`: cierra la pestaña.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.repo_path(cx) else {
            return;
        };
        let tab_id = self.tab_id.clone();
        self.model.update(cx, |model, cx| {
            model.close_workspace_tab(&path, &tab_id, cx)
        });
    }

    /// "Open in editor" del menú: la versión del working tree del archivo
    /// (avisa si ya no existe). La línea solo se pasa si el archivo es el que
    /// se ve: los números del diff de un commit pueden no coincidir con los
    /// del working tree, pero son la mejor aproximación.
    pub fn open_in_editor(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(repo_path) = self.repo_path(cx) else {
            return;
        };
        let line = self.editor_line_for(path, cx);
        self.model
            .update(cx, |model, cx| {
                model.open_working_tree_file_in_editor(&repo_path, path, line, cx)
            })
            .detach();
    }

    /// La línea del diff a la vista si `path` es el archivo de la pestaña.
    fn editor_line_for(&self, path: &str, cx: &mut App) -> Option<u32> {
        let shown = self
            .model
            .read(cx)
            .repository(self.repo_id.as_str())
            .map(|repo| repo.path.clone())
            .and_then(|repo_path| {
                self.model
                    .read(cx)
                    .repository_workspace(&repo_path)
                    .tabs
                    .into_iter()
                    .find(|tab| tab.id == self.tab_id)
            })
            .is_some_and(|tab| tab.path == path && tab.source == TabSource::Commit);
        if !shown {
            return None;
        }
        self.diff_viewer
            .update(cx, |viewer, _| viewer.editor_line())
    }

    /// "Open in editor" de la cabecera de la pestaña: el archivo que se ve.
    pub fn open_shown_file_in_editor(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.shown_path(cx) else {
            return;
        };
        if path.is_empty() {
            // El árbol entero: el repositorio.
            if let Some(repo_path) = self.repo_path(cx) {
                self.model
                    .update(cx, |model, cx| model.open_in_editor(&repo_path, None, cx))
                    .detach();
            }
            return;
        }
        self.open_in_editor(&path, cx);
    }

    fn shown_path(&self, cx: &App) -> Option<String> {
        let repo_path = self.repo_path(cx)?;
        self.model
            .read(cx)
            .repository_workspace(&repo_path)
            .tabs
            .into_iter()
            .find(|tab| tab.id == self.tab_id)
            .map(|tab| tab.path)
    }

    /// "Copy Path" del menú.
    pub fn copy_path(&mut self, path: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(path.to_string()));
        self.model.update(cx, |model, cx| {
            model.notify_success("Path copied to clipboard", cx);
        });
    }

    /// La URL web de GitHub del repositorio, si tiene un remoto de GitHub.
    pub fn github_web_url(&self, cx: &App) -> Option<String> {
        let repository = self.model.read(cx).repository(self.repo_id.as_str())?;
        crate::github_link::open_repository_github_url(repository)
    }

    /// "Copy GitHub link" del menú: el archivo en GitHub tal como estaba en
    /// el commit del descriptor (o en el de la pestaña).
    pub fn copy_github_link(&mut self, descriptor: &FileTabDescriptor, cx: &mut Context<Self>) {
        let Some(url) = self.github_web_url(cx) else {
            return;
        };
        let commit = descriptor
            .commit_hash
            .as_deref()
            .filter(|hash| !hash.is_empty())
            .unwrap_or(&self.commit_hash);
        if commit.is_empty() {
            return;
        }
        let link = crate::github_link::github_blob_url(&url, commit, &descriptor.path, None);
        cx.write_to_clipboard(ClipboardItem::new_string(link));
        self.model.update(cx, |model, cx| {
            model.notify_success("GitHub link copied to clipboard", cx);
        });
    }

    fn toggle_diff_view(&mut self, cx: &mut Context<Self>) {
        self.model
            .update(cx, |model, cx| model.toggle_diff_view(cx).detach());
    }

    fn toggle_ignore_whitespace(&mut self, cx: &mut Context<Self>) {
        self.line_anchor = self
            .diff_viewer
            .update(cx, |viewer, _| viewer.line_anchor());
        self.model.update(cx, |model, cx| {
            model.toggle_ignore_whitespace(cx).detach();
        });
    }

    fn toggle_whole_file(&mut self, cx: &mut Context<Self>) {
        self.line_anchor = self
            .diff_viewer
            .update(cx, |viewer, _| viewer.line_anchor());
        self.model.update(cx, |model, cx| {
            model.toggle_whole_file(cx).detach();
        });
    }

    fn handle_resizer_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = match event.keystroke.key.as_str() {
            "left" => -16.,
            "right" => 16.,
            _ => return,
        };
        cx.stop_propagation();
        let width = self.root_width;
        self.model.update(cx, |model, cx| {
            model.nudge_file_list_width(delta, width, cx).detach();
        });
    }

    fn drag_files(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let (pointer, origin, width) = (
            f64::from(f32::from(position.x)),
            f64::from(f32::from(self.root_left)),
            self.root_width,
        );
        let changed = self.model.update(cx, |model, _| {
            model.file_list_width().resizing && model.drag_file_list_resize(pointer, origin, width)
        });
        if changed {
            // El modelo no avisa en cada paso del arrastre: lo que se pinta
            // sale de la foto, que se pone al día aquí.
            let width = *self.model.read(cx).file_list_width();
            if let Some(snapshot) = &mut self.snapshot {
                snapshot.file_list_width = width;
            }
            cx.notify();
        }
    }

    fn stop_files_resize(&mut self, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| {
            if model.file_list_width().resizing {
                model.stop_file_list_resize(cx).detach();
            }
        });
    }

    /// Cabecera del diff de una revisión: hash (lleva al commit), asunto,
    /// autor y fecha.
    fn revision_summary(
        &self,
        revision: &FileRevision,
        shown_path: &str,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let palette = *cx.palette();
        let hash = revision.hash.clone();
        let date = if revision.date.is_empty() {
            String::new()
        } else {
            mergeon_model::format::format_relative_date(revision.date.as_str())
        };
        div()
            .id("revision-summary")
            .flex()
            .items_center()
            .gap(px(8.))
            .min_w_0()
            .px(px(4.))
            .text_size(px(12.))
            .child(
                div()
                    .id("revision-summary-hash")
                    .test_support()
                    .aria_label(SharedString::from(revision.short_hash.clone()))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(4.))
                    .px(px(5.))
                    .h(px(20.))
                    .rounded(px(2.))
                    .border_1()
                    .border_color(palette.border_hair)
                    .bg(palette.bg_inset)
                    .cursor_pointer()
                    .font_family(mergeon_view_diff::code_font_family(cx))
                    .text_size(px(11.5))
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(palette.primary_text)
                    .hover(|style| style.bg(palette.bg_hover))
                    .tooltip(mergeon_ui::tooltip("Show this commit in the history"))
                    .on_click(cx.listener(move |view, _, _, cx| view.show_commit(&hash, cx)))
                    .child(
                        mergeon_ui::icon(Lucide::GitCommitHorizontal)
                            .size(px(12.))
                            .tone(mergeon_ui::IconTone::Current),
                    )
                    .child(revision.short_hash.clone()),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(palette.text_1)
                    .child(revision.subject.clone()),
            )
            // Con otro nombre en esa revisión, cuál.
            .when(revision.path != shown_path, |summary| {
                summary.child(
                    div()
                        .id("revision-summary-path")
                        .test_support()
                        .aria_label(SharedString::from(revision.path.clone()))
                        .flex_shrink_0()
                        .max_w(px(240.))
                        .truncate()
                        .font_family(mergeon_view_diff::code_font_family(cx))
                        .text_size(px(11.))
                        .text_color(palette.text_3)
                        .tooltip(mergeon_ui::tooltip(format!(
                            "The file was {} in this revision",
                            revision.path
                        )))
                        .child(format!("as {}", revision.path)),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(6.))
                    .text_color(palette.text_3)
                    .child(monogram(&revision.author, 16., 8.5, false, cx))
                    .child(
                        div()
                            .text_color(palette.text_2)
                            .child(revision.author.clone()),
                    )
                    .child(date),
            )
            .into_any_element()
    }

    fn render_content(
        &mut self,
        tab: &FileTab,
        preferences: DiffPreferences,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let palette = *cx.palette();
        let state = |text: SharedString| {
            div()
                .id("file-state")
                .test_support()
                .aria_label(text.clone())
                .p(px(24.))
                .text_size(px(13.))
                .text_color(palette.muted_foreground)
                .child(text)
        };

        if tab.source == TabSource::Tree {
            if tab.path.is_empty() {
                return state("Pick a file on the left to see it as it was at this commit.".into())
                    .into_any_element();
            }
            return div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(self.file_viewer.clone())
                .into_any_element();
        }

        if tab.source == TabSource::History && self.revision.is_none() {
            return state(self.history_status(cx)).into_any_element();
        }

        let shown_path = match (&self.revision, tab.source) {
            (Some(revision), TabSource::History) => revision.path.as_str(),
            _ => tab.path.as_str(),
        };
        // Una imagen se ve entera, antes y después: las opciones del diff de
        // texto no le dicen nada. Un SVG con "Text" elegido es texto.
        let image = self.image_viewer.read(cx).shows_image(shown_path);
        let svg_toggle = mergeon_view_diff::is_svg_path(shown_path)
            .then(|| mergeon_view_diff::image_text_toggle(&self.image_viewer, cx));
        let whole_file = preferences.whole_file;
        let toggle_label = match preferences.diff_view_mode {
            DiffViewMode::Unified => "Split",
            DiffViewMode::Split => "Unified",
        };
        let lead: gpui_kit::AnyElement = match (&self.revision, tab.source) {
            (Some(revision), TabSource::History) => self.revision_summary(revision, &tab.path, cx),
            // El modo historial de la pestaña de un commit: el mismo archivo,
            // revisión a revisión.
            _ => mergeon_ui::button("open-file-history")
                .ghost()
                .sm()
                .icon(Lucide::FileClock)
                .label("File history")
                .tooltip("Every commit that touched this file, following renames")
                .on_click(cx.listener(|view, _, _, cx| {
                    if let Some(tab) = view.tab(cx) {
                        view.open_file_history(&FileTabDescriptor::from(&tab), cx);
                    }
                }))
                .into_any_element(),
        };
        let toolbar = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .p(px(5.))
            .border_b_1()
            .border_color(palette.border)
            .child(div().flex().flex_1().min_w_0().items_center().child(lead))
            .children(svg_toggle)
            .when(!image, |toolbar| {
                toolbar.child(
                    mergeon_ui::button("toggle-whole-file")
                        .ghost()
                        .sm()
                        .label("Whole file")
                        .tooltip(mergeon_view_diff::whole_file_tooltip(whole_file))
                        .when(whole_file, |button| {
                            button
                                .bg(with_alpha(palette.primary, 0.14))
                                .text_color(palette.primary_text)
                        })
                        .on_click(cx.listener(|view, _, _, cx| view.toggle_whole_file(cx))),
                )
            })
            .when(!image, |toolbar| {
                toolbar.child(
                    mergeon_ui::button("toggle-ignore-whitespace")
                        .ghost()
                        .sm()
                        .label("Ignore spaces")
                        // El original le pone la clase `active`, pero la regla
                        // que la pinta vive en el CSS de otro componente: el
                        // botón se ve igual encendido o apagado.
                        .on_click(cx.listener(|view, _, _, cx| view.toggle_ignore_whitespace(cx))),
                )
            })
            .when(!image, |toolbar| {
                toolbar.child(
                    mergeon_ui::button("toggle-diff-view")
                        .ghost()
                        .sm()
                        .label(toggle_label)
                        .on_click(cx.listener(|view, _, _, cx| view.toggle_diff_view(cx))),
                )
            });

        let body = if image && self.error.is_empty() {
            div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(self.image_viewer.clone())
                .into_any_element()
        } else if self.loading {
            state("Loading diff…".into()).into_any_element()
        } else if !self.error.is_empty() {
            div()
                .id("file-state")
                .test_support()
                .aria_label(SharedString::from(self.error.clone()))
                .flex()
                .flex_col()
                .items_start()
                .gap_2()
                .p(px(24.))
                .text_size(px(13.))
                .text_color(palette.muted_foreground)
                .child(self.error.clone())
                .child(
                    mergeon_ui::button("retry")
                        .outline()
                        .sm()
                        .label("Retry")
                        .on_click(cx.listener(|view, _, _, cx| view.load(cx))),
                )
                .into_any_element()
        } else if self.too_large {
            state(
                format!(
                    "Diff too large to display (over {})",
                    format_size(self.size as f64)
                )
                .into(),
            )
            .into_any_element()
        } else if self.diff.is_empty() {
            state("No changes to display".into()).into_any_element()
        } else if let (TabSource::History, Some(revision), false) =
            (tab.source, &self.revision, self.revision_has_hunks)
        {
            let text = match (&revision.previous_path, revision.is_rename()) {
                (Some(previous), true) => format!(
                    "Renamed from {previous} to {}, without changes to its content.",
                    revision.path
                ),
                _ => "No changes to the content in this revision.".to_string(),
            };
            state(text.into()).into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(self.diff_viewer.clone())
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(toolbar)
            .child(body)
            .when(!image && self.whole_file_fell_back, |this| {
                this.child(
                    div()
                        .id("file-whole-file-fallback")
                        .test_support()
                        .flex_none()
                        .py(px(5.))
                        .px(px(12.))
                        .border_t_1()
                        .border_color(palette.border)
                        .text_size(px(11.))
                        .text_color(palette.muted_foreground)
                        .child("File too large for Whole file — showing changes only."),
                )
            })
            .into_any_element()
    }
}

/// El menú de un archivo de la lista lateral.
fn file_menu(
    view: &gpui_kit::WeakEntity<HistoricalFileTabView>,
    descriptor: FileTabDescriptor,
    on_github: bool,
) -> Vec<MenuItem> {
    let here = (view.clone(), descriptor.clone());
    let new_tab = (view.clone(), descriptor.clone());
    let copy = (view.clone(), descriptor.path.clone());
    let link = (view.clone(), descriptor.clone());
    let history = (view.clone(), descriptor.clone());
    let editor = (view.clone(), descriptor.path.clone());
    let mut items = vec![
        MenuItem::new("Open here")
            .icon(Lucide::FileCodeCorner)
            .on_click(move |_, cx| {
                let (view, descriptor) = here.clone();
                let _ = view.update(cx, |view, cx| view.open_here(descriptor, cx));
            }),
        MenuItem::new("Open in new tab")
            .icon(Lucide::SquarePlus)
            .on_click(move |_, cx| {
                let (view, descriptor) = new_tab.clone();
                let _ = view.update(cx, |view, cx| view.open_in_new_tab(descriptor, cx));
            }),
        MenuItem::new("Open in editor")
            .icon(Lucide::CodeXml)
            .on_click(move |_, cx| {
                let (view, path) = editor.clone();
                let _ = view.update(cx, |view, cx| view.open_in_editor(&path, cx));
            }),
        MenuItem::separator(),
        MenuItem::new("File history")
            .icon(Lucide::FileClock)
            .on_click(move |_, cx| {
                let (view, descriptor) = history.clone();
                let _ = view.update(cx, |view, cx| view.open_file_history(&descriptor, cx));
            }),
        MenuItem::separator(),
        MenuItem::new("Copy Path")
            .icon(Lucide::Copy)
            .on_click(move |_, cx| {
                let (view, path) = copy.clone();
                let _ = view.update(cx, |view, cx| view.copy_path(&path, cx));
            }),
    ];
    if on_github {
        items.push(
            MenuItem::new("Copy GitHub link")
                .icon(Lucide::Link2)
                .on_click(move |_, cx| {
                    let (view, descriptor) = link.clone();
                    let _ = view.update(cx, |view, cx| view.copy_github_link(&descriptor, cx));
                }),
        );
    }
    items
}

impl Render for HistoricalFileTabView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let palette = *cx.palette();
        // Lo del modelo sale de la foto del último aviso, no de leerlo aquí:
        // leerlo al pintar haría que cualquier aviso del modelo (un toast,
        // otra pestaña) repintara la pestaña.
        let Some(snapshot) = self.snapshot.clone() else {
            return div().id("historical-file").into_any_element();
        };
        let Some(tab) = snapshot.tab else {
            return div().id("historical-file").into_any_element();
        };
        let show_files = self.shows_files_for(Some(&tab));
        let (list_width, resizing) = (
            snapshot.file_list_width.width,
            snapshot.file_list_width.resizing,
        );
        let lit = with_alpha(palette.primary, 0.42);
        let resizer_focused = self.resizer_focus.is_focused(window);

        let entity = cx.weak_entity();
        let measure = {
            let entity = entity.clone();
            canvas(
                move |bounds, _, cx| {
                    let _ = entity.update(cx, |view, _| {
                        view.root_left = bounds.left();
                        view.root_width = f64::from(f32::from(bounds.size.width));
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };

        let side: gpui_kit::AnyElement = match tab.source {
            TabSource::Tree => self.tree.clone().into_any_element(),
            TabSource::History => self.history.clone().into_any_element(),
            _ => self.changes.clone().into_any_element(),
        };

        let content = self.render_content(&tab, snapshot.preferences, cx);

        div()
            .id("historical-file")
            .relative()
            .flex()
            .flex_1()
            .size_full()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .line_height(relative(1.5))
            .child(measure)
            .child(drag_tracker(
                resizing,
                entity,
                |view, position, _, cx| view.drag_files(position, cx),
                |view, _, cx| view.stop_files_resize(cx),
            ))
            .when(show_files, |root| {
                root.child(
                    div()
                        .id("context-files")
                        .flex_none()
                        .w(px(list_width as f32))
                        .h_full()
                        .min_w_0()
                        .overflow_hidden()
                        .child(side),
                )
                .child(
                    div()
                        .id("files-resizer")
                        .test_support()
                        .aria_label("Resize file list")
                        .track_focus(&self.resizer_focus)
                        .flex_none()
                        .w(px(RESIZER_WIDTH))
                        .h_full()
                        .cursor_col_resize()
                        .bg(if resizing || resizer_focused {
                            lit
                        } else {
                            with_alpha(palette.border, 0.55)
                        })
                        .hover(move |style| style.bg(lit))
                        .tooltip(mergeon_ui::tooltip(
                            "Drag to resize · double-click to reset",
                        ))
                        .on_key_down(cx.listener(Self::handle_resizer_key))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|view, event: &gpui_kit::MouseDownEvent, window, cx| {
                                window.prevent_default();
                                if event.click_count >= 2 {
                                    view.model.update(cx, |model, cx| {
                                        model.reset_file_list_width(cx).detach();
                                    });
                                } else {
                                    view.model
                                        .update(cx, |model, cx| model.start_file_list_resize(cx));
                                }
                            }),
                        ),
                )
            })
            .child(
                div()
                    .id("historical-content")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .child(content),
            )
            .into_any_element()
    }
}
