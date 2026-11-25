//! Test de humo: un flujo completo de la app solo a través de `Backend`, como
//! lo usará la capa de arriba (abrir repo → status → stage → commit → crear
//! rama → merge con conflicto → resolver → continuar).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mergeon_backend::{
    Backend, BackendEvent, BackendOptions, CommitLogOptions, CommitOptions, DiffOptions,
    MergeOptions, OperationPhase, OperationType, Outcome, PlainTextCipher, Store, StoreOptions,
};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("LC_ALL", "C")
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Un directorio de proyecto todavía sin git, con identidad lista para
/// cuando lo tenga (config local, sin depender de la global).
struct Project {
    _root: tempfile::TempDir,
    path: PathBuf,
}

impl Project {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = mergeon_gitcmd::paths::canonicalize(root.path())
            .unwrap()
            .join("project");
        std::fs::create_dir(&path).unwrap();
        Self { _root: root, path }
    }

    fn configure(&self) {
        git(&self.path, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        for (key, value) in [
            ("user.name", "Smoke Test"),
            ("user.email", "smoke@example.com"),
            ("commit.gpgsign", "false"),
            ("core.autocrlf", "false"),
        ] {
            git(&self.path, &["config", key, value]);
        }
    }

    fn write(&self, name: &str, content: &str) {
        std::fs::write(self.path.join(name), content).unwrap();
    }
}

