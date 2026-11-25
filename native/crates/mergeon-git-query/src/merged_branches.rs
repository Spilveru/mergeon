//! Ramas locales que se pueden limpiar: las ya integradas en la rama
//! principal (sin original en la app Electron).
//!
//! Tres procesos fijos (`symbolic-ref` de `origin/HEAD`, un `for-each-ref`
//! con todo lo que se muestra y otro con `--merged`) más, para las ramas no
//! integradas, la detección de squash-merge: lo que la rama cambió desde su
//! base común ya está igual en la base. Esa parte cuesta dos `git diff` por
//! rama y se limita a las [`SQUASH_CHECK_LIMIT`] más recientes.

use std::collections::HashSet;
use std::path::Path;

use mergeon_gitcmd::Git;
use serde::{Deserialize, Serialize};

use crate::error::{QueryResult, read};
use crate::exec;
use crate::text::js_trim;

const FIELD_SEPARATOR: char = '\u{1f}';

/// Ramas no integradas en las que se busca un squash-merge, como mucho.
pub const SQUASH_CHECK_LIMIT: usize = 100;

/// Nombres que nunca se proponen borrar (además de la base y la actual).
const PROTECTED_NAMES: &[&str] = &[
    "main",
    "master",
    "develop",
    "development",
    "dev",
    "trunk",
    "staging",
    "production",
    "prod",
    "release",
    "gh-pages",
];

/// Prefijos de ramas protegidas (`release/1.2`).
const PROTECTED_PREFIXES: &[&str] = &["release/", "releases/"];

/// Rama de larga vida que la limpieza no toca nunca.
pub fn is_protected_branch(name: &str) -> bool {
    PROTECTED_NAMES.contains(&name)
        || PROTECTED_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

/// Cómo está integrada una rama en la base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MergeState {
    /// Su punta es alcanzable desde la base (merge o fast-forward).
    Merged,
    /// Sus cambios ya están en la base, pero no sus commits (squash o
    /// rebase): "probably merged".
    Squashed,
    /// Tiene commits sin integrar; solo se lista si su upstream ya no existe.
    Unmerged,
}

/// Una rama que se puede proponer borrar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupCandidate {
    pub name: String,
    pub state: MergeState,
    /// Hash completo de la punta.
    pub tip: String,
    /// Hash abreviado de la punta.
    pub short_tip: String,
    /// Asunto del último commit.
    pub subject: String,
    /// Fecha del último commit (segundos Unix).
    pub committed_at: i64,
    /// Upstream configurado (`origin/feature`).
    pub upstream: Option<String>,
    /// El upstream configurado ya no existe (se borró en el remoto).
    pub upstream_gone: bool,
    /// Commits de la rama que no están en la base (0 si está integrada).
    pub unmerged_commits: u32,
}

impl CleanupCandidate {
    /// Se marca por defecto en el diálogo: solo las integradas de verdad.
    pub fn selected_by_default(&self) -> bool {
        self.state == MergeState::Merged
    }

    /// Borrarla necesita `-D` (git no la ve integrada).
    pub fn needs_force(&self) -> bool {
        self.state != MergeState::Merged
    }
}

/// Resultado de [`get_merged_branches`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergedBranchesReport {
    /// Base contra la que se comparó.
    pub base: String,
    /// La base que se elige sin pedir otra (`origin/HEAD`, `main`, `master`).
    pub default_base: Option<String>,
    /// Bases posibles: las ramas locales y la rama por defecto del remoto.
    pub base_options: Vec<String>,
    pub current_branch: Option<String>,
    /// Candidatas, primero las integradas y dentro de cada grupo las más
    /// recientes primero.
    pub candidates: Vec<CleanupCandidate>,
    /// Hubo más ramas sin integrar de las que se miraron por squash.
    pub squash_check_truncated: bool,
}

#[derive(Debug, Clone)]
struct LocalBranch {
    name: String,
    tip: String,
    short_tip: String,
    upstream: Option<String>,
    upstream_gone: bool,
    committed_at: i64,
    subject: String,
    /// Extraída en algún worktree (este o uno enlazado).
    checked_out: bool,
}

