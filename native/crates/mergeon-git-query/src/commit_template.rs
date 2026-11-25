//! La plantilla del mensaje de commit (`commit.template`).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mergeon_gitcmd::{Git, GitError};

use crate::error::{Inner, QueryResult, read};
use crate::exec;

/// Una plantilla más grande que esto no es una plantilla: no se carga.
pub const MAX_COMMIT_TEMPLATE_BYTES: u64 = 64 * 1024;

/// El contenido de la plantilla de commit del repositorio (o de la
/// configuración global), tal cual, con sus comentarios. `None` si no hay
/// `commit.template`, si el archivo no existe o si es demasiado grande
/// ([`MAX_COMMIT_TEMPLATE_BYTES`]).
///
/// La ruta puede empezar por `~` (la expande `git config --path`) o ser
/// relativa: como al hacer commit desde la raíz, es relativa al repositorio.
pub fn get_commit_template(repo_path: &Path) -> QueryResult<Option<String>> {
    read("Failed to read commit template", || {
        commit_template_with(&exec::open(repo_path)?, repo_path)
    })
}

/// Qué archivo de plantilla había la última vez y cómo estaba: si ni la
/// configuración ni el archivo cambiaron, no hace falta volver a leerlo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitTemplateStamp {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
}

/// Lo que da [`read_commit_template_if_changed`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommitTemplateRead {
    /// El sello de la plantilla vigente; `None` si no hay (sin
    /// `commit.template`, archivo que no existe o demasiado grande).
    pub stamp: Option<CommitTemplateStamp>,
    /// Su contenido, solo si `stamp` es distinto del de la última vez (si es
    /// el mismo, no se relee y vale lo que ya se tenía).
    pub content: Option<String>,
}

impl CommitTemplateRead {
    /// La plantilla cambió (o se quitó) respecto de `last`.
    pub fn changed_since(&self, last: Option<&CommitTemplateStamp>) -> bool {
        self.stamp.as_ref() != last
    }
}

/// Relee `commit.template` (un `git config`) y la fecha y el tamaño de su
/// archivo; el archivo solo se lee si algo cambió respecto de `last`. Para
/// mirar barato, fuera del hilo de la interfaz, si el usuario cambió o quitó
/// la plantilla.
pub fn read_commit_template_if_changed(
    repo_path: &Path,
    last: Option<&CommitTemplateStamp>,
) -> QueryResult<CommitTemplateRead> {
    read("Failed to read commit template", || {
        template_read_with(&exec::open(repo_path)?, repo_path, last)
    })
}

fn commit_template_with(git: &Git, repo_path: &Path) -> Inner<Option<String>> {
    Ok(template_read_with(git, repo_path, None)?.content)
}

fn template_read_with(
    git: &Git,
    repo_path: &Path,
    last: Option<&CommitTemplateStamp>,
) -> Inner<CommitTemplateRead> {
    let output = exec::exec(git, &["config", "--path", "--get", "commit.template"])?;
    match output.exit_code {
        Some(0) => {}
        // 1: la clave no está.
        Some(1) => return Ok(CommitTemplateRead::default()),
        _ => return Err(GitError::from_output(&output).into()),
    }
    let value = output.stdout_text();
    let value = value.trim_end_matches(['\n', '\r']);
    if value.is_empty() {
        return Ok(CommitTemplateRead::default());
    }
    let path = template_path(repo_path, value);
    let Ok(metadata) = std::fs::metadata(&path) else {
        return Ok(CommitTemplateRead::default());
    };
    if !metadata.is_file() || metadata.len() > MAX_COMMIT_TEMPLATE_BYTES {
        return Ok(CommitTemplateRead::default());
    }
    let stamp = CommitTemplateStamp {
        path,
        modified: metadata.modified().ok(),
        len: metadata.len(),
    };
    if last == Some(&stamp) {
        return Ok(CommitTemplateRead {
            stamp: Some(stamp),
            content: None,
        });
    }
    match std::fs::read(&stamp.path) {
        Ok(bytes) => Ok(CommitTemplateRead {
            content: Some(String::from_utf8_lossy(&bytes).into_owned()),
            stamp: Some(stamp),
        }),
        Err(_) => Ok(CommitTemplateRead::default()),
    }
}

