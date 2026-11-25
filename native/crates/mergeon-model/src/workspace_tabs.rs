//! Pestañas del espacio de trabajo de un repositorio (`src/lib/workspaceTabs.js`):
//! las dos vistas fijas (Commits y Local Changes) más una pestaña por archivo
//! abierto (los "documentos").
//!
//! Como en VS Code, a lo sumo un documento por repositorio es la pestaña de
//! vista previa ([`FileTab::preview`]): abrir otro archivo desde Commits (o la
//! paleta) la reutiliza en vez de acumular pestañas, y fijarla (doble clic,
//! "Keep open" o arrastrarla) la vuelve permanente.
//!
//! La sesión guardada llega como JSON sin garantías, así que la normalización
//! trabaja sobre `serde_json::Value`; deserializar un [`RepositoryWorkspace`]
//! pasa siempre por ella.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::repositories::distinguishing_parent_suffixes;

pub const HOME_TAB: &str = "commits";
pub const CHANGES_TAB: &str = "changes";

pub fn is_fixed_tab(id: &str) -> bool {
    id == HOME_TAB || id == CHANGES_TAB
}

/// De dónde sale el contenido de una pestaña de archivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabSource {
    /// Working tree frente al índice.
    Working,
    /// Índice frente a HEAD.
    Index,
    /// Diff de un archivo en un commit.
    Commit,
    /// Árbol (o archivo) de un commit.
    Tree,
    /// Historial del archivo siguiendo sus renombres ("File history"). El
    /// commit es la revisión desde la que se recorre; vacío, `HEAD`.
    History,
}

