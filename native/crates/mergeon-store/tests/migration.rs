// Port de `electron/store-migration.test.js`.
//
// Al renombrar la app a Mergeon, Electron cambió de carpeta de datos y la
// configuración anterior (cuentas de GitHub y repos recientes) quedaría atrás.
// La migración corre una sola vez, en el arranque, y se traga cualquier fallo:
// si se rompe, el usuario pierde su configuración sin ver un solo error. Estos
// casos fijan lo que tiene que hacer y, sobre todo, lo que no puede hacer nunca.
//
// El original mockea `fs`; aquí se usa un directorio temporal real.

use std::fs;
use std::path::{Path, PathBuf};

use mergeon_store::{LegacyMigration, migrate_legacy_config};
use tempfile::TempDir;

struct Dirs {
    _root: TempDir,
    app_data: PathBuf,
    user_data: PathBuf,
    new_config: PathBuf,
    legacy_config: PathBuf,
    old_casing_config: PathBuf,
}

fn dirs() -> Dirs {
    let root = tempfile::tempdir().expect("tempdir");
    let app_data = root.path().join("home").join("u").join(".config");
    let user_data = app_data.join("Mergeon");
    fs::create_dir_all(&app_data).expect("app data dir");

    Dirs {
        new_config: user_data.join("config.json"),
        legacy_config: app_data.join("git-gui").join("config.json"),
        old_casing_config: app_data.join("GitGUI").join("config.json"),
        user_data,
        app_data,
        _root: root,
    }
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
    fs::write(path, contents).expect("write");
}

fn migrate(dirs: &Dirs) -> LegacyMigration {
    migrate_legacy_config(Some(&dirs.user_data), Some(&dirs.app_data))
}

#[test]
fn copies_the_previous_configuration_when_the_new_one_does_not_exist_yet() {
    let dirs = dirs();
    write(&dirs.legacy_config, r#"{"from":"git-gui"}"#);

    let outcome = migrate(&dirs);

    assert_eq!(
        outcome,
        LegacyMigration::Copied {
            source: dirs.legacy_config.clone()
        }
    );
    // Crea la carpeta de datos nueva y deja la copia en su sitio.
    assert!(dirs.user_data.is_dir());
    assert_eq!(
        fs::read_to_string(&dirs.new_config).expect("copied"),
        r#"{"from":"git-gui"}"#
    );
    // Es una copia: la configuración anterior sigue donde estaba.
    assert!(dirs.legacy_config.exists());
}

#[test]
fn also_finds_the_folder_with_the_old_casing() {
    let dirs = dirs();
    write(&dirs.old_casing_config, r#"{"from":"GitGUI"}"#);

    let outcome = migrate(&dirs);

    assert_eq!(
        outcome,
        LegacyMigration::Copied {
            source: dirs.old_casing_config.clone()
        }
    );
    assert_eq!(
        fs::read_to_string(&dirs.new_config).expect("copied"),
        r#"{"from":"GitGUI"}"#
    );
}

#[test]
fn never_overwrites_a_configuration_that_already_exists() {
    let dirs = dirs();
    write(&dirs.new_config, r#"{"from":"Mergeon"}"#);
    write(&dirs.legacy_config, r#"{"from":"git-gui"}"#);

    let outcome = migrate(&dirs);

    assert_eq!(outcome, LegacyMigration::TargetExists);
    assert_eq!(
        fs::read_to_string(&dirs.new_config).expect("untouched"),
        r#"{"from":"Mergeon"}"#
    );
}

#[test]
fn does_nothing_when_there_is_nothing_to_migrate() {
    let dirs = dirs();

    let outcome = migrate(&dirs);

    assert_eq!(outcome, LegacyMigration::NothingToMigrate);
    assert!(!dirs.new_config.exists());
    assert!(!dirs.user_data.exists());
}

#[test]
fn copies_only_the_first_legacy_folder_it_finds() {
    let dirs = dirs();
    write(&dirs.legacy_config, r#"{"from":"git-gui"}"#);
    write(&dirs.old_casing_config, r#"{"from":"GitGUI"}"#);

    let outcome = migrate(&dirs);

    assert_eq!(
        outcome,
        LegacyMigration::Copied {
            source: dirs.legacy_config.clone()
        }
    );
    assert_eq!(
        fs::read_to_string(&dirs.new_config).expect("copied"),
        r#"{"from":"git-gui"}"#
    );
}

#[test]
fn stays_quiet_when_there_are_still_no_paths_to_use() {
    let dirs = dirs();
    write(&dirs.legacy_config, r#"{"from":"git-gui"}"#);

    assert_eq!(
        migrate_legacy_config(None, None),
        LegacyMigration::MissingPaths
    );
    assert_eq!(
        migrate_legacy_config(Some(&dirs.user_data), None),
        LegacyMigration::MissingPaths
    );
    assert_eq!(
        migrate_legacy_config(None, Some(&dirs.app_data)),
        LegacyMigration::MissingPaths
    );
    assert!(!dirs.new_config.exists());
}

#[test]
fn does_not_break_the_startup_when_the_copy_fails() {
    let dirs = dirs();
    write(&dirs.legacy_config, r#"{"from":"git-gui"}"#);
    // Un archivo donde debería ir la carpeta de datos: no se puede crear ni
    // copiar dentro, en cualquier plataforma.
    fs::write(&dirs.user_data, "not a directory").expect("blocker");

    let outcome = migrate(&dirs);

    match outcome {
        LegacyMigration::Failed { source, message } => {
            assert_eq!(source, dirs.legacy_config);
            assert!(!message.is_empty());
        }
        other => panic!("expected a failed migration, got {other:?}"),
    }
    assert_eq!(
        fs::read_to_string(&dirs.user_data).expect("blocker untouched"),
        "not a directory"
    );
}
