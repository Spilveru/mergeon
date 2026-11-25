//! Port de `electron/ipc/shell-handlers.test.js`, con un lanzador que solo
//! registra lo que se lanzaría.

use std::sync::Arc;

use super::support::{FakeLauncher, Launched};
use crate::shell::{LaunchError, Shell, ShellPlatform};

fn shell(launcher: &Arc<FakeLauncher>, platform: ShellPlatform) -> Shell {
    // Sin `$TERMINAL`, como el `delete process.env.TERMINAL` del original.
    Shell::with_launcher(launcher.clone(), platform).with_env(|_| None)
}

fn shell_with_terminal(launcher: &Arc<FakeLauncher>, terminal: &'static str) -> Shell {
    Shell::with_launcher(launcher.clone(), ShellPlatform::Linux)
        .with_env(move |name| (name == "TERMINAL").then(|| terminal.to_string()))
}

fn launched(command: &str, args: &[&str], cwd: Option<&str>, detached: bool) -> Launched {
    Launched {
        command: command.to_string(),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        cwd: cwd.map(str::to_string),
        detached,
    }
}

// --- shell:openExternal -----------------------------------------------------

#[test]
fn opens_http_and_https_urls() {
    let launcher = FakeLauncher::new();
    launcher.add("xdg-open");
    let shell = shell(&launcher, ShellPlatform::Linux);

    shell.open_external("https://github.com/user/repo").unwrap();
    shell.open_external("http://localhost:3000/").unwrap();

    assert_eq!(
        launcher.launched(),
        [
            launched("xdg-open", &["https://github.com/user/repo"], None, true),
            launched("xdg-open", &["http://localhost:3000/"], None, true),
        ]
    );
}

#[test]
fn opens_urls_with_the_program_of_each_platform() {
    let url = "https://github.com/search?q=a&type=code";

    let mac = FakeLauncher::new();
    mac.add("open");
    shell(&mac, ShellPlatform::MacOs)
        .open_external(url)
        .unwrap();
    assert_eq!(mac.launched(), [launched("open", &[url], None, true)]);

    // En Windows la URL no pasa por cmd, que cortaría en el `&`.
    let windows = FakeLauncher::new();
    windows.add("rundll32.exe");
    shell(&windows, ShellPlatform::Windows)
        .open_external(url)
        .unwrap();
    assert_eq!(
        windows.launched(),
        [launched(
            "rundll32.exe",
            &["url.dll,FileProtocolHandler", url],
            None,
            true
        )]
    );
}

#[test]
fn the_url_is_opened_as_the_parser_normalizes_it() {
    let launcher = FakeLauncher::new();
    launcher.add("xdg-open");
    let shell = shell(&launcher, ShellPlatform::Linux);

    // Como `new URL(...).toString()`: esquema y host en minúsculas, raíz explícita.
    shell.open_external("HTTPS://GitHub.com").unwrap();
    shell.open_external("https://example.com/a b").unwrap();

    assert_eq!(
        launcher.launched(),
        [
            launched("xdg-open", &["https://github.com/"], None, true),
            launched("xdg-open", &["https://example.com/a%20b"], None, true),
        ]
    );
}

#[test]
fn rejects_non_http_schemes() {
    let launcher = FakeLauncher::new();
    launcher.add("xdg-open");
    let shell = shell(&launcher, ShellPlatform::Linux);

    let file = shell.open_external("file:///etc/passwd").unwrap_err();
    assert_eq!(
        file.message(),
        "Blocked URL scheme 'file:'. Only http and https are allowed."
    );
    let script = shell.open_external("javascript:alert(1)").unwrap_err();
    assert_eq!(
        script.to_string(),
        "Blocked URL scheme 'javascript:'. Only http and https are allowed."
    );
    assert!(launcher.launched().is_empty());
}

#[test]
fn rejects_strings_that_are_not_urls() {
    let launcher = FakeLauncher::new();
    launcher.add("xdg-open");
    let shell = shell(&launcher, ShellPlatform::Linux);

    assert_eq!(
        shell.open_external("not a url").unwrap_err().message(),
        "Invalid URL."
    );
    // El `null` del original.
    assert_eq!(
        shell.open_external("").unwrap_err().message(),
        "Invalid URL."
    );
    assert!(launcher.launched().is_empty());
}

#[test]
fn reports_a_browser_that_cannot_be_launched() {
    let launcher = FakeLauncher::new();
    let shell = shell(&launcher, ShellPlatform::Linux);

    let error = shell.open_external("https://example.com/").unwrap_err();
    assert_eq!(error.message(), "spawn xdg-open ENOENT");
}