impl TabSource {
    pub fn as_str(self) -> &'static str {
        match self {
            TabSource::Working => "working",
            TabSource::Index => "index",
            TabSource::Commit => "commit",
            TabSource::Tree => "tree",
            TabSource::History => "history",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "working" => Some(TabSource::Working),
            "index" => Some(TabSource::Index),
            "commit" => Some(TabSource::Commit),
            "tree" => Some(TabSource::Tree),
            "history" => Some(TabSource::History),
            _ => None,
        }
    }

    fn needs_commit(self) -> bool {
        matches!(self, TabSource::Commit | TabSource::Tree)
    }

    fn default_origin(self) -> &'static str {
        match self {
            TabSource::Working | TabSource::Index => CHANGES_TAB,
            TabSource::Commit | TabSource::Tree | TabSource::History => HOME_TAB,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Scroll {
    pub top: f64,
    pub left: f64,
}

fn normalize_offset(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

impl Scroll {
    /// Posición válida: finita y no negativa.
    pub fn normalized(self) -> Self {
        Self {
            top: normalize_offset(self.top),
            left: normalize_offset(self.left),
        }
    }
}

/// `normalizeScroll` sobre el valor crudo de la sesión guardada.
pub fn normalize_scroll(value: Option<&Value>) -> Scroll {
    let offset = |key: &str| {
        value
            .and_then(|value| value.get(key))
            .and_then(Value::as_f64)
            .map_or(0.0, normalize_offset)
    };

    Scroll {
        top: offset("top"),
        left: offset("left"),
    }
}

/// Identificador de una pestaña de archivo: `JSON.stringify([source, commitHash, path])`.
pub fn file_tab_id(source: TabSource, commit_hash: &str, path: &str) -> String {
    serde_json::to_string(&[source.as_str(), commit_hash, path]).unwrap_or_default()
}

/// Lo que se pide al abrir un archivo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTabDescriptor {
    pub path: String,
    pub source: TabSource,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub commit_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub scroll: Option<Scroll>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_tab: Option<String>,
}

impl FileTabDescriptor {
    pub fn new(source: TabSource, path: impl Into<String>, commit_hash: Option<&str>) -> Self {
        Self {
            path: path.into(),
            source,
            commit_hash: commit_hash.map(str::to_string),
            scroll: None,
            origin_tab: None,
        }
    }

    pub fn working(path: impl Into<String>) -> Self {
        Self::new(TabSource::Working, path, None)
    }

    pub fn index(path: impl Into<String>) -> Self {
        Self::new(TabSource::Index, path, None)
    }

    pub fn commit(path: impl Into<String>, commit_hash: &str) -> Self {
        Self::new(TabSource::Commit, path, Some(commit_hash))
    }

    pub fn tree(path: impl Into<String>, commit_hash: &str) -> Self {
        Self::new(TabSource::Tree, path, Some(commit_hash))
    }

    /// "File history" de `path` desde `commit_hash` (o desde `HEAD`, que
    /// sigue a la rama: la ruta es la del working tree).
    pub fn history(path: impl Into<String>, commit_hash: Option<&str>) -> Self {
        Self::new(
            TabSource::History,
            path,
            commit_hash.filter(|hash| !hash.is_empty()),
        )
    }

    /// `fileTabId(descriptor)`.
    pub fn id(&self) -> String {
        file_tab_id(
            self.source,
            self.commit_hash.as_deref().unwrap_or_default(),
            &self.path,
        )
    }
}

impl From<&FileTab> for FileTabDescriptor {
    fn from(tab: &FileTab) -> Self {
        Self {
            path: tab.path.clone(),
            source: tab.source,
            commit_hash: Some(tab.commit_hash.clone()),
            scroll: Some(tab.scroll),
            origin_tab: Some(tab.origin_tab.clone()),
        }
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Pestaña de archivo abierta.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTab {
    pub path: String,
    pub source: TabSource,
    /// Vacío en las pestañas de working tree e índice.
    pub commit_hash: String,
    pub scroll: Scroll,
    /// Vista fija a la que se vuelve al cerrarla (`commits` o `changes`).
    pub origin_tab: String,
    pub id: String,
    /// Pestaña de vista previa: la reutiliza el próximo archivo que se abra
    /// desde Commits o la paleta. Se pinta en cursiva. Las sesiones viejas
    /// no la traen (permanente) y solo se guarda cuando es verdad.
    #[serde(default, skip_serializing_if = "is_false")]
    pub preview: bool,
}

impl FileTab {
    pub fn context(&self) -> String {
        tab_context(self.source, &self.commit_hash, &self.path)
    }

    pub fn label(&self) -> &str {
        tab_label(&self.path)
    }
}

fn is_commit_hash(text: &str) -> bool {
    // /^[a-f0-9]{40,64}$/i
    (40..=64).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn build_tab(
    source: TabSource,
    path: &str,
    commit_hash: &str,
    scroll: Scroll,
    origin_tab: Option<&str>,
) -> Option<FileTab> {
    // Solo el árbol admite ruta vacía: es la pestaña "Repo tree" de un commit,
    // sin archivo elegido todavía.
    if path.is_empty() && source != TabSource::Tree {
        return None;
    }
    if source.needs_commit() && !is_commit_hash(commit_hash) {
        return None;
    }
    // El historial parte de un commit concreto o, sin él, de HEAD.
    if source == TabSource::History && !commit_hash.is_empty() && !is_commit_hash(commit_hash) {
        return None;
    }

    Some(FileTab {
        path: path.to_string(),
        source,
        commit_hash: commit_hash.to_string(),
        scroll,
        origin_tab: origin_tab
            .filter(|origin| is_fixed_tab(origin))
            .unwrap_or_else(|| source.default_origin())
            .to_string(),
        id: file_tab_id(source, commit_hash, path),
        preview: false,
    })
}

/// Pestaña válida a partir de un descriptor, o `None` si no describe nada que
/// se pueda abrir (ruta vacía, commit sin hash completo...).
pub fn normalize_file_tab(descriptor: &FileTabDescriptor) -> Option<FileTab> {
    build_tab(
        descriptor.source,
        &descriptor.path,
        descriptor.commit_hash.as_deref().unwrap_or_default(),
        descriptor.scroll.unwrap_or_default().normalized(),
        descriptor.origin_tab.as_deref(),
    )
}

fn normalize_tab_candidate(candidate: &Value) -> Option<FileTab> {
    let source = candidate
        .get("source")
        .and_then(Value::as_str)
        .and_then(TabSource::parse)?;
    let path = candidate.get("path").and_then(Value::as_str)?;
    let commit_hash = candidate
        .get("commitHash")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let mut tab = build_tab(
        source,
        path,
        commit_hash,
        normalize_scroll(candidate.get("scroll")),
        candidate.get("originTab").and_then(Value::as_str),
    )?;
    tab.preview = candidate
        .get("preview")
        .and_then(Value::as_bool)
        .unwrap_or_default();
    Some(tab)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceSection {
    #[default]
    Commits,
    Changes,
}

impl WorkspaceSection {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkspaceSection::Commits => "commits",
            WorkspaceSection::Changes => "changes",
        }
    }
}

/// Estado de las pestañas de un repositorio, tal como se guarda en la sesión.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryWorkspace {
    /// Siempre 2 tras normalizar.
    pub layout_version: u32,
    pub tabs: Vec<FileTab>,
    /// `commits`, `changes` o el `id` de una pestaña de archivo.
    pub active_tab: String,
    pub section: WorkspaceSection,
    pub selected_commit: Option<String>,
    /// Mensaje de commit a medio escribir en Local Changes. Se guarda en la
    /// sesión para que sobreviva a cerrar el repositorio o la aplicación.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub commit_draft: String,
    /// "Sign" de la caja de commit: firmar los commits de este repositorio.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub commit_sign: bool,
}

impl Default for RepositoryWorkspace {
    /// `normalizeRepositoryWorkspace()` sin argumentos.
    fn default() -> Self {
        Self {
            layout_version: 2,
            tabs: Vec::new(),
            active_tab: HOME_TAB.to_string(),
            section: WorkspaceSection::Commits,
            selected_commit: None,
            commit_draft: String::new(),
            commit_sign: false,
        }
    }
}

impl<'de> Deserialize<'de> for RepositoryWorkspace {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(normalize_repository_workspace(&value))
    }
}

/// Deja un estado guardado (posiblemente viejo o corrupto) en una forma válida:
/// descarta pestañas que ya no describen nada y recoloca la pestaña activa.
pub fn normalize_repository_workspace(value: &Value) -> RepositoryWorkspace {
    let mut tabs: Vec<FileTab> = Vec::new();
    for candidate in value
        .get("tabs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(tab) = normalize_tab_candidate(candidate) else {
            continue;
        };
        if !tabs.iter().any(|item| item.id == tab.id) {
            tabs.push(tab);
        }
    }
    // A lo sumo una pestaña de vista previa: la última que se guardó así.
    if let Some(last) = tabs.iter().rposition(|tab| tab.preview) {
        for (index, tab) in tabs.iter_mut().enumerate() {
            tab.preview = index == last;
        }
    }

    let section = value.get("section").and_then(Value::as_str);
    // Older sessions used Commits as a container for both fixed views.
    let fallback = if section == Some("changes") {
        CHANGES_TAB
    } else {
        HOME_TAB
    };
    let is_current_layout = value.get("layoutVersion").and_then(Value::as_f64) == Some(2.0);
    let requested = value.get("activeTab").and_then(Value::as_str);
    let active_tab = match requested {
        Some(HOME_TAB) if !is_current_layout => fallback,
        Some(tab) if is_fixed_tab(tab) || tabs.iter().any(|item| item.id == tab) => tab,
        _ => fallback,
    };

    RepositoryWorkspace {
        layout_version: 2,
        active_tab: active_tab.to_string(),
        tabs,
        section: if section == Some("changes") {
            WorkspaceSection::Changes
        } else {
            WorkspaceSection::Commits
        },
        selected_commit: value
            .get("selectedCommit")
            .and_then(Value::as_str)
            .map(str::to_string),
        commit_draft: value
            .get("commitDraft")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        commit_sign: value
            .get("commitSign")
            .and_then(Value::as_bool)
            .unwrap_or_default(),
    }
}

