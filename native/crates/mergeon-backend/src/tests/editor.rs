//! `Shell::open_in_editor`: detección de editores, argumentos por editor y
//! plataforma, y avisos, con un lanzador que solo registra.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use mergeon_store::{ExternalEditorSettings, PlainTextCipher, SettingsUpdate, Store, StoreOptions};

use super::support::{FakeLauncher, Launched};
use crate::editor::{EditorLaunch, EditorOpened};
use crate::shell::{LaunchError, Shell, ShellPlatform};
use crate::{Backend, BackendOptions};

/// Un sistema de mentira: variables de entorno y archivos que existen.
fn system(
    launcher: &Arc<FakeLauncher>,
    platform: ShellPlatform,
    env: &[(&str, &str)],
    files: &[&str],
) -> Shell {
    let env: HashMap<String, String> = env
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    let files: HashSet<String> = files.iter().map(|file| file.to_string()).collect();
    Shell::with_launcher(launcher.clone(), platform)
        .with_env(move |name| env.get(name).cloned())
        .with_file_probe(move |path| files.contains(path))
}

fn detached(command: &str, args: &[&str]) -> Launched {
    Launched {
        command: command.to_string(),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        cwd: None,
        detached: true,
    }
}

fn ids(shell: &Shell) -> Vec<String> {
    shell
        .available_editors()
        .into_iter()
        .map(|editor| editor.id)
        .collect()
}

#[test]
fn detects_editors_in_the_path_in_catalog_order() {
    let launcher = FakeLauncher::new();
    let shell = system(
        &launcher,
        ShellPlatform::Linux,
        &[("PATH", "/usr/local/bin::/usr/bin/:/home/ana/.local/bin")],
        &[
            "/usr/bin/subl",
            "/home/ana/.local/bin/zeditor",
            "/usr/local/bin/code",
            "/usr/bin/idea",
            // No es Linux: no cuenta aunque exista.
            "/usr/bin/notepad++",
        ],
    );

    assert_eq!(ids(&shell), ["vscode", "zed", "sublime", "intellij"]);
    let zed = shell
        .available_editors()
        .into_iter()
        .find(|editor| editor.id == "zed")
        .unwrap();
    assert_eq!(zed.name, "Zed");
    assert_eq!(zed.executable, "/home/ana/.local/bin/zeditor");
    // Buscar no lanza nada (ni siquiera `which`).
    assert!(launcher.launched().is_empty());
}