fn parse_local_branches(raw: &str) -> Vec<LocalBranch> {
    raw.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.splitn(8, FIELD_SEPARATOR).collect();
            let [
                refname,
                tip,
                short_tip,
                upstream,
                track,
                date,
                worktree,
                subject,
            ] = fields.as_slice()
            else {
                return None;
            };
            let name = refname.strip_prefix("refs/heads/")?.to_string();
            let upstream = js_trim(upstream);
            Some(LocalBranch {
                name,
                tip: tip.to_string(),
                short_tip: short_tip.to_string(),
                upstream: (!upstream.is_empty()).then(|| upstream.to_string()),
                upstream_gone: track.contains("gone"),
                committed_at: date.trim().parse().unwrap_or(0),
                subject: subject.to_string(),
                checked_out: !worktree.trim().is_empty(),
            })
        })
        .collect()
}

fn local_branches(git: &Git) -> Result<Vec<LocalBranch>, mergeon_gitcmd::GitError> {
    let format = [
        "%(refname)",
        "%(objectname)",
        "%(objectname:short)",
        "%(upstream:short)",
        "%(upstream:track)",
        "%(committerdate:unix)",
        "%(worktreepath)",
        "%(contents:subject)",
    ]
    .join(&FIELD_SEPARATOR.to_string());
    let raw = exec::raw(
        git,
        &[
            "for-each-ref".to_string(),
            format!("--format={format}"),
            "refs/heads".to_string(),
        ],
    )?;
    Ok(parse_local_branches(&raw))
}

