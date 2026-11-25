//! Vista de ramas (`electron/services/branch-status.js`) y el `git.branch()`
//! de simple-git del que depende.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use crate::error::{QueryResult, read};
use crate::exec;
use crate::text::{js_trim, sort_like_object_keys};

const FIELD_SEPARATOR: char = '\u{1f}';

/// Resultado de `parseUpstreamTrack`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UpstreamTrack {
    pub ahead: u32,
    pub behind: u32,
    /// El upstream configurado ya no existe.
    pub gone: bool,
}

/// Upstream de una rama local según `for-each-ref`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchTracking {
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub gone: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    pub current: bool,
    /// La rama está extraída en otro worktree.
    pub linked_work_tree: bool,
    pub name: String,
    /// Hash abreviado de la punta.
    pub commit: String,
    /// Asunto del commit de la punta, como lo imprime `git branch -v`.
    pub label: String,
    pub tracking: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

/// Forma de `git:getBranches`: `{ current, all, branches }`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchesOverview {
    /// Rama actual; vacío si el repo aún no tiene ramas.
    pub current: String,
    /// Todas las ramas, incluidas las remotas como `remotes/<remoto>/<rama>`.
    pub all: Vec<String>,
    /// Solo las ramas locales. Se serializa como objeto `nombre -> rama`.
    #[serde(with = "branch_map")]
    pub branches: Vec<Branch>,
}

impl BranchesOverview {
    pub fn branch(&self, name: &str) -> Option<&Branch> {
        self.branches.iter().find(|branch| branch.name == name)
    }
}

mod branch_map {
    use super::Branch;
    use serde::de::{MapAccess, Visitor};
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(branches: &[Branch], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(branches.iter().map(|branch| (&branch.name, branch)))
    }

    struct BranchMapVisitor;

    impl<'de> Visitor<'de> for BranchMapVisitor {
        type Value = Vec<Branch>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a map of branch name to branch")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut branches = Vec::new();
            while let Some((_, branch)) = map.next_entry::<String, Branch>()? {
                branches.push(branch);
            }
            Ok(branches)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<Branch>, D::Error> {
        deserializer.deserialize_map(BranchMapVisitor)
    }
}

static AHEAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"ahead ([0-9]+)").unwrap());
static BEHIND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"behind ([0-9]+)").unwrap());

fn captured_number(pattern: &Regex, text: &str) -> u32 {
    pattern
        .captures(text)
        .and_then(|captures| captures[1].parse().ok())
        .unwrap_or(0)
}

/*
 * Parsea el campo %(upstream:track) de for-each-ref:
 * '' | '[ahead 2]' | '[behind 3]' | '[ahead 2, behind 1]' | '[gone]'
 * LC_ALL=C (fijado por `Git`) garantiza el formato en inglés.
 */
pub fn parse_upstream_track(track: &str) -> UpstreamTrack {
    let normalized = js_trim(track);

    if normalized.is_empty() {
        return UpstreamTrack::default();
    }

    if normalized.to_ascii_lowercase().contains("[gone]") {
        return UpstreamTrack {
            ahead: 0,
            behind: 0,
            gone: true,
        };
    }

    UpstreamTrack {
        ahead: captured_number(&AHEAD, normalized),
        behind: captured_number(&BEHIND, normalized),
        gone: false,
    }
}

/*
 * Parsea la salida cruda de:
 *   for-each-ref refs/heads --format='%(refname:short)\x1f%(upstream:short)\x1f%(upstream:track)'
 * en un mapa rama -> { upstream, ahead, behind, gone }.
 */
pub fn parse_branch_tracking_refs(raw: &str) -> HashMap<String, BranchTracking> {
    let mut tracking = HashMap::new();

    for line in raw.split('\n') {
        if js_trim(line).is_empty() {
            continue;
        }

        let mut fields = line.split(FIELD_SEPARATOR);
        let name = js_trim(fields.next().unwrap_or_default());
        let upstream = js_trim(fields.next().unwrap_or_default());
        let track = parse_upstream_track(fields.next().unwrap_or_default());
        if name.is_empty() {
            continue;
        }

        tracking.insert(
            name.to_string(),
            BranchTracking {
                upstream: (!upstream.is_empty()).then(|| upstream.to_string()),
                ahead: track.ahead,
                behind: track.behind,
                gone: track.gone,
            },
        );
    }

    tracking
}

/// `BranchSummaryResult` de simple-git.
#[derive(Debug, Default, PartialEq, Eq)]
struct BranchSummary {
    all: Vec<String>,
    branches: Vec<SummaryBranch>,
    current: String,
}

