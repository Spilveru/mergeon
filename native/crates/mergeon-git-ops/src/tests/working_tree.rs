//! Preparar, quitar del stage, descartar y confirmar.

use super::support::*;
use crate::{
    ApplyPatchOptions, Code, CommitOptions, MergeOptions, MutationResult, Operation, Outcome,
    apply_patch_operation, commit_operation, discard_all_operation, discard_changes_operation,
    discard_patch_operation, merge_operation, stage_all_operation, stage_operation,
    unstage_all_operation, unstage_operation,
};

fn index_of(result: &MutationResult, path: &str) -> Option<String> {
    result
        .status
        .as_ref()?
        .files
        .iter()
        .find(|file| file.path == path)
        .map(|file| file.index.clone())
}

#[test]
fn stages_and_unstages_a_tracked_file_with_structured_results() {
    let (_root, repo_path) = create_repository();
    std::fs::write(repo_path.join("file.txt"), "changed\n").unwrap();

    let stage_result = stage_operation(&repo_path, "file.txt");
    let unstage_result = unstage_operation(&repo_path, "file.txt");

    assert!(stage_result.success);
    assert_eq!(stage_result.code, Code::Ok);
    assert_eq!(stage_result.message, "Staged file.txt.");
    assert_eq!(stage_result.file_path.as_deref(), Some("file.txt"));
    assert_eq!(stage_result.files_affected, ["file.txt"]);
    assert_eq!(index_of(&stage_result, "file.txt").as_deref(), Some("M"));
    assert!(unstage_result.success);
    assert_eq!(unstage_result.code, Code::Ok);
    assert_eq!(unstage_result.message, "Unstaged file.txt.");
    assert_eq!(
        index_of(&unstage_result, "file.txt").unwrap_or_else(|| " ".to_string()),
        " "
    );
}

#[test]
fn staging_or_unstaging_an_unchanged_file_is_a_noop() {
    let (_root, repo_path) = create_repository();

    let stage_result = stage_operation(&repo_path, "file.txt");
    assert!(!stage_result.success);
    assert_eq!(stage_result.outcome, Outcome::Noop);
    assert_eq!(stage_result.code, Code::NothingToStage);
    assert_eq!(
        stage_result.message,
        "No unstaged changes found for file.txt."
    );
    assert!(stage_result.files_affected.is_empty());

    let unstage_result = unstage_operation(&repo_path, "file.txt");
    assert!(!unstage_result.success);
    assert_eq!(unstage_result.outcome, Outcome::Noop);
    assert_eq!(unstage_result.code, Code::NothingToUnstage);

    assert_eq!(stage_all_operation(&repo_path).code, Code::NothingToStage);
    assert_eq!(
        unstage_all_operation(&repo_path).code,
        Code::NothingToUnstage
    );
    assert_eq!(
        discard_changes_operation(&repo_path, "file.txt").code,
        Code::NothingToDiscard
    );
    assert_eq!(
        discard_all_operation(&repo_path, None).code,
        Code::NothingToDiscard
    );
}

#[test]
fn stages_untracked_files_with_awkward_names() {
    let (_root, repo_path) = create_repository();
    std::fs::write(repo_path.join("new file with spaces.txt"), "new\n").unwrap();

    let result = stage_operation(&repo_path, "new file with spaces.txt");

    assert!(result.success, "{}", result.message);
    assert_eq!(
        index_of(&result, "new file with spaces.txt").as_deref(),
        Some("A")
    );
}

#[test]
fn stages_and_unstages_all_tracked_changes_with_structured_results() {
    let (_root, repo_path) = create_repository();
    std::fs::write(repo_path.join("file.txt"), "changed\n").unwrap();
    std::fs::write(repo_path.join("extra.txt"), "new\n").unwrap();

    let stage_result = stage_all_operation(&repo_path);
    let unstage_result = unstage_all_operation(&repo_path);

    assert!(stage_result.success);
    assert_eq!(stage_result.operation, Operation::StageAll);
    assert_eq!(stage_result.message, "Staged 2 files.");
    assert!(
        stage_result
            .files_affected
            .contains(&"file.txt".to_string())
    );
    assert!(
        stage_result
            .files_affected
            .contains(&"extra.txt".to_string())
    );
    assert!(unstage_result.success);
    assert_eq!(unstage_result.message, "Unstaged 2 files.");
    assert!(
        unstage_result
            .files_affected
            .contains(&"file.txt".to_string())
    );
}

