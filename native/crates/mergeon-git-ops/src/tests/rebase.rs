//! Reescritura de historia: acciones sobre un commit y rebase interactivo completo.

use mergeon_gitcmd::{OperationType, get_structured_status};
use std::path::Path;

use super::support::*;
use crate::rebase_edit::build_rebase_sequence_editor_script;
use crate::{
    Code, InteractiveRebaseOptions, Operation, Outcome, RebaseActionParams, RebaseTodoEntry,
    interactive_rebase_operation, rebase_action_operation,
};

fn todo(action: &str, hash: &str) -> RebaseTodoEntry {
    RebaseTodoEntry {
        action: action.to_string(),
        hash: hash.to_string(),
        message: None,
    }
}

fn reword(hash: &str, message: &str) -> RebaseTodoEntry {
    RebaseTodoEntry {
        message: Some(message.to_string()),
        ..todo("reword", hash)
    }
}

fn rebase_onto(
    base_commit: Option<String>,
    todo: Vec<RebaseTodoEntry>,
) -> InteractiveRebaseOptions {
    InteractiveRebaseOptions { base_commit, todo }
}

/// Hash completo de 40 caracteres que empieza por `prefix`.
fn full_hash(prefix: &str) -> String {
    format!("{prefix:0<40}")
}

/// Aplica el editor de secuencia de `action` sobre un todo-list. Devuelve el
/// todo-list resultante y si el editor encontró el commit.
fn edit_todo(action: &str, full_hash: &str, todo_lines: &[&str]) -> (String, bool) {
    let root = create_temp_root();
    let todo_path = root.path().join("git-rebase-todo");
    let script_path = root.path().join("sequence-editor.sh");
    let missing_marker = root.path().join("missing");
    std::fs::write(&todo_path, todo_lines.join("\n")).unwrap();
    std::fs::write(
        &script_path,
        build_rebase_sequence_editor_script(action, full_hash, &missing_marker),
    )
    .unwrap();

    let found = try_run_sh_script(root.path(), &script_path, &todo_path);
    assert_eq!(found, !missing_marker.exists());
    (std::fs::read_to_string(&todo_path).unwrap(), found)
}

#[test]
fn blocks_reword_rebase_actions_without_a_replacement_message() {
    let (_root, repo_path) = create_repository();
    let target_commit = commit_file(&repo_path, "updated\n", "old message");

    let result = rebase_action_operation(
        &repo_path,
        "reword",
        &target_commit,
        &RebaseActionParams {
            new_message: Some(String::new()),
        },
    );

    assert!(!result.success);
    assert_eq!(result.code, Code::GitError);
    assert_eq!(result.action.as_deref(), Some("reword"));
    assert_eq!(result.commit.as_deref(), Some(target_commit.as_str()));
    assert_eq!(result.outcome, Outcome::Blocked);
    assert_eq!(
        result.message,
        "A new commit message is required to reword a commit."
    );
    assert_eq!(
        result.operation_state.map(|state| state.in_progress),
        Some(false)
    );
    assert_eq!(result.conflict_state.pending_count, 0);
}

#[test]
fn blocks_unsupported_rebase_actions() {
    let (_root, repo_path) = create_repository();
    let commit = head(&repo_path);

    let result =
        rebase_action_operation(&repo_path, "edit", &commit, &RebaseActionParams::default());

    assert!(!result.success);
    assert_eq!(result.operation, Operation::RebaseAction);
    assert_eq!(result.outcome, Outcome::Blocked);
    assert_eq!(result.message, "Unsupported rebase action 'edit'.");
    assert_eq!(result.action.as_deref(), Some("edit"));
}

#[test]
fn drop_sequence_editor_only_removes_the_todo_line_anchored_to_the_hash() {
    let (updated, found) = edit_todo(
        "drop",
        &full_hash("abc1234"),
        &[
            "pick abc1234 fix something",
            "pick aaa1111 Revert \"fix abc1234\"",
            "pick abc12345 shares the seven-char prefix",
            "",
        ],
    );

    assert!(found);
    assert!(
        !updated
            .lines()
            .any(|line| line.starts_with("pick abc1234 "))
    );
    assert!(updated.contains("Revert \"fix abc1234\""));
    assert!(updated.contains("pick abc12345 shares the seven-char prefix"));
    assert_eq!(
        updated,
        "pick aaa1111 Revert \"fix abc1234\"\npick abc12345 shares the seven-char prefix\n"
    );
}

