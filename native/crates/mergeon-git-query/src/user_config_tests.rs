//! Las lecturas no dependen de la configuración de salida del usuario.
//!
//! Con `color.ui=always` git pinta con ANSI aunque escriba en una tubería, un
//! `diff.external` cambia el parche por la salida de otro programa y
//! `core.quotePath`/`diff.noprefix` cambian las rutas de las cabeceras. Este
//! repo lleva todo eso en su config local y las lecturas deben parsear igual
//! que en un repo limpio (`mergeon_gitcmd::Git` impone su propia config).

use crate::testutil::TestRepo;
use crate::*;

fn hostile_repo() -> TestRepo {
    let repo = TestRepo::new();
    repo.commit_file("año.txt", "uno\n", "first");
    repo.git(&["checkout", "-b", "feature"]);
    repo.commit_file("año.txt", "uno\ndos\n", "second");
    repo.git(&["tag", "-a", "v1", "-m", "one"]);
    repo.git(&["checkout", "main"]);
    repo.write("año.txt", "uno\ncambio\n");

    for (key, value) in [
        ("color.ui", "always"),
        ("color.diff", "always"),
        ("color.branch", "always"),
        ("color.status", "always"),
        ("diff.external", "echo EXTERNAL"),
        ("core.quotePath", "true"),
        ("diff.noprefix", "true"),
        ("diff.mnemonicPrefix", "true"),
    ] {
        repo.git(&["config", key, value]);
    }
    repo
}

fn assert_clean(text: &str) {
    assert!(!text.contains('\u{1b}'), "ANSI escape in {text:?}");
    assert!(!text.contains("EXTERNAL"), "external diff ran: {text:?}");
}

#[test]
fn diffs_ignore_color_external_diff_quoting_and_prefixes() {
    let repo = hostile_repo();
    let options = DiffOptions::default();

    let working = get_diff(&repo.path, "año.txt", false, false, &options).unwrap();
    assert_clean(&working.diff);
    assert!(
        working.diff.starts_with("diff --git a/año.txt b/año.txt\n"),
        "{}",
        working.diff
    );
    assert!(working.diff.contains("\n+cambio"), "{}", working.diff);

    let feature = repo.git(&["rev-parse", "feature"]).trim().to_string();
    let commit = get_commit_diff(&repo.path, &feature, "año.txt", &options).unwrap();
    assert_clean(&commit.diff);
    assert!(commit.diff.starts_with("diff --git a/año.txt b/año.txt\n"));
    assert!(commit.diff.contains("\n+dos"), "{}", commit.diff);
}

#[test]
fn log_status_branches_tags_and_comparisons_parse_under_hostile_config() {
    let repo = hostile_repo();

    let commits = get_commits(&repo.path, &CommitLogOptions::default()).unwrap();
    let messages: Vec<_> = commits.iter().map(|c| c.message.as_str()).collect();
    assert_eq!(messages, ["second", "first"]);
    for commit in &commits {
        assert_clean(&commit.refs);
        assert_eq!(commit.full_hash.len(), 40, "{:?}", commit.full_hash);
    }

    let status = get_status(&repo.path).unwrap();
    assert_eq!(status.current.as_deref(), Some("main"));
    assert_eq!(status.modified, ["año.txt"]);

    let branches = get_branches(&repo.path).unwrap();
    assert_eq!(branches.current, "main");
    let mut names: Vec<_> = branches.branches.iter().map(|b| b.name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["feature", "main"]);
    assert_eq!(branches.branch("feature").unwrap().label, "second");

    assert_eq!(get_tags(&repo.path).unwrap(), ["v1"]);

    let comparison = compare_branches(&repo.path, "main", "feature").unwrap();
    assert_eq!(comparison.ahead, 1);
    assert_eq!(comparison.ahead_commits[0].message, "second");
    assert_eq!(comparison.files_changed, 1);
    assert_eq!(comparison.insertions, 1);
}