pub fn open_file_tab(workspace: &mut RepositoryWorkspace, descriptor: &FileTabDescriptor) {
    let Some(mut tab) = normalize_file_tab(descriptor) else {
        return;
    };

    if is_fixed_tab(&workspace.active_tab) {
        tab.origin_tab = workspace.active_tab.clone();
    } else if let Some(current_file) = workspace
        .tabs
        .iter()
        .find(|item| item.id == workspace.active_tab)
        && !current_file.origin_tab.is_empty()
    {
        tab.origin_tab = current_file.origin_tab.clone();
    }

    workspace.active_tab = tab.id.clone();
    match workspace.tabs.iter_mut().find(|item| item.id == tab.id) {
        Some(existing) => existing.origin_tab = tab.origin_tab,
        None => workspace.tabs.push(tab),
    }
}

/// Abre `descriptor` como pestaña de vista previa: si ya está abierto, solo
/// se activa (sin tocar si es permanente); si hay otra de vista previa, el
/// archivo nuevo ocupa su lugar; si no, se abre una nueva al final. Devuelve
/// el id de la pestaña de vista previa sustituida, si la hubo.
pub fn open_preview_file_tab(
    workspace: &mut RepositoryWorkspace,
    descriptor: &FileTabDescriptor,
) -> Option<String> {
    let id = descriptor.id();
    if workspace.tabs.iter().any(|item| item.id == id) {
        open_file_tab(workspace, descriptor);
        return None;
    }
    let mut tab = normalize_file_tab(descriptor)?;
    if is_fixed_tab(&workspace.active_tab) {
        tab.origin_tab = workspace.active_tab.clone();
    } else if let Some(current) = workspace
        .tabs
        .iter()
        .find(|item| item.id == workspace.active_tab)
    {
        tab.origin_tab = current.origin_tab.clone();
    }
    tab.preview = true;
    workspace.active_tab = tab.id.clone();
    match workspace.tabs.iter_mut().find(|item| item.preview) {
        Some(previous) => Some(std::mem::replace(previous, tab).id),
        None => {
            workspace.tabs.push(tab);
            None
        }
    }
}

/// Fija una pestaña de archivo: deja de ser la de vista previa. `false` si
/// no existía o ya era permanente.
pub fn pin_file_tab(workspace: &mut RepositoryWorkspace, id: &str) -> bool {
    match workspace
        .tabs
        .iter_mut()
        .find(|tab| tab.id == id && tab.preview)
    {
        Some(tab) => {
            tab.preview = false;
            true
        }
        None => false,
    }
}

/// Mueve una pestaña de archivo de `from` a `to` (arrastrar y soltar). La
/// que se arrastra queda fijada, como en los editores.
pub fn move_file_tab(workspace: &mut RepositoryWorkspace, from: usize, to: usize) -> bool {
    let count = workspace.tabs.len();
    if from >= count || to >= count {
        return false;
    }
    let mut tab = workspace.tabs.remove(from);
    tab.preview = false;
    workspace.tabs.insert(to, tab);
    true
}

/// Un clic dentro de una pestaña de archivo cambia el archivo de esa misma
/// pestaña, como un editor en modo vista previa; abrir otra pestaña queda para
/// el doble clic o el menú. Si el archivo ya está abierto en otra pestaña se va
/// a esa, para no tener dos pestañas del mismo archivo. Devuelve la pestaña que
/// se sustituyó, y así el doble clic puede devolverla a su sitio.
pub fn replace_file_tab(
    workspace: &mut RepositoryWorkspace,
    id: &str,
    descriptor: &FileTabDescriptor,
) -> Option<FileTab> {
    let tab = normalize_file_tab(descriptor)?;
    let index = workspace.tabs.iter().position(|item| item.id == id)?;

    if let Some(existing) = workspace.tabs.iter().find(|item| item.id == tab.id) {
        workspace.active_tab = existing.id.clone();
        return None;
    }

    workspace.active_tab = tab.id.clone();
    let origin_tab = workspace.tabs[index].origin_tab.clone();
    let preview = workspace.tabs[index].preview;
    Some(std::mem::replace(
        &mut workspace.tabs[index],
        FileTab {
            origin_tab,
            preview,
            ..tab
        },
    ))
}

/// Una pestaña cambió de identidad sin dejar de ser la misma pestaña: lo que el
/// original avisa con `onReplace(from, to)` para que el panel siga montado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabReplacement {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq)]
struct ReplacedTab {
    previous: FileTab,
    id: String,
}