#[test]
fn drop_sequence_editor_keeps_comments_and_awkward_lines_intact() {
    let (updated, found) = edit_todo(
        "drop",
        &full_hash("abc1234"),
        &[
            "pick   abc1234\t# subject with a tab",
            "pick def5678 back\\slash $HOME `tick` 'quote' \"double\" *",
            "  pick abc1234 indented lines are not commands",
            "",
            "# Rebase 1111111..2222222 onto 1111111 (3 commands)",
        ],
    );

    assert!(found);
    assert_eq!(
        updated,
        "pick def5678 back\\slash $HOME `tick` 'quote' \"double\" *\n  pick abc1234 indented lines are not commands\n\n# Rebase 1111111..2222222 onto 1111111 (3 commands)\n"
    );
}

/*
 * El original buscaba el hash abreviado a 7 caracteres: con `core.abbrev` más
 * largo (o en un repo grande, donde git abrevia a más) `drop` no encontraba la
 * línea, no hacía nada y anunciaba éxito. Ahora vale cualquier abreviatura del
 * hash completo, y si no hay ninguna el editor falla sin tocar nada.
 */
#[test]
fn sequence_editors_match_any_abbreviation_of_the_full_hash() {
    let full = full_hash("abc1234567890def");
    let lines = [
        "pick aaa111122223 first",
        "pick abc123456789 longer abbreviation",
        "pick abc9999 other",
        "",
    ];

    assert_eq!(
        edit_todo("drop", &full, &lines),
        (
            "pick aaa111122223 first\npick abc9999 other\n".to_string(),
            true
        )
    );
    assert_eq!(
        edit_todo(
            "drop",
            &full,
            &["pick abc1234567890def0000000000000000000000 whole"]
        ),
        (String::new(), true)
    );

    let (untouched, found) = edit_todo("drop", &full_hash("fff9999"), &lines);
    assert!(!found);
    assert_eq!(untouched, lines.join("\n"));
}

#[test]
fn reword_and_squash_sequence_editors_change_only_the_first_matching_command() {
    let full = full_hash("abc12345");
    let lines = [
        "pick aaa1111 first",
        "pick abc12345 # longer abbreviation",
        "pick abc1234 again",
        "",
    ];

    assert_eq!(
        edit_todo("reword", &full, &lines).0,
        "pick aaa1111 first\nreword abc12345 # longer abbreviation\npick abc1234 again\n"
    );
    assert_eq!(
        edit_todo("squash", &full, &lines).0,
        "pick aaa1111 first\nsquash abc12345 # longer abbreviation\npick abc1234 again\n"
    );
    // Sin coincidencia el todo-list queda igual y el editor falla.
    let (untouched, found) = edit_todo("squash", &full_hash("fff9999"), &lines);
    assert!(!found);
    assert_eq!(untouched, lines.join("\n"));
}

fn repository_with_three_changes() -> (tempfile::TempDir, std::path::PathBuf, [String; 3]) {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");
    init_repository(&repo_path);
    commit_file_at(&repo_path, "base\n", "base commit", "base.txt");
    let first = commit_file_at(&repo_path, "a\n", "first change", "a.txt");
    let second = commit_file_at(&repo_path, "b\n", "second change", "b.txt");
    let third = commit_file_at(&repo_path, "c\n", "third change", "c.txt");
    (root, repo_path, [first, second, third])
}

fn assert_clean_and_idle(repo_path: &Path) {
    let status = get_structured_status(repo_path).unwrap();
    assert!(status.is_clean);
    assert!(!status.operation_state.in_progress);
}

