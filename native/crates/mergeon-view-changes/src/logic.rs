//! Reglas de la vista de cambios locales que no dependen de la ventana: en
//! qué sección cae cada archivo, qué acción de bloque se ofrece, qué pestaña
//! abre un archivo, qué entradas lleva su menú y los textos de la caja de
//! commit. Son los `computed` de `ChangesView.vue`,
//! `RepositoryChangesSidebar.vue` y `RepositoryCommitPanel.vue`.

use mergeon_backend::Commit;
use mergeon_gitcmd::FileStatus;
use mergeon_model::repositories::{RepoLifecycleState, TrackingStatus};
use mergeon_model::workspace_tabs::{FileTabDescriptor, TabSource};
use mergeon_ui::Lucide;
use mergeon_view_diff::HunkAction;

/// Un archivo del estado del repositorio: ruta, columna del índice y columna
/// del working tree (`{ path, index, working_dir }`).
pub type ChangedFile = FileStatus;

fn is_untracked_code(code: &str) -> bool {
    matches!(code, "?" | "??")
}

/// Un archivo sin fusionar: cualquiera de los siete códigos de conflicto de
/// `git status` (`UU`, `AA`, `DD`, `AU`, `UA`, `DU`, `UD`). El original solo
/// miraba `working_dir === 'U'`, y `AA`, `DD` y `UD` acababan en Unstaged y
/// Staged a la vez, sin diff y sin forma de resolverlos.
pub fn is_conflicted(file: &ChangedFile) -> bool {
    is_conflict_code(&file.index, &file.working_dir)
}

fn is_conflict_code(index: &str, working_dir: &str) -> bool {
    index == "U" || working_dir == "U" || matches!((index, working_dir), ("A", "A") | ("D", "D"))
}

/// `file.working_dir === '?' || file.working_dir === '??'`.
pub fn is_untracked(file: &ChangedFile) -> bool {
    is_untracked_code(&file.working_dir)
}

/// `conflictedFiles`.
pub fn conflicted_files(files: &[ChangedFile]) -> Vec<ChangedFile> {
    files
        .iter()
        .filter(|file| is_conflicted(file))
        .cloned()
        .collect()
}

/// `stagedFiles`: lo que tiene algo en el índice y no está en conflicto.
pub fn staged_files(files: &[ChangedFile]) -> Vec<ChangedFile> {
    files
        .iter()
        .filter(|file| is_staged(file))
        .cloned()
        .collect()
}

/// Cuántos archivos hay en [`staged_files`], sin clonarlos (se pide en cada
/// render del panel de commit).
pub fn staged_count(files: &[ChangedFile]) -> usize {
    files.iter().filter(|file| is_staged(file)).count()
}

fn is_staged(file: &ChangedFile) -> bool {
    !is_conflicted(file)
        && !file.index.is_empty()
        && file.index != " "
        && !is_untracked_code(&file.index)
}

/// `unstagedFiles`: lo que cambió en el working tree. Cada archivo sale con el
/// índice en blanco: en esta lista representa su lado sin preparar, aunque el
/// mismo archivo tenga además algo en el índice.
pub fn unstaged_files(files: &[ChangedFile]) -> Vec<ChangedFile> {
    files
        .iter()
        .filter(|file| {
            !file.working_dir.is_empty()
                && !is_conflicted(file)
                && !matches!(file.working_dir.as_str(), " " | "?" | "??")
        })
        .map(|file| ChangedFile {
            index: " ".to_string(),
            ..file.clone()
        })
        .collect()
}

/// `untrackedFiles`.
pub fn untracked_files(files: &[ChangedFile]) -> Vec<ChangedFile> {
    files
        .iter()
        .filter(|file| is_untracked_code(&file.working_dir) || is_untracked_code(&file.index))
        .cloned()
        .collect()
}

/// `getEffectiveStatus`: la letra que pinta el badge de un archivo en la
/// lista. Manda el working tree; si está limpio, el índice.
pub fn effective_status(file: &ChangedFile) -> &str {
    if is_untracked(file) {
        return &file.working_dir;
    }
    let working = file.working_dir.trim();
    if working.is_empty() {
        file.index.trim()
    } else {
        working
    }
}

/// `fileDescriptor`: la pestaña que abre un archivo. Va a `index` si tiene
/// algo preparado y no está en conflicto; si no, a `working`.
pub fn file_descriptor(file: &ChangedFile) -> FileTabDescriptor {
    let staged = !file.index.is_empty() && !matches!(file.index.as_str(), " " | "?" | "??");
    if staged && !is_conflicted(file) {
        FileTabDescriptor::index(file.path.clone())
    } else {
        FileTabDescriptor::working(file.path.clone())
    }
}

/// `hunkAction`: preparar un bloque suelto solo tiene sentido sobre un diff
/// aplicable.
///
/// - con *Ignore spaces* el diff que se ve no vale como parche, y pedir uno
///   limpio tampoco, porque los números de bloque no coincidirían;
/// - un archivo sin seguir no tiene diff real —se sintetiza—, así que se
///   prepara entero;
/// - y un conflicto se resuelve, no se trocea.
pub fn hunk_action(selected: Option<&ChangedFile>, ignore_whitespace: bool) -> Option<HunkAction> {
    let file = selected?;
    if ignore_whitespace || is_conflicted(file) || is_untracked(file) {
        return None;
    }
    let is_staged = !file.index.is_empty() && file.index != " ";
    Some(if is_staged {
        HunkAction::Unstage
    } else {
        HunkAction::Stage
    })
}

/// De dónde salen los archivos de la lista (`source` de
/// `RepositoryChangesSidebar.vue`): en una pestaña de archivo solo se enseña
/// el lado de esa pestaña.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SidebarSource {
    /// Conflictos, sin preparar y preparados (la pestaña "Local Changes").
    #[default]
    All,
    /// Conflictos y sin preparar (pestaña de un archivo del working tree).
    Working,
    /// Solo lo preparado (pestaña de un archivo del índice).
    Index,
}

impl SidebarSource {
    pub fn from_tab_source(source: TabSource) -> Self {
        match source {
            TabSource::Index => Self::Index,
            TabSource::Working => Self::Working,
            TabSource::Commit | TabSource::Tree | TabSource::History => Self::All,
        }
    }