const HUNK_PATCH: &str = "diff --git a/file.txt b/file.txt\n--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-base\n+changed\n";

#[test]
fn stages_and_unstages_a_single_block_through_a_patch() {
    let (_root, repo_path) = create_repository();
    std::fs::write(repo_path.join("file.txt"), "changed\n").unwrap();
    let options = ApplyPatchOptions {
        reverse: false,
        file_path: Some("file.txt".to_string()),
        worktree: false,
    };

    let staged = apply_patch_operation(&repo_path, HUNK_PATCH, &options);
    assert!(staged.success, "{}", staged.message);
    assert_eq!(staged.operation, Operation::StageHunk);
    assert_eq!(staged.message, "Staged the selected changes.");
    assert_eq!(staged.files_affected, ["file.txt"]);
    assert_eq!(index_of(&staged, "file.txt").as_deref(), Some("M"));
    // `--cached`: el working tree no se toca.
    assert_eq!(read_file(&repo_path.join("file.txt")), "changed\n");

    let unstaged = apply_patch_operation(
        &repo_path,
        HUNK_PATCH,
        &ApplyPatchOptions {
            reverse: true,
            ..options
        },
    );
    assert!(unstaged.success, "{}", unstaged.message);
    assert_eq!(unstaged.operation, Operation::UnstageHunk);
    assert_eq!(unstaged.message, "Unstaged the selected changes.");
    assert_eq!(index_of(&unstaged, "file.txt").as_deref(), Some(" "));
}

#[test]
fn rejects_empty_and_stale_patches() {
    let (_root, repo_path) = create_repository();

    let empty = apply_patch_operation(&repo_path, "  \n", &ApplyPatchOptions::default());
    assert!(!empty.success);
    assert_eq!(empty.outcome, Outcome::Blocked);
    assert_eq!(empty.code, Code::EmptyPatch);
    assert_eq!(empty.message, "There is nothing to apply.");

    // El parche espera "base" → "changed", pero ya no aplica al revés. Lo
    // detecta `git apply --check` y el aviso dice qué hacer.
    let stale = apply_patch_operation(
        &repo_path,
        HUNK_PATCH,
        &ApplyPatchOptions {
            reverse: true,
            file_path: None,
            worktree: false,
        },
    );
    assert!(!stale.success);
    assert_eq!(stale.operation, Operation::UnstageHunk);
    assert_eq!(stale.outcome, Outcome::Failed);
    assert_eq!(stale.code, Code::PatchDoesNotApply);
    assert!(
        stale
            .message
            .starts_with("The selected changes no longer apply to the file"),
        "{}",
        stale.message
    );
    assert!(
        stale.message.contains("Reload the diff"),
        "{}",
        stale.message
    );
    assert!(stale.message.contains("patch"), "{}", stale.message);
    assert!(stale.files_affected.is_empty());
}

// ---------------------------------------------------------------------------
// Descartar un bloque o unas líneas del working tree
// ---------------------------------------------------------------------------

fn numbered(count: usize) -> Vec<String> {
    (1..=count).map(|number| format!("line {number}")).collect()
}

fn joined(lines: &[String], ending: &str) -> String {
    let mut text = lines.join(ending);
    text.push_str(ending);
    text
}

/// Lo que sale de `git diff` con la cabecera del archivo y solo el bloque
/// `index` (lo mismo que `mergeon_model::patch::extract_hunk_patch`).
fn hunk_of(diff: &str, index: usize) -> String {
    let mut header = Vec::new();
    let mut hunks: Vec<Vec<&str>> = Vec::new();
    // `split('\n')` y no `lines()`: este se come el `\r` de cada línea.
    let lines = diff.strip_suffix('\n').unwrap_or(diff);
    for line in lines.split('\n') {
        if line.starts_with("@@ ") {
            hunks.push(vec![line]);
        } else if let Some(hunk) = hunks.last_mut() {
            hunk.push(line);
        } else {
            header.push(line);
        }
    }
    header.extend(&hunks[index]);
    format!("{}\n", header.join("\n"))
}