/// La rama por defecto: la de `origin/HEAD` (en local si existe), si no
/// `main` o `master`.
fn default_base(git: &Git, locals: &HashSet<&str>) -> Option<String> {
    let remote_head = exec::exec(
        git,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .ok()
    .filter(|output| output.success())
    .map(|output| js_trim(&output.stdout_text()).to_string())
    .filter(|name| !name.is_empty());

    if let Some(remote_head) = remote_head {
        let local = remote_head
            .strip_prefix("origin/")
            .unwrap_or(&remote_head)
            .to_string();
        return Some(if locals.contains(local.as_str()) {
            local
        } else {
            remote_head
        });
    }
    ["main", "master"]
        .into_iter()
        .find(|name| locals.contains(name))
        .map(str::to_string)
}

/// Resuelve `base` a un commit, sin dejar que un nombre con pinta de opción
/// llegue a git como tal.
fn resolve_commit(git: &Git, base: &str) -> Option<String> {
    if base.is_empty() || base.starts_with('-') {
        return None;
    }
    let output = exec::exec(
        git,
        &[
            "rev-parse".to_string(),
            "--verify".to_string(),
            "--quiet".to_string(),
            "--end-of-options".to_string(),
            format!("{base}^{{commit}}"),
        ],
    )
    .ok()?;
    output
        .success()
        .then(|| js_trim(&output.stdout_text()).to_string())
        .filter(|oid| !oid.is_empty())
}

/// Archivos (rutas `-z`) que difieren entre dos commits.
fn changed_paths(git: &Git, from: &str, to: &str) -> Option<HashSet<String>> {
    let output = exec::exec(
        git,
        &["diff", "--name-only", "-z", "--no-renames", from, to],
    )
    .ok()?;
    output.success().then(|| {
        output
            .stdout_text()
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect()
    })
}

/*
 * Squash-merge (o rebase) comparando árboles: lo que la rama cambió desde su
 * base común con `base` ya está igual en `base`. Si `base` volvió a tocar
 * esos archivos después, no se reconoce (falso negativo, el lado seguro).
 */
fn is_squash_merged(git: &Git, base_oid: &str, tip: &str) -> bool {
    let Some(merge_base) = exec::exec(git, &["merge-base", base_oid, tip])
        .ok()
        .filter(|output| output.success())
        .map(|output| js_trim(&output.stdout_text()).to_string())
    else {
        return false;
    };
    let Some(changed) = changed_paths(git, &merge_base, tip) else {
        return false;
    };
    if changed.is_empty() {
        return false;
    }
    let Some(differs) = changed_paths(git, tip, base_oid) else {
        return false;
    };
    changed.is_disjoint(&differs)
}

fn count_unmerged(git: &Git, base_oid: &str, tip: &str) -> u32 {
    exec::exec(
        git,
        &["rev-list", "--count", format!("{base_oid}..{tip}").as_str()],
    )
    .ok()
    .filter(|output| output.success())
    .and_then(|output| js_trim(&output.stdout_text()).parse().ok())
    .unwrap_or(0)
}

/// Canal `git:getMergedBranches`: ramas locales integradas en `base` (o en la
/// rama por defecto), sin la actual, la base ni las protegidas.
pub fn get_merged_branches(
    repo_path: &Path,
    base: Option<&str>,
) -> QueryResult<MergedBranchesReport> {
    read("Failed to find merged branches", || {
        let git = exec::open(repo_path)?;
        let locals = local_branches(&git)?;
        let names: HashSet<&str> = locals.iter().map(|branch| branch.name.as_str()).collect();
        let default_base = default_base(&git, &names);

        let base = base
            .map(js_trim)
            .filter(|base| !base.is_empty())
            .map(str::to_string)
            .or_else(|| default_base.clone())
            .ok_or("No main branch found. Choose a base branch.")?;
        let base_oid = resolve_commit(&git, &base)
            .ok_or_else(|| format!("Branch '{base}' does not exist."))?;

        let current_branch = exec::exec(&git, &["symbolic-ref", "--quiet", "--short", "HEAD"])
            .ok()
            .filter(|output| output.success())
            .map(|output| js_trim(&output.stdout_text()).to_string())
            .filter(|name| !name.is_empty());

        let mut base_options: Vec<String> =
            locals.iter().map(|branch| branch.name.clone()).collect();
        if let Some(default) = default_base
            .as_ref()
            .filter(|default| !names.contains(default.as_str()))
        {
            base_options.insert(0, default.clone());
        }
        if !base_options.contains(&base) {
            base_options.insert(0, base.clone());
        }

        let merged: HashSet<String> = exec::raw(
            &git,
            &[
                "for-each-ref".to_string(),
                format!("--merged={base_oid}"),
                "--format=%(refname)".to_string(),
                "refs/heads".to_string(),
            ],
        )?
        .lines()
        .filter_map(|line| line.strip_prefix("refs/heads/"))
        .map(str::to_string)
        .collect();

        let base_local = base.strip_prefix("origin/").unwrap_or(&base);
        let mut eligible: Vec<&LocalBranch> = locals
            .iter()
            .filter(|branch| {
                !branch.checked_out
                    && Some(&branch.name) != current_branch.as_ref()
                    && branch.name != base
                    && branch.name != base_local
                    && Some(&branch.name) != default_base.as_ref()
                    && !is_protected_branch(&branch.name)
            })
            .collect();
        eligible.sort_by(|left, right| {
            right
                .committed_at
                .cmp(&left.committed_at)
                .then_with(|| left.name.cmp(&right.name))
        });

        let mut candidates = Vec::new();
        let mut squash_checked = 0;
        let mut squash_check_truncated = false;
        for branch in eligible {
            let state = if merged.contains(&branch.name) {
                MergeState::Merged
            } else {
                let squashed = if squash_checked < SQUASH_CHECK_LIMIT {
                    squash_checked += 1;
                    is_squash_merged(&git, &base_oid, &branch.tip)
                } else {
                    squash_check_truncated = true;
                    false
                };
                if squashed {
                    MergeState::Squashed
                } else if branch.upstream_gone {
                    MergeState::Unmerged
                } else {
                    continue;
                }
            };
            let unmerged_commits = match state {
                MergeState::Merged => 0,
                _ => count_unmerged(&git, &base_oid, &branch.tip),
            };
            candidates.push(CleanupCandidate {
                name: branch.name.clone(),
                state,
                tip: branch.tip.clone(),
                short_tip: branch.short_tip.clone(),
                subject: branch.subject.clone(),
                committed_at: branch.committed_at,
                upstream: branch.upstream.clone(),
                upstream_gone: branch.upstream_gone,
                unmerged_commits,
            });
        }
        // Estable: dentro de cada estado sigue el orden por fecha.
        candidates.sort_by_key(|candidate| match candidate.state {
            MergeState::Merged => 0,
            MergeState::Squashed => 1,
            MergeState::Unmerged => 2,
        });

        Ok(MergedBranchesReport {
            base,
            default_base,
            base_options,
            current_branch,
            candidates,
            squash_check_truncated,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    fn names(report: &MergedBranchesReport, state: MergeState) -> Vec<&str> {
        report
            .candidates
            .iter()
            .filter(|candidate| candidate.state == state)
            .map(|candidate| candidate.name.as_str())
            .collect()
    }

    fn candidate<'a>(report: &'a MergedBranchesReport, name: &str) -> &'a CleanupCandidate {
        report
            .candidates
            .iter()
            .find(|candidate| candidate.name == name)
            .unwrap_or_else(|| panic!("{name} is not a candidate: {report:#?}"))
    }

    /// `main` con ramas en cada situación que la limpieza distingue.
    fn repository() -> TestRepo {
        let repo = TestRepo::new();
        repo.commit_file("README.md", "base\n", "Base");

        // Integrada con un merge commit.
        repo.git(&["checkout", "-q", "-b", "feature/merged"]);
        repo.commit_file("merged.txt", "merged\n", "Merged work");
        repo.git(&["checkout", "-q", "main"]);
        repo.git(&[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "Merge feature/merged",
            "feature/merged",
        ]);

        // Integrada por fast-forward.
        repo.git(&["checkout", "-q", "-b", "rápido"]);
        repo.commit_file("ff.txt", "ff\n", "Fast-forward work");
        repo.git(&["checkout", "-q", "main"]);
        repo.git(&["merge", "-q", "--ff-only", "rápido"]);

        // Sin integrar.
        repo.git(&["checkout", "-q", "-b", "wip"]);
        repo.commit_file("wip.txt", "wip\n", "Work in progress");

        // Integrada con squash: sus dos commits se aplastan en uno en main.
        repo.git(&["checkout", "-q", "main"]);
        repo.git(&["checkout", "-q", "-b", "squashed"]);
        repo.commit_file("squash.txt", "one\n", "Squash 1");
        repo.commit_file("squash.txt", "one\ntwo\n", "Squash 2");
        repo.git(&["checkout", "-q", "main"]);
        repo.git(&["merge", "-q", "--squash", "squashed"]);
        repo.git(&["commit", "-q", "-m", "Squashed work"]);

        // Protegidas aunque estén integradas.
        repo.git(&["branch", "develop"]);
        repo.git(&["branch", "release/1.0"]);
        // La actual tampoco se propone.
        repo.git(&["checkout", "-q", "-b", "current"]);
        repo
    }

    #[test]
    fn classifies_merged_squashed_and_skips_the_rest() {
        let repo = repository();
        let report = get_merged_branches(&repo.path, None).unwrap();

        assert_eq!(report.base, "main");
        assert_eq!(report.default_base.as_deref(), Some("main"));
        assert_eq!(report.current_branch.as_deref(), Some("current"));
        let mut merged = names(&report, MergeState::Merged);
        merged.sort_unstable();
        assert_eq!(merged, ["feature/merged", "rápido"]);
        assert_eq!(names(&report, MergeState::Squashed), ["squashed"]);
        // `wip` no está integrada y su upstream no falta: no se propone.
        assert!(names(&report, MergeState::Unmerged).is_empty());
        for skipped in ["main", "develop", "release/1.0", "current", "wip"] {
            assert!(
                report
                    .candidates
                    .iter()
                    .all(|candidate| candidate.name != skipped),
                "{skipped}"
            );
        }

        let merged = candidate(&report, "feature/merged");
        assert_eq!(merged.subject, "Merged work");
        assert_eq!(merged.unmerged_commits, 0);
        assert!(merged.committed_at > 0);
        assert!(merged.tip.starts_with(&merged.short_tip));
        assert!(merged.selected_by_default());

        let squashed = candidate(&report, "squashed");
        assert_eq!(squashed.unmerged_commits, 2);
        assert!(!squashed.selected_by_default());
        assert!(squashed.needs_force());
        assert!(report.base_options.contains(&"wip".to_string()));
    }

    #[test]
    fn lists_unmerged_branches_whose_upstream_is_gone() {
        let repo = repository();
        repo.add_bare_origin();
        repo.git(&["push", "-q", "origin", "main", "wip", "feature/merged"]);
        repo.git(&["branch", "--set-upstream-to=origin/wip", "wip"]);
        repo.git(&[
            "branch",
            "--set-upstream-to=origin/feature/merged",
            "feature/merged",
        ]);
        // Se borran en el remoto (el PR se cerró) y el fetch con prune lo ve.
        repo.git(&["push", "-q", "origin", "--delete", "wip", "feature/merged"]);
        repo.git(&["fetch", "-q", "--prune", "origin"]);

        let report = get_merged_branches(&repo.path, None).unwrap();
        let wip = candidate(&report, "wip");
        assert_eq!(wip.state, MergeState::Unmerged);
        assert!(wip.upstream_gone);
        assert_eq!(wip.upstream.as_deref(), Some("origin/wip"));
        assert_eq!(wip.unmerged_commits, 1);
        assert!(wip.needs_force());
        assert!(!wip.selected_by_default());
        let merged = candidate(&report, "feature/merged");
        assert_eq!(merged.state, MergeState::Merged);
        assert!(merged.upstream_gone);
    }

    #[test]
    fn follows_the_remote_default_branch_and_accepts_another_base() {
        let repo = repository();
        repo.add_bare_origin();
        repo.git(&["push", "-q", "origin", "main"]);
        repo.git(&["remote", "set-head", "origin", "main"]);
        // La base por defecto es la local de `origin/HEAD`.
        let report = get_merged_branches(&repo.path, None).unwrap();
        assert_eq!(report.default_base.as_deref(), Some("main"));

        // Contra `wip`, lo integrado en `wip` (la base de `main` de entonces)
        // y no lo de después.
        let report = get_merged_branches(&repo.path, Some("wip")).unwrap();
        assert_eq!(report.base, "wip");
        assert!(
            report
                .candidates
                .iter()
                .all(|candidate| candidate.name != "wip")
        );
        let mut merged = names(&report, MergeState::Merged);
        merged.sort_unstable();
        assert_eq!(merged, ["feature/merged", "rápido"]);

        // Una base remota también vale.
        let report = get_merged_branches(&repo.path, Some("origin/main")).unwrap();
        assert_eq!(report.base, "origin/main");
        assert!(report.base_options.contains(&"origin/main".to_string()));
    }

    #[test]
    fn rejects_unknown_or_option_like_bases() {
        let repo = repository();
        let error = get_merged_branches(&repo.path, Some("nope")).unwrap_err();
        assert_eq!(
            error.message(),
            "Failed to find merged branches: Branch 'nope' does not exist."
        );
        assert!(get_merged_branches(&repo.path, Some("--output=/tmp/x")).is_err());
    }

    #[test]
    fn falls_back_to_master_and_fails_without_a_main_branch() {
        let repo = TestRepo::new();
        repo.commit_file("a.txt", "a\n", "A");
        repo.git(&["branch", "-m", "master"]);
        repo.git(&["branch", "topic"]);
        let report = get_merged_branches(&repo.path, None).unwrap();
        assert_eq!(report.base, "master");
        assert_eq!(names(&report, MergeState::Merged), ["topic"]);

        repo.git(&["branch", "-m", "trunk-ish"]);
        let error = get_merged_branches(&repo.path, None).unwrap_err();
        assert!(error.message().contains("No main branch found"), "{error}");
    }

    #[test]
    fn protected_names() {
        for name in ["main", "master", "develop", "release/2.0", "releases/x"] {
            assert!(is_protected_branch(name), "{name}");
        }
        for name in ["feature/main", "mainline", "release-notes", "fix"] {
            assert!(!is_protected_branch(name), "{name}");
        }
    }
}
