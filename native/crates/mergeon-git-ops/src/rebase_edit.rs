//! Reescritura de historia con `git rebase -i` sin intervención del usuario.
//!
//! Git pide el todo-list y los mensajes de commit a través de un "editor":
//! el comando de `GIT_SEQUENCE_EDITOR` / `GIT_EDITOR`, al que añade la ruta del
//! archivo a editar. La app Electron usaba como editor el propio binario en
//! modo Node; aquí el editor es un script de `sh` generado en un directorio
//! temporal. Git lanza sus editores con `sh -c` en todas las plataformas (en
//! Windows, el `sh` que trae Git for Windows), así que no hace falta Node ni
//! reinvocar el ejecutable de Mergeon. Los scripts usan solo builtins de POSIX
//! sh (`read`, `printf`, `case`): no dependen de `cp`, `sed` ni `awk`.

use mergeon_gitcmd::git_dir_path;
use std::path::Path;

use crate::exec::{OperationOutcome, git, raw};
use crate::messages::js_trim;
use crate::results::{
    MutationExtra, MutationPayload, build_mutation_error_result, finalize_mutation_result,
};
use crate::types::{
    Code, InteractiveRebaseOptions, MutationResult, Operation, Outcome, RebaseActionParams,
};

/// Entrecomilla `value` como una sola palabra literal de sh.
fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Ruta tal como la entiende sh: en Windows, con `/` en lugar de `\`.
fn sh_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    }
}

/// Valor de `GIT_EDITOR` / `GIT_SEQUENCE_EDITOR` que ejecuta `script`.
fn editor_command(script: &Path) -> String {
    format!("sh {}", sh_quote(&sh_path(script)))
}

/// Script que copia `source` sobre el archivo que git pasa al editor.
fn copy_file_script(source: &Path) -> String {
    const TEMPLATE: &str = r#"while IFS= read -r line || [ -n "$line" ]; do
  printf '%s\n' "$line"
done < @SOURCE@ > "$1"
"#;
    TEMPLATE.replace("@SOURCE@", &sh_quote(&sh_path(source)))
}

/*
 * Editor de secuencia de las acciones sobre un commit: busca la primera línea
 * `pick <hash>` cuyo hash abreviado es prefijo del hash completo del commit
 * (git abrevia según `core.abbrev`, a 7 caracteres o a más) y la quita
 * (`drop`) o le cambia el comando (`reword`, `squash`). Si no la encuentra
 * crea `@MISSING@` y sale con error sin tocar el todo-list: git cancela
 * entonces el rebase y lo deja todo como estaba.
 */
const ACTION_SCRIPT: &str = r#"full=@FULL@
action=@ACTION@
found=0
out=
while IFS= read -r line || [ -n "$line" ]; do
  if [ "$found" = 0 ]; then
    case "$line" in
      "pick "*|"pick@TAB@"*)
        rest=${line#pick}
        while :; do
          case "$rest" in
            " "*|"@TAB@"*) rest=${rest#?} ;;
            *) break ;;
          esac
        done
        word=${rest%%[ @TAB@]*}
        if [ ${#word} -ge 4 ]; then
          case "$full" in
            "$word"*)
              found=1
              [ "$action" = drop ] && continue
              line="$action $rest"
              ;;
          esac
        fi
        ;;
    esac
  fi
  out="$out$line
"
done < "$1"
if [ "$found" = 0 ]; then
  : > @MISSING@
  exit 1
fi
printf '%s' "$out" > "$1"
"#;

/// `buildRebaseSequenceEditorScript`: script de sh que edita el todo-list que
/// propone git para aplicar `action` sobre el commit `full_commit_hash`
/// (hash completo); si no encuentra su línea crea `missing_marker` y falla.
/// Vacío si la acción no se conoce.
///
/// La app Electron buscaba el hash abreviado a 7 caracteres: si git abreviaba
/// a más (`core.abbrev` o un repo grande), `drop` no encontraba la línea, no
/// hacía nada y aun así anunciaba éxito.
pub(crate) fn build_rebase_sequence_editor_script(
    action: &str,
    full_commit_hash: &str,
    missing_marker: &Path,
) -> String {
    /*
     * La línea debe matchearse anclada a `pick <hash>` al inicio y con el hash
     * como palabra: buscar el hash en toda la línea puede tocar el commit
     * equivocado si otro subject del todo-list lo menciona (p. ej. un revert).
     * Solo cambia la primera línea que coincide.
     */
    if !matches!(action, "drop" | "reword" | "squash") {
        return String::new();
    }

    ACTION_SCRIPT
        .replace("@FULL@", &sh_quote(full_commit_hash))
        .replace("@ACTION@", &sh_quote(action))
        .replace("@MISSING@", &sh_quote(&sh_path(missing_marker)))
        .replace("@TAB@", "\t")
}

/// Script de `GIT_EDITOR` para el rebase interactivo completo: mira la última
/// línea de `rebase-merge/done` para saber qué entrada se está aplicando y, si
/// hay mensaje para ese hash, lo escribe; si no, sale sin tocar el archivo.
fn commit_editor_script(done_path: &Path, messages_directory: &Path) -> String {
    const TEMPLATE: &str = r#"target=$1
hash=
if [ -r @DONE@ ]; then
  while read -r action candidate rest || [ -n "$action" ]; do
    if [ -n "$action" ]; then
      hash=$candidate
    fi
  done < @DONE@
fi
[ -n "$hash" ] || exit 0
match=
for file in @MESSAGES@/*; do
  [ -f "$file" ] || continue
  key=${file##*/}
  key=${key#*-}
  if [ "$key" = "$hash" ]; then
    match=$file
    break
  fi
done
if [ -z "$match" ]; then
  for file in @MESSAGES@/*; do
    [ -f "$file" ] || continue
    key=${file##*/}
    key=${key#*-}
    case "$key" in
      "$hash"*) match=$file; break ;;
    esac
    case "$hash" in
      "$key"*) match=$file; break ;;
    esac
  done
fi
[ -n "$match" ] || exit 0
while IFS= read -r line || [ -n "$line" ]; do
  printf '%s\n' "$line"
done < "$match" > "$target"
"#;
    TEMPLATE
        .replace("@DONE@", &sh_quote(&sh_path(done_path)))
        .replace("@MESSAGES@", &sh_quote(&sh_path(messages_directory)))
}

fn blocked_rebase_action(
    repo_path: &Path,
    message: String,
    extra: MutationExtra,
) -> MutationResult {
    finalize_mutation_result(
        repo_path,
        MutationPayload::failure(
            Operation::RebaseAction,
            Outcome::Blocked,
            Code::GitError,
            message,
        )
        .with(extra),
    )
}

/// Asunto de un commit que `--autosquash` mueve y combina.
fn is_autosquash_subject(subject: &str) -> bool {
    ["fixup! ", "squash! ", "amend! "]
        .iter()
        .any(|prefix| subject.starts_with(prefix))
}

/// Cuántos commits `fixup!` / `squash!` / `amend!` hay entre `base` (sin
/// incluir; `None` es desde la raíz) y HEAD.
fn count_autosquash_commits(repo_path: &Path, base: Option<&str>) -> OperationOutcome<usize> {
    let range = base.map_or_else(|| "HEAD".to_string(), |base| format!("{base}..HEAD"));
    let output = raw(
        &git(repo_path),
        &["log", "-z", "--format=%s", range.as_str(), "--"],
    )?;
    Ok(output
        .split('\0')
        .filter(|subject| is_autosquash_subject(subject.trim_start_matches('\n')))
        .count())
}

/// "Autosquash fixups": `git rebase -i --autosquash` desde el padre del
/// commit (o desde la raíz) sin editor: git ordena el todo-list y combina los
/// mensajes él solo (`GIT_SEQUENCE_EDITOR` y `GIT_EDITOR` son `:`). Con
/// `--autostash`, unos cambios sin commitear no lo impiden. Si un fixup no
/// encaja, el rebase se queda parado en el conflicto como cualquier otro.
fn autosquash(
    repo_path: &Path,
    full_commit_hash: &str,
    base: Option<&str>,
    extra: MutationExtra,
) -> MutationResult {
    let short_commit_hash: String = full_commit_hash.chars().take(7).collect();
    let run = || -> OperationOutcome<MutationResult> {
        let ancestor =
            git(repo_path).exec(["merge-base", "--is-ancestor", full_commit_hash, "HEAD"])?;
        if !ancestor.success() {
            return Ok(blocked_rebase_action(
                repo_path,
                format!("Commit {short_commit_hash} is not part of the current branch history."),
                extra.clone(),
            ));
        }
        let range = base.map_or_else(|| "HEAD".to_string(), |base| format!("{base}..HEAD"));
        let merges = raw(
            &git(repo_path),
            &["rev-list", "--count", "--merges", range.as_str()],
        )?;
        if js_trim(&merges) != "0" {
            return Ok(blocked_rebase_action(
                repo_path,
                format!(
                    "There is a merge commit above {short_commit_hash}: autosquash would flatten it."
                ),
                extra.clone(),
            ));
        }

        let before = count_autosquash_commits(repo_path, base)?;
        if before == 0 {
            return Ok(finalize_mutation_result(
                repo_path,
                MutationPayload::failure(
                    Operation::RebaseAction,
                    Outcome::Noop,
                    Code::NoChanges,
                    format!("There are no fixup commits from {short_commit_hash} up to HEAD."),
                )
                .with(extra.clone()),
            ));
        }

        let runner = git(repo_path)
            .with_config(["rebase.abbreviateCommands=false"])
            .with_env("GIT_SEQUENCE_EDITOR", ":")
            .with_env("GIT_EDITOR", ":");
        match base {
            Some(base) => raw(
                &runner,
                &["rebase", "-i", "--autosquash", "--autostash", base],
            )?,
            None => raw(
                &runner,
                &["rebase", "-i", "--autosquash", "--autostash", "--root"],
            )?,
        };

        let squashed = before.saturating_sub(count_autosquash_commits(repo_path, base)?);
        let message = if squashed == 0 {
            format!(
                "Rebased from {short_commit_hash}, but no fixup commit matched a commit in that range."
            )
        } else {
            match squashed {
                1 => "Squashed 1 fixup commit into its target.".to_string(),
                count => format!("Squashed {count} fixup commits into their targets."),
            }
        };
        Ok(finalize_mutation_result(
            repo_path,
            MutationPayload::completed(Operation::RebaseAction, message).with(extra.clone()),
        ))
    };

    run().unwrap_or_else(|error| {
        build_mutation_error_result(repo_path, Operation::RebaseAction, &error, extra.clone())
    })
}

/// `rebaseActionOperation`: `drop`, `reword` o `squash` de un commit concreto
/// mediante un rebase interactivo sobre su padre, o `autosquash`: combinar
/// los `fixup!` / `squash!` / `amend!` que haya de ese commit hasta HEAD.
pub fn rebase_action_operation(
    repo_path: &Path,
    action: &str,
    commit_hash: &str,
    params: &RebaseActionParams,
) -> MutationResult {
    let extra = MutationExtra {
        action: Some(action.to_string()),
        commit: Some(commit_hash.to_string()),
        ..Default::default()
    };

    if !matches!(action, "drop" | "reword" | "squash" | "autosquash") {
        return blocked_rebase_action(
            repo_path,
            format!("Unsupported rebase action '{action}'."),
            extra,
        );
    }

    let new_message = js_trim(params.new_message.as_deref().unwrap_or_default());
    if action == "reword" && new_message.is_empty() {
        return blocked_rebase_action(
            repo_path,
            "A new commit message is required to reword a commit.".to_string(),
            extra,
        );
    }

    let short_commit_hash: String = commit_hash.chars().take(7).collect();
    let base_git = git(repo_path);

    // El hash del commit y el del padre, resueltos antes de empezar: un commit
    // que no existe falla aquí, sin llegar a arrancar un rebase.
    let resolved = (|| -> OperationOutcome<(String, Option<String>)> {
        let full = js_trim(&raw(
            &base_git,
            &[
                "rev-parse",
                "--verify",
                &format!("{commit_hash}^{{commit}}"),
            ],
        )?)
        .to_string();
        let parent = base_git
            .exec(["rev-parse", "--verify", "--quiet", &format!("{full}^")])?
            .success()
            .then(|| format!("{full}^"));
        Ok((full, parent))
    })();
    let (full_commit_hash, parent) = match resolved {
        Ok(resolved) => resolved,
        Err(error) => {
            return build_mutation_error_result(repo_path, Operation::RebaseAction, &error, extra);
        }
    };

    if action == "autosquash" {
        return autosquash(repo_path, &full_commit_hash, parent.as_deref(), extra);
    }

    /*
     * Dónde arranca el rebase. `drop` y `reword` desde el padre del commit (o
     * desde la raíz si no lo tiene). `squash` necesita que el padre esté en el
     * todo-list para combinarse con él, así que arranca un commit antes: la app
     * Electron arrancaba en `<commit>^`, la línea `squash` quedaba la primera
     * del todo-list y git la rechazaba dejando el rebase a medias.
     */
    let base = if action == "squash" {
        let Some(parent) = parent else {
            return blocked_rebase_action(
                repo_path,
                "The root commit has no parent to squash into.".to_string(),
                extra,
            );
        };
        let grandparent = format!("{parent}^");
        match base_git.exec(["rev-parse", "--verify", "--quiet", &grandparent]) {
            Ok(output) if output.success() => Some(grandparent),
            Ok(_) => None,
            Err(error) => {
                return build_mutation_error_result(
                    repo_path,
                    Operation::RebaseAction,
                    &error.into(),
                    extra,
                );
            }
        }
    } else {
        parent
    };

    let directory = match tempfile::Builder::new().prefix("mergeon-rebase-").tempdir() {
        Ok(directory) => directory,
        Err(error) => {
            return build_mutation_error_result(
                repo_path,
                Operation::RebaseAction,
                &error.into(),
                extra,
            );
        }
    };
    let missing_marker = directory.path().join("commit-not-in-todo");

    let run = || -> OperationOutcome<()> {
        let sequence_editor_path = directory.path().join("sequence-editor.sh");
        std::fs::write(
            &sequence_editor_path,
            build_rebase_sequence_editor_script(action, &full_commit_hash, &missing_marker),
        )?;
        let mut git = git(repo_path)
            // Con `p` en vez de `pick` el editor no reconocería las líneas.
            .with_config(["rebase.abbreviateCommands=false"])
            .with_env("GIT_SEQUENCE_EDITOR", editor_command(&sequence_editor_path));

        match action {
            "reword" => {
                let message_path = directory.path().join("message.txt");
                let commit_editor_path = directory.path().join("commit-editor.sh");
                std::fs::write(&message_path, format!("{new_message}\n"))?;
                std::fs::write(&commit_editor_path, copy_file_script(&message_path))?;
                git = git.with_env("GIT_EDITOR", editor_command(&commit_editor_path));
            }
            // El mensaje combinado que prepara git se acepta tal cual: `:` es el
            // editor que git reconoce como "no abrir nada".
            "squash" => git = git.with_env("GIT_EDITOR", ":"),
            _ => {}
        }

        match base.as_deref() {
            Some(base) => raw(&git, &["rebase", "-i", base])?,
            None => raw(&git, &["rebase", "-i", "--root"])?,
        };
        Ok(())
    };

    let outcome = run();
    if outcome.is_err() && missing_marker.exists() {
        // El editor no encontró el commit en el todo-list: git canceló el rebase
        // sin tocar nada (p. ej. un commit que no es de la rama actual).
        return blocked_rebase_action(
            repo_path,
            format!("Commit {short_commit_hash} is not part of the current branch history."),
            extra,
        );
    }

    match outcome {
        Ok(()) => {
            let message = match action {
                "drop" => format!("Dropped commit {short_commit_hash}."),
                "reword" => format!("Reworded commit {short_commit_hash}."),
                _ => format!("Squashed commit {short_commit_hash} into its parent."),
            };
            finalize_mutation_result(
                repo_path,
                MutationPayload::completed(Operation::RebaseAction, message).with(extra),
            )
        }
        Err(error) => {
            build_mutation_error_result(repo_path, Operation::RebaseAction, &error, extra)
        }
    }
}

const INTERACTIVE_REBASE_ACTIONS: [&str; 5] = ["pick", "reword", "squash", "fixup", "drop"];

struct TodoEntry {
    action: String,
    hash: String,
    message: String,
}

fn is_commit_hash(value: &str) -> bool {
    (7..=40).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/*
 * Rebase interactivo completo: la app arma el todo-list entero (orden y
 * acciones) y lo escribe a disco; GIT_SEQUENCE_EDITOR es un script mínimo que
 * copia ese archivo sobre el que git propone. Los mensajes de reword van cada
 * uno en un archivo y GIT_EDITOR decide por commit: lee la última línea de
 * `rebase-merge/done` para saber qué entrada se está aplicando; si hay mensaje
 * para ese hash lo escribe, si no sale sin tocar (acepta el prellenado — cubre
 * squash y cualquier otra parada). Todo por archivo, nunca inline: sin límites
 * de línea de comando ni escapado de shell.
 */
pub fn interactive_rebase_operation(
    repo_path: &Path,
    options: &InteractiveRebaseOptions,
) -> MutationResult {
    let normalized_todo: Vec<TodoEntry> = options
        .todo
        .iter()
        .map(|entry| TodoEntry {
            action: js_trim(&entry.action).to_lowercase(),
            hash: js_trim(&entry.hash).to_lowercase(),
            message: js_trim(entry.message.as_deref().unwrap_or_default()).to_string(),
        })
        .collect();

    let blocked = |message: String| {
        finalize_mutation_result(
            repo_path,
            MutationPayload::failure(
                Operation::InteractiveRebase,
                Outcome::Blocked,
                Code::GitError,
                message,
            ),
        )
    };

    let Some(first_entry) = normalized_todo.first() else {
        return blocked("The rebase todo list is empty.".to_string());
    };
    let invalid_entry = normalized_todo.iter().find(|entry| {
        !INTERACTIVE_REBASE_ACTIONS.contains(&entry.action.as_str()) || !is_commit_hash(&entry.hash)
    });
    if let Some(entry) = invalid_entry {
        return blocked(format!(
            "Unsupported todo entry '{} {}'.",
            entry.action, entry.hash
        ));
    }
    if matches!(first_entry.action.as_str(), "squash" | "fixup") {
        return blocked("The first todo entry cannot be squash or fixup.".to_string());
    }

    let run = || -> OperationOutcome<()> {
        let git_dir = git_dir_path(repo_path)?;
        // El directorio se borra al salir, también si el rebase falla.
        let directory = tempfile::Builder::new()
            .prefix("mergeon-rebase-")
            .tempdir()?;

        let todo_path = directory.path().join("todo.txt");
        let messages_directory = directory.path().join("messages");
        let sequence_editor_path = directory.path().join("sequence-editor.sh");
        let commit_editor_path = directory.path().join("commit-editor.sh");

        let todo: String = normalized_todo
            .iter()
            .map(|entry| format!("{} {}\n", entry.action, entry.hash))
            .collect();
        std::fs::write(&todo_path, todo)?;

        // Un mensaje por hash; si un hash se repite gana el último, en la
        // posición del primero.
        let mut reword_messages: Vec<(&str, &str)> = Vec::new();
        for entry in &normalized_todo {
            if entry.action != "reword" || entry.message.is_empty() {
                continue;
            }
            match reword_messages
                .iter_mut()
                .find(|(hash, _)| *hash == entry.hash)
            {
                Some(existing) => existing.1 = &entry.message,
                None => reword_messages.push((&entry.hash, &entry.message)),
            }
        }
        std::fs::create_dir(&messages_directory)?;
        for (index, (hash, message)) in reword_messages.iter().enumerate() {
            // El índice conserva el orden del todo-list al recorrer el directorio.
            std::fs::write(
                messages_directory.join(format!("{index:06}-{hash}")),
                format!("{message}\n"),
            )?;
        }

        std::fs::write(&sequence_editor_path, copy_file_script(&todo_path))?;
        std::fs::write(
            &commit_editor_path,
            commit_editor_script(
                &git_dir.join("rebase-merge").join("done"),
                &messages_directory,
            ),
        )?;

        let git = git(repo_path)
            .with_env("GIT_SEQUENCE_EDITOR", editor_command(&sequence_editor_path))
            .with_env("GIT_EDITOR", editor_command(&commit_editor_path));

        match options
            .base_commit
            .as_deref()
            .filter(|base| !base.is_empty())
        {
            Some(base_commit) => raw(&git, &["rebase", "-i", base_commit])?,
            None => raw(&git, &["rebase", "-i", "--root"])?,
        };
        Ok(())
    };

    match run() {
        Ok(()) => finalize_mutation_result(
            repo_path,
            MutationPayload::completed(
                Operation::InteractiveRebase,
                format!(
                    "Rebase completed: {} todo entr{} applied.",
                    normalized_todo.len(),
                    if normalized_todo.len() == 1 {
                        "y"
                    } else {
                        "ies"
                    }
                ),
            ),
        ),
        Err(error) => build_mutation_error_result(
            repo_path,
            Operation::InteractiveRebase,
            &error,
            MutationExtra::default(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_paths_for_sh() {
        assert_eq!(sh_quote("plain"), "'plain'");
        assert_eq!(sh_quote("it's $HOME"), r"'it'\''s $HOME'");
        assert_eq!(
            editor_command(Path::new("/tmp/my dir/editor.sh")),
            "sh '/tmp/my dir/editor.sh'"
        );
    }

    #[test]
    fn unknown_actions_have_no_sequence_editor() {
        let marker = Path::new("/tmp/marker");
        assert_eq!(
            build_rebase_sequence_editor_script("exec", "abc1234", marker),
            ""
        );
        assert!(
            build_rebase_sequence_editor_script("drop", "abc1234", marker)
                .contains("full='abc1234'")
        );
        assert!(
            build_rebase_sequence_editor_script("squash", "abc1234", marker)
                .contains("action='squash'")
        );
    }

    #[test]
    fn validates_todo_hashes() {
        assert!(is_commit_hash("abc1234"));
        assert!(is_commit_hash(&"a".repeat(40)));
        assert!(!is_commit_hash("abc123"));
        assert!(!is_commit_hash(&"a".repeat(41)));
        assert!(!is_commit_hash("ABC1234"));
        assert!(!is_commit_hash("abc123g"));
    }
}