fn hunk_count(diff: &str) -> usize {
    diff.lines().filter(|line| line.starts_with("@@ ")).count()
}

/// Archivo de 40 líneas con tres cambios separados: tres bloques.
fn three_hunks(repo_path: &std::path::Path, ending: &str) -> (Vec<String>, Vec<String>) {
    let base = numbered(40);
    commit_file_at(repo_path, &joined(&base, ending), "forty", "file.txt");
    let mut edited = base.clone();
    edited[1] = "line two".to_string();
    edited[19] = "line twenty".to_string();
    edited[37] = "line thirty-eight".to_string();
    std::fs::write(repo_path.join("file.txt"), joined(&edited, ending)).unwrap();
    (base, edited)
}

#[test]
fn discarding_one_hunk_keeps_the_others() {
    let (_root, repo_path) = create_repository();
    let (base, edited) = three_hunks(&repo_path, "\n");
    let diff = run_git(&repo_path, &["diff", "--", "file.txt"]);
    assert_eq!(hunk_count(&diff), 3);

    let result =
        discard_patch_operation(&repo_path, &hunk_of(&diff, 1), Some("file.txt".to_string()));

    assert!(result.success, "{}", result.message);
    assert_eq!(result.operation, Operation::DiscardHunk);
    assert_eq!(result.code, Code::Ok);
    assert_eq!(result.message, "Discarded the selected changes.");
    assert_eq!(result.files_affected, ["file.txt"]);
    let mut expected = edited.clone();
    expected[19] = base[19].clone();
    assert_eq!(
        read_file(&repo_path.join("file.txt")),
        joined(&expected, "\n")
    );
    // Solo el working tree: el índice sigue en el commit.
    assert_eq!(run_git(&repo_path, &["diff", "--cached"]), "");
    let left = run_git(&repo_path, &["diff", "--", "file.txt"]);
    assert_eq!(hunk_count(&left), 2);
    assert!(left.contains("+line two") && left.contains("+line thirty-eight"));
    assert!(!left.contains("line twenty"));
}

#[test]
fn discarding_the_last_hunk_leaves_the_file_as_staged() {
    let (_root, repo_path) = create_repository();
    let (_, edited) = three_hunks(&repo_path, "\n");
    // Lo preparado manda: descartar devuelve el bloque a como está en el índice.
    run_git(&repo_path, &["add", "file.txt"]);
    let mut worktree = edited.clone();
    worktree[37] = "line 38 again".to_string();
    std::fs::write(repo_path.join("file.txt"), joined(&worktree, "\n")).unwrap();
    let diff = run_git(&repo_path, &["diff", "--", "file.txt"]);
    assert_eq!(hunk_count(&diff), 1);

    let result = discard_patch_operation(&repo_path, &hunk_of(&diff, 0), None);

    assert!(result.success, "{}", result.message);
    assert_eq!(
        read_file(&repo_path.join("file.txt")),
        joined(&edited, "\n")
    );
    assert_eq!(run_git(&repo_path, &["diff", "--", "file.txt"]), "");
}

#[test]
fn discarding_the_hunk_of_a_deleted_file_brings_it_back() {
    let (_root, repo_path) = create_repository();
    std::fs::remove_file(repo_path.join("file.txt")).unwrap();
    let diff = run_git(&repo_path, &["diff", "--", "file.txt"]);
    assert!(diff.contains("deleted file mode"));

    let result = discard_patch_operation(&repo_path, &hunk_of(&diff, 0), None);

    assert!(result.success, "{}", result.message);
    assert_eq!(read_file(&repo_path.join("file.txt")), "base\n");
}