/// Abre archivos desde una pestaña de archivo (`createFileOpener`). Un clic
/// (`replace`) reutiliza la pestaña actual; sin `replace` se abre otra. Pero el
/// doble clic llega después de sus dos clics, que ya habrán cambiado la pestaña
/// actual: se recuerda qué había antes y, si acto seguido se pide ese mismo
/// archivo en pestaña nueva, la pestaña recupera su archivo y el nuevo se abre
/// aparte.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileOpener {
    replaced: Option<ReplacedTab>,
}

impl FileOpener {
    pub fn new() -> Self {
        Self::default()
    }

    fn replace(
        workspace: &mut RepositoryWorkspace,
        descriptor: &FileTabDescriptor,
    ) -> (Option<FileTab>, Option<TabReplacement>) {
        let current = workspace.active_tab.clone();
        let previous = replace_file_tab(workspace, &current, descriptor);
        let moved = previous.as_ref().map(|previous| TabReplacement {
            from: previous.id.clone(),
            to: workspace.active_tab.clone(),
        });
        (previous, moved)
    }

    /// Abre `descriptor`. Devuelve qué pestaña tomó el lugar de cuál, si alguna
    /// lo hizo.
    pub fn open(
        &mut self,
        workspace: &mut RepositoryWorkspace,
        descriptor: &FileTabDescriptor,
        replace: bool,
    ) -> Option<TabReplacement> {
        if replace {
            // El segundo clic del doble clic pide el archivo que ya se muestra:
            // no hay nada que cambiar ni que olvidar.
            if descriptor.id() == workspace.active_tab {
                return None;
            }
            let (previous, moved) = Self::replace(workspace, descriptor);
            self.replaced = previous.map(|previous| ReplacedTab {
                previous,
                id: workspace.active_tab.clone(),
            });
            return moved;
        }

        let mut moved = None;
        if let Some(replaced) = self.replaced.take()
            && replaced.id == workspace.active_tab
            && replaced.id == descriptor.id()
        {
            moved = Self::replace(workspace, &FileTabDescriptor::from(&replaced.previous)).1;
            // El doble clic pide una pestaña aparte: esa es permanente.
            open_file_tab(workspace, descriptor);
            pin_file_tab(workspace, &descriptor.id());
            return moved;
        }
        // Desde Commits un archivo se abre en la pestaña de vista previa;
        // desde Local Changes o desde otro archivo ("Open in new tab"), en
        // una permanente.
        if workspace.active_tab == HOME_TAB {
            open_preview_file_tab(workspace, descriptor);
        } else {
            open_file_tab(workspace, descriptor);
        }
        moved
    }

    /// Cambiar de pestaña por otro camino cierra la ventana del doble clic.
    pub fn sync(&mut self, workspace: &RepositoryWorkspace) {
        if self
            .replaced
            .as_ref()
            .is_some_and(|replaced| replaced.id != workspace.active_tab)
        {
            self.replaced = None;
        }
    }
}

/// Cierra una pestaña de archivo. Si era la activa pasa a serlo su vecina
/// de la derecha (o, siendo la última, la de la izquierda); solo al cerrar el
/// último documento se vuelve a la vista fija desde la que se abrió.
pub fn close_file_tab(workspace: &mut RepositoryWorkspace, id: &str) {
    let Some(index) = workspace.tabs.iter().position(|tab| tab.id == id) else {
        return;
    };
    let closed = workspace.tabs.remove(index);
    if workspace.active_tab == id {
        workspace.active_tab = match workspace.tabs.get(index).or_else(|| workspace.tabs.last()) {
            Some(neighbour) => neighbour.id.clone(),
            None if is_fixed_tab(&closed.origin_tab) => closed.origin_tab,
            None => HOME_TAB.to_string(),
        };
    }
}

/// El documento siguiente (o anterior) a la pestaña activa, dando la
/// vuelta. Desde una vista fija, el primero (o el último). `None` sin
/// documentos abiertos.
pub fn adjacent_file_tab(workspace: &RepositoryWorkspace, forward: bool) -> Option<&FileTab> {
    let count = workspace.tabs.len();
    if count == 0 {
        return None;
    }
    let current = workspace
        .tabs
        .iter()
        .position(|tab| tab.id == workspace.active_tab);
    let next = match (current, forward) {
        (Some(index), true) => (index + 1) % count,
        (Some(index), false) => (index + count - 1) % count,
        (None, true) => 0,
        (None, false) => count - 1,
    };
    workspace.tabs.get(next)
}

/// La carpeta que distingue a cada pestaña de otra abierta con el mismo
/// nombre de archivo (dos `mod.rs`: `view` y `model`). Las pestañas del mismo
/// archivo (otra revisión, índice frente a working tree) no cuentan: ya las
/// distingue su contexto.
pub fn file_tab_suffixes(tabs: &[FileTab]) -> Vec<Option<String>> {
    let mut unique: Vec<&str> = Vec::new();
    for tab in tabs {
        if !tab.path.is_empty() && !unique.contains(&tab.path.as_str()) {
            unique.push(&tab.path);
        }
    }
    let suffixes =
        distinguishing_parent_suffixes(unique.iter().map(|path| (tab_label(path), *path)));
    tabs.iter()
        .map(|tab| {
            unique
                .iter()
                .position(|path| *path == tab.path)
                .and_then(|index| suffixes[index].clone())
        })
        .collect()
}