#[test]
fn a_full_flow_through_the_backend() {
    let events: Arc<Mutex<Vec<BackendEvent>>> = Arc::default();
    let sink = events.clone();
    let backend = Arc::new(
        Backend::new(
            BackendOptions::new()
                .store(Store::in_memory(StoreOptions::new(Arc::new(
                    PlainTextCipher,
                ))))
                .on_event(move |event| sink.lock().unwrap().push(event)),
        )
        .expect("backend"),
    );
    let git_api = backend.git();
    let project = Project::new();
    let repo = project.path.as_path();

    // Abrir: todavía no es un repositorio; se inicializa.
    assert!(!git_api.is_valid_repository(repo));
    let created = git_api.init_repository(repo);
    assert!(created.success(), "{}", created.message());
    project.configure();
    let inspection = git_api.inspect_repository(repo);
    assert!(inspection.valid);
    assert_eq!(
        inspection.canonical_path.as_deref(),
        Some(&*repo.to_string_lossy())
    );
    backend
        .storage()
        .add_recent_repository(
            &serde_json::from_value(serde_json::json!({
                "id": "smoke",
                "name": "project",
                "path": repo.to_string_lossy()
            }))
            .unwrap(),
        )
        .unwrap();
    assert_eq!(backend.storage().get_recent_repositories().len(), 1);

    // Status: un archivo sin seguimiento.
    project.write("notes.txt", "line one\n");
    let status = git_api.get_status(repo).expect("status");
    assert_eq!(status.current.as_deref(), Some("main"));
    assert_eq!(status.not_added, ["notes.txt"]);
    assert!(!status.is_clean);
    let untracked = git_api
        .get_diff(repo, "notes.txt", false, true, &DiffOptions::default())
        .expect("diff");
    assert!(untracked.diff.contains("+line one"));

    // Stage y commit: cada mutación devuelve el estado resultante.
    let staged = git_api.stage(repo, "notes.txt");
    assert!(staged.success(), "{}", staged.message());
    assert_eq!(staged.status().unwrap().staged, ["notes.txt"]);
    let committed = git_api.commit(repo, "Add notes", &CommitOptions::default());
    assert!(committed.success(), "{}", committed.message());
    assert!(committed.status().unwrap().is_clean);
    assert!(committed.message().starts_with("Created commit "));
    let base_commit = git_api.resolve_ref(repo, "main").expect("main");
    let log = git_api
        .get_commits(repo, &CommitLogOptions::default())
        .expect("commits");
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].full_hash, base_commit);
    assert_eq!(log[0].message, "Add notes");

    // Crear una rama (con checkout) y cambiar la línea en ella.
    let branch = git_api.create_branch(repo, "feature", None, true);
    assert!(branch.success(), "{}", branch.message());
    assert_eq!(
        git_api.get_status(repo).unwrap().current.as_deref(),
        Some("feature")
    );
    project.write("notes.txt", "line from feature\n");
    assert!(git_api.stage_all(repo).success());
    assert!(
        git_api
            .commit(repo, "Feature edit", &CommitOptions::default())
            .success()
    );

    // Volver a main y cambiar la misma línea: las ramas divergen.
    let checkout = git_api.checkout(repo, "main");
    assert!(checkout.success(), "{}", checkout.message());
    project.write("notes.txt", "line from main\n");
    assert!(git_api.stage(repo, "notes.txt").success());
    assert!(
        git_api
            .commit(repo, "Main edit", &CommitOptions::default())
            .success()
    );
    let branches = git_api.get_branches(repo).expect("branches");
    assert_eq!(branches.current, "main");
    assert!(branches.branch("feature").is_some());
    let comparison = git_api
        .compare_branches(repo, "main", "feature")
        .expect("comparison");
    assert_eq!((comparison.ahead, comparison.behind), (1, 1));
    assert_eq!(comparison.ahead_commits[0].message, "Feature edit");

    // Merge: se detiene en conflicto.
    let merged = git_api.merge(repo, "feature", &MergeOptions::default());
    assert!(!merged.success());
    assert_eq!(merged.outcome(), Some(Outcome::Conflicted));
    assert_eq!(merged.code(), "CONFLICT");
    assert_eq!(merged.result().unwrap().conflict_files, ["notes.txt"]);
    let operation_state = git_api.get_operation_state(repo).expect("operation state");
    assert!(operation_state.in_progress);
    assert_eq!(operation_state.kind, Some(OperationType::Merge));
    let status = git_api.get_status(repo).unwrap();
    assert_eq!(status.conflicted, ["notes.txt"]);
    assert!(status.conflict_state.in_conflict);

    // No se puede continuar con conflictos pendientes.
    let blocked = git_api.sequence_action(repo, "continue");
    assert!(!blocked.success());
    assert_eq!(blocked.code(), "CONFLICTS_REMAIN");

    // Resolver: leer el archivo con sus marcas, escribir la resolución.
    let conflicted = git_api
        .read_file(repo, "notes.txt")
        .expect("conflicted file");
    assert!(conflicted.contains("<<<<<<<"));
    assert!(conflicted.contains("line from main"));
    assert!(conflicted.contains("line from feature"));
    let resolved = git_api.resolve_conflict(repo, "notes.txt", "line from both\n");
    assert!(resolved.success(), "{}", resolved.message());
    let resolved_status = resolved.status().expect("status after resolving");
    assert!(resolved_status.conflicted.is_empty());
    assert!(resolved_status.conflict_state.ready_to_continue);

    // Continuar: el merge termina con su commit.
    let continued = git_api.sequence_action(repo, "continue");
    assert!(continued.success(), "{}", continued.message());
    assert_eq!(
        continued.result().unwrap().operation,
        Some(OperationType::Merge)
    );
    let status = git_api.get_status(repo).unwrap();
    assert!(status.is_clean);
    assert!(!status.operation_state.in_progress);
    assert_eq!(
        std::fs::read_to_string(repo.join("notes.txt")).unwrap(),
        "line from both\n"
    );

    let commits = git_api
        .get_commits(repo, &CommitLogOptions::default())
        .expect("commits");
    assert_eq!(commits.len(), 4);
    assert_eq!(commits[0].parents.split(' ').count(), 2);
    let subjects: Vec<&str> = commits
        .iter()
        .map(|commit| commit.message.as_str())
        .collect();
    assert!(subjects.contains(&"Add notes"));
    assert!(subjects.contains(&"Feature edit"));
    assert!(subjects.contains(&"Main edit"));

    // La rama ya fusionada se puede borrar.
    let deleted = git_api.delete_branch(repo, "feature");
    assert!(deleted.success(), "{}", deleted.message());
    assert!(
        git_api
            .get_branches(repo)
            .unwrap()
            .branch("feature")
            .is_none()
    );

    // Cada mutación pasó por la cola y dejó su ciclo de vida en los eventos.
    let events = events.lock().unwrap();
    let operations: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            BackendEvent::Operation(event) => Some(event),
            _ => None,
        })
        .collect();
    let finished: Vec<(&str, OperationPhase)> = operations
        .iter()
        .filter(|event| event.phase.is_terminal())
        .map(|event| (event.operation.as_str(), event.phase))
        .collect();
    assert_eq!(
        finished,
        [
            ("init-repository", OperationPhase::Completed),
            ("stage", OperationPhase::Completed),
            ("commit", OperationPhase::Completed),
            ("create-branch", OperationPhase::Completed),
            ("stage-all", OperationPhase::Completed),
            ("commit", OperationPhase::Completed),
            ("checkout", OperationPhase::Completed),
            ("stage", OperationPhase::Completed),
            ("commit", OperationPhase::Completed),
            ("merge", OperationPhase::Failed),
            ("sequence-action", OperationPhase::Failed),
            ("resolve-conflict", OperationPhase::Completed),
            ("sequence-action", OperationPhase::Completed),
            ("delete-branch", OperationPhase::Completed),
        ]
    );
    // Una operación en cola, una en curso y una final por mutación.
    assert_eq!(operations.len(), finished.len() * 3);
    assert!(git_api.get_operation_queue(repo).active.is_none());
    assert_eq!(git(repo, &["status", "--porcelain"]), "");
}