// --- shell:openTerminal -----------------------------------------------------

const WINDOWS_REPO_PATH: &str = "C:\\Users\\me\\my repo";

#[test]
fn prefers_windows_terminal_at_the_repository_folder() {
    let launcher = FakeLauncher::new();
    launcher.add("wt.exe");

    let result = shell(&launcher, ShellPlatform::Windows)
        .open_terminal(WINDOWS_REPO_PATH)
        .unwrap();

    assert!(result.success);
    assert_eq!(
        launcher.launched(),
        [launched("wt.exe", &["-d", WINDOWS_REPO_PATH], None, true)]
    );
}

#[test]
fn falls_back_to_a_visible_cmd_started_from_the_folder_when_windows_terminal_is_missing() {
    let launcher = FakeLauncher::new();
    launcher.add("cmd.exe");

    let result = shell(&launcher, ShellPlatform::Windows)
        .open_terminal(WINDOWS_REPO_PATH)
        .unwrap();

    assert!(result.success);
    assert_eq!(
        launcher.launched(),
        [
            launched("wt.exe", &["-d", WINDOWS_REPO_PATH], None, true),
            launched(
                "cmd.exe",
                &["/c", "start", "cmd.exe"],
                Some(WINDOWS_REPO_PATH),
                true
            ),
        ]
    );
}

#[test]
fn surfaces_windows_launch_errors_other_than_a_missing_executable() {
    let launcher = FakeLauncher::new();
    launcher.fail_with(LaunchError::other("spawn wt.exe EACCES"));

    let error = shell(&launcher, ShellPlatform::Windows)
        .open_terminal(WINDOWS_REPO_PATH)
        .unwrap_err();

    assert!(error.message().contains("EACCES"), "{error}");
    assert_eq!(launcher.launched().len(), 1);
}

#[test]
fn opens_terminal_app_on_macos() {
    let launcher = FakeLauncher::new();
    launcher.add("open");

    shell(&launcher, ShellPlatform::MacOs)
        .open_terminal("/Users/me/repo")
        .unwrap();

    assert_eq!(
        launcher.launched(),
        [launched(
            "open",
            &["-a", "Terminal", "/Users/me/repo"],
            None,
            true
        )]
    );
}

#[test]
fn honours_terminal_on_linux_with_the_flag_that_terminal_understands() {
    let launcher = FakeLauncher::new();
    launcher.add("/usr/bin/kitty").add("gnome-terminal");

    shell_with_terminal(&launcher, "/usr/bin/kitty")
        .open_terminal("/home/me/repo")
        .unwrap();

    assert_eq!(
        launcher.launched(),
        [launched(
            "/usr/bin/kitty",
            &["--directory", "/home/me/repo"],
            Some("/home/me/repo"),
            true
        )]
    );
}

#[test]
fn launches_an_unknown_terminal_without_flags_and_relies_on_cwd() {
    let launcher = FakeLauncher::new();
    launcher.add("foot");

    shell_with_terminal(&launcher, " foot ")
        .open_terminal("/home/me/repo")
        .unwrap();

    assert_eq!(
        launcher.launched(),
        [launched("foot", &[], Some("/home/me/repo"), true)]
    );
}

#[test]
fn walks_the_known_linux_terminals_and_never_passes_a_directory_flag_to_x_terminal_emulator() {
    let launcher = FakeLauncher::new();
    launcher.add("x-terminal-emulator");

    shell(&launcher, ShellPlatform::Linux)
        .open_terminal("/home/me/repo")
        .unwrap();

    assert_eq!(
        launcher.launched(),
        [launched(
            "x-terminal-emulator",
            &[],
            Some("/home/me/repo"),
            true
        )]
    );
}

#[test]
fn picks_the_first_known_terminal_that_is_installed() {
    let launcher = FakeLauncher::new();
    launcher.add("xterm").add("konsole").add("alacritty");

    shell(&launcher, ShellPlatform::Linux)
        .open_terminal("/home/me/repo")
        .unwrap();

    assert_eq!(
        launcher.launched(),
        [launched(
            "konsole",
            &["--workdir", "/home/me/repo"],
            Some("/home/me/repo"),
            true
        )]
    );
}

#[test]
fn explains_what_to_install_when_no_linux_terminal_is_found() {
    let launcher = FakeLauncher::new();

    let error = shell(&launcher, ShellPlatform::Linux)
        .open_terminal("/home/me/repo")
        .unwrap_err();

    assert_eq!(
        error.message(),
        "No terminal emulator found. Set $TERMINAL or install ptyxis, gnome-terminal, konsole, xfce4-terminal, or x-terminal-emulator."
    );
    assert!(launcher.launched().is_empty());
}