    /// `source !== 'index'`: se enseñan conflictos y cambios sin preparar.
    pub fn shows_working(self) -> bool {
        self != Self::Index
    }

    /// `source !== 'working'`: se enseña lo preparado.
    pub fn shows_staged(self) -> bool {
        self != Self::Working
    }
}

/// La mutación en curso (`busyMutationAction`). Mientras hay una, la vista no
/// admite otra.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BusyAction {
    Ignore,
    DiscardFile,
    StageFile,
    UnstageFile,
    StageHunk,
    UnstageHunk,
    DiscardHunk,
    Commit,
    CommitPush,
    StageAll,
    UnstageAll,
    DiscardAll,
}

impl BusyAction {
    /// El nombre del original (`'stage-file'`, `'commit-push'`…).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ignore => "ignore",
            Self::DiscardFile => "discard-file",
            Self::StageFile => "stage-file",
            Self::UnstageFile => "unstage-file",
            Self::StageHunk => "stage-hunk",
            Self::UnstageHunk => "unstage-hunk",
            Self::DiscardHunk => "discard-hunk",
            Self::Commit => "commit",
            Self::CommitPush => "commit-push",
            Self::StageAll => "stage-all",
            Self::UnstageAll => "unstage-all",
            Self::DiscardAll => "discard-all",
        }
    }

    /// `isCommitBusy`.
    pub fn is_commit(self) -> bool {
        matches!(self, Self::Commit | Self::CommitPush)
    }
}

/// `canStage` / `canUnstage` / `canDiscard`: fuera de `idle` y `conflicted`
/// hay una operación en curso y el working tree no se toca.
pub fn lifecycle_allows_changes(state: RepoLifecycleState) -> bool {
    matches!(
        state,
        RepoLifecycleState::Idle | RepoLifecycleState::Conflicted
    )
}

// ---------------------------------------------------------------------------
// Menú contextual de un archivo
// ---------------------------------------------------------------------------

/// Lo que ofrece el menú contextual de un archivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileMenuAction {
    /// Solo en una pestaña de archivo: lo mismo que el clic.
    OpenHere,
    /// Solo en una pestaña de archivo: lo mismo que el doble clic.
    OpenInNewTab,
    Unstage,
    Stage,
    Discard,
    Ignore,
    Blame,
    /// Pestaña "File history": los commits que tocaron el archivo,
    /// siguiendo sus renombres, desde HEAD.
    FileHistory,
    /// Abre el archivo en el editor externo (en la línea del diff a la vista
    /// si es el archivo seleccionado).
    OpenInEditor,
    Reveal,
    CopyPath,
    /// "Stash selected files…": el diálogo de stash con los archivos marcados
    /// (ver [`stash_selection`]).
    StashSelected,
}

impl FileMenuAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenHere => "Open here",
            Self::OpenInNewTab => "Open in new tab",
            Self::Unstage => "Unstage",
            Self::Stage => "Stage",
            Self::Discard => "Discard Changes",
            Self::Ignore => "Ignore",
            Self::Blame => "Blame",
            Self::FileHistory => "File history",
            Self::OpenInEditor => "Open in editor",
            Self::Reveal => "Reveal in Explorer",
            Self::CopyPath => "Copy Path",
            Self::StashSelected => "Stash selected files…",
        }
    }

    pub fn icon(self) -> Lucide {
        match self {
            Self::OpenHere => Lucide::FileCodeCorner,
            Self::OpenInNewTab => Lucide::SquarePlus,
            Self::Unstage => Lucide::GitPullRequestArrow,
            Self::Stage => Lucide::GitCommitHorizontal,
            Self::Discard => Lucide::Trash,
            Self::Ignore => Lucide::Ban,
            Self::Blame => Lucide::Search,
            Self::FileHistory => Lucide::FileClock,
            Self::OpenInEditor => Lucide::CodeXml,
            Self::Reveal => Lucide::FolderSearch2,
            Self::CopyPath => Lucide::Copy,
            Self::StashSelected => Lucide::Archive,
        }
    }

    pub fn is_danger(self) -> bool {
        self == Self::Discard
    }
}

/// Una entrada del menú contextual de un archivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileMenuEntry {
    Separator,
    Action {
        action: FileMenuAction,
        disabled: bool,
    },
}

impl FileMenuEntry {
    fn enabled(action: FileMenuAction) -> Self {
        Self::Action {
            action,
            disabled: false,
        }
    }

    pub fn action(self) -> Option<FileMenuAction> {
        match self {
            Self::Separator => None,
            Self::Action { action, .. } => Some(action),
        }
    }
}

/// `contextMenuItems`: las entradas del menú de un archivo. `in_file_tab`
/// añade arriba lo mismo que hacen el clic y el doble clic, para quien no
/// conozca el gesto; `locked` (hay una mutación en curso) apaga las que tocan
/// el working tree.
pub fn file_menu_entries(
    file: &ChangedFile,
    in_file_tab: bool,
    locked: bool,
) -> Vec<FileMenuEntry> {
    let is_staged = !file.index.is_empty() && file.index != " " && !is_untracked_code(&file.index);
    let untracked = is_untracked(file);
    let conflicted = is_conflicted(file);
    let is_unstaged = !is_staged && !untracked && !conflicted;
    let mutation = |action| FileMenuEntry::Action {
        action,
        disabled: locked,
    };

    let mut entries = Vec::new();
    if in_file_tab {
        entries.push(FileMenuEntry::enabled(FileMenuAction::OpenHere));
        entries.push(FileMenuEntry::enabled(FileMenuAction::OpenInNewTab));
        entries.push(FileMenuEntry::Separator);
    }

    if !conflicted && is_staged {
        entries.push(mutation(FileMenuAction::Unstage));
    } else if !conflicted {
        entries.push(mutation(FileMenuAction::Stage));
    }

    if is_unstaged || untracked {
        entries.push(mutation(FileMenuAction::Discard));
    }

    if untracked {
        entries.push(FileMenuEntry::Separator);
        entries.push(mutation(FileMenuAction::Ignore));
    }

    // Ver y abrir; luego su historial, el stash y, al final, copiar.
    entries.push(FileMenuEntry::Separator);
    entries.push(FileMenuEntry::enabled(FileMenuAction::Blame));
    // Un archivo borrado ya no está en el working tree: no hay qué abrir. En
    // un conflicto git deja el archivo del lado que lo conserva (`UD`, `DU`),
    // salvo si lo borraron los dos (`DD`).
    let gone = if conflicted {
        file.index == "D" && file.working_dir == "D"
    } else {
        file.working_dir == "D" || (file.index == "D" && file.working_dir != "?")
    };
    entries.push(FileMenuEntry::Action {
        action: FileMenuAction::OpenInEditor,
        disabled: gone,
    });
    entries.push(FileMenuEntry::enabled(FileMenuAction::Reveal));

    // Un archivo sin seguimiento todavía no tiene commits.
    entries.push(FileMenuEntry::Separator);
    entries.push(FileMenuEntry::Action {
        action: FileMenuAction::FileHistory,
        disabled: untracked,
    });

    // Un archivo en conflicto no se puede guardar en un stash.
    if !conflicted {
        entries.push(FileMenuEntry::Separator);
        entries.push(mutation(FileMenuAction::StashSelected));
    }

    entries.push(FileMenuEntry::Separator);
    entries.push(FileMenuEntry::enabled(FileMenuAction::CopyPath));
    entries
}

