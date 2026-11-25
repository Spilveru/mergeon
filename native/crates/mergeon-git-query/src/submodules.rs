//! Submódulos (`electron/services/submodules.js`).

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::LazyLock;

use crate::error::{QueryResult, read};
use crate::exec;
use crate::text::js_trim;

/*
 * `git submodule status` marca cada entrada con un prefijo de un carácter:
 *   ' ' → checkout en el commit registrado (ok)
 *   '-' → sin inicializar (no hay working tree)
 *   '+' → checkout en un commit distinto al registrado
 *   'U' → con conflictos de merge
 * Después vienen el SHA, la ruta y opcionalmente `(describe)`.
 */
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubmoduleState {
    Ok,
    Uninitialized,
    OutOfSync,
    Conflict,
}

impl SubmoduleState {
    pub fn as_str(self) -> &'static str {
        match self {
            SubmoduleState::Ok => "ok",
            SubmoduleState::Uninitialized => "uninitialized",
            SubmoduleState::OutOfSync => "out-of-sync",
            SubmoduleState::Conflict => "conflict",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submodule {
    pub path: String,
    pub sha: String,
    pub state: SubmoduleState,
    /// Lo que va entre paréntesis en `git submodule status` (`git describe`).
    pub describe: Option<String>,
}

static DESCRIBE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s\(([^)]*)\)$").unwrap());

pub(crate) fn parse_submodule_status(output: &str) -> Vec<Submodule> {
    output
        .split('\n')
        .filter(|line| !js_trim(line).is_empty())
        .map(|line| {
            let marker = line.chars().next().unwrap_or(' ');
            let state = match marker {
                '-' => SubmoduleState::Uninitialized,
                '+' => SubmoduleState::OutOfSync,
                'U' => SubmoduleState::Conflict,
                _ => SubmoduleState::Ok,
            };
            let rest = js_trim(&line[marker.len_utf8()..]);

            let describe_match = DESCRIBE.captures(rest);
            let describe = describe_match
                .as_ref()
                .map(|captures| captures[1].to_string());
            let without_describe = match &describe_match {
                Some(captures) => &rest[..captures.get(0).map_or(rest.len(), |all| all.start())],
                None => rest,
            };

            let (sha, path) = match without_describe.split_once(' ') {
                Some((sha, path)) => (sha, js_trim(path)),
                None => (without_describe, ""),
            };

            Submodule {
                path: path.to_string(),
                sha: sha.to_string(),
                state,
                describe,
            }
        })
        .filter(|entry| !entry.path.is_empty())
        .collect()
}

/// Canal `git:getSubmodules`.
pub fn get_submodules(repo_path: &Path) -> QueryResult<Vec<Submodule>> {
    read("Failed to read submodules", || {
        let output = exec::raw(&exec::open(repo_path)?, &["submodule", "status"])?;
        Ok(parse_submodule_status(&output))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;
    use mergeon_gitcmd::Git;

    fn submodule(
        path: &str,
        sha: &str,
        state: SubmoduleState,
        describe: Option<&str>,
    ) -> Submodule {
        Submodule {
            path: path.into(),
            sha: sha.into(),
            state,
            describe: describe.map(str::to_string),
        }
    }

    #[test]
    fn parses_the_four_states_of_git_submodule_status() {
        let output = [
            " 1111111111111111111111111111111111111111 libs/ok (v1.2.0)",
            "-2222222222222222222222222222222222222222 libs/uninitialized",
            "+3333333333333333333333333333333333333333 libs/out-of-sync (heads/main)",
            "U4444444444444444444444444444444444444444 libs/conflicted",
        ]
        .join("\n");

        assert_eq!(
            parse_submodule_status(&output),
            [
                submodule(
                    "libs/ok",
                    "1111111111111111111111111111111111111111",
                    SubmoduleState::Ok,
                    Some("v1.2.0")
                ),
                submodule(
                    "libs/uninitialized",
                    "2222222222222222222222222222222222222222",
                    SubmoduleState::Uninitialized,
                    None
                ),
                submodule(
                    "libs/out-of-sync",
                    "3333333333333333333333333333333333333333",
                    SubmoduleState::OutOfSync,
                    Some("heads/main")
                ),
                submodule(
                    "libs/conflicted",
                    "4444444444444444444444444444444444444444",
                    SubmoduleState::Conflict,
                    None
                ),
            ]
        );
    }

    #[test]
    fn keeps_paths_with_spaces_intact() {
        assert_eq!(
            parse_submodule_status(" 1111111111111111111111111111111111111111 vendor/my lib (v2)"),
            [submodule(
                "vendor/my lib",
                "1111111111111111111111111111111111111111",
                SubmoduleState::Ok,
                Some("v2")
            )]
        );
    }

    #[test]
    fn returns_an_empty_list_for_repositories_without_submodules() {
        assert!(parse_submodule_status("").is_empty());
        assert!(parse_submodule_status("\n").is_empty());
    }

    #[test]
    fn serializes_states_with_the_wire_names() {
        let entry = submodule("libs/x", "abc", SubmoduleState::OutOfSync, None);
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["state"], "out-of-sync");
        assert_eq!(json["describe"], serde_json::Value::Null);
        assert_eq!(SubmoduleState::Uninitialized.as_str(), "uninitialized");
    }

    #[test]
    fn lists_the_submodules_of_a_real_repository() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "base\n", "base");
        assert!(get_submodules(&repo.path).unwrap().is_empty());

        let library = TestRepo::new();
        let library_head = library.commit_file("lib.txt", "lib\n", "library commit");

        // Git ya no clona submódulos por `file://` salvo permiso explícito.
        Git::new(&repo.path)
            .with_config(["protocol.file.allow=always"])
            .raw([
                "submodule",
                "add",
                &*library.path.to_string_lossy(),
                "libs/my lib",
            ])
            .unwrap();

        let submodules = get_submodules(&repo.path).unwrap();

        assert_eq!(submodules.len(), 1);
        assert_eq!(submodules[0].path, "libs/my lib");
        assert_eq!(submodules[0].sha, library_head);
        assert_eq!(submodules[0].state, SubmoduleState::Ok);
        assert_eq!(submodules[0].describe.as_deref(), Some("heads/main"));
    }

    #[test]
    fn fails_with_the_read_prefix_outside_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        let error = get_submodules(dir.path()).unwrap_err();
        assert!(
            error
                .message()
                .starts_with("Failed to read submodules: fatal: not a git repository"),
            "{error}"
        );
    }
}