/// Migas de la ruta de un archivo: las carpetas y el nombre (`src/view/a.rs`
/// → `["src", "view", "a.rs"]`). Vacío en el árbol sin archivo.
pub fn path_breadcrumbs(path: &str) -> Vec<&str> {
    path.split('/').filter(|part| !part.is_empty()).collect()
}

/// De dónde viene lo que muestra la pestaña: "Local", "Staged" o el commit.
pub fn tab_context(source: TabSource, commit_hash: &str, path: &str) -> String {
    let kind = match source {
        TabSource::Working => return "Local".to_string(),
        TabSource::Index => return "Staged".to_string(),
        TabSource::Commit => "Diff",
        TabSource::Tree if !path.is_empty() => "File",
        TabSource::Tree => "Tree",
        TabSource::History if commit_hash.is_empty() => return "History".to_string(),
        TabSource::History => "History",
    };
    let short_hash: String = commit_hash.chars().take(7).collect();
    format!("{short_hash} · {kind}")
}

pub fn tab_label(path: &str) -> &str {
    if path.is_empty() {
        return "Repo tree";
    }
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hash(character: char) -> String {
        character.to_string().repeat(40)
    }

    fn paths(workspace: &RepositoryWorkspace) -> Vec<&str> {
        workspace.tabs.iter().map(|tab| tab.path.as_str()).collect()
    }

    fn reload(workspace: &RepositoryWorkspace) -> RepositoryWorkspace {
        serde_json::from_str(&serde_json::to_string(workspace).unwrap()).unwrap()
    }

    #[test]
    fn deduplicates_a_file_version_while_retaining_different_revisions_and_index_worktree_sides() {
        let mut state = RepositoryWorkspace::default();
        let file = FileTabDescriptor::commit("src/app.js", &hash('a'));
        open_file_tab(&mut state, &file);
        open_file_tab(
            &mut state,
            &FileTabDescriptor::commit("src/app.js", &hash('b')),
        );
        open_file_tab(&mut state, &FileTabDescriptor::working("src/app.js"));
        open_file_tab(&mut state, &FileTabDescriptor::index("src/app.js"));
        open_file_tab(&mut state, &file);

        assert_eq!(state.tabs.len(), 4);
        assert_eq!(state.active_tab, state.tabs[0].id);
    }

    #[test]
    fn restores_a_session_and_drops_invalid_historical_references_without_losing_valid_tabs() {
        let mut state = RepositoryWorkspace::default();
        open_file_tab(&mut state, &FileTabDescriptor::working("src/app.js"));

        assert_eq!(reload(&state), state);

        let mut tabs = serde_json::to_value(&state.tabs).unwrap();
        tabs.as_array_mut()
            .unwrap()
            .push(json!({ "path": "bad", "source": "commit", "commitHash": "HEAD" }));
        assert_eq!(
            normalize_repository_workspace(&json!({ "tabs": tabs })).tabs,
            state.tabs
        );

        assert_eq!(
            normalize_repository_workspace(&json!({})).active_tab,
            HOME_TAB
        );
        assert_eq!(
            normalize_repository_workspace(&Value::Null).active_tab,
            HOME_TAB
        );
        assert_eq!(
            normalize_repository_workspace(&json!({})),
            RepositoryWorkspace::default()
        );
    }

    #[test]
    fn closes_only_the_requested_tab_and_returns_to_commits_after_the_last_file_closes() {
        let mut state = RepositoryWorkspace::default();
        open_file_tab(&mut state, &FileTabDescriptor::working("one.js"));
        open_file_tab(&mut state, &FileTabDescriptor::working("two.js"));
        let second = state.active_tab.clone();
        let first = state.tabs[0].id.clone();

        close_file_tab(&mut state, &first);
        assert_eq!(state.active_tab, second);
        close_file_tab(&mut state, &second);
        assert_eq!(state.active_tab, HOME_TAB);
        close_file_tab(&mut state, HOME_TAB);
        assert_eq!(state.tabs, []);
    }

    #[test]
    fn migrates_the_former_combined_commits_view_to_the_separate_local_changes_tab() {
        let active = |value: Value| normalize_repository_workspace(&value).active_tab;

        assert_eq!(
            active(json!({ "activeTab": HOME_TAB, "section": "changes" })),
            CHANGES_TAB
        );
        assert_eq!(
            active(json!({ "layoutVersion": 2, "activeTab": HOME_TAB, "section": "changes" })),
            HOME_TAB
        );
        assert_eq!(active(json!({ "activeTab": CHANGES_TAB })), CHANGES_TAB);
    }

    #[test]
    fn closing_a_file_moves_to_its_neighbour_and_only_the_last_one_returns_to_its_view() {
        let mut state = RepositoryWorkspace::default();
        open_file_tab(
            &mut state,
            &FileTabDescriptor::commit("history.js", &hash('a')),
        );
        state.active_tab = CHANGES_TAB.to_string();
        open_file_tab(&mut state, &FileTabDescriptor::working("local.js"));
        open_file_tab(&mut state, &FileTabDescriptor::working("other.js"));
        let [history, local, other] = [0, 1, 2].map(|index| state.tabs[index].id.clone());

        let mut restored = reload(&state);
        // La del medio: pasa a la de su derecha.
        restored.active_tab = local.clone();
        close_file_tab(&mut restored, &local);
        assert_eq!(restored.active_tab, other);
        // La última: pasa a la de su izquierda.
        close_file_tab(&mut restored, &other);
        assert_eq!(restored.active_tab, history);
        // Las vistas fijas no se cierran.
        close_file_tab(&mut restored, CHANGES_TAB);
        assert_eq!(restored.tabs.len(), 1);
        // El último documento vuelve a la vista desde la que se abrió.
        close_file_tab(&mut restored, &history);
        assert_eq!(restored.active_tab, HOME_TAB);
        assert!(restored.tabs.is_empty());

        let mut state = RepositoryWorkspace {
            active_tab: CHANGES_TAB.to_string(),
            ..Default::default()
        };
        open_file_tab(&mut state, &FileTabDescriptor::working("only.js"));
        let only = state.active_tab.clone();
        close_file_tab(&mut state, &only);
        assert_eq!(state.active_tab, CHANGES_TAB);
    }

    #[test]
    fn files_opened_from_commits_share_one_preview_tab_until_it_is_pinned() {
        let mut state = RepositoryWorkspace::default();
        let mut opener = FileOpener::new();
        let one = FileTabDescriptor::commit("src/one.rs", &hash('a'));
        let two = FileTabDescriptor::commit("src/two.rs", &hash('a'));
        let three = FileTabDescriptor::commit("src/three.rs", &hash('a'));

        opener.open(&mut state, &one, false);
        assert!(state.tabs[0].preview);
        state.active_tab = HOME_TAB.to_string();
        opener.open(&mut state, &two, false);
        // La de vista previa pasó a mostrar el otro archivo.
        assert_eq!(paths(&state), ["src/two.rs"]);
        assert!(state.tabs[0].preview);
        assert_eq!(state.active_tab, two.id());

        assert!(pin_file_tab(&mut state, &two.id()));
        assert!(!pin_file_tab(&mut state, &two.id()));
        state.active_tab = HOME_TAB.to_string();
        opener.open(&mut state, &three, false);
        assert_eq!(paths(&state), ["src/two.rs", "src/three.rs"]);
        assert_eq!(
            state.tabs.iter().map(|tab| tab.preview).collect::<Vec<_>>(),
            [false, true]
        );

        // Volver a pedir uno abierto solo lo activa, sin volverlo preview.
        state.active_tab = HOME_TAB.to_string();
        opener.open(&mut state, &two, false);
        assert_eq!(state.active_tab, two.id());
        assert!(!state.tabs[0].preview);

        // Desde Local Changes o desde otro archivo se abre una permanente.
        state.active_tab = CHANGES_TAB.to_string();
        opener.open(&mut state, &FileTabDescriptor::working("a.rs"), false);
        assert!(!state.tabs[2].preview);

        // La paleta reutiliza la de vista previa aunque haya un archivo activo.
        let replaced = open_preview_file_tab(&mut state, &FileTabDescriptor::working("b.rs"));
        assert_eq!(replaced, Some(three.id()));
        assert_eq!(paths(&state), ["src/two.rs", "b.rs", "a.rs"]);
        assert_eq!(state.tabs.iter().filter(|tab| tab.preview).count(), 1);
    }

    #[test]
    fn the_preview_flag_survives_a_reload_but_old_sessions_have_none() {
        let mut state = RepositoryWorkspace::default();
        open_preview_file_tab(&mut state, &FileTabDescriptor::working("a.rs"));
        assert!(reload(&state).tabs[0].preview);
        assert_eq!(
            serde_json::to_value(&state).unwrap()["tabs"][0]["preview"],
            true
        );

        let old = normalize_repository_workspace(&json!({
            "tabs": [{ "path": "a.rs", "source": "working" }]
        }));
        assert!(!old.tabs[0].preview);

        // Una sesión corrupta con dos: queda solo la última.
        let two = normalize_repository_workspace(&json!({
            "tabs": [
                { "path": "a.rs", "source": "working", "preview": true },
                { "path": "b.rs", "source": "working", "preview": true }
            ]
        }));
        assert_eq!(
            two.tabs.iter().map(|tab| tab.preview).collect::<Vec<_>>(),
            [false, true]
        );
    }

    #[test]
    fn dragging_a_tab_moves_and_pins_it() {
        let mut state = RepositoryWorkspace::default();
        open_file_tab(&mut state, &FileTabDescriptor::working("a.rs"));
        open_preview_file_tab(&mut state, &FileTabDescriptor::working("b.rs"));
        assert!(move_file_tab(&mut state, 1, 0));
        assert_eq!(paths(&state), ["b.rs", "a.rs"]);
        assert!(!state.tabs[0].preview);
        assert!(!move_file_tab(&mut state, 0, 5));
    }

    #[test]
    fn walks_the_documents_in_order_from_wherever_the_user_is() {
        let mut state = RepositoryWorkspace::default();
        assert!(adjacent_file_tab(&state, true).is_none());
        open_file_tab(&mut state, &FileTabDescriptor::working("a.rs"));
        open_file_tab(&mut state, &FileTabDescriptor::working("b.rs"));
        let label = |tab: Option<&FileTab>| tab.map(|tab| tab.path.clone());

        assert_eq!(label(adjacent_file_tab(&state, true)), Some("a.rs".into()));
        assert_eq!(label(adjacent_file_tab(&state, false)), Some("a.rs".into()));
        state.active_tab = HOME_TAB.to_string();
        assert_eq!(label(adjacent_file_tab(&state, true)), Some("a.rs".into()));
        assert_eq!(label(adjacent_file_tab(&state, false)), Some("b.rs".into()));
    }

    #[test]
    fn names_files_with_the_same_name_apart_by_their_folder() {
        let mut state = RepositoryWorkspace::default();
        for path in ["src/view/mod.rs", "src/model/mod.rs", "README.md"] {
            open_file_tab(&mut state, &FileTabDescriptor::working(path));
        }
        open_file_tab(&mut state, &FileTabDescriptor::index("README.md"));
        open_file_tab(&mut state, &FileTabDescriptor::tree("", &hash('a')));

        assert_eq!(
            file_tab_suffixes(&state.tabs),
            [
                Some("view".to_string()),
                Some("model".to_string()),
                None,
                None,
                None
            ]
        );
        assert_eq!(
            path_breadcrumbs("src/view/mod.rs"),
            ["src", "view", "mod.rs"]
        );
        assert!(path_breadcrumbs("").is_empty());
    }

    #[test]
    fn replaces_the_file_shown_by_a_tab_in_place_and_hands_back_the_previous_one() {
        let mut state = RepositoryWorkspace::default();
        open_file_tab(&mut state, &FileTabDescriptor::working("one.js"));
        open_file_tab(&mut state, &FileTabDescriptor::working("two.js"));
        let first = state.tabs[0].clone();

        let previous =
            replace_file_tab(&mut state, &first.id, &FileTabDescriptor::index("three.js"));

        assert_eq!(previous.as_ref(), Some(&first));
        assert_eq!(paths(&state), ["three.js", "two.js"]);
        assert_eq!(state.tabs[0].origin_tab, first.origin_tab);
        assert_eq!(state.active_tab, state.tabs[0].id);
    }

    #[test]
    fn switches_to_an_already_open_tab_instead_of_duplicating_it_when_replacing() {
        let mut state = RepositoryWorkspace::default();
        open_file_tab(&mut state, &FileTabDescriptor::working("one.js"));
        open_file_tab(&mut state, &FileTabDescriptor::working("two.js"));
        let second = state.tabs[1].id.clone();

        let previous = replace_file_tab(&mut state, &second, &FileTabDescriptor::working("one.js"));

        assert_eq!(previous, None);
        assert_eq!(state.tabs.len(), 2);
        assert_eq!(state.active_tab, state.tabs[0].id);
        assert_eq!(
            replace_file_tab(&mut state, "missing", &FileTabDescriptor::working("x.js")),
            None
        );
    }

    #[test]
    fn reuses_the_current_tab_on_click_and_still_opens_a_new_one_on_the_double_click_that_follows()
    {
        let mut state = RepositoryWorkspace::default();
        open_file_tab(&mut state, &FileTabDescriptor::working("one.js"));
        let mut opener = FileOpener::new();
        let two = FileTabDescriptor::working("two.js");

        // Un doble clic llega como clic, clic, dblclick.
        opener.open(&mut state, &two, true);
        opener.open(&mut state, &two, true);
        assert_eq!(paths(&state), ["two.js"]);
        opener.open(&mut state, &two, false);
        assert_eq!(paths(&state), ["one.js", "two.js"]);
        assert_eq!(state.active_tab, state.tabs[1].id);
    }

    #[test]
    fn forgets_the_replaced_file_once_the_user_has_moved_to_another_tab() {
        let mut state = RepositoryWorkspace::default();
        open_file_tab(&mut state, &FileTabDescriptor::working("one.js"));
        let mut opener = FileOpener::new();
        let two = FileTabDescriptor::working("two.js");

        opener.open(&mut state, &two, true);
        state.active_tab = CHANGES_TAB.to_string();
        opener.sync(&state);
        opener.open(&mut state, &two, false);

        assert_eq!(paths(&state), ["two.js"]);
    }

    #[test]
    fn accepts_a_commit_tree_tab_without_a_file_and_labels_it_as_such() {
        let hash = hash('c');
        let state = normalize_repository_workspace(&json!({
            "tabs": [
                { "path": "", "source": "tree", "commitHash": hash },
                { "path": "", "source": "commit", "commitHash": hash },
                { "path": "", "source": "working" }
            ]
        }));

        assert_eq!(state.tabs.len(), 1);
        assert_eq!(state.tabs[0].label(), "Repo tree");
        assert_eq!(state.tabs[0].context(), "ccccccc · Tree");
        assert_eq!(tab_label("src/app.js"), "app.js");
        assert_eq!(
            tab_context(TabSource::Tree, &hash, "src/app.js"),
            "ccccccc · File"
        );
    }

    #[test]
    fn tells_the_host_which_tab_id_took_over_which_so_the_panel_can_stay_mounted() {
        let mut state = RepositoryWorkspace::default();
        open_file_tab(&mut state, &FileTabDescriptor::working("one.js"));
        let first = state.active_tab.clone();
        let mut opener = FileOpener::new();

        let moved = opener.open(&mut state, &FileTabDescriptor::working("two.js"), true);

        assert_eq!(
            moved,
            Some(TabReplacement {
                from: first,
                to: state.active_tab.clone()
            })
        );
    }

    #[test]
    fn labels_local_and_historical_tabs() {
        assert_eq!(tab_context(TabSource::Working, "", "a.js"), "Local");
        assert_eq!(tab_context(TabSource::Index, "", "a.js"), "Staged");
        assert_eq!(
            tab_context(TabSource::Commit, &hash('d'), "a.js"),
            "ddddddd · Diff"
        );
    }

    #[test]
    fn builds_the_same_ids_and_shape_as_the_renderer() {
        assert_eq!(
            file_tab_id(TabSource::Working, "", "src/app.js"),
            r#"["working","","src/app.js"]"#
        );
        assert_eq!(
            FileTabDescriptor::working("a \"b\".js").id(),
            r#"["working","","a \"b\".js"]"#
        );

        let mut state = RepositoryWorkspace::default();
        open_file_tab(&mut state, &FileTabDescriptor::working("src/app.js"));
        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            json!({
                "layoutVersion": 2,
                "tabs": [{
                    "path": "src/app.js",
                    "source": "working",
                    "commitHash": "",
                    "scroll": { "top": 0.0, "left": 0.0 },
                    "originTab": "commits",
                    "id": r#"["working","","src/app.js"]"#
                }],
                "activeTab": r#"["working","","src/app.js"]"#,
                "section": "commits",
                "selectedCommit": null
            })
        );
    }

    #[test]
    fn keeps_scroll_and_origin_only_when_they_make_sense() {
        let state = normalize_repository_workspace(&json!({
            "layoutVersion": 2,
            "section": "changes",
            "selectedCommit": "abc",
            "activeTab": "gone",
            "tabs": [
                { "path": "a.js", "source": "working", "scroll": { "top": 120.5, "left": -4 }, "originTab": "commits" },
                { "path": "b.js", "source": "index", "scroll": "nope", "originTab": "elsewhere" },
                { "path": 7, "source": "working" },
                { "path": "c.js", "source": "blame" },
                null,
                "texto"
            ]
        }));

        assert_eq!(state.tabs.len(), 2);
        assert_eq!(
            state.tabs[0].scroll,
            Scroll {
                top: 120.5,
                left: 0.0
            }
        );
        assert_eq!(state.tabs[0].origin_tab, HOME_TAB);
        assert_eq!(state.tabs[1].scroll, Scroll::default());
        assert_eq!(state.tabs[1].origin_tab, CHANGES_TAB);
        // La pestaña activa ya no existe: se cae a la vista fija de la sección.
        assert_eq!(state.active_tab, CHANGES_TAB);
        assert_eq!(state.section, WorkspaceSection::Changes);
        assert_eq!(state.selected_commit.as_deref(), Some("abc"));
        assert_eq!(
            Scroll {
                top: f64::NAN,
                left: 3.0
            }
            .normalized(),
            Scroll {
                top: 0.0,
                left: 3.0
            }
        );
    }

    #[test]
    fn keeps_the_commit_draft_across_a_save_and_only_writes_it_when_there_is_one() {
        let state = RepositoryWorkspace {
            commit_draft: "Fix the login\n\nDetails".to_string(),
            ..Default::default()
        };
        assert_eq!(reload(&state).commit_draft, "Fix the login\n\nDetails");

        let empty = serde_json::to_value(RepositoryWorkspace::default()).unwrap();
        assert!(empty.get("commitDraft").is_none());
        // Sesiones viejas (sin el campo) o corruptas quedan sin borrador.
        assert_eq!(
            normalize_repository_workspace(&json!({ "commitDraft": 3 })).commit_draft,
            ""
        );
    }

    #[test]
    fn keeps_the_sign_choice_across_a_save_and_only_writes_it_when_set() {
        let state = RepositoryWorkspace {
            commit_sign: true,
            ..Default::default()
        };
        assert!(reload(&state).commit_sign);

        let unset = serde_json::to_value(RepositoryWorkspace::default()).unwrap();
        assert!(unset.get("commitSign").is_none());
        assert!(!normalize_repository_workspace(&json!({})).commit_sign);
        assert!(!normalize_repository_workspace(&json!({ "commitSign": "yes" })).commit_sign);
    }

    #[test]
    fn a_file_opened_from_another_file_inherits_where_that_one_came_from() {
        let mut state = RepositoryWorkspace {
            active_tab: CHANGES_TAB.to_string(),
            ..Default::default()
        };
        open_file_tab(&mut state, &FileTabDescriptor::working("local.js"));
        open_file_tab(
            &mut state,
            &FileTabDescriptor::commit("history.js", &hash('a')),
        );

        assert_eq!(state.tabs[1].origin_tab, CHANGES_TAB);
    }

    #[test]
    fn a_history_tab_starts_at_a_commit_or_at_head() {
        let at_head = normalize_file_tab(&FileTabDescriptor::history("src/a.rs", None)).unwrap();
        assert_eq!(at_head.source, TabSource::History);
        assert_eq!(at_head.commit_hash, "");
        assert_eq!(at_head.id, r#"["history","","src/a.rs"]"#);
        assert_eq!(at_head.context(), "History");
        assert_eq!(at_head.label(), "a.rs");
        assert_eq!(at_head.origin_tab, HOME_TAB);

        let at_commit =
            normalize_file_tab(&FileTabDescriptor::history("src/a.rs", Some(&hash('b')))).unwrap();
        assert_eq!(at_commit.context(), "bbbbbbb · History");
        assert_ne!(at_commit.id, at_head.id);

        // Sin ruta o con un hash a medias no hay nada que abrir.
        assert!(normalize_file_tab(&FileTabDescriptor::history("", None)).is_none());
        assert!(normalize_file_tab(&FileTabDescriptor::history("a", Some("abc"))).is_none());

        // Sobrevive a la sesión guardada.
        let workspace = normalize_repository_workspace(&json!({
            "layoutVersion": 2,
            "tabs": [{ "source": "history", "path": "src/a.rs", "commitHash": "" }],
            "activeTab": r#"["history","","src/a.rs"]"#,
        }));
        assert_eq!(workspace.tabs.len(), 1);
        assert_eq!(workspace.active_tab, at_head.id);
    }
}