#[test]
fn remembers_the_detected_terminal() {
    let launcher = FakeLauncher::new();
    launcher.add("tilix");
    let shell = shell(&launcher, ShellPlatform::Linux);

    shell.open_terminal("/home/me/one").unwrap();
    // Aunque después aparezca otro mejor colocado, no se vuelve a detectar.
    launcher.add("ptyxis");
    shell.open_terminal("/home/me/two").unwrap();

    assert_eq!(
        launcher.launched(),
        [
            launched(
                "tilix",
                &["--working-directory", "/home/me/one"],
                Some("/home/me/one"),
                true
            ),
            launched(
                "tilix",
                &["--working-directory", "/home/me/two"],
                Some("/home/me/two"),
                true
            ),
        ]
    );
}

#[test]
fn inside_a_flatpak_sandbox_the_terminal_is_launched_on_the_host() {
    let launcher = FakeLauncher::new();
    // `flatpak-spawn` existe en la sandbox; el terminal, en el host.
    launcher.add("flatpak-spawn").add("host:gnome-terminal");

    shell(&launcher, ShellPlatform::Linux)
        .open_terminal("/home/me/repo")
        .unwrap();

    assert_eq!(
        launcher.launched(),
        [launched(
            "flatpak-spawn",
            &[
                "--host",
                "gnome-terminal",
                "--working-directory",
                "/home/me/repo"
            ],
            Some("/home/me/repo"),
            true
        )]
    );
}

// --- shell:openPath ---------------------------------------------------------

#[test]
fn opens_a_path_with_the_file_manager_of_each_platform() {
    let linux = FakeLauncher::new();
    linux.add("xdg-open");
    let result = shell(&linux, ShellPlatform::Linux)
        .open_path("/home/me/repo")
        .unwrap();
    assert!(result.success);
    // Se espera a `xdg-open`, lanzado desde la carpeta que contiene la ruta.
    assert_eq!(
        linux.launched(),
        [launched(
            "xdg-open",
            &["/home/me/repo"],
            Some("/home/me"),
            false
        )]
    );

    let mac = FakeLauncher::new();
    mac.add("open");
    shell(&mac, ShellPlatform::MacOs)
        .open_path("/Users/me/repo")
        .unwrap();
    assert_eq!(
        mac.launched(),
        [launched("open", &["/Users/me/repo"], None, false)]
    );

    let directory = tempfile::tempdir().unwrap();
    let existing = directory.path().to_string_lossy().into_owned();
    let windows = FakeLauncher::new();
    windows.add("explorer.exe");
    shell(&windows, ShellPlatform::Windows)
        .open_path(&existing)
        .unwrap();
    // El Explorador recibe la ruta solo con `\\` (aquí la carpeta temporal es
    // de Linux y trae `/`).
    let for_explorer = existing.replace('/', "\\");
    assert_eq!(
        windows.launched(),
        [launched("explorer.exe", &[&for_explorer], None, true)]
    );
}

#[test]
fn reports_why_a_path_could_not_be_opened() {
    let linux = FakeLauncher::new();
    linux.add("xdg-open").exit_with("xdg-open", 2);
    let error = shell(&linux, ShellPlatform::Linux)
        .open_path("/home/me/gone")
        .unwrap_err();
    assert_eq!(
        error.message(),
        "Failed to open path: The item does not exist"
    );

    let unknown_code = FakeLauncher::new();
    unknown_code.add("xdg-open").exit_with("xdg-open", 7);
    assert_eq!(
        shell(&unknown_code, ShellPlatform::Linux)
            .open_path("/home/me/repo")
            .unwrap_err()
            .message(),
        "Failed to open path: Failed to open path"
    );

    // Sin `xdg-open` instalado.
    let missing_tool = FakeLauncher::new();
    assert_eq!(
        shell(&missing_tool, ShellPlatform::Linux)
            .open_path("/home/me/repo")
            .unwrap_err()
            .message(),
        "Failed to open path: Failed to open path"
    );

    let mac = FakeLauncher::new();
    mac.add("open").exit_with("open", 1);
    assert_eq!(
        shell(&mac, ShellPlatform::MacOs)
            .open_path("/Users/me/gone")
            .unwrap_err()
            .message(),
        "Failed to open path: Failed to open path"
    );

    // En Windows una ruta que no existe ni siquiera llega a `explorer`.
    let windows = FakeLauncher::new();
    windows.add("explorer.exe");
    let directory = tempfile::tempdir().unwrap();
    let gone = directory.path().join("gone").to_string_lossy().into_owned();
    assert_eq!(
        shell(&windows, ShellPlatform::Windows)
            .open_path(&gone)
            .unwrap_err()
            .message(),
        "Failed to open path: Failed to open path"
    );
    assert!(windows.launched().is_empty());
}