/// Las rutas de "Stash selected files…" pedido sobre `file`: si su casilla
/// está marcada (tiene algo preparado), todos los archivos marcados; si no,
/// solo él (la fila del menú es la selección).
pub fn stash_selection(file: &ChangedFile, files: &[ChangedFile]) -> Vec<String> {
    if !is_staged(file) {
        return vec![file.path.clone()];
    }
    let mut paths: Vec<String> = Vec::new();
    for staged in files.iter().filter(|candidate| is_staged(candidate)) {
        if !paths.contains(&staged.path) {
            paths.push(staged.path.clone());
        }
    }
    if !paths.contains(&file.path) {
        paths.insert(0, file.path.clone());
    }
    paths
}

/// `getFileName`: el último tramo de una ruta.
pub fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

// ---------------------------------------------------------------------------
// Caja de commit
// ---------------------------------------------------------------------------

/// `canCommit`: solo en `idle`, con mensaje y con algo preparado. Enmendar
/// puede ser solo cambiar el mensaje, sin nada preparado.
pub fn can_commit(
    lifecycle: RepoLifecycleState,
    message: &str,
    staged_count: usize,
    amend: bool,
) -> bool {
    lifecycle == RepoLifecycleState::Idle
        && !message.trim().is_empty()
        && (staged_count > 0 || amend)
}

/// `amendIsPushed`: si el último commit ya está en la rama de seguimiento,
/// enmendarlo lo reescribe y el remoto rechazará el push. `ahead` cuenta lo
/// que aún no se subió: con cero por delante, el último commit ya viajó.
pub fn amend_is_pushed(tracking: Option<&TrackingStatus>) -> bool {
    tracking.is_some_and(|tracking| {
        tracking
            .tracking
            .as_deref()
            .is_some_and(|upstream| !upstream.is_empty())
            && tracking.ahead == 0
    })
}

/// El mensaje con el que parte una enmienda: asunto y cuerpo del último
/// commit. Reescribirlo desde cero no es lo que se busca casi nunca.
pub fn amend_message(last_commit: &Commit) -> String {
    [last_commit.message.as_str(), last_commit.body.as_str()]
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// El commit en el que está HEAD, de entre los que devuelve una lectura del
/// historial (su decoración lleva `HEAD` o `HEAD -> refs/heads/…`).
pub fn head_commit(commits: Vec<Commit>) -> Option<Commit> {
    commits.into_iter().find(|commit| {
        commit
            .refs
            .split(", ")
            .any(|reference| reference == "HEAD" || reference.starts_with("HEAD -> "))
    })
}

/// `commitLabel`: lo que va a hacer el botón, con la rama a la que va
/// ("Commit to main"). Sin rama (HEAD separado) solo la acción.
pub fn commit_label(amend: bool, push_after_commit: bool, branch: &str) -> String {
    let (action, preposition) = match (amend, push_after_commit) {
        (true, true) => ("Amend & Push", "on"),
        (true, false) => ("Amend", "on"),
        (false, true) => ("Commit & Push", "to"),
        (false, false) => ("Commit", "to"),
    };
    if branch.is_empty() {
        action.to_string()
    } else {
        format!("{action} {preposition} {branch}")
    }
}

/// Largo recomendado del asunto de un commit (lo que `git log --oneline` y
/// las herramientas de revisión enseñan sin cortar).
pub const SUBJECT_LIMIT: usize = 72;

/// Hasta aquí el asunto es cómodo de leer en cualquier lista (la regla de los
/// 50 caracteres); entre esto y [`SUBJECT_LIMIT`] el contador avisa en ámbar.
pub const SUBJECT_SOFT_LIMIT: usize = 50;

/// El color del contador del asunto, por tramos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubjectTone {
    /// Hasta [`SUBJECT_SOFT_LIMIT`].
    Neutral,
    /// De 51 a [`SUBJECT_LIMIT`].
    Warning,
    /// Más de [`SUBJECT_LIMIT`].
    Danger,
}

/// El tramo del contador para `count` caracteres.
pub fn subject_tone(count: usize) -> SubjectTone {
    if count > SUBJECT_LIMIT {
        SubjectTone::Danger
    } else if count > SUBJECT_SOFT_LIMIT {
        SubjectTone::Warning
    } else {
        SubjectTone::Neutral
    }
}

/// `"1 file"` / `"3 files"`.
pub fn files_count_label(count: usize) -> String {
    if count == 1 {
        "1 file".to_string()
    } else {
        format!("{count} files")
    }
}

/// El texto del botón de commit: qué hace y con cuántos archivos
/// preparados ("Commit 3 files", "Amend & Push with 1 file"). La rama va
/// aparte, en la línea de destino ([`commit_target`]).
pub fn commit_button_label(amend: bool, push_after_commit: bool, staged_count: usize) -> String {
    let action = match (amend, push_after_commit) {
        (true, true) => "Amend & Push",
        (true, false) => "Amend",
        (false, true) => "Commit & Push",
        (false, false) => "Commit",
    };
    match (staged_count, amend) {
        (0, _) => action.to_string(),
        (count, true) => format!("{action} with {}", files_count_label(count)),
        (count, false) => format!("{action} {}", files_count_label(count)),
    }
}

