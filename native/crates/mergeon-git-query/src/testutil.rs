//! Repositorios git reales en directorios temporales para los tests.

use std::path::{Path, PathBuf};

use mergeon_gitcmd::Git;

pub(crate) fn git_in(dir: &Path, args: &[&str]) -> String {
    match Git::new(dir).raw(args) {
        Ok(output) => output,
        Err(error) => panic!("git {args:?} failed: {}", error.message),
    }
}

/// Un repo con identidad propia y rama `main`, sin depender de la
/// configuración global de la máquina.
pub(crate) struct TestRepo {
    root: tempfile::TempDir,
    pub path: PathBuf,
}

impl TestRepo {
    pub fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("repo");
        std::fs::create_dir(&path).unwrap();
        git_in(&path, &["init", "-b", "main"]);
        for (key, value) in [
            ("user.name", "Test User"),
            ("user.email", "test@example.com"),
            ("commit.gpgsign", "false"),
            ("tag.gpgsign", "false"),
            ("core.autocrlf", "false"),
        ] {
            git_in(&path, &["config", key, value]);
        }
        Self { root, path }
    }

    /// Directorio que contiene al repo (para remotos bare y otros repos).
    pub fn root(&self) -> &Path {
        self.root.path()
    }

    pub fn git(&self, args: &[&str]) -> String {
        git_in(&self.path, args)
    }

    pub fn git_stdin(&self, args: &[&str], stdin: &str) -> String {
        match Git::new(&self.path).raw_with_stdin(args, stdin.as_bytes()) {
            Ok(output) => output,
            Err(error) => panic!("git {args:?} failed: {}", error.message),
        }
    }

    pub fn write(&self, name: &str, content: impl AsRef<[u8]>) {
        let path = self.path.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    pub fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    /// Escribe `name`, lo añade y hace commit. Devuelve el hash completo.
    pub fn commit_file(&self, name: &str, content: &str, message: &str) -> String {
        self.write(name, content);
        self.git(&["add", "--", name]);
        self.git(&["commit", "-m", message]);
        self.head()
    }

    pub fn empty_commit(&self, message: &str) -> String {
        self.git(&["commit", "--allow-empty", "-m", message]);
        self.head()
    }

    /// Crea un remoto bare local, lo registra como `origin` y devuelve su ruta.
    pub fn add_bare_origin(&self) -> PathBuf {
        let remote = self.root().join("remote.git");
        std::fs::create_dir(&remote).unwrap();
        git_in(&remote, &["init", "--bare", "-b", "main"]);
        self.git(&["remote", "add", "origin", &remote.to_string_lossy()]);
        remote
    }
}
