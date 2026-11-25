//! El generador `native/packaging/update-manifest.py` contra el parser real del
//! actualizador: lo que publica el pipeline tiene que poder leerlo la app.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use mergeon_manifest_check::check_manifest;
use mergeon_updater::{
    Arch, ArtifactKind, Platform, parse_update_manifest, select_artifact, sha512_base64,
};

fn generator() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../update-manifest.py")
}

/// Intérprete de Python: `PYTHON`, o el primero de `python3` / `python` que exista.
fn python() -> String {
    if let Ok(explicit) = std::env::var("PYTHON")
        && !explicit.is_empty()
    {
        return explicit;
    }
    for candidate in ["python3", "python"] {
        let works = Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success());
        if works {
            return candidate.to_string();
        }
    }
    panic!("hace falta Python 3 (python3 o python en el PATH, o la variable PYTHON)");
}

fn run_generator(args: &[&str]) -> std::process::Output {
    Command::new(python())
        .arg(generator())
        .args(args)
        .env_remove("SOURCE_DATE_EPOCH")
        .output()
        .expect("no se pudo ejecutar el generador")
}

fn generate(args: &[&str]) {
    let output = run_generator(args);
    assert!(
        output.status.success(),
        "el generador falló: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write(dir: &Path, name: &str, contents: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, contents).unwrap();
    path
}

fn dir_arg(dir: &Path) -> &str {
    dir.to_str().expect("ruta temporal UTF-8")
}

#[test]
fn linux_manifest_lists_the_appimage_first_and_the_deb() {
    let dist = tempfile::tempdir().unwrap();
    let appimage: Vec<u8> = (0..200_000u32).map(|n| (n % 251) as u8).collect();
    write(dist.path(), "Mergeon-0.9.1.AppImage", &appimage);
    write(dist.path(), "mergeon_0.9.1_amd64.deb", b"deb package");
    // Artefactos de otra versión y de otra plataforma: no entran.
    write(dist.path(), "Mergeon-0.9.10.AppImage", b"other version");
    write(dist.path(), "Mergeon-Setup-0.9.1.exe", b"windows");

    generate(&[
        "--platform",
        "linux",
        "--version",
        "0.9.1",
        "--dir",
        dir_arg(dist.path()),
        "--release-date",
        "2026-09-30T18:21:04.512Z",
    ]);

    let manifest_path = dist.path().join("latest-linux.yml");
    let text = fs::read_to_string(&manifest_path).unwrap();
    let manifest = parse_update_manifest(&text).unwrap();

    assert_eq!(manifest.version, "0.9.1");
    assert_eq!(
        manifest
            .files
            .iter()
            .map(|file| file.url.as_str())
            .collect::<Vec<_>>(),
        ["Mergeon-0.9.1.AppImage", "mergeon_0.9.1_amd64.deb"]
    );
    assert_eq!(manifest.files[0].size, Some(200_000));
    assert_eq!(
        manifest.files[0].sha512.as_deref(),
        Some(sha512_base64(&appimage).as_str())
    );
    assert_eq!(manifest.files[1].size, Some(11));
    assert_eq!(
        manifest.files[1].sha512.as_deref(),
        Some(sha512_base64(b"deb package").as_str())
    );
    assert_eq!(manifest.path.as_deref(), Some("Mergeon-0.9.1.AppImage"));
    assert_eq!(manifest.sha512, manifest.files[0].sha512);
    assert_eq!(
        manifest.release_date.as_deref(),
        Some("2026-09-30T18:21:04.512Z")
    );

    // Mismo texto, byte a byte, que escribiría electron-builder.
    let expected = format!(
        "version: 0.9.1\nfiles:\n  - url: Mergeon-0.9.1.AppImage\n    sha512: {a}\n    size: 200000\n  - url: mergeon_0.9.1_amd64.deb\n    sha512: {d}\n    size: 11\npath: Mergeon-0.9.1.AppImage\nsha512: {a}\nreleaseDate: '2026-09-30T18:21:04.512Z'\n",
        a = sha512_base64(&appimage),
        d = sha512_base64(b"deb package"),
    );
    assert_eq!(text, expected);

    let checked = check_manifest(
        &manifest_path,
        dist.path(),
        &Platform::Linux,
        &Arch::X64,
        Some("0.9.1"),
    )
    .unwrap();
    assert_eq!(checked.selected.url, "Mergeon-0.9.1.AppImage");
    assert_eq!(checked.verified_files.len(), 2);
}

#[test]
fn linux_arm64_artifacts_get_their_own_channel_file() {
    let dist = tempfile::tempdir().unwrap();
    write(dist.path(), "Mergeon-0.9.1.AppImage", b"x64");
    write(dist.path(), "mergeon_0.9.1_amd64.deb", b"x64 deb");
    write(dist.path(), "Mergeon-0.9.1-arm64.AppImage", b"arm64");
    write(dist.path(), "mergeon_0.9.1_arm64.deb", b"arm64 deb");

    generate(&[
        "--platform",
        "linux",
        "--version",
        "0.9.1",
        "--dir",
        dir_arg(dist.path()),
    ]);

    let x64 = check_manifest(
        &dist.path().join("latest-linux.yml"),
        dist.path(),
        &Platform::Linux,
        &Arch::X64,
        Some("0.9.1"),
    )
    .unwrap();
    assert_eq!(x64.selected.url, "Mergeon-0.9.1.AppImage");
    assert_eq!(x64.manifest.files.len(), 2);

    let arm64 = check_manifest(
        &dist.path().join("latest-linux-arm64.yml"),
        dist.path(),
        &Platform::Linux,
        &Arch::Arm64,
        Some("0.9.1"),
    )
    .unwrap();
    assert_eq!(arm64.selected.url, "Mergeon-0.9.1-arm64.AppImage");
    assert_eq!(arm64.manifest.files.len(), 2);
}

#[test]
fn windows_manifest_points_at_the_nsis_installer() {
    let dist = tempfile::tempdir().unwrap();
    write(dist.path(), "Mergeon-Setup-1.2.3.exe", b"MZ installer");
    write(dist.path(), "Mergeon-1.2.3.AppImage", b"linux");

    generate(&[
        "--platform",
        "windows",
        "--version",
        "1.2.3",
        "--dir",
        dir_arg(dist.path()),
    ]);

    let checked = check_manifest(
        &dist.path().join("latest.yml"),
        dist.path(),
        &Platform::Win32,
        &Arch::X64,
        Some("1.2.3"),
    )
    .unwrap();
    assert_eq!(checked.manifest.files.len(), 1);
    assert_eq!(checked.selected.url, "Mergeon-Setup-1.2.3.exe");
    assert_eq!(
        checked.selected.sha512.as_deref(),
        Some(sha512_base64(b"MZ installer").as_str())
    );
    assert_eq!(checked.selected.size, Some(12));
    // Sin --release-date la fecha es la actual, en el formato de toISOString().
    let date = checked.manifest.release_date.unwrap();
    assert_eq!(date.len(), "2026-09-30T18:21:04.512Z".len(), "{date}");
    assert!(date.ends_with('Z') && date.contains('T'), "{date}");
}

#[test]
fn mac_manifest_serves_each_architecture_its_zip() {
    // Un zip por arquitectura: cada Mac recibe el suyo.
    let dist = tempfile::tempdir().unwrap();
    write(dist.path(), "Mergeon-0.9.1-arm64-mac.zip", b"arm64 zip");
    write(dist.path(), "Mergeon-0.9.1-mac.zip", b"x64 zip");
    generate(&[
        "--platform",
        "mac",
        "--version",
        "0.9.1",
        "--dir",
        dir_arg(dist.path()),
    ]);
    let manifest_path = dist.path().join("latest-mac.yml");
    let arm64 = check_manifest(
        &manifest_path,
        dist.path(),
        &Platform::Darwin,
        &Arch::Arm64,
        Some("0.9.1"),
    )
    .unwrap();
    assert_eq!(arm64.selected.url, "Mergeon-0.9.1-arm64-mac.zip");
    let x64 = check_manifest(
        &manifest_path,
        dist.path(),
        &Platform::Darwin,
        &Arch::X64,
        Some("0.9.1"),
    )
    .unwrap();
    assert_eq!(x64.selected.url, "Mergeon-0.9.1-mac.zip");

    // Un único zip universal sirve a las dos.
    let universal = tempfile::tempdir().unwrap();
    write(
        universal.path(),
        "Mergeon-0.9.1-universal-mac.zip",
        b"universal zip",
    );
    generate(&[
        "--platform",
        "mac",
        "--version",
        "0.9.1",
        "--dir",
        dir_arg(universal.path()),
    ]);
    for arch in [Arch::Arm64, Arch::X64] {
        let checked = check_manifest(
            &universal.path().join("latest-mac.yml"),
            universal.path(),
            &Platform::Darwin,
            &arch,
            Some("0.9.1"),
        )
        .unwrap();
        assert_eq!(checked.selected.url, "Mergeon-0.9.1-universal-mac.zip");
    }

    // Solo el zip arm64 (lo que publica el CI por defecto): un Mac Intel no
    // tiene nada que instalar y el actualizador lo dice en vez de bajarlo.
    let arm_only = tempfile::tempdir().unwrap();
    write(arm_only.path(), "Mergeon-0.9.1-arm64-mac.zip", b"arm64 zip");
    generate(&[
        "--platform",
        "mac",
        "--version",
        "0.9.1",
        "--dir",
        dir_arg(arm_only.path()),
    ]);
    let text = fs::read_to_string(arm_only.path().join("latest-mac.yml")).unwrap();
    let manifest = parse_update_manifest(&text).unwrap();
    assert!(
        select_artifact(
            &manifest.files,
            ArtifactKind::MacZip,
            &Platform::Darwin,
            &Arch::X64
        )
        .is_err()
    );
    assert_eq!(
        select_artifact(
            &manifest.files,
            ArtifactKind::MacZip,
            &Platform::Darwin,
            &Arch::Arm64
        )
        .unwrap()
        .url,
        "Mergeon-0.9.1-arm64-mac.zip"
    );
}

#[test]
fn explicit_files_with_awkward_names_survive_the_round_trip() {
    let dist = tempfile::tempdir().unwrap();
    let first = write(dist.path(), "Mergeon Setup 2.0.0-beta.1 'ñ'.exe", b"one");
    let second = write(dist.path(), "true", b"two");
    let output = dist.path().join("feed").join("latest.yml");

    generate(&[
        "--version",
        "2.0.0-beta.1",
        "--output",
        output.to_str().unwrap(),
        first.to_str().unwrap(),
        second.to_str().unwrap(),
    ]);

    let manifest = parse_update_manifest(&fs::read_to_string(&output).unwrap()).unwrap();
    assert_eq!(manifest.version, "2.0.0-beta.1");
    assert_eq!(manifest.files[0].url, "Mergeon Setup 2.0.0-beta.1 'ñ'.exe");
    assert_eq!(manifest.files[1].url, "true");
    assert_eq!(
        manifest.path.as_deref(),
        Some("Mergeon Setup 2.0.0-beta.1 'ñ'.exe")
    );
    assert_eq!(
        manifest.files[1].sha512.as_deref(),
        Some(sha512_base64(b"two").as_str())
    );
}

#[test]
fn honours_source_date_epoch_for_reproducible_manifests() {
    let dist = tempfile::tempdir().unwrap();
    write(dist.path(), "Mergeon-0.9.1.AppImage", b"appimage");
    let output = Command::new(python())
        .arg(generator())
        .args([
            "--platform",
            "linux",
            "--version",
            "0.9.1",
            "--dir",
            dir_arg(dist.path()),
        ])
        .env("SOURCE_DATE_EPOCH", "1790792464")
        .output()
        .unwrap();
    assert!(output.status.success());

    let text = fs::read_to_string(dist.path().join("latest-linux.yml")).unwrap();
    let manifest = parse_update_manifest(&text).unwrap();
    assert_eq!(
        manifest.release_date.as_deref(),
        Some("2026-09-30T18:21:04.000Z")
    );
}

#[test]
fn generator_fails_when_there_is_nothing_to_publish() {
    let dist = tempfile::tempdir().unwrap();
    write(dist.path(), "Mergeon-0.9.0.AppImage", b"old");
    let output = run_generator(&[
        "--platform",
        "linux",
        "--version",
        "0.9.1",
        "--dir",
        dir_arg(dist.path()),
    ]);
    assert!(!output.status.success());
    assert!(!dist.path().join("latest-linux.yml").exists());
}

#[test]
fn check_reports_tampered_or_missing_artifacts() {
    let dist = tempfile::tempdir().unwrap();
    let appimage = write(dist.path(), "Mergeon-0.9.1.AppImage", b"appimage");
    write(dist.path(), "mergeon_0.9.1_amd64.deb", b"deb");
    generate(&[
        "--platform",
        "linux",
        "--version",
        "0.9.1",
        "--dir",
        dir_arg(dist.path()),
    ]);
    let manifest_path = dist.path().join("latest-linux.yml");
    let check = |version: &str, platform: Platform| {
        check_manifest(
            &manifest_path,
            dist.path(),
            &platform,
            &Arch::X64,
            Some(version),
        )
    };
    assert!(check("0.9.1", Platform::Linux).is_ok());

    // Versión que no es la esperada, o manifiesto de otra plataforma.
    let problems = check("0.9.2", Platform::Linux).unwrap_err();
    assert!(problems.iter().any(|p| p.contains("0.9.2")), "{problems:?}");
    let problems = check("0.9.1", Platform::Win32).unwrap_err();
    assert!(
        problems.iter().any(|p| p.contains("latest.yml")),
        "{problems:?}"
    );

    // El artefacto cambió después de generar el manifiesto (mismo tamaño).
    fs::write(&appimage, b"APPIMAGE").unwrap();
    let problems = check("0.9.1", Platform::Linux).unwrap_err();
    assert!(
        problems.iter().any(|p| p.contains("sha512")),
        "{problems:?}"
    );

    // Otro tamaño.
    fs::write(&appimage, b"bigger appimage").unwrap();
    let problems = check("0.9.1", Platform::Linux).unwrap_err();
    assert!(problems.iter().any(|p| p.contains("bytes")), "{problems:?}");

    fs::remove_file(&appimage).unwrap();
    let problems = check("0.9.1", Platform::Linux).unwrap_err();
    assert!(
        problems.iter().any(|p| p.contains("no existe")),
        "{problems:?}"
    );
}