/// La línea discreta sobre la caja: adónde va el commit.
pub fn commit_target(amend: bool, branch: &str) -> String {
    match (branch.is_empty(), amend) {
        (true, true) => "Amending HEAD (detached)".to_string(),
        (true, false) => "Committing on a detached HEAD".to_string(),
        (false, true) => format!("Amending the last commit on {branch}"),
        (false, false) => format!("Committing to {branch}"),
    }
}

/// El resumen de varios archivos elegidos: `"3 files · +12 −4"` (sin cifras
/// si no se conocen).
pub fn selection_summary(count: usize, additions: u64, deletions: u64) -> String {
    let mut summary = files_count_label(count);
    if additions > 0 || deletions > 0 {
        summary.push_str(&format!(" · +{additions} \u{2212}{deletions}"));
    }
    summary
}

/// El texto de la confirmación de descartar: el archivo, o cuántos.
pub fn discard_description(paths: &[String]) -> String {
    match paths {
        [one] => format!("Discard local changes in {one}?"),
        many => format!(
            "Discard local changes in {}?",
            files_count_label(many.len())
        ),
    }
}

/// El contador del asunto: caracteres escritos (no bytes) y si ya pasó de
/// [`SUBJECT_LIMIT`].
pub fn subject_counter(title: &str) -> (usize, bool) {
    let count = title.chars().count();
    (count, count > SUBJECT_LIMIT)
}

/// El mensaje de commit a partir del asunto y el cuerpo: `asunto\n\ncuerpo`,
/// o solo el asunto si no hay cuerpo. También es el formato del borrador.
pub fn compose_commit_message(title: &str, body: &str) -> String {
    if body.trim().is_empty() {
        title.to_string()
    } else {
        format!("{title}\n\n{body}")
    }
}

/// Lo contrario de [`compose_commit_message`]: asunto hasta el primer salto
/// de línea y cuerpo con lo demás (sin las líneas en blanco que lo separan).
/// Sirve también para los borradores de cuando el mensaje era una sola caja.
pub fn split_commit_message(message: &str) -> (String, String) {
    match message.split_once('\n') {
        Some((title, body)) => (
            title.trim_end_matches('\r').to_string(),
            body.trim_start_matches(['\r', '\n']).to_string(),
        ),
        None => (message.to_string(), String::new()),
    }
}

/// El mensaje inicial que da una plantilla de commit (`commit.template`):
/// sin las líneas de comentario (las que empiezan por `#`, que git quitaría
/// al hacer commit), sin líneas en blanco al principio ni al final y sin
/// varias seguidas. Puede quedar vacío.
pub fn commit_template_message(template: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    for line in template.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with('#') {
            continue;
        }
        let blank = line.trim().is_empty();
        if blank && lines.last().is_none_or(|last| last.trim().is_empty()) {
            continue;
        }
        lines.push(if blank { "" } else { line });
    }
    while lines.last().is_some_and(|last| last.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Dos archivos de la lista con la misma ruta: el lado sin preparar y el
/// preparado de un archivo a medio preparar. En la sección de preparados se
/// reconoce porque su working tree también cambió.
pub fn is_partially_staged(staged_file: &ChangedFile) -> bool {
    !staged_file.working_dir.is_empty()
        && !is_conflicted(staged_file)
        && !matches!(staged_file.working_dir.as_str(), " " | "?" | "??")
}

/// `blockedReason`: qué falta para poder hacer commit. Un botón apagado no lo
/// dice.
pub fn commit_blocked_reason(
    can_commit: bool,
    busy: bool,
    staged_count: usize,
    amend: bool,
    message: &str,
) -> &'static str {
    if can_commit || busy {
        return "";
    }
    if staged_count == 0 && !amend {
        return "Stage something to commit";
    }
    if message.trim().is_empty() {
        return "Write a message to commit";
    }
    ""
}

/// `shortcutLabel`: las teclas del atajo de commit tal como se enseñan en el
/// botón, una por `Kbd`.
pub fn commit_shortcut_label() -> [&'static str; 2] {
    if cfg!(target_os = "macos") {
        ["⌘", "↵"]
    } else {
        ["Ctrl", "↵"]
    }
}

/// `shortcutKeys` (`aria-keyshortcuts`).
pub fn commit_shortcut_keys() -> &'static str {
    if cfg!(target_os = "macos") {
        "Meta+Enter"
    } else {
        "Control+Enter"
    }
}

/// El aviso de que enmendar reescribe un commit ya publicado.
pub fn amend_warning(upstream: &str) -> String {
    let remote = if upstream.is_empty() {
        "the remote"
    } else {
        upstream
    };
    format!(
        "That commit is already on {remote}. Amending rewrites it, and the remote will reject the push until you force it."
    )
}

// ---------------------------------------------------------------------------
// Ancho de la lista
// ---------------------------------------------------------------------------

/// Ancho inicial de la lista en "Local Changes" (`localSidebarWidth`).
pub const DEFAULT_LIST_WIDTH: f64 = 330.0;
/// Ancho del divisor entre la lista y lo demás (`8px` de la rejilla).
pub const RESIZER_WIDTH: f64 = 8.0;
/// Por debajo de este ancho de ventana la pestaña "Local Changes" apila la
/// lista (con la caja de commit) sobre el diff. Con la columna de 400px el
/// diff tiene sitio de sobra hasta aquí.
pub const STACKED_LAYOUT_MAX_WIDTH: f32 = 900.0;

/// Ancho inicial de la columna izquierda de "Local Changes" (lista y caja
/// de commit): lo justo para la lista y una caja de commit compacta, y el
/// resto para el diff. Se recuerda el que elija el usuario.
pub const DEFAULT_LOCAL_CHANGES_WIDTH: f64 = 400.0;