#[test]
fn discarding_the_hunk_of_a_new_file_removes_it() {
    let (_root, repo_path) = create_repository();
    std::fs::write(repo_path.join("new.txt"), "one\ntwo\n").unwrap();
    // `add -N`: git lo sigue sin prepararlo y `git diff` lo enseña entero.
    run_git(&repo_path, &["add", "-N", "new.txt"]);
    let diff = run_git(&repo_path, &["diff", "--", "new.txt"]);
    assert!(diff.contains("new file mode"));

    let result =
        discard_patch_operation(&repo_path, &hunk_of(&diff, 0), Some("new.txt".to_string()));

    assert!(result.success, "{}", result.message);
    assert!(!repo_path.join("new.txt").exists());
}

#[test]
fn discarding_a_hunk_keeps_windows_line_endings() {
    let (_root, repo_path) = create_repository();
    let (base, edited) = three_hunks(&repo_path, "\r\n");
    let diff = run_git(&repo_path, &["diff", "--", "file.txt"]);
    assert_eq!(hunk_count(&diff), 3);
    assert!(diff.contains("+line twenty\r"));

    let result = discard_patch_operation(&repo_path, &hunk_of(&diff, 2), None);

    assert!(result.success, "{}", result.message);
    let mut expected = edited.clone();
    expected[37] = base[37].clone();
    let content = std::fs::read_to_string(repo_path.join("file.txt")).unwrap();
    assert_eq!(content, joined(&expected, "\r\n"));
}

#[test]
fn discards_only_the_chosen_lines_of_a_hunk() {
    let (_root, repo_path) = create_repository();
    commit_file(&repo_path, "uno\ndos\ntres\n", "three");
    std::fs::write(repo_path.join("file.txt"), "uno\nDOS\ntres\ncuatro\n").unwrap();
    // El recorte de `mergeon_model::patch` para descartar solo "+cuatro": la
    // otra alta pasa a contexto y la baja no elegida se omite.
    let patch = "diff --git a/file.txt b/file.txt\n--- a/file.txt\n+++ b/file.txt\n\
@@ -1,3 +1,4 @@\n uno\n DOS\n tres\n+cuatro\n";

    let result = discard_patch_operation(&repo_path, patch, None);

    assert!(result.success, "{}", result.message);
    assert_eq!(read_file(&repo_path.join("file.txt")), "uno\nDOS\ntres\n");
}

#[test]
fn a_stale_discard_touches_nothing() {
    let (_root, repo_path) = create_repository();
    let (_, edited) = three_hunks(&repo_path, "\n");
    let diff = run_git(&repo_path, &["diff", "--", "file.txt"]);
    let patch = hunk_of(&diff, 0);
    // El archivo cambió desde que se leyó el diff.
    let mut changed = edited.clone();
    changed[1] = "line two, edited again".to_string();
    std::fs::write(repo_path.join("file.txt"), joined(&changed, "\n")).unwrap();

    let result = discard_patch_operation(&repo_path, &patch, Some("file.txt".to_string()));

    assert!(!result.success);
    assert_eq!(result.operation, Operation::DiscardHunk);
    assert_eq!(result.code, Code::PatchDoesNotApply);
    assert!(
        result
            .message
            .starts_with("The selected changes no longer apply to file.txt"),
        "{}",
        result.message
    );
    assert_eq!(
        read_file(&repo_path.join("file.txt")),
        joined(&changed, "\n")
    );
}

#[test]
fn discards_tracked_changes_with_structured_status() {
    let (_root, repo_path) = create_repository();
    std::fs::write(repo_path.join("file.txt"), "changed\n").unwrap();

    let result = discard_changes_operation(&repo_path, "file.txt");

    assert!(result.success);
    assert_eq!(result.code, Code::Ok);
    assert_eq!(result.message, "Discarded changes in file.txt.");
    assert_eq!(read_file(&repo_path.join("file.txt")), "base\n");
}

#[test]
fn discards_untracked_files_with_structured_status() {
    let (_root, repo_path) = create_repository();
    std::fs::write(repo_path.join("extra.txt"), "temp\n").unwrap();

    let result = discard_changes_operation(&repo_path, "extra.txt");
    let exists = run_git(&repo_path, &["status", "--porcelain", "--", "extra.txt"]);

    assert!(result.success);
    assert_eq!(result.code, Code::Ok);
    assert_eq!(exists.trim(), "");
    assert!(!repo_path.join("extra.txt").exists());
}

