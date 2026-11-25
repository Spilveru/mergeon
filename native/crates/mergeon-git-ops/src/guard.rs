//! Bloqueo de argumentos peligrosos.
//!
//! simple-git revisa cada comando antes de lanzarlo (`blockUnsafeOperationsPlugin`
//! con `@simple-git/argv-parser`) y rechaza las opciones que permiten ejecutar
//! un programa arbitrario. Mergeon solo desactiva los guards ligados al
//! entorno; los de argumentos siguen activos, y son lo que impide que un
//! nombre de remoto o de rama con pinta de opción (`--upload-pack=...`) acabe
//! ejecutando algo. Aquí se reproduce esa revisión de opciones con sus mismos
//! mensajes. Los guards de claves de configuración (`-c alias...`,
//! `git config core.hooksPath ...`) no aplican: este crate solo escribe claves
//! fijas (`core.abbrev`, `http.extraHeader`, `user.name`, `user.email`).

use mergeon_gitcmd::{GitError, GitResult};

/// Opciones que consumen el argumento siguiente, por comando.
struct FlagSpec {
    short: &'static [(char, bool)],
    long: &'static [&'static str],
}

impl FlagSpec {
    fn short(&self, flag: char) -> Option<bool> {
        self.short
            .iter()
            .find(|(candidate, _)| *candidate == flag)
            .map(|(_, needs_value)| *needs_value)
    }
}

/// Opciones globales de git (antes del subcomando).
const GLOBAL_FLAGS: FlagSpec = FlagSpec {
    short: &[
        ('C', true),
        ('P', false),
        ('h', false),
        ('p', false),
        ('v', false),
        ('c', true),
    ],
    long: &[
        "attr-source",
        "config-env",
        "exec-path",
        "git-dir",
        "list-cmds",
        "namespace",
        "super-prefix",
        "work-tree",
    ],
};

fn task_flags(task: &str) -> FlagSpec {
    match task {
        "clone" => FlagSpec {
            short: &[
                ('c', true),
                ('b', true),
                ('j', true),
                ('l', false),
                ('n', false),
                ('o', true),
                ('q', false),
                ('s', false),
                ('u', true),
            ],
            long: &[
                "branch",
                "config",
                "jobs",
                "origin",
                "upload-pack",
                "u",
                "template",
            ],
        },
        "commit" => FlagSpec {
            short: &[
                ('c', true),
                ('C', true),
                ('F', true),
                ('m', true),
                ('t', true),
            ],
            long: &[
                "file",
                "message",
                "reedit-message",
                "reuse-message",
                "template",
            ],
        },
        "config" => FlagSpec {
            short: &[('c', true), ('e', false), ('f', true), ('l', false)],
            long: &["blob", "comment", "default", "file", "type", "value"],
        },
        "fetch" | "pull" => FlagSpec {
            short: &[('c', true)],
            long: &["upload-pack"],
        },
        "init" => FlagSpec {
            short: &[('c', true)],
            long: &["template"],
        },
        "push" => FlagSpec {
            short: &[('c', true)],
            long: &["exec", "receive-pack"],
        },
        _ => FlagSpec {
            short: &[('c', true)],
            long: &[],
        },
    }
}

struct Flag {
    name: String,
    has_value: bool,
    needs_next: bool,
}

impl Flag {
    fn bare(name: impl Into<String>, needs_next: bool) -> Self {
        Self {
            name: name.into(),
            has_value: false,
            needs_next,
        }
    }

    fn with_value(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            has_value: true,
            needs_next: false,
        }
    }
}

/// Descompone un argumento con forma de opción en las opciones que contiene.
fn tokenize(argument: &str, spec: &FlagSpec) -> Vec<Flag> {
    if let Some(long) = argument.strip_prefix("--") {
        return match argument.find('=') {
            Some(at) if at > 2 => vec![Flag::with_value(&argument[..at])],
            _ => vec![Flag::bare(argument, spec.long.contains(&long))],
        };
    }

    let letters: Vec<char> = argument.chars().skip(1).collect();
    if letters.len() == 1 {
        return vec![Flag::bare(argument, spec.short(letters[0]) == Some(true))];
    }

    // Opciones cortas agrupadas (`-am`, `-bmain`).
    let mut flags = Vec::new();
    for (index, letter) in letters.iter().enumerate() {
        let Some(needs_value) = spec.short(*letter) else {
            return vec![Flag::bare(argument, false)];
        };
        if needs_value {
            let rest = &letters[index + 1..];
            if !rest.is_empty() && !rest.iter().all(|other| spec.short(*other).is_some()) {
                flags.push(Flag::with_value(format!("-{letter}")));
                return flags;
            }
        }
        flags.push(Flag::bare(format!("-{letter}"), needs_value));
    }
    flags
}

fn looks_like_flag(argument: &str) -> bool {
    argument.starts_with('-') && argument.chars().count() >= 2
}

/// Añade a `names` las opciones de `argument` y devuelve cuántos argumentos se consumieron.
fn collect_flags(arguments: &[&str], at: usize, spec: &FlagSpec, names: &mut Vec<String>) -> usize {
    let mut next = at + 1;
    for flag in tokenize(arguments[at], spec) {
        if flag.needs_next && !flag.has_value && next < arguments.len() {
            next += 1;
        }
        names.push(flag.name);
    }
    next - at
}