/// El ancho de la columna izquierda de "Local Changes": la caja de commit
/// necesita al menos 320px y al diff de la derecha le quedan al menos 360.
/// `container_width` 0 es "desconocido".
pub fn clamp_local_changes_width(next_width: f64, container_width: f64) -> f64 {
    const MIN_WIDTH: f64 = 320.0;
    const MAX_WIDTH: f64 = 900.0;
    let max_width = if container_width > 0.0 {
        (container_width - 360.0).min(MAX_WIDTH)
    } else {
        MAX_WIDTH
    };
    next_width.max(MIN_WIDTH).min(max_width.max(MIN_WIDTH))
}

/// `clampSidebarWidth` fuera de una pestaña de archivo: entre 160 y 560, sin
/// dejar menos de 180 a lo de al lado. `container_width` 0 es "desconocido".
pub fn clamp_list_width(next_width: f64, container_width: f64) -> f64 {
    const MIN_WIDTH: f64 = 160.0;
    let max_width = if container_width > 0.0 {
        (container_width - 180.0).min(560.0)
    } else {
        560.0
    };
    next_width.max(MIN_WIDTH).min(max_width.max(MIN_WIDTH))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, index: &str, working_dir: &str) -> ChangedFile {
        ChangedFile {
            path: path.to_string(),
            index: index.to_string(),
            working_dir: working_dir.to_string(),
            from: None,
        }
    }

    fn paths(files: &[ChangedFile]) -> Vec<&str> {
        files.iter().map(|file| file.path.as_str()).collect()
    }

    #[test]
    fn splits_the_status_into_its_sections() {
        let files = [
            file("conflict.js", "U", "U"),
            file("modified.js", " ", "M"),
            file("staged.js", "M", " "),
            file("both.js", "M", "M"),
            file("added.js", "A", " "),
            file("deleted.js", " ", "D"),
            file("new.js", "?", "?"),
            file("newer.js", "??", "??"),
        ];

        assert_eq!(paths(&conflicted_files(&files)), ["conflict.js"]);
        assert_eq!(
            paths(&unstaged_files(&files)),
            ["modified.js", "both.js", "deleted.js"]
        );
        assert_eq!(
            paths(&staged_files(&files)),
            ["staged.js", "both.js", "added.js"]
        );
        assert_eq!(paths(&untracked_files(&files)), ["new.js", "newer.js"]);
    }

    /// Los siete códigos sin fusionar van solo a "Conflicted files", una vez:
    /// ni a Unstaged ni a Staged (antes `AA`, `DD`, `UD`… salían en las dos).
    #[test]
    fn every_unmerged_code_is_a_conflict_and_only_a_conflict() {
        let codes = [
            ("UU", "U", "U"),
            ("AA", "A", "A"),
            ("DD", "D", "D"),
            ("AU", "A", "U"),
            ("UA", "U", "A"),
            ("DU", "D", "U"),
            ("UD", "U", "D"),
        ];
        let files: Vec<ChangedFile> = codes
            .iter()
            .map(|(name, index, working)| file(name, index, working))
            .chain([file("plain.txt", "M", "M"), file("added.txt", "A", "M")])
            .collect();

        assert_eq!(
            paths(&conflicted_files(&files)),
            ["UU", "AA", "DD", "AU", "UA", "DU", "UD"]
        );
        assert_eq!(paths(&unstaged_files(&files)), ["plain.txt", "added.txt"]);
        assert_eq!(paths(&staged_files(&files)), ["plain.txt", "added.txt"]);
        assert_eq!(staged_count(&files), 2);
        for conflict in &files[..codes.len()] {
            assert!(!is_partially_staged(conflict), "{}", conflict.path);
            assert_eq!(hunk_action(Some(conflict), false), None);
            // Se abre su versión del working tree, no la del índice.
            assert_eq!(file_descriptor(conflict).source, TabSource::Working);
        }

        // Solo `DD` no deja archivo que abrir en el editor.
        let open_disabled = |file: &ChangedFile| {
            file_menu_entries(file, false, false)
                .into_iter()
                .find_map(|entry| match entry {
                    FileMenuEntry::Action {
                        action: FileMenuAction::OpenInEditor,
                        disabled,
                    } => Some(disabled),
                    _ => None,
                })
        };
        assert_eq!(open_disabled(&file("x", "D", "D")), Some(true));
        assert_eq!(open_disabled(&file("x", "U", "D")), Some(false));
        assert_eq!(open_disabled(&file("x", "D", "U")), Some(false));
        assert_eq!(open_disabled(&file("x", "A", "A")), Some(false));
    }

    /// Un archivo a medio preparar sale en las dos listas; en la de sin
    /// preparar pierde su columna de índice, y con ella abre la pestaña del
    /// working tree.
    #[test]
    fn an_unstaged_entry_stands_for_the_working_tree_side() {
        let files = [file("src/a.js", "M", "M")];
        let unstaged = unstaged_files(&files);
        let staged = staged_files(&files);

        assert_eq!(unstaged[0].index, " ");
        assert_eq!(file_descriptor(&unstaged[0]).source, TabSource::Working);
        assert_eq!(file_descriptor(&staged[0]).source, TabSource::Index);
    }

    #[test]
    fn conflicts_and_untracked_files_open_on_the_working_side() {
        assert_eq!(
            file_descriptor(&file("a.js", "U", "U")).source,
            TabSource::Working
        );
        assert_eq!(
            file_descriptor(&file("a.js", "?", "?")).source,
            TabSource::Working
        );
        assert_eq!(
            file_descriptor(&file("a.js", "??", "??")).source,
            TabSource::Working
        );
        assert_eq!(
            file_descriptor(&file("a.js", "A", " ")).source,
            TabSource::Index
        );
    }

    #[test]
    fn the_badge_follows_the_working_tree_then_the_index() {
        assert_eq!(effective_status(&file("a", " ", "M")), "M");
        assert_eq!(effective_status(&file("a", "A", " ")), "A");
        assert_eq!(effective_status(&file("a", "M", "D")), "D");
        assert_eq!(effective_status(&file("a", "?", "?")), "?");
        assert_eq!(effective_status(&file("a", "??", "??")), "??");
        assert_eq!(effective_status(&file("a", " ", " ")), "");
    }

    // `ChangesView.test.js` › "staging one block" (la regla; el resto, con
    // ventana, está en `tests/ui`).

    #[test]
    fn offers_to_stage_a_block_of_an_unstaged_file() {
        assert_eq!(
            hunk_action(Some(&file("src/a.js", " ", "M")), false),
            Some(HunkAction::Stage)
        );
    }

    #[test]
    fn offers_to_unstage_when_the_file_is_already_staged() {
        assert_eq!(
            hunk_action(Some(&file("src/a.js", "M", " ")), false),
            Some(HunkAction::Unstage)
        );
    }

    #[test]
    fn does_not_offer_blocks_while_whitespace_is_ignored() {
        assert_eq!(hunk_action(Some(&file("src/a.js", " ", "M")), true), None);
    }

    #[test]
    fn does_not_offer_blocks_for_an_untracked_file() {
        assert_eq!(
            hunk_action(Some(&file("src/nuevo.js", "?", "?")), false),
            None
        );
        assert_eq!(
            hunk_action(Some(&file("src/nuevo.js", "??", "??")), false),
            None
        );
    }

    #[test]
    fn does_not_offer_blocks_for_a_conflict_or_without_a_file() {
        assert_eq!(hunk_action(Some(&file("src/a.js", "U", "U")), false), None);
        assert_eq!(hunk_action(None, false), None);
    }

    fn labels(entries: &[FileMenuEntry]) -> Vec<&'static str> {
        entries
            .iter()
            .map(|entry| entry.action().map_or("---", FileMenuAction::label))
            .collect()
    }

    #[test]
    fn the_menu_of_a_modified_file() {
        let entries = file_menu_entries(&file("src/app.js", " ", "M"), false, false);
        assert_eq!(
            labels(&entries),
            [
                "Stage",
                "Discard Changes",
                "---",
                "Blame",
                "Open in editor",
                "Reveal in Explorer",
                "---",
                "File history",
                "---",
                "Stash selected files…",
                "---",
                "Copy Path"
            ]
        );
    }

    #[test]
    fn the_menu_of_an_untracked_file_adds_ignore() {
        let entries = file_menu_entries(&file("notes.txt", "?", "?"), false, false);
        assert_eq!(
            labels(&entries),
            [
                "Stage",
                "Discard Changes",
                "---",
                "Ignore",
                "---",
                "Blame",
                "Open in editor",
                "Reveal in Explorer",
                "---",
                "File history",
                "---",
                "Stash selected files…",
                "---",
                "Copy Path"
            ]
        );
    }

    #[test]
    fn the_menu_of_a_staged_file_only_unstages() {
        let entries = file_menu_entries(&file("README.md", "M", " "), false, false);
        assert_eq!(
            labels(&entries),
            [
                "Unstage",
                "---",
                "Blame",
                "Open in editor",
                "Reveal in Explorer",
                "---",
                "File history",
                "---",
                "Stash selected files…",
                "---",
                "Copy Path"
            ]
        );
    }

    #[test]
    fn the_menu_of_a_conflict_does_not_stage() {
        let entries = file_menu_entries(&file("src/app.js", "U", "U"), false, false);
        assert_eq!(
            labels(&entries),
            [
                "---",
                "Blame",
                "Open in editor",
                "Reveal in Explorer",
                "---",
                "File history",
                "---",
                "Copy Path"
            ]
        );
    }

    #[test]
    fn the_menu_inside_a_file_tab_spells_out_the_gestures() {
        let entries = file_menu_entries(&file("src/b.js", " ", "M"), true, false);
        assert_eq!(
            labels(&entries)[..4],
            ["Open here", "Open in new tab", "---", "Stage"]
        );
    }

    #[test]
    fn a_running_mutation_disables_what_touches_the_working_tree() {
        let entries = file_menu_entries(&file("notes.txt", "?", "?"), true, true);
        let disabled: Vec<&str> = entries
            .iter()
            .filter_map(|entry| match entry {
                FileMenuEntry::Action {
                    action,
                    disabled: true,
                } => Some(action.label()),
                _ => None,
            })
            .collect();
        assert_eq!(
            disabled,
            [
                "Stage",
                "Discard Changes",
                "Ignore",
                "File history",
                "Stash selected files…"
            ]
        );
    }

    #[test]
    fn stash_selection_takes_the_checked_files_or_just_the_clicked_one() {
        let files = [
            file("staged.txt", "M", " "),
            file("both.txt", "M", "M"),
            file("modified.txt", " ", "M"),
            file("new.txt", "?", "?"),
        ];
        assert_eq!(
            stash_selection(&files[1], &files),
            ["staged.txt", "both.txt"]
        );
        assert_eq!(stash_selection(&files[2], &files), ["modified.txt"]);
        assert_eq!(stash_selection(&files[3], &files), ["new.txt"]);
    }

    #[test]
    fn a_deleted_file_cannot_be_opened_in_the_editor() {
        let open_in_editor = |file: &ChangedFile| {
            file_menu_entries(file, false, false)
                .into_iter()
                .find_map(|entry| match entry {
                    FileMenuEntry::Action {
                        action: FileMenuAction::OpenInEditor,
                        disabled,
                    } => Some(disabled),
                    _ => None,
                })
        };
        assert_eq!(open_in_editor(&file("gone.txt", " ", "D")), Some(true));
        assert_eq!(open_in_editor(&file("gone.txt", "D", " ")), Some(true));
        assert_eq!(open_in_editor(&file("src/app.js", " ", "M")), Some(false));
        assert_eq!(open_in_editor(&file("new.txt", "?", "?")), Some(false));
        assert_eq!(open_in_editor(&file("both.txt", "U", "U")), Some(false));
    }

    #[test]
    fn only_discard_is_destructive() {
        for action in [
            FileMenuAction::OpenHere,
            FileMenuAction::OpenInNewTab,
            FileMenuAction::Unstage,
            FileMenuAction::Stage,
            FileMenuAction::Ignore,
            FileMenuAction::Blame,
            FileMenuAction::FileHistory,
            FileMenuAction::OpenInEditor,
            FileMenuAction::Reveal,
            FileMenuAction::CopyPath,
            FileMenuAction::StashSelected,
        ] {
            assert!(!action.is_danger(), "{action:?}");
        }
        assert!(FileMenuAction::Discard.is_danger());
    }

    // `RepositoryCommitPanel.test.js` (los textos; lo que es interacción está
    // en `tests/ui`).

    #[test]
    fn says_what_the_button_will_do() {
        assert_eq!(commit_label(false, false, "main"), "Commit to main");
        assert_eq!(commit_label(false, true, "main"), "Commit & Push to main");
        assert_eq!(commit_label(true, false, "main"), "Amend on main");
        assert_eq!(commit_label(true, true, "main"), "Amend & Push on main");
        // Sin rama (HEAD separado) no hay a dónde.
        assert_eq!(commit_label(false, false, ""), "Commit");
        assert_eq!(commit_label(true, true, ""), "Amend & Push");
    }

    #[test]
    fn counts_the_subject_in_characters_and_flags_it_past_72() {
        assert_eq!(subject_counter(""), (0, false));
        assert_eq!(subject_counter(&"a".repeat(72)), (72, false));
        assert_eq!(subject_counter(&"a".repeat(73)), (73, true));
        // Caracteres, no bytes: "envío" son 5.
        assert_eq!(subject_counter("envío"), (5, false));
    }

    #[test]
    fn colours_the_subject_counter_in_three_steps() {
        assert_eq!(subject_tone(0), SubjectTone::Neutral);
        assert_eq!(subject_tone(50), SubjectTone::Neutral);
        assert_eq!(subject_tone(51), SubjectTone::Warning);
        assert_eq!(subject_tone(72), SubjectTone::Warning);
        assert_eq!(subject_tone(73), SubjectTone::Danger);
    }

    #[test]
    fn the_commit_button_says_how_many_files_go_in() {
        assert_eq!(commit_button_label(false, false, 3), "Commit 3 files");
        assert_eq!(commit_button_label(false, false, 1), "Commit 1 file");
        assert_eq!(commit_button_label(false, true, 2), "Commit & Push 2 files");
        assert_eq!(commit_button_label(false, false, 0), "Commit");
        assert_eq!(commit_button_label(true, false, 0), "Amend");
        assert_eq!(
            commit_button_label(true, true, 1),
            "Amend & Push with 1 file"
        );
        assert_eq!(commit_target(false, "main"), "Committing to main");
        assert_eq!(
            commit_target(true, "main"),
            "Amending the last commit on main"
        );
        assert_eq!(commit_target(false, ""), "Committing on a detached HEAD");
    }

    #[test]
    fn summarises_a_multiple_selection() {
        assert_eq!(selection_summary(3, 12, 4), "3 files · +12 \u{2212}4");
        assert_eq!(selection_summary(2, 0, 0), "2 files");
        assert_eq!(
            discard_description(&["a.js".to_string()]),
            "Discard local changes in a.js?"
        );
        assert_eq!(
            discard_description(&["a.js".to_string(), "b.js".to_string()]),
            "Discard local changes in 2 files?"
        );
    }

    #[test]
    fn joins_subject_and_body_with_a_blank_line() {
        assert_eq!(compose_commit_message("Fix it", ""), "Fix it");
        assert_eq!(compose_commit_message("Fix it", "  \n "), "Fix it");
        assert_eq!(
            compose_commit_message("Fix it", "Because.\nReally."),
            "Fix it\n\nBecause.\nReally."
        );
        assert_eq!(compose_commit_message("", "Only body"), "\n\nOnly body");
    }

    #[test]
    fn splits_a_message_or_an_old_single_box_draft() {
        // Lo que guarda la caja nueva vuelve igual.
        for (title, body) in [
            ("Fix it", ""),
            ("Fix it", "Because.\nReally."),
            ("", "Body"),
        ] {
            let (got_title, got_body) = split_commit_message(&compose_commit_message(title, body));
            assert_eq!((got_title.as_str(), got_body.as_str()), (title, body));
        }
        // Borradores de una sola caja: asunto hasta el primer salto, el resto
        // al cuerpo.
        assert_eq!(
            split_commit_message("Half a message"),
            ("Half a message".to_string(), String::new())
        );
        assert_eq!(
            split_commit_message("Subject\nsecond line\n\nthird"),
            ("Subject".to_string(), "second line\n\nthird".to_string())
        );
        assert_eq!(
            split_commit_message("Subject\r\n\r\nBody"),
            ("Subject".to_string(), "Body".to_string())
        );
    }

    #[test]
    fn a_staged_file_is_partial_when_its_working_tree_also_changed() {
        let file = |index: &str, working_dir: &str| ChangedFile {
            path: "a.txt".to_string(),
            index: index.to_string(),
            working_dir: working_dir.to_string(),
            from: None,
        };
        assert!(is_partially_staged(&file("M", "M")));
        assert!(is_partially_staged(&file("A", "D")));
        assert!(!is_partially_staged(&file("M", " ")));
        assert!(!is_partially_staged(&file("A", "")));
    }

    #[test]
    fn says_what_is_missing_when_nothing_is_staged() {
        assert_eq!(
            commit_blocked_reason(false, false, 0, false, "arregla el parser"),
            "Stage something to commit"
        );
    }

    #[test]
    fn says_what_is_missing_when_there_is_no_message() {
        assert_eq!(
            commit_blocked_reason(false, false, 2, false, "   "),
            "Write a message to commit"
        );
    }

    #[test]
    fn says_nothing_when_everything_is_in_place() {
        assert_eq!(
            commit_blocked_reason(true, false, 2, false, "arregla el parser"),
            ""
        );
        // Ni mientras el commit está en curso.
        assert_eq!(commit_blocked_reason(false, true, 0, false, ""), "");
    }

    #[test]
    fn does_not_ask_to_stage_anything_when_amending() {
        assert_eq!(
            commit_blocked_reason(false, false, 0, true, "arregla el parser"),
            ""
        );
        assert_eq!(
            commit_blocked_reason(false, false, 0, true, ""),
            "Write a message to commit"
        );
    }

    #[test]
    fn warns_before_rewriting_a_commit_that_is_already_pushed() {
        assert!(amend_warning("origin/main").contains("already on origin/main"));
        assert!(amend_warning("").contains("already on the remote"));
    }

    #[test]
    fn a_commit_needs_an_idle_repository_a_message_and_something_staged() {
        use RepoLifecycleState::*;
        assert!(can_commit(Idle, "Ship it", 1, false));
        assert!(!can_commit(Idle, "   ", 1, false));
        assert!(!can_commit(Idle, "Ship it", 0, false));
        assert!(can_commit(Idle, "Ship it", 0, true));
        for state in [Busy, Conflicted, ReadyToContinue] {
            assert!(!can_commit(state, "Ship it", 1, false), "{state:?}");
        }
    }

    #[test]
    fn the_last_commit_is_pushed_when_nothing_is_ahead_of_the_upstream() {
        let tracking = |tracking: Option<&str>, ahead| TrackingStatus {
            current: "main".to_string(),
            tracking: tracking.map(str::to_string),
            ahead,
            behind: 0,
        };
        assert!(amend_is_pushed(Some(&tracking(Some("origin/main"), 0))));
        assert!(!amend_is_pushed(Some(&tracking(Some("origin/main"), 1))));
        assert!(!amend_is_pushed(Some(&tracking(None, 0))));
        assert!(!amend_is_pushed(Some(&tracking(Some(""), 0))));
        assert!(!amend_is_pushed(None));
    }

    #[test]
    fn an_amend_starts_from_the_previous_message() {
        let commit = |message: &str, body: &str| Commit {
            hash: "abc1234".to_string(),
            full_hash: "abc1234".repeat(6),
            message: message.to_string(),
            author: "Ana".to_string(),
            email: "ana@example.com".to_string(),
            date: "2026-09-01T10:00:00+00:00".to_string(),
            body: body.to_string(),
            refs: String::new(),
            parents: String::new(),
        };
        assert_eq!(amend_message(&commit("Fix parser", "")), "Fix parser");
        assert_eq!(
            amend_message(&commit("Fix parser", "It dropped the last token.\n")),
            "Fix parser\n\nIt dropped the last token.\n"
        );
        assert_eq!(amend_message(&commit("Fix parser", "  \n")), "Fix parser");
    }

    #[test]
    fn the_commit_to_amend_is_the_one_head_points_at() {
        let commit = |hash: &str, refs: &str| Commit {
            hash: hash.to_string(),
            full_hash: hash.repeat(6),
            message: hash.to_string(),
            author: "Ana".to_string(),
            email: "ana@example.com".to_string(),
            date: String::new(),
            body: String::new(),
            refs: refs.to_string(),
            parents: String::new(),
        };
        let found = |commits| head_commit(commits).map(|commit: Commit| commit.hash);
        assert_eq!(
            found(vec![
                commit("stash", "refs/stash"),
                commit("main", "HEAD -> refs/heads/main, refs/remotes/origin/main"),
            ]),
            Some("main".to_string())
        );
        assert_eq!(
            found(vec![commit("detached", "HEAD, tag: refs/tags/v1.0.0")]),
            Some("detached".to_string())
        );
        // Un mensaje que menciona HEAD no basta.
        assert_eq!(found(vec![commit("other", "refs/heads/HEADS-UP")]), None);
        assert_eq!(found(Vec::new()), None);
    }

    #[test]
    fn the_list_width_stays_within_its_limits() {
        assert_eq!(clamp_list_width(330.0, 1392.0), 330.0);
        assert_eq!(clamp_list_width(40.0, 1392.0), 160.0);
        assert_eq!(clamp_list_width(900.0, 1392.0), 560.0);
        // Sin dejar menos de 180 a la caja de commit.
        assert_eq!(clamp_list_width(500.0, 600.0), 420.0);
        // Y en una ventana mínima, nunca por debajo del mínimo.
        assert_eq!(clamp_list_width(500.0, 200.0), 160.0);
        assert_eq!(clamp_list_width(900.0, 0.0), 560.0);
    }

    #[test]
    fn a_file_tab_only_lists_its_own_side() {
        assert!(SidebarSource::All.shows_working() && SidebarSource::All.shows_staged());
        assert!(SidebarSource::Working.shows_working() && !SidebarSource::Working.shows_staged());
        assert!(!SidebarSource::Index.shows_working() && SidebarSource::Index.shows_staged());
        assert_eq!(
            SidebarSource::from_tab_source(TabSource::Index),
            SidebarSource::Index
        );
    }

    #[test]
    fn the_file_name_is_the_last_segment() {
        assert_eq!(file_name("src/lib/util.js"), "util.js");
        assert_eq!(file_name("README.md"), "README.md");
        assert_eq!(file_name(""), "");
    }

    #[test]
    fn a_commit_template_drops_its_comments_and_extra_blank_lines() {
        assert_eq!(
            commit_template_message("feat: \n\n# Why this change?\nRefs: #\n"),
            "feat: \n\nRefs: #"
        );
        assert_eq!(
            commit_template_message("# Subject\n\n\nfix(ui): \r\n\r\n\r\n\r\nBody\n\n\n"),
            "fix(ui): \n\nBody"
        );
        assert_eq!(commit_template_message("# only comments\n#\n\n"), "");
        assert_eq!(commit_template_message(""), "");
        // Un `#` que no empieza la línea no es un comentario.
        assert_eq!(commit_template_message(" #123 subject"), " #123 subject");
        let (title, body) = split_commit_message(&commit_template_message(
            "feat: \n\n# Why?\nRefs: #\nSigned-off-by: Ana\n",
        ));
        assert_eq!(title, "feat: ");
        assert_eq!(body, "Refs: #\nSigned-off-by: Ana");
    }

    #[test]
    fn the_local_changes_list_clamps_its_width() {
        assert_eq!(
            clamp_local_changes_width(DEFAULT_LOCAL_CHANGES_WIDTH, 1392.0),
            400.0
        );
        assert_eq!(clamp_local_changes_width(100.0, 1392.0), 320.0);
        assert_eq!(clamp_local_changes_width(2000.0, 1392.0), 900.0);
        // Al diff le quedan al menos 360px.
        assert_eq!(clamp_local_changes_width(600.0, 900.0), 540.0);
        // Ni en una ventana diminuta baja del mínimo.
        assert_eq!(clamp_local_changes_width(600.0, 500.0), 320.0);
        assert_eq!(clamp_local_changes_width(2000.0, 0.0), 900.0);
    }
}