#[test]
fn discards_all_tracked_changes_with_a_single_structured_result() {
    let (_root, repo_path) = create_repository();
    std::fs::write(repo_path.join("file.txt"), "changed\n").unwrap();

    let requested = vec!["file.txt".to_string(), "file.txt".to_string()];
    let result = discard_all_operation(&repo_path, Some(&requested));

    assert!(result.success);
    assert_eq!(result.files_affected, ["file.txt"]);
    assert_eq!(result.message, "Discarded changes in 1 file.");
    assert!(result.status.unwrap().is_clean);
}

#[test]
fn discard_all_defaults_to_every_unstaged_change() {
    let (_root, repo_path) = create_repository();
    commit_file_at(&repo_path, "other\n", "other", "other.txt");
    std::fs::write(repo_path.join("file.txt"), "changed\n").unwrap();
    std::fs::write(repo_path.join("other.txt"), "changed\n").unwrap();
    std::fs::write(repo_path.join("untracked.txt"), "new\n").unwrap();

    let result = discard_all_operation(&repo_path, None);

    assert!(result.success, "{}", result.message);
    assert_eq!(result.files_affected.len(), 3);
    assert_eq!(result.message, "Discarded changes in 3 files.");
    assert!(result.status.unwrap().is_clean);
    assert_eq!(read_file(&repo_path.join("other.txt")), "other\n");
    assert!(!repo_path.join("untracked.txt").exists());
}

#[test]
fn blocks_commit_when_the_message_is_empty() {
    let (_root, repo_path) = create_repository();
    std::fs::write(repo_path.join("file.txt"), "changed\n").unwrap();
    stage_operation(&repo_path, "file.txt");

    let result = commit_operation(&repo_path, "   ", &CommitOptions::default());

    assert!(!result.success);
    assert_eq!(result.code, Code::CommitMessageRequired);
    assert_eq!(result.outcome, Outcome::Blocked);
}

#[test]
fn blocks_commit_when_nothing_is_staged() {
    let (_root, repo_path) = create_repository();

    let clean = commit_operation(&repo_path, "update file", &CommitOptions::default());
    assert!(!clean.success);
    assert_eq!(clean.code, Code::NoChanges);
    assert_eq!(clean.outcome, Outcome::Noop);

    std::fs::write(repo_path.join("file.txt"), "changed\n").unwrap();

    let result = commit_operation(&repo_path, "update file", &CommitOptions::default());

    assert!(!result.success);
    assert_eq!(result.code, Code::NothingToStage);
    assert_eq!(result.outcome, Outcome::Blocked);
    assert_eq!(result.message, "Stage changes before creating a commit.");
}