// --- shell:showItemInFolder -------------------------------------------------

#[test]
fn shows_an_item_in_its_folder_on_each_platform() {
    let mac = FakeLauncher::new();
    mac.add("open");
    assert!(
        shell(&mac, ShellPlatform::MacOs)
            .show_item_in_folder("/Users/me/repo/a.txt")
            .success
    );
    assert_eq!(
        mac.launched(),
        [launched(
            "open",
            &["-R", "/Users/me/repo/a.txt"],
            None,
            true
        )]
    );

    let windows = FakeLauncher::new();
    windows.add("explorer.exe");
    shell(&windows, ShellPlatform::Windows).show_item_in_folder("C:\\repo\\a file.txt");
    assert_eq!(
        windows.launched(),
        [launched(
            "explorer.exe",
            &["/select,", "C:\\repo\\a file.txt"],
            None,
            true
        )]
    );
}

// Antes la ruta llegaba tal cual: con `/` (como la devuelve git) o mezclada
// (repo + "/" + archivo), el Explorador no encontraba el elemento y abría
// otra carpeta.
#[test]
fn on_windows_the_explorer_gets_backslashes_only() {
    let windows = FakeLauncher::new();
    windows.add("explorer.exe");
    let shell = shell(&windows, ShellPlatform::Windows);
    shell.show_item_in_folder("C:/Users/Ana Torres/repo");
    shell.show_item_in_folder("C:\\Users\\Ana Torres\\repo/src/app.js");
    assert_eq!(
        windows.launched(),
        [
            launched(
                "explorer.exe",
                &["/select,", "C:\\Users\\Ana Torres\\repo"],
                None,
                true
            ),
            launched(
                "explorer.exe",
                &["/select,", "C:\\Users\\Ana Torres\\repo\\src\\app.js"],
                None,
                true
            ),
        ]
    );
}

#[cfg(unix)]
#[test]
fn on_linux_the_file_manager_is_asked_to_select_the_item() {
    let launcher = FakeLauncher::new();
    launcher.add("dbus-send").add("xdg-open");

    let result =
        shell(&launcher, ShellPlatform::Linux).show_item_in_folder("/home/me/my repo/a,b.txt");

    assert!(result.success);
    assert_eq!(
        launcher.launched(),
        [launched(
            "dbus-send",
            &[
                "--session",
                "--print-reply",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
                "array:string:file:///home/me/my%20repo/a%2Cb.txt",
                "string:"
            ],
            None,
            false
        )]
    );
}

#[cfg(unix)]
#[test]
fn on_linux_the_folder_is_opened_when_no_file_manager_answers() {
    // Hay `dbus-send`, pero ningún gestor de archivos atiende la llamada.
    let no_service = FakeLauncher::new();
    no_service
        .add("dbus-send")
        .exit_with("dbus-send", 1)
        .add("xdg-open");
    shell(&no_service, ShellPlatform::Linux).show_item_in_folder("/home/me/repo/a.txt");
    assert_eq!(
        no_service.launched().last(),
        Some(&launched("xdg-open", &["/home/me/repo"], None, true))
    );

    // Ni siquiera hay `dbus-send`.
    let no_dbus = FakeLauncher::new();
    no_dbus.add("xdg-open");
    shell(&no_dbus, ShellPlatform::Linux).show_item_in_folder("/home/me/repo/a.txt");
    assert_eq!(
        no_dbus.launched().last(),
        Some(&launched("xdg-open", &["/home/me/repo"], None, true))
    );

    // Como en el original, el canal responde con éxito aunque nada arranque.
    let nothing = FakeLauncher::new();
    assert!(
        shell(&nothing, ShellPlatform::Linux)
            .show_item_in_folder("/home/me/repo/a.txt")
            .success
    );
}

#[test]
fn the_current_platform_is_one_of_the_three() {
    let platform = ShellPlatform::current();
    assert_eq!(
        platform,
        if cfg!(target_os = "windows") {
            ShellPlatform::Windows
        } else if cfg!(target_os = "macos") {
            ShellPlatform::MacOs
        } else {
            ShellPlatform::Linux
        }
    );
    assert_eq!(Shell::new().platform(), platform);
}