#[derive(Debug, PartialEq, Eq)]
struct SummaryBranch {
    current: bool,
    linked_work_tree: bool,
    name: String,
    commit: String,
    label: String,
}

static DETACHED_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([*+]\s)?\((?:HEAD )?detached (?:from|at) (\S+)\)\s+([a-z0-9]+)\s(.*)$").unwrap()
});
/// `* (no branch, rebasing main)` / `(no branch, bisect started on main)`: HEAD
/// desanclado por una operación en curso. No es una rama.
static NO_BRANCH_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([*+]\s)?\(no branch[,)]").unwrap());
static BRANCH_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)^([*+]\s)?(\S+)\s+([a-z0-9]+)\s?(.*)$").unwrap());

/// `parseBranchSummary`: la salida de `git branch -v -a`, con las mismas dos
/// expresiones que simple-git (y por tanto las mismas líneas ignoradas, como
/// `remotes/origin/HEAD -> origin/main`).
fn parse_branch_summary(stdout: &str) -> BranchSummary {
    let mut summary = BranchSummary::default();

    for line in stdout.split('\n').map(js_trim) {
        // simple-git la leía con la expresión general y salía una rama fantasma
        // llamada "(no" (y como rama actual) durante un rebase o un bisect.
        if line.is_empty() || NO_BRANCH_LINE.is_match(line) {
            continue;
        }
        let Some(captures) = DETACHED_LINE
            .captures(line)
            .or_else(|| BRANCH_LINE.captures(line))
        else {
            continue;
        };

        let status = captures
            .get(1)
            .and_then(|marker| marker.as_str().chars().next());
        let branch = SummaryBranch {
            current: status == Some('*'),
            linked_work_tree: status == Some('+'),
            name: captures[2].to_string(),
            commit: captures[3].to_string(),
            label: captures[4].to_string(),
        };

        if branch.current {
            summary.current = branch.name.clone();
        }
        summary.all.push(branch.name.clone());
        // `branches` es un objeto en simple-git: un nombre repetido pisa al anterior.
        match summary
            .branches
            .iter_mut()
            .find(|existing| existing.name == branch.name)
        {
            Some(existing) => *existing = branch,
            None => summary.branches.push(branch),
        }
    }

    sort_like_object_keys(&mut summary.branches, |branch| &branch.name);
    summary
}

/*
 * Vista de ramas con ahead/behind en exactamente 2 procesos git:
 * `git branch` (aporta current/all con remotes/... y metadata por rama) +
 * un único for-each-ref con %(upstream:track) — sin el N+1 de rev-list por rama.
 * Preserva el shape de wire { current, all, branches } del handler original.
 */