#[test]
fn commits_staged_changes_and_amends_the_last_commit() {
    let (_root, repo_path) = create_repository();
    std::fs::write(repo_path.join("file.txt"), "changed\n").unwrap();
    stage_operation(&repo_path, "file.txt");

    let result = commit_operation(
        &repo_path,
        "  update file\n\nwith a body  ",
        &CommitOptions::default(),
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(result.operation, Operation::Commit);
    assert_eq!(result.code, Code::Ok);
    let created = head(&repo_path);
    assert_eq!(result.message, format!("Created commit {}.", &created[..7]));
    assert!(result.status.unwrap().is_clean);
    assert_eq!(
        run_git(&repo_path, &["log", "-1", "--pretty=%B"]).trim(),
        "update file\n\nwith a body"
    );

    // Enmendar solo el mensaje no exige nada preparado.
    let amended = commit_operation(
        &repo_path,
        "better message",
        &CommitOptions {
            amend: true,
            ..Default::default()
        },
    );

    assert!(amended.success, "{}", amended.message);
    let amended_head = head(&repo_path);
    assert_ne!(amended_head, created);
    assert_eq!(
        amended.message,
        format!("Created commit {}.", &amended_head[..7])
    );
    assert_eq!(log_subjects(&repo_path), ["better message", "base"]);
}

#[test]
fn signs_the_commit_with_the_configured_ssh_key() {
    let (root, repo_path) = create_repository();
    let key = root.path().join("signing-key");
    // Sin `ssh-keygen` no hay con qué firmar en SSH (lo trae cualquier git
    // de escritorio; en CI está).
    let Ok(keygen) = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "test", "-f"])
        .arg(&key)
        .status()
    else {
        eprintln!("ssh-keygen no está disponible: se omite el test de firma SSH");
        return;
    };
    assert!(keygen.success());
    run_git(&repo_path, &["config", "gpg.format", "ssh"]);
    run_git(
        &repo_path,
        &[
            "config",
            "user.signingkey",
            path_str(&key.with_extension("pub")),
        ],
    );
    std::fs::write(repo_path.join("file.txt"), "changed\n").unwrap();
    stage_operation(&repo_path, "file.txt");

    let unsigned = commit_operation(&repo_path, "unsigned", &CommitOptions::default());
    assert!(unsigned.success, "{}", unsigned.message);
    assert!(!run_git(&repo_path, &["cat-file", "commit", "HEAD"]).contains("gpgsig"));

    let signed = commit_operation(
        &repo_path,
        "signed",
        &CommitOptions {
            amend: true,
            sign: true,
            ..Default::default()
        },
    );

    assert!(signed.success, "{}", signed.message);
    assert_eq!(signed.code, Code::Ok);
    let object = run_git(&repo_path, &["cat-file", "commit", "HEAD"]);
    assert!(
        object.contains("gpgsig -----BEGIN SSH SIGNATURE-----"),
        "{object}"
    );
    assert_eq!(log_subjects(&repo_path), ["signed", "base"]);
}

#[test]
fn a_commit_that_cannot_be_signed_explains_what_is_missing() {
    let (root, repo_path) = create_repository();
    // Un programa de firma que no existe: falla igual con cualquier
    // configuración global de quien corra el test.
    let missing = root.path().join("no-such-gpg");
    run_git(&repo_path, &["config", "gpg.format", "openpgp"]);
    run_git(&repo_path, &["config", "gpg.program", path_str(&missing)]);
    std::fs::write(repo_path.join("file.txt"), "changed\n").unwrap();
    stage_operation(&repo_path, "file.txt");
    let before = head(&repo_path);

    let result = commit_operation(
        &repo_path,
        "signed",
        &CommitOptions {
            sign: true,
            ..Default::default()
        },
    );

    assert!(!result.success);
    assert_eq!(result.code, Code::SigningFailed);
    assert_eq!(result.outcome, Outcome::Failed);
    // Primero cómo arreglarlo (cabe entero en un aviso), luego lo que dijo git.
    assert!(
        result.message.starts_with(
            "Could not sign the commit: set a signing key (user.signingkey; gpg.format ssh for SSH keys) or turn off Sign. Git said: "
        ),
        "{}",
        result.message
    );
    assert!(
        result.message.contains("failed to sign"),
        "{}",
        result.message
    );
    assert!(!result.message.contains("failed to write commit object"));
    // Nada cambió: el commit no se creó y lo preparado sigue preparado.
    assert_eq!(head(&repo_path), before);
    assert_eq!(index_of(&result, "file.txt").as_deref(), Some("M"));
}

#[test]
fn blocks_commit_while_another_operation_is_in_progress() {
    let fixture = create_diverged_repository();
    merge_operation(&fixture.repo_path, "feature", &MergeOptions::default());

    let result = commit_operation(&fixture.repo_path, "sneaky", &CommitOptions::default());

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Blocked);
    assert_eq!(result.code, Code::CommitBlockedByOperation);
    assert_eq!(
        result.message,
        "Cannot create a commit while merge is in progress."
    );
}

#[test]
fn a_path_that_is_not_a_repository_is_a_structured_failure() {
    let root = create_temp_root();

    let result = stage_all_operation(root.path());

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Failed);
    assert_eq!(result.code, Code::GitError);
    assert!(
        result.message.contains("not a git repository"),
        "{}",
        result.message
    );
    assert_eq!(result.status.unwrap().current.as_deref(), Some(""));
}
