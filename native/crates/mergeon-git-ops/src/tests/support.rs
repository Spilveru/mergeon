//! Fixtures compartidas por los tests.

use std::path::{Path, PathBuf};

use crate::exec::git;

/// Entorno base de los git que lanza este crate durante los tests: sin
/// configuración global ni de sistema, para no depender de la máquina.
pub(crate) fn hermetic_env() -> Vec<(String, String)> {
    vec![
        ("GIT_CONFIG_GLOBAL".to_string(), "/dev/null".to_string()),
        ("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()),
    ]
}

pub(crate) fn create_temp_root() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("git-ops-")
        .tempdir()
        .expect("temp root")
}

/// Ejecuta git en `cwd` y devuelve stdout; el test falla si git falla.
pub(crate) fn run_git(cwd: &Path, arguments: &[&str]) -> String {
    git(cwd)
        .raw(arguments)
        .unwrap_or_else(|error| panic!("git {arguments:?} failed: {}", error.message))
}

pub(crate) fn path_str(path: &Path) -> &str {
    path.to_str().expect("utf-8 path")
}

/// Repositorio con identidad propia, sin firma de commits y con rama inicial `main`.
pub(crate) fn init_repository(repo_path: &Path) {
    let parent = repo_path.parent().expect("repository parent");
    run_git(parent, &["init", "-b", "main", path_str(repo_path)]);
    configure_identity(repo_path, "Test User", "test@example.com");
}

pub(crate) fn configure_identity(repo_path: &Path, name: &str, email: &str) {
    run_git(repo_path, &["config", "user.name", name]);
    run_git(repo_path, &["config", "user.email", email]);
    run_git(repo_path, &["config", "commit.gpgsign", "false"]);
}

/// Repositorio bare que hace de remoto. Con `main_head`, su HEAD apunta a
/// `main` para que los clones nazcan en esa rama.
pub(crate) fn init_bare(path: &Path, main_head: bool) {
    let parent = path.parent().expect("bare parent");
    if main_head {
        run_git(parent, &["init", "--bare", "-b", "main", path_str(path)]);
    } else {
        run_git(parent, &["init", "--bare", path_str(path)]);
    }
}

pub(crate) fn clone_repository(source: &Path, destination: &Path, name: &str, email: &str) {
    let parent = destination.parent().expect("clone parent");
    run_git(parent, &["clone", path_str(source), path_str(destination)]);
    configure_identity(destination, name, email);
}

/// Escribe `content` en `file_path`, lo confirma y devuelve el hash del commit.
pub(crate) fn commit_file_at(
    repo_path: &Path,
    content: &str,
    message: &str,
    file_path: &str,
) -> String {
    std::fs::write(repo_path.join(file_path), content).expect("write file");
    run_git(repo_path, &["add", file_path]);
    run_git(repo_path, &["commit", "-m", message]);
    head(repo_path)
}

pub(crate) fn commit_file(repo_path: &Path, content: &str, message: &str) -> String {
    commit_file_at(repo_path, content, message, "file.txt")
}

pub(crate) fn head(repo_path: &Path) -> String {
    run_git(repo_path, &["rev-parse", "HEAD"])
        .trim()
        .to_string()
}

/// Asuntos de los commits de HEAD, del más reciente al más antiguo.
pub(crate) fn log_subjects(repo_path: &Path) -> Vec<String> {
    run_git(repo_path, &["log", "--pretty=%s"])
        .trim()
        .split('\n')
        .map(str::to_string)
        .collect()
}

/// Contenido de un archivo con finales de línea normalizados.
pub(crate) fn read_file(path: &Path) -> String {
    std::fs::read_to_string(path)
        .expect("read file")
        .replace("\r\n", "\n")
}

pub(crate) struct DivergedRepository {
    /// Mantiene vivo el directorio temporal.
    pub _root: tempfile::TempDir,
    pub repo_path: PathBuf,
    pub feature_commit: String,
}

/// `main` y `feature` modifican la misma línea de `file.txt`: fusionar,
/// rebasar o hacer cherry-pick entre ellas da conflicto. Queda en `main`.
pub(crate) fn create_diverged_repository() -> DivergedRepository {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");
    init_repository(&repo_path);

    commit_file(&repo_path, "base\n", "base");
    run_git(&repo_path, &["checkout", "-b", "feature"]);
    let feature_commit = commit_file(&repo_path, "feature\n", "feature change");
    run_git(&repo_path, &["checkout", "main"]);
    commit_file(&repo_path, "main\n", "main change");

    DivergedRepository {
        _root: root,
        repo_path,
        feature_commit,
    }
}

/// Repositorio con un commit base en `main`.
pub(crate) fn create_repository() -> (tempfile::TempDir, PathBuf) {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");
    init_repository(&repo_path);
    commit_file(&repo_path, "base\n", "base");
    (root, repo_path)
}

/// Ejecuta un script con el mismo `sh` que usa git para sus editores (un alias
/// de shell), de modo que el test no dependa de tener `sh` en el PATH. Devuelve
/// si el script terminó bien.
pub(crate) fn try_run_sh_script(directory: &Path, script: &Path, argument: &Path) -> bool {
    git(directory)
        .exec([
            "-c",
            "alias.run-script=!sh",
            "run-script",
            path_str(script),
            path_str(argument),
        ])
        .expect("run sh script")
        .success()
}