/// La ruta ya expandida por git; si es relativa, desde la raíz del repositorio.
fn template_path(repo_path: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo_path.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    const TEMPLATE: &str = "feat: \n\n# Why this change?\nRefs: #\n";

    /// git con una configuración global propia y un `HOME` propio, sin tocar
    /// los del proceso (los tests corren en paralelo).
    fn isolated_git(repo: &TestRepo, home: &Path) -> Git {
        Git::new(&repo.path)
            .with_env("HOME", home)
            .with_env("GIT_CONFIG_GLOBAL", home.join(".gitconfig"))
            .with_env("GIT_CONFIG_NOSYSTEM", "1")
    }

    fn home(repo: &TestRepo) -> PathBuf {
        let home = repo.root().join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join(".gitconfig"), "").unwrap();
        home
    }

    #[test]
    fn no_template_is_none() {
        let repo = TestRepo::new();
        let home = home(&repo);
        assert_eq!(
            commit_template_with(&isolated_git(&repo, &home), &repo.path).unwrap(),
            None
        );
    }

    #[test]
    fn reads_a_template_relative_to_the_repository() {
        let repo = TestRepo::new();
        let home = home(&repo);
        repo.write(".github/commit template.txt", TEMPLATE);
        repo.git(&["config", "commit.template", ".github/commit template.txt"]);

        assert_eq!(
            commit_template_with(&isolated_git(&repo, &home), &repo.path).unwrap(),
            Some(TEMPLATE.to_string())
        );
        // La función pública lee la misma.
        assert_eq!(
            get_commit_template(&repo.path).unwrap(),
            Some(TEMPLATE.to_string())
        );
    }

    #[test]
    fn expands_the_home_directory_of_a_global_template() {
        let repo = TestRepo::new();
        let home = home(&repo);
        std::fs::write(home.join(".gitmessage"), "chore: ñandú\n").unwrap();
        std::fs::write(
            home.join(".gitconfig"),
            "[commit]\n\ttemplate = ~/.gitmessage\n",
        )
        .unwrap();

        assert_eq!(
            commit_template_with(&isolated_git(&repo, &home), &repo.path).unwrap(),
            Some("chore: ñandú\n".to_string())
        );
    }

    #[test]
    fn the_repository_template_wins_over_the_global_one() {
        let repo = TestRepo::new();
        let home = home(&repo);
        std::fs::write(home.join("global.txt"), "global\n").unwrap();
        std::fs::write(
            home.join(".gitconfig"),
            format!(
                "[commit]\n\ttemplate = \"{}\"\n",
                // En un gitconfig `\` escapa: las rutas de Windows lo llevan.
                home.join("global.txt")
                    .to_string_lossy()
                    .replace('\\', "\\\\")
            ),
        )
        .unwrap();
        repo.write("local.txt", "local\n");
        repo.git(&["config", "commit.template", "local.txt"]);

        assert_eq!(
            commit_template_with(&isolated_git(&repo, &home), &repo.path).unwrap(),
            Some("local\n".to_string())
        );
    }

    #[test]
    fn a_missing_or_huge_template_is_none() {
        let repo = TestRepo::new();
        let home = home(&repo);
        let git = isolated_git(&repo, &home);

        repo.git(&["config", "commit.template", "missing.txt"]);
        assert_eq!(commit_template_with(&git, &repo.path).unwrap(), None);

        repo.write(
            "huge.txt",
            "x".repeat(MAX_COMMIT_TEMPLATE_BYTES as usize + 1),
        );
        repo.git(&["config", "commit.template", "huge.txt"]);
        assert_eq!(commit_template_with(&git, &repo.path).unwrap(), None);
    }

    /// Lo que se relee al vaciarse la caja o al volver a "Local Changes":
    /// solo el `git config` y la fecha del archivo, salvo que algo cambiara.
    #[test]
    fn rereads_the_template_only_when_it_changed() {
        let repo = TestRepo::new();
        let home = home(&repo);
        let git = isolated_git(&repo, &home);
        repo.write("one.txt", "one\n");
        repo.git(&["config", "commit.template", "one.txt"]);

        let first = template_read_with(&git, &repo.path, None).unwrap();
        assert_eq!(first.content.as_deref(), Some("one\n"));
        assert!(first.changed_since(None));

        // Nada cambió: no se relee el archivo.
        let same = template_read_with(&git, &repo.path, first.stamp.as_ref()).unwrap();
        assert!(!same.changed_since(first.stamp.as_ref()));
        assert_eq!(same.content, None);

        // Otro contenido (y otro tamaño) en el mismo archivo.
        repo.write("one.txt", "one, edited\n");
        let edited = template_read_with(&git, &repo.path, first.stamp.as_ref()).unwrap();
        assert!(edited.changed_since(first.stamp.as_ref()));
        assert_eq!(edited.content.as_deref(), Some("one, edited\n"));

        // Otra plantilla en la configuración.
        repo.write("two.txt", "two\n");
        repo.git(&["config", "commit.template", "two.txt"]);
        let other = template_read_with(&git, &repo.path, edited.stamp.as_ref()).unwrap();
        assert!(other.changed_since(edited.stamp.as_ref()));
        assert_eq!(other.content.as_deref(), Some("two\n"));

        // Sin plantilla: cambió, y ya no hay.
        repo.git(&["config", "--unset", "commit.template"]);
        let gone = template_read_with(&git, &repo.path, other.stamp.as_ref()).unwrap();
        assert!(gone.changed_since(other.stamp.as_ref()));
        assert_eq!(gone, CommitTemplateRead::default());
        assert!(!gone.changed_since(None));
        // La función pública hace lo mismo (con la plantilla del repositorio,
        // que gana a una global que pudiera tener la máquina).
        repo.git(&["config", "commit.template", "two.txt"]);
        let public = read_commit_template_if_changed(&repo.path, None).unwrap();
        assert_eq!(public.content.as_deref(), Some("two\n"));
        let again = read_commit_template_if_changed(&repo.path, public.stamp.as_ref()).unwrap();
        assert_eq!(again.content, None);
    }
}