#[test]
fn drops_a_commit_through_a_rebase_action() {
    let (_root, repo_path, [_, second, _]) = repository_with_three_changes();

    let result =
        rebase_action_operation(&repo_path, "drop", &second, &RebaseActionParams::default());

    assert!(result.success, "{}", result.message);
    assert_eq!(result.operation, Operation::RebaseAction);
    assert_eq!(result.action.as_deref(), Some("drop"));
    assert_eq!(result.commit.as_deref(), Some(second.as_str()));
    assert_eq!(result.message, format!("Dropped commit {}.", &second[..7]));
    assert_eq!(
        log_subjects(&repo_path),
        ["third change", "first change", "base commit"]
    );
    assert!(!repo_path.join("b.txt").exists());
    assert_clean_and_idle(&repo_path);
}

#[test]
fn rewords_a_commit_through_a_rebase_action() {
    let (_root, repo_path, [_, second, _]) = repository_with_three_changes();
    let new_message =
        "  second change, it's \"reworded\" for $HOME\n\nBody line with a \\ backslash  ";

    let result = rebase_action_operation(
        &repo_path,
        "reword",
        &second,
        &RebaseActionParams {
            new_message: Some(new_message.to_string()),
        },
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(result.message, format!("Reworded commit {}.", &second[..7]));
    assert_eq!(
        log_subjects(&repo_path),
        [
            "third change",
            "second change, it's \"reworded\" for $HOME",
            "first change",
            "base commit"
        ]
    );
    assert_eq!(
        run_git(&repo_path, &["log", "-1", "--pretty=%B", "HEAD~1"]).trim(),
        "second change, it's \"reworded\" for $HOME\n\nBody line with a \\ backslash"
    );
    assert_eq!(read_file(&repo_path.join("b.txt")), "b\n");
    assert_clean_and_idle(&repo_path);
}

/*
 * La app Electron arrancaba el rebase en `<commit>^`: la línea `squash` quedaba
 * la primera del todo-list, git la rechazaba ("cannot 'squash' without a
 * previous commit") y el repositorio se quedaba con un rebase a medias. Ahora
 * arranca un commit antes y acepta el mensaje combinado sin abrir editor.
 */
#[test]
fn squashes_a_commit_into_its_parent_through_a_rebase_action() {
    let (_root, repo_path, [_, second, _]) = repository_with_three_changes();

    let result = rebase_action_operation(
        &repo_path,
        "squash",
        &second,
        &RebaseActionParams::default(),
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(result.outcome, Outcome::Completed);
    assert_eq!(result.action.as_deref(), Some("squash"));
    assert_eq!(
        result.message,
        format!("Squashed commit {} into its parent.", &second[..7])
    );
    assert_eq!(
        log_subjects(&repo_path),
        ["third change", "first change", "base commit"]
    );
    assert_eq!(
        run_git(&repo_path, &["log", "-1", "--pretty=%B", "HEAD~1"]).trim(),
        "first change\n\nsecond change"
    );
    assert_eq!(read_file(&repo_path.join("a.txt")), "a\n");
    assert_eq!(read_file(&repo_path.join("b.txt")), "b\n");
    assert_eq!(read_file(&repo_path.join("c.txt")), "c\n");
    assert_clean_and_idle(&repo_path);
}

#[test]
fn squashes_into_the_root_commit_and_refuses_to_squash_the_root() {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");
    init_repository(&repo_path);
    let base = commit_file_at(&repo_path, "base\n", "base commit", "base.txt");
    let first = commit_file_at(&repo_path, "a\n", "first change", "a.txt");

    let refused =
        rebase_action_operation(&repo_path, "squash", &base, &RebaseActionParams::default());
    assert!(!refused.success);
    assert_eq!(refused.outcome, Outcome::Blocked);
    assert_eq!(
        refused.message,
        "The root commit has no parent to squash into."
    );
    assert_eq!(log_subjects(&repo_path), ["first change", "base commit"]);

    // El padre es la raíz: el rebase arranca con `--root`.
    let result =
        rebase_action_operation(&repo_path, "squash", &first, &RebaseActionParams::default());
    assert!(result.success, "{}", result.message);
    assert_eq!(log_subjects(&repo_path), ["base commit"]);
    assert_eq!(
        run_git(&repo_path, &["log", "-1", "--pretty=%B"]).trim(),
        "base commit\n\nfirst change"
    );
    assert_clean_and_idle(&repo_path);
}

#[test]
fn drops_a_commit_whatever_abbreviation_git_uses() {
    let (_root, repo_path, [_, second, _]) = repository_with_three_changes();
    run_git(&repo_path, &["config", "core.abbrev", "12"]);

    let result = rebase_action_operation(
        &repo_path,
        "drop",
        &second[..10],
        &RebaseActionParams::default(),
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(
        log_subjects(&repo_path),
        ["third change", "first change", "base commit"]
    );
    assert!(!repo_path.join("b.txt").exists());
    assert_clean_and_idle(&repo_path);
}

#[test]
fn a_rebase_action_on_a_commit_outside_the_branch_is_blocked_without_changes() {
    let (_root, repo_path, _) = repository_with_three_changes();
    run_git(&repo_path, &["checkout", "-q", "-b", "side", "HEAD~2"]);
    let side = commit_file_at(&repo_path, "s\n", "side change", "s.txt");
    run_git(&repo_path, &["checkout", "-q", "main"]);
    let before = head(&repo_path);

    let result = rebase_action_operation(&repo_path, "drop", &side, &RebaseActionParams::default());

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Blocked);
    assert_eq!(
        result.message,
        format!(
            "Commit {} is not part of the current branch history.",
            &side[..7]
        )
    );
    assert_eq!(head(&repo_path), before);
    assert_clean_and_idle(&repo_path);
}

#[test]
fn a_rebase_action_on_an_unknown_commit_fails_without_starting_a_rebase() {
    let (_root, repo_path) = create_repository();

    let result = rebase_action_operation(
        &repo_path,
        "drop",
        "0123456789abcdef0123456789abcdef01234567",
        &RebaseActionParams::default(),
    );

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Failed);
    assert_eq!(result.code, Code::GitError);
    assert_eq!(result.action.as_deref(), Some("drop"));
    assert_clean_and_idle(&repo_path);
}

#[test]
fn runs_a_full_interactive_rebase_reorder_fixup_reword_and_drop() {
    let (_root, repo_path, [first, second, third]) = repository_with_three_changes();
    let fourth = commit_file_at(&repo_path, "d\n", "droppable change", "d.txt");

    // Reordenar (second antes que first), fixup del third sobre second,
    // reword de first y drop del cuarto.
    let result = interactive_rebase_operation(
        &repo_path,
        &rebase_onto(
            Some(format!("{first}^")),
            vec![
                todo("pick", &second),
                todo("fixup", &third),
                reword(&first, "first change, reworded"),
                todo("drop", &fourth),
            ],
        ),
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(result.operation, Operation::InteractiveRebase);
    assert_eq!(result.message, "Rebase completed: 4 todo entries applied.");

    // Quedan: reworded (tip), second (con fixup absorbido), base
    assert_eq!(
        log_subjects(&repo_path),
        ["first change, reworded", "second change", "base commit"]
    );

    // El fixup conservó el contenido de ambos commits
    assert_eq!(read_file(&repo_path.join("b.txt")), "b\n");
    assert_eq!(read_file(&repo_path.join("c.txt")), "c\n");
    // El drop eliminó su archivo
    assert!(!repo_path.join("d.txt").exists());
    assert_clean_and_idle(&repo_path);
}

#[test]
fn squash_keeps_content_and_interactive_rebase_blocks_invalid_todos() {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");
    init_repository(&repo_path);
    commit_file_at(&repo_path, "base\n", "base commit", "base.txt");
    let first = commit_file_at(&repo_path, "a\n", "first change", "a.txt");
    let second = commit_file_at(&repo_path, "b\n", "second change", "b.txt");
    let base = Some(format!("{first}^"));

    let squash_result = interactive_rebase_operation(
        &repo_path,
        &rebase_onto(
            base.clone(),
            vec![todo("pick", &first), todo("squash", &second)],
        ),
    );
    assert!(squash_result.success, "{}", squash_result.message);

    assert_eq!(log_subjects(&repo_path).len(), 2);
    assert_eq!(read_file(&repo_path.join("a.txt")), "a\n");
    assert_eq!(read_file(&repo_path.join("b.txt")), "b\n");
    // Sin mensaje propio, el squash acepta el mensaje combinado que propone git.
    let combined = run_git(&repo_path, &["log", "-1", "--pretty=%B"]);
    assert!(combined.contains("first change") && combined.contains("second change"));

    // Validaciones
    let empty = interactive_rebase_operation(&repo_path, &rebase_onto(None, vec![]));
    assert_eq!(empty.outcome, Outcome::Blocked);
    assert_eq!(empty.message, "The rebase todo list is empty.");

    let bad_first = interactive_rebase_operation(
        &repo_path,
        &rebase_onto(base.clone(), vec![todo("squash", &first)]),
    );
    assert_eq!(bad_first.outcome, Outcome::Blocked);
    assert_eq!(
        bad_first.message,
        "The first todo entry cannot be squash or fixup."
    );

    let bad_action = interactive_rebase_operation(
        &repo_path,
        &rebase_onto(base.clone(), vec![todo("exec", &first)]),
    );
    assert_eq!(bad_action.outcome, Outcome::Blocked);
    assert_eq!(
        bad_action.message,
        format!("Unsupported todo entry 'exec {first}'.")
    );

    let bad_hash = interactive_rebase_operation(
        &repo_path,
        &rebase_onto(base, vec![todo("pick", "HEAD; rm -rf /")]),
    );
    assert_eq!(bad_hash.outcome, Outcome::Blocked);
    assert_eq!(bad_hash.code, Code::GitError);

    // Los rechazos ocurren antes de tocar git
    assert_eq!(log_subjects(&repo_path).len(), 2);
}

#[test]
fn interactive_rebase_from_the_root_rewords_with_arbitrary_text() {
    let root = create_temp_root();
    let repo_path = root.path().join("repo with 'quotes' and spaces");
    init_repository(&repo_path);
    let base = commit_file_at(&repo_path, "base\n", "base commit", "base.txt");
    let first = commit_file_at(&repo_path, "a\n", "first change", "a.txt");
    let second = commit_file_at(&repo_path, "b\n", "second change", "b.txt");

    // Hashes abreviados y en mayúsculas, acciones con espacios: se normalizan.
    let result = interactive_rebase_operation(
        &repo_path,
        &rebase_onto(
            None,
            vec![
                reword(
                    &base[..12].to_uppercase(),
                    "root: it's \"new\" `text` $(true) \\n",
                ),
                todo(" Pick ", &first[..10]),
                reword(&second, "second: 100% *done*\n\nWith a body; and | pipes"),
            ],
        ),
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(result.message, "Rebase completed: 3 todo entries applied.");
    assert_eq!(
        log_subjects(&repo_path),
        [
            "second: 100% *done*",
            "first change",
            "root: it's \"new\" `text` $(true) \\n"
        ]
    );
    assert_eq!(
        run_git(&repo_path, &["log", "-1", "--pretty=%B"]).trim(),
        "second: 100% *done*\n\nWith a body; and | pipes"
    );
    assert_clean_and_idle(&repo_path);
}

#[test]
fn interactive_rebase_with_a_single_entry_uses_the_singular_message() {
    let (_root, repo_path, [_, _, third]) = repository_with_three_changes();

    let result = interactive_rebase_operation(
        &repo_path,
        &rebase_onto(Some(format!("{third}^")), vec![todo("pick", &third)]),
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(result.message, "Rebase completed: 1 todo entry applied.");
}

#[test]
fn an_interactive_rebase_that_conflicts_reports_the_stopped_rebase() {
    let (_root, repo_path) = create_repository();
    let first = commit_file(&repo_path, "one\n", "first edit");
    let second = commit_file(&repo_path, "two\n", "second edit");

    // Invertir dos commits que tocan la misma línea no puede aplicarse limpio.
    let result = interactive_rebase_operation(
        &repo_path,
        &rebase_onto(
            Some(format!("{first}^")),
            vec![todo("pick", &second), todo("pick", &first)],
        ),
    );

    assert!(!result.success);
    assert_eq!(result.operation, Operation::InteractiveRebase);
    assert_eq!(result.outcome, Outcome::Conflicted);
    assert_eq!(result.code, Code::GitError);
    assert_eq!(result.conflict_files, ["file.txt"]);
    assert_eq!(
        result.operation_state.and_then(|state| state.kind),
        Some(OperationType::Rebase)
    );
}