fn violation(task: Option<&str>, name: &str) -> Option<&'static str> {
    if name.contains("--upload-pack") || name.contains("--receive-pack") {
        return Some(
            "Use of --upload-pack or --receive-pack is not permitted without enabling allowUnsafePack",
        );
    }
    if task == Some("clone") {
        // /^-\w*u/
        let has_short_u = name.strip_prefix('-').is_some_and(|rest| {
            rest.chars()
                .take_while(|letter| letter.is_ascii_alphanumeric() || *letter == '_')
                .any(|letter| letter == 'u')
        });
        if has_short_u {
            return Some(
                "Use of clone with option /^-\\w*u/ is not permitted without enabling allowUnsafePack",
            );
        }
        if name.contains("--u") {
            return Some(
                "Use of clone with option --u is not permitted without enabling allowUnsafePack",
            );
        }
    }
    if task == Some("push") && name.contains("--exec") {
        return Some(
            "Use of push with option --exec is not permitted without enabling allowUnsafePack",
        );
    }
    if name.contains("--template") {
        return Some("Use of --template is not permitted without enabling allowUnsafeTemplateDir");
    }
    None
}

/// Mensaje del primer argumento no permitido de `arguments`, si lo hay.
pub(crate) fn unsafe_argument<S: AsRef<str>>(arguments: &[S]) -> Option<&'static str> {
    let arguments: Vec<&str> = arguments.iter().map(AsRef::as_ref).collect();
    let mut names = Vec::new();

    let mut index = 0;
    while index < arguments.len() && looks_like_flag(arguments[index]) {
        index += collect_flags(&arguments, index, &GLOBAL_FLAGS, &mut names);
    }

    let task = arguments.get(index).map(|task| task.to_lowercase());
    if let Some(task) = &task {
        let spec = task_flags(task);
        index += 1;
        while index < arguments.len() {
            let argument = arguments[index];
            if argument == "--" {
                break;
            }
            if looks_like_flag(argument) {
                index += collect_flags(&arguments, index, &spec, &mut names);
            } else {
                index += 1;
            }
        }
    }

    names
        .iter()
        .find_map(|name| violation(task.as_deref(), name))
}

/// Falla, sin lanzar git, si `arguments` lleva una opción no permitida.
pub(crate) fn check_arguments<S: AsRef<str>>(arguments: &[S]) -> GitResult<()> {
    match unsafe_argument(arguments) {
        Some(message) => Err(GitError::message(message)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACK: &str =
        "Use of --upload-pack or --receive-pack is not permitted without enabling allowUnsafePack";
    const PUSH_EXEC: &str =
        "Use of push with option --exec is not permitted without enabling allowUnsafePack";
    const TEMPLATE: &str =
        "Use of --template is not permitted without enabling allowUnsafeTemplateDir";
    const CLONE_SHORT_U: &str =
        "Use of clone with option /^-\\w*u/ is not permitted without enabling allowUnsafePack";
    const CLONE_LONG_U: &str =
        "Use of clone with option --u is not permitted without enabling allowUnsafePack";

    /// Casos contrastados con `vulnerabilityCheck` de `@simple-git/argv-parser` 1.1.1.
    #[test]
    fn matches_the_simple_git_argument_guard() {
        let cases: &[(&[&str], Option<&str>)] = &[
            (
                &["pull", "--no-rebase", "--upload-pack=touch /tmp/x", "main"],
                Some(PACK),
            ),
            (&["push", "--receive-pack", "sh", "main"], Some(PACK)),
            (&["push", "origin", "--exec=sh"], Some(PUSH_EXEC)),
            (
                &["tag", "-a", "v1", "HEAD", "-m", "--template x"],
                Some(TEMPLATE),
            ),
            (&["clone", "-u", "sh", "url"], Some(CLONE_SHORT_U)),
            // El mensaje de commit lo consume `-m`: no es una opción.
            (
                &["-c", "core.abbrev=40", "commit", "-m", "--template x"],
                None,
            ),
            (&["commit", "-am", "--upload-pack"], Some(PACK)),
            (&["commit", "-mfoo", "--upload-pack"], Some(PACK)),
            // Tras `--` todo son rutas.
            (&["checkout", "--", "--template"], None),
            (&["fetch", "--exec=x"], None),
            (&["push", "-u", "origin", "main"], None),
            (
                &[
                    "push",
                    "origin",
                    "main",
                    "--delete",
                    "--verbose",
                    "--porcelain",
                ],
                None,
            ),
            (&[], None),
            (&["stash", "push", "-m", "--template-ish"], Some(TEMPLATE)),
            (&["PUSH", "--exec"], Some(PUSH_EXEC)),
            (&["-C", "/x", "push", "--exec", "y"], Some(PUSH_EXEC)),
            (&["--git-dir", "--upload-pack", "status"], None),
            (&["-", "push", "--exec"], None),
            (&["commit", "--message", "--receive-pack"], None),
            (&["commit", "-m"], None),
            (&["checkout", "-b", "--template", "x"], Some(TEMPLATE)),
            (&["clone", "--upload-pack", "x"], Some(PACK)),
            (&["clone", "-qu"], Some(CLONE_SHORT_U)),
            (&["clone", "--u=1"], Some(CLONE_LONG_U)),
        ];

        for (arguments, expected) in cases {
            assert_eq!(unsafe_argument(arguments), *expected, "{arguments:?}");
        }
    }

    #[test]
    fn the_error_carries_the_simple_git_message() {
        let error = check_arguments(&["pull", "--upload-pack=x"]).unwrap_err();
        assert_eq!(error.message, PACK);
        assert!(check_arguments(&["pull", "--no-rebase", "origin", "main"]).is_ok());
    }
}