#[test]
fn on_windows_the_typical_install_folders_come_before_the_path() {
    let launcher = FakeLauncher::new();
    let shell = system(
        &launcher,
        ShellPlatform::Windows,
        &[
            ("LOCALAPPDATA", r"C:\Users\Ana Torres\AppData\Local\"),
            ("ProgramFiles", r"C:\Program Files"),
            ("ProgramFiles(x86)", r"C:\Program Files (x86)"),
            (
                "PATH",
                r#"C:\Windows;"C:\Users\Ana Torres\AppData\Local\Programs\Microsoft VS Code\bin";C:\tools"#,
            ),
        ],
        &[
            r"C:\Users\Ana Torres\AppData\Local\Programs\Microsoft VS Code\Code.exe",
            r"C:\Users\Ana Torres\AppData\Local\Programs\Microsoft VS Code\bin\code.cmd",
            r"C:\Users\Ana Torres\AppData\Local\Programs\cursor\Cursor.exe",
            r"C:\Program Files (x86)\Notepad++\notepad++.exe",
            r"C:\tools\subl.exe",
        ],
    );

    let found: Vec<(String, String)> = shell
        .available_editors()
        .into_iter()
        .map(|editor| (editor.id, editor.executable))
        .collect();
    assert_eq!(
        found,
        [
            (
                "vscode".to_string(),
                r"C:\Users\Ana Torres\AppData\Local\Programs\Microsoft VS Code\Code.exe"
                    .to_string()
            ),
            (
                "cursor".to_string(),
                r"C:\Users\Ana Torres\AppData\Local\Programs\cursor\Cursor.exe".to_string()
            ),
            ("sublime".to_string(), r"C:\tools\subl.exe".to_string()),
            (
                "notepad++".to_string(),
                r"C:\Program Files (x86)\Notepad++\notepad++.exe".to_string()
            ),
        ]
    );
}

#[test]
fn on_macos_the_launcher_inside_the_app_bundle_is_found() {
    let launcher = FakeLauncher::new();
    let shell = system(
        &launcher,
        ShellPlatform::MacOs,
        &[("PATH", "/usr/bin:/bin")],
        &["/Applications/Zed.app/Contents/MacOS/cli"],
    );
    let editors = shell.available_editors();
    assert_eq!(editors.len(), 1);
    assert_eq!(
        editors[0].executable,
        "/Applications/Zed.app/Contents/MacOS/cli"
    );
}

#[test]
fn opens_vs_code_at_the_line_with_goto() {
    let launcher = FakeLauncher::new();
    launcher.add("/usr/bin/code");
    let shell = system(
        &launcher,
        ShellPlatform::Linux,
        &[("PATH", "/usr/bin")],
        &["/usr/bin/code"],
    );
    shell.set_external_editor(ExternalEditorSettings::editor("vscode"));

    shell
        .open_in_editor("/home/ana/my repo/src/ñandú.rs", Some(42))
        .unwrap();
    // Sin línea (el repositorio entero), solo la ruta.
    shell.open_in_editor("/home/ana/my repo", None).unwrap();

    assert_eq!(
        launcher.launched(),
        [
            detached(
                "/usr/bin/code",
                &["--goto", "/home/ana/my repo/src/ñandú.rs:42:1"]
            ),
            detached("/usr/bin/code", &["/home/ana/my repo"]),
        ]
    );
}

#[test]
fn each_editor_gets_the_line_in_its_own_format() {
    let cases: [(&str, &str, &[&str]); 4] = [
        ("zed", "/bin/zed", &["/r/a b.txt:7:1"]),
        ("sublime", "/bin/subl", &["/r/a b.txt:7:1"]),
        ("pycharm", "/bin/pycharm", &["--line", "7", "/r/a b.txt"]),
        ("cursor", "/bin/cursor", &["--goto", "/r/a b.txt:7:1"]),
    ];
    for (id, executable, args) in cases {
        let launcher = FakeLauncher::new();
        launcher.add(executable);
        let shell = system(
            &launcher,
            ShellPlatform::Linux,
            &[("PATH", "/bin")],
            &[executable],
        );
        shell.set_external_editor(ExternalEditorSettings::editor(id));
        shell.open_in_editor("/r/a b.txt", Some(7)).unwrap();
        assert_eq!(launcher.launched(), [detached(executable, args)], "{id}");
    }
}

#[test]
fn on_windows_paths_use_backslashes_and_stay_one_argument() {
    let exe = r"C:\Program Files\Notepad++\notepad++.exe";
    let launcher = FakeLauncher::new();
    launcher.add(exe);
    let shell = system(
        &launcher,
        ShellPlatform::Windows,
        &[("ProgramFiles", r"C:\Program Files")],
        &[exe],
    );
    shell.set_external_editor(ExternalEditorSettings::editor("notepad++"));

    shell
        .open_in_editor("C:/Users/Ana Torres/repo/src/app & co.js", Some(3))
        .unwrap();

    assert_eq!(
        launcher.launched(),
        [detached(
            exe,
            &["-n3", r"C:\Users\Ana Torres\repo\src\app & co.js"]
        )]
    );
}

#[test]
fn automatic_takes_the_first_detected_editor_or_the_system_default() {
    let launcher = FakeLauncher::new();
    let shell = system(
        &launcher,
        ShellPlatform::Linux,
        &[("PATH", "/usr/bin")],
        &["/usr/bin/subl", "/usr/bin/zed"],
    );
    assert_eq!(
        shell
            .editor_launch(&ExternalEditorSettings::default(), "/r/x.rs", Some(2))
            .unwrap(),
        EditorLaunch::Command {
            name: "Zed".to_string(),
            program: "/usr/bin/zed".to_string(),
            args: vec!["/r/x.rs:2:1".to_string()],
        }
    );

    // Sin ninguno instalado: el programa por defecto (`xdg-open`), sin línea.
    let launcher = FakeLauncher::new();
    launcher.add("xdg-open").exit_with("xdg-open", 0);
    let nothing = system(
        &launcher,
        ShellPlatform::Linux,
        &[("PATH", "/usr/bin")],
        &[],
    );
    nothing.open_in_editor("/r/x.rs", Some(2)).unwrap();
    assert_eq!(
        launcher.launched(),
        [Launched {
            command: "xdg-open".to_string(),
            args: vec!["/r/x.rs".to_string()],
            cwd: Some("/r".to_string()),
            detached: false,
        }]
    );
}

#[test]
fn system_default_uses_the_open_path_program_of_each_platform() {
    let launcher = FakeLauncher::new();
    launcher.add("open").exit_with("open", 0);
    let shell = system(&launcher, ShellPlatform::MacOs, &[], &[]);
    shell.set_external_editor(ExternalEditorSettings::editor("system"));
    shell.open_in_editor("/Users/ana/r/a.txt", Some(9)).unwrap();
    assert_eq!(launcher.launched()[0].command, "open");
    assert_eq!(launcher.launched()[0].args, ["/Users/ana/r/a.txt"]);
}

/// Sin editor elegido ni instalado se abre con el programa del sistema, y el
/// resultado lo dice (la interfaz ofrece elegir uno). Si eso también falla,
/// el error explica las dos cosas.
#[test]
fn the_system_default_fallback_is_reported() {
    let launcher = FakeLauncher::new();
    launcher.add("xdg-open").exit_with("xdg-open", 0);
    let shell = system(
        &launcher,
        ShellPlatform::Linux,
        &[("PATH", "/usr/bin")],
        &[],
    );
    assert_eq!(
        shell.open_in_editor("/r/a.txt", None).unwrap(),
        EditorOpened::SystemDefault { fallback: true }
    );
    // Elegido a propósito no es un recurso a falta de otro.
    shell.set_external_editor(ExternalEditorSettings::editor("system"));
    assert_eq!(
        shell.open_in_editor("/r/a.txt", None).unwrap(),
        EditorOpened::SystemDefault { fallback: false }
    );

    let launcher = FakeLauncher::new();
    launcher.add("xdg-open").exit_with("xdg-open", 3);
    let shell = system(
        &launcher,
        ShellPlatform::Linux,
        &[("PATH", "/usr/bin")],
        &[],
    );
    let error = shell.open_in_editor("/r/a.txt", Some(4)).unwrap_err();
    assert_eq!(
        error.message(),
        "No external editor was found, and the system couldn't open the file either (A required tool could not be found)."
    );
    shell.set_external_editor(ExternalEditorSettings::editor("system"));
    let error = shell.open_in_editor("/r/a.txt", None).unwrap_err();
    assert_eq!(
        error.message(),
        "Failed to open path: A required tool could not be found"
    );

    // Con un editor del catálogo, su nombre.
    let launcher = FakeLauncher::new();
    launcher.add("/usr/bin/zed");
    let shell = system(
        &launcher,
        ShellPlatform::Linux,
        &[("PATH", "/usr/bin")],
        &["/usr/bin/zed"],
    );
    assert_eq!(
        shell.open_in_editor("/r/a.txt", None).unwrap(),
        EditorOpened::Editor {
            name: "Zed".to_string()
        }
    );
}

#[test]
fn a_custom_command_gets_its_template_with_the_markers_filled() {
    let launcher = FakeLauncher::new();
    launcher.add("/opt/My Editor/edit");
    let shell = system(&launcher, ShellPlatform::Linux, &[], &[]);
    shell.set_external_editor(ExternalEditorSettings::custom(
        " /opt/My Editor/edit ",
        r#"--new-window "{path}" +{line}:{column}"#,
    ));

    shell
        .open_in_editor("/home/ana/dir with spaces/$HOME;rm.txt", Some(15))
        .unwrap();

    assert_eq!(
        launcher.launched(),
        [detached(
            "/opt/My Editor/edit",
            &[
                "--new-window",
                "/home/ana/dir with spaces/$HOME;rm.txt",
                "+15:1"
            ]
        )]
    );
}

#[test]
fn a_custom_command_on_windows_gets_a_backslashed_path() {
    let launcher = FakeLauncher::new();
    launcher.add(r"C:\Tools\ed.exe");
    let shell = system(&launcher, ShellPlatform::Windows, &[], &[]);
    shell.set_external_editor(ExternalEditorSettings::custom(
        r"C:\Tools\ed.exe",
        "{path}:{line}",
    ));
    shell.open_in_editor("C:/repo/src/a.rs", None).unwrap();
    assert_eq!(
        launcher.launched(),
        [detached(r"C:\Tools\ed.exe", &[r"C:\repo\src\a.rs:1"])]
    );
}

#[test]
fn a_missing_editor_is_a_clear_error() {
    let launcher = FakeLauncher::new();
    let shell = system(
        &launcher,
        ShellPlatform::Linux,
        &[("PATH", "/usr/bin")],
        &[],
    );

    // Elegido pero no instalado: se intenta por su nombre y se explica.
    shell.set_external_editor(ExternalEditorSettings::editor("sublime"));
    let error = shell.open_in_editor("/r/a.txt", Some(1)).unwrap_err();
    assert_eq!(
        error.message(),
        "Sublime Text was not found (subl). Install it or choose another editor in Settings → Git → External editor."
    );
    assert_eq!(launcher.launched(), [detached("subl", &["/r/a.txt:1:1"])]);

    shell.set_external_editor(ExternalEditorSettings::custom("/nope/ed", ""));
    let error = shell.open_in_editor("/r/a.txt", None).unwrap_err();
    assert!(
        error.message().starts_with("/nope/ed was not found"),
        "{error}"
    );

    // Un comando propio vacío no lanza nada.
    let quiet = FakeLauncher::new();
    let shell = system(&quiet, ShellPlatform::Linux, &[], &[]);
    shell.set_external_editor(ExternalEditorSettings::custom("  ", "{path}"));
    let error = shell.open_in_editor("/r/a.txt", None).unwrap_err();
    assert_eq!(
        error.message(),
        "No custom editor command is set. Choose one in Settings → Git → External editor."
    );
    assert!(quiet.launched().is_empty());

    // Otros fallos al lanzar también llevan el nombre del editor.
    let failing = FakeLauncher::new();
    failing.fail_with(LaunchError::other("spawn code EACCES"));
    let shell = system(&failing, ShellPlatform::Linux, &[], &[]);
    shell.set_external_editor(ExternalEditorSettings::editor("vscode"));
    assert_eq!(
        shell.open_in_editor("/r", None).unwrap_err().message(),
        "Could not open Visual Studio Code: spawn code EACCES"
    );
}

#[test]
fn an_unknown_editor_id_is_launched_as_a_command() {
    let launcher = FakeLauncher::new();
    launcher.add("kate");
    let shell = system(&launcher, ShellPlatform::Linux, &[], &[]);
    shell.set_external_editor(ExternalEditorSettings::editor("kate"));
    shell.open_in_editor("/r/a.txt", Some(4)).unwrap();
    assert_eq!(launcher.launched(), [detached("kate", &["/r/a.txt"])]);
}

#[test]
fn the_backend_starts_with_the_saved_editor() {
    let store = Store::in_memory(StoreOptions::new(Arc::new(PlainTextCipher)));
    store
        .update_settings(&SettingsUpdate {
            external_editor: Some(ExternalEditorSettings::editor("zed")),
            ..Default::default()
        })
        .unwrap();
    let launcher = FakeLauncher::new();
    let backend = Backend::new(
        BackendOptions::new()
            .store(store)
            .shell(Shell::with_launcher(launcher, ShellPlatform::Linux)),
    )
    .unwrap();
    assert_eq!(
        backend.shell().external_editor(),
        ExternalEditorSettings::editor("zed")
    );
}

/// `xdg-open` sin escritorio ejecuta el programa en primer plano: no se le
/// espera más de un plazo, y si sigue vivo se da por abierto.
#[cfg(unix)]
#[test]
fn run_for_does_not_wait_for_a_program_that_keeps_running() {
    use std::time::{Duration, Instant};

    use crate::shell::{ProcessLauncher, RunOutcome, SystemLauncher};

    let launcher = SystemLauncher;
    let started = Instant::now();
    let outcome = launcher
        .run_for(
            "sleep",
            &["5".to_string()],
            None,
            Duration::from_millis(200),
        )
        .unwrap();
    assert_eq!(outcome, RunOutcome::Running);
    assert!(started.elapsed() < Duration::from_secs(3));

    let outcome = launcher
        .run_for(
            "sh",
            &["-c".to_string(), "exit 3".to_string()],
            None,
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(outcome, RunOutcome::Exited(Some(3)));
    assert!(
        launcher
            .run_for("no-such-program-mergeon", &[], None, Duration::from_secs(1))
            .is_err()
    );
}