/// Canal `git:getBranches`.
pub fn get_branches(repo_path: &Path) -> QueryResult<BranchesOverview> {
    read("Failed to get branches", || {
        let git = exec::open(repo_path)?;
        let mut summary = parse_branch_summary(&exec::raw(&git, &["branch", "-v", "-a"])?);

        /*
         * En un repo sin commits (o en una rama huérfana) `git branch` no lista
         * la rama actual, y con `current` vacío la interfaz caía en "Detached ·
         * HEAD". La rama existe aunque aún no apunte a nada: se lee de HEAD.
         * Con HEAD desanclado (rebase en curso) `symbolic-ref` falla y `current`
         * sigue vacío.
         */
        if summary.current.is_empty()
            && let Ok(output) = exec::exec(&git, &["symbolic-ref", "--quiet", "--short", "HEAD"])
            && output.success()
        {
            summary.current = js_trim(&output.stdout_text()).to_string();
        }

        // Sin metadata de upstream: se devuelven las ramas con ahead/behind en 0.
        let tracking = exec::raw(
            &git,
            &[
                "for-each-ref".to_string(),
                "refs/heads".to_string(),
                format!(
                    "--format=%(refname:short){FIELD_SEPARATOR}%(upstream:short){FIELD_SEPARATOR}%(upstream:track)"
                ),
            ],
        )
        .map(|raw| parse_branch_tracking_refs(&raw))
        .unwrap_or_default();

        let branches = summary
            .branches
            .into_iter()
            .filter(|branch| !branch.name.starts_with("remotes/"))
            .map(|branch| {
                let ref_info = tracking.get(&branch.name);
                // `[gone]` (upstream borrado) queda en 0/0, igual que el fallback previo.
                let counted = ref_info.filter(|info| !info.gone);
                Branch {
                    current: branch.current,
                    linked_work_tree: branch.linked_work_tree,
                    tracking: ref_info.and_then(|info| info.upstream.clone()),
                    ahead: counted.map_or(0, |info| info.ahead),
                    behind: counted.map_or(0, |info| info.behind),
                    name: branch.name,
                    commit: branch.commit,
                    label: branch.label,
                }
            })
            .collect();

        Ok(BranchesOverview {
            current: summary.current,
            all: summary.all,
            branches,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;
    use mergeon_gitcmd::Git;

    const FS: char = '\u{1f}';

    #[test]
    fn parses_every_track_shape() {
        let track = |ahead, behind, gone| UpstreamTrack {
            ahead,
            behind,
            gone,
        };
        assert_eq!(parse_upstream_track(""), track(0, 0, false));
        assert_eq!(parse_upstream_track("[ahead 2]"), track(2, 0, false));
        assert_eq!(parse_upstream_track("[behind 3]"), track(0, 3, false));
        assert_eq!(
            parse_upstream_track("[ahead 2, behind 1]"),
            track(2, 1, false)
        );
        assert_eq!(parse_upstream_track("[gone]"), track(0, 0, true));
        assert_eq!(parse_upstream_track("  \n"), track(0, 0, false));
    }

    #[test]
    fn parses_for_each_ref_output_into_a_per_branch_map() {
        let raw = [
            format!("main{FS}origin/main{FS}[ahead 1, behind 2]"),
            format!("feature/x{FS}{FS}"),
            format!("stale{FS}origin/stale{FS}[gone]"),
            String::new(),
        ]
        .join("\n");

        let tracking = parse_branch_tracking_refs(&raw);

        assert_eq!(
            tracking["main"],
            BranchTracking {
                upstream: Some("origin/main".into()),
                ahead: 1,
                behind: 2,
                gone: false
            }
        );
        assert_eq!(tracking["feature/x"], BranchTracking::default());
        assert_eq!(
            tracking["stale"],
            BranchTracking {
                upstream: Some("origin/stale".into()),
                ahead: 0,
                behind: 0,
                gone: true
            }
        );
        assert_eq!(tracking.len(), 3);
    }

    #[test]
    fn parses_git_branch_output_like_simple_git() {
        let stdout = [
            "  feature/login        e2a59c1 Add login",
            "* main                 3282e3f [ahead 1] Fix: thing",
            "+ linked               1111111 (/tmp/wt) In another worktree",
            "  remotes/origin/HEAD  -> origin/main",
            "  remotes/origin/main  3282e3f Fix: thing",
            "",
        ]
        .join("\n");

        let summary = parse_branch_summary(&stdout);

        assert_eq!(summary.current, "main");
        assert_eq!(
            summary.all,
            ["feature/login", "main", "linked", "remotes/origin/main"]
        );
        let main = &summary.branches[1];
        assert!(main.current && !main.linked_work_tree);
        assert_eq!(main.commit, "3282e3f");
        assert_eq!(main.label, "[ahead 1] Fix: thing");
        let linked = &summary.branches[2];
        assert!(linked.linked_work_tree && !linked.current);
        assert_eq!(summary.branches[0].label, "Add login");
    }

    #[test]
    fn a_detached_head_is_listed_under_the_commit_it_points_at() {
        let summary = parse_branch_summary(
            "* (HEAD detached at 3282e3f) 3282e3f Fix: thing\n  main                       3282e3f Fix: thing\n",
        );

        assert_eq!(summary.current, "3282e3f");
        assert_eq!(summary.all, ["3282e3f", "main"]);
        assert_eq!(summary.branches[0].label, "Fix: thing");
    }

    #[test]
    fn reports_ahead_count_for_a_local_commit_not_yet_pushed() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "base\n", "base");
        repo.add_bare_origin();
        repo.git(&["push", "-u", "origin", "main"]);

        repo.commit_file("file.txt", "ahead\n", "ahead commit");

        let overview = get_branches(&repo.path).unwrap();
        let main = overview.branch("main").unwrap();

        assert_eq!(overview.current, "main");
        assert!(overview.all.contains(&"remotes/origin/main".to_string()));
        assert_eq!(main.tracking.as_deref(), Some("origin/main"));
        assert_eq!(main.ahead, 1);
        assert_eq!(main.behind, 0);
        assert!(main.current);
        assert_eq!(main.label, "[ahead 1] ahead commit");
        // Las remotas van en `all`, no en `branches`.
        assert_eq!(overview.branches.len(), 1);
    }

    #[test]
    fn reports_zero_when_the_upstream_is_gone() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "base\n", "base");
        let remote = repo.add_bare_origin();
        crate::testutil::git_in(&remote, &["config", "receive.denyDeleteCurrent", "ignore"]);
        repo.git(&["push", "-u", "origin", "main"]);

        // Borrar la rama remota y su ref local deja el upstream en estado [gone].
        repo.git(&["push", "origin", "--delete", "main"]);
        let _ = Git::new(&repo.path).exec(["update-ref", "-d", "refs/remotes/origin/main"]);

        let overview = get_branches(&repo.path).unwrap();
        let main = overview.branch("main").unwrap();

        assert_eq!(main.ahead, 0);
        assert_eq!(main.behind, 0);
        assert_eq!(main.tracking.as_deref(), Some("origin/main"));
    }

    #[test]
    fn returns_branches_without_upstream_metadata_as_zero_with_null_tracking() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "base\n", "base");
        repo.git(&["branch", "feature/local"]);

        let overview = get_branches(&repo.path).unwrap();
        let feature = overview.branch("feature/local").unwrap();

        assert_eq!(feature.tracking, None);
        assert_eq!(feature.ahead, 0);
        assert_eq!(feature.behind, 0);
        assert!(!feature.current);
    }

    // La app Electron devolvía `current: ""` y la interfaz mostraba
    // "Detached · HEAD" en un repo recién creado.
    #[test]
    fn an_empty_repository_names_its_unborn_branch() {
        let repo = TestRepo::new();
        assert_eq!(
            get_branches(&repo.path).unwrap(),
            BranchesOverview {
                current: "main".to_string(),
                ..Default::default()
            }
        );

        // Una rama huérfana tampoco aparece en `git branch` hasta su primer commit.
        repo.commit_file("file.txt", "base\n", "base");
        repo.git(&["checkout", "--orphan", "fresh"]);
        let overview = get_branches(&repo.path).unwrap();
        assert_eq!(overview.current, "fresh");
        assert_eq!(overview.all, ["main"]);
        assert!(!overview.branch("main").unwrap().current);
    }

    #[test]
    fn an_operation_in_progress_does_not_add_a_phantom_branch() {
        let summary = parse_branch_summary(
            "* (no branch, rebasing main) 3282e3f Fix: thing\n  main                       1111111 Older\n",
        );
        assert_eq!(summary.current, "");
        assert_eq!(summary.all, ["main"]);
        let summary = parse_branch_summary("* (no branch, bisect started on main) 3282e3f x\n");
        assert!(summary.all.is_empty());
        assert!(summary.branches.is_empty());
    }

    // Durante un rebase la app Electron listaba una rama "(no" y la daba por actual.
    #[test]
    fn a_stopped_rebase_lists_only_real_branches() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "base\n", "base");
        repo.git(&["checkout", "-b", "feature"]);
        repo.commit_file("file.txt", "feature\n", "feature");
        repo.git(&["checkout", "main"]);
        repo.commit_file("file.txt", "main\n", "main");
        repo.git(&["checkout", "feature"]);
        let rebase = Git::new(&repo.path).exec(["rebase", "main"]).unwrap();
        assert!(!rebase.success(), "the rebase must stop on the conflict");
        assert!(
            repo.git(&["branch"])
                .contains("(no branch, rebasing feature)")
        );

        let overview = get_branches(&repo.path).unwrap();

        assert_eq!(overview.current, "");
        assert_eq!(overview.all, ["feature", "main"]);
        assert!(overview.branches.iter().all(|branch| !branch.current));
        assert!(overview.branch("(no").is_none());
    }

    #[test]
    fn serializes_branches_as_an_object_keyed_by_name() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "base\n", "base");
        repo.git(&["branch", "feature/local"]);

        let overview = get_branches(&repo.path).unwrap();
        let json = serde_json::to_value(&overview).unwrap();

        assert_eq!(json["current"], "main");
        assert_eq!(json["branches"]["main"]["current"], true);
        assert_eq!(json["branches"]["main"]["linkedWorkTree"], false);
        assert_eq!(
            json["branches"]["feature/local"]["tracking"],
            serde_json::Value::Null
        );
        assert_eq!(json["branches"]["feature/local"]["ahead"], 0);

        let back: BranchesOverview = serde_json::from_value(json).unwrap();
        assert_eq!(back.all, overview.all);
        assert_eq!(back.branches.len(), overview.branches.len());
    }

    #[test]
    fn fails_with_the_read_prefix_outside_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        let error = get_branches(dir.path()).unwrap_err();
        assert!(
            error
                .message()
                .starts_with("Failed to get branches: fatal: not a git repository"),
            "{error}"
        );
    }
}
