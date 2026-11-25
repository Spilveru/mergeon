//! `mergeon-manifest-check <manifiesto> --platform <linux|windows|mac>
//!      [--arch <x64|arm64>] [--dir <artefactos>] [--version <x.y.z>]`
//!
//! Sale con código 1 si el actualizador no podría usar el manifiesto.

use std::path::PathBuf;
use std::process::ExitCode;

use mergeon_manifest_check::{
    check_manifest, parse_arch, parse_platform, sign_manifest_file, verify_manifest_file_signature,
};
use mergeon_updater::{Arch, generate_signing_keypair};

const USAGE: &str = "uso:\n  mergeon-manifest-check <manifiesto.yml> --platform <linux|windows|mac> \
[--arch <x64|arm64>] [--dir <directorio de artefactos>] [--version <x.y.z>] [--public-key <base64>]\n  \
mergeon-manifest-check sign <manifiesto.yml>   (lee MERGEON_UPDATE_SIGNING_KEY)\n  \
mergeon-manifest-check keygen                  (genera un par Ed25519)";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).peekable();
    match args.peek().map(String::as_str) {
        Some("keygen") => {
            return match generate_signing_keypair() {
                Ok((private_key, public_key)) => {
                    println!("MERGEON_UPDATE_SIGNING_KEY (GitHub Secret):\n{private_key}\n");
                    println!("MERGEON_UPDATE_PUBLIC_KEY (GitHub Variable):\n{public_key}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    ExitCode::FAILURE
                }
            };
        }
        Some("sign") => {
            args.next();
            let Some(path) = args.next() else {
                eprintln!("error: falta la ruta del manifiesto\n{USAGE}");
                return ExitCode::from(2);
            };
            let private_key = match std::env::var("MERGEON_UPDATE_SIGNING_KEY") {
                Ok(key) if !key.trim().is_empty() => key,
                _ => {
                    eprintln!("error: falta la variable MERGEON_UPDATE_SIGNING_KEY");
                    return ExitCode::from(2);
                }
            };
            return match sign_manifest_file(&PathBuf::from(path), &private_key) {
                Ok(sig_path) => {
                    println!("firma escrita en {}", sig_path.display());
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    ExitCode::FAILURE
                }
            };
        }
        _ => {}
    }

    let mut manifest = None;
    let mut platform = None;
    let mut arch = Arch::X64;
    let mut dir = None;
    let mut version = None;
    let mut public_key = None;

    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .ok_or_else(|| format!("{name} necesita un valor"))
        };
        let parsed = match arg.as_str() {
            "--platform" => value("--platform").and_then(|name| {
                parse_platform(&name)
                    .map(|parsed| platform = Some(parsed))
                    .ok_or_else(|| format!("plataforma desconocida: {name}"))
            }),
            "--arch" => value("--arch").and_then(|name| {
                parse_arch(&name)
                    .map(|parsed| arch = parsed)
                    .ok_or_else(|| format!("arquitectura desconocida: {name}"))
            }),
            "--dir" => value("--dir").map(|path| dir = Some(PathBuf::from(path))),
            "--version" => value("--version").map(|text| version = Some(text)),
            "--public-key" => value("--public-key").map(|key| public_key = Some(key)),
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other if other.starts_with('-') => Err(format!("opción desconocida: {other}")),
            other => {
                manifest = Some(PathBuf::from(other));
                Ok(())
            }
        };
        if let Err(message) = parsed {
            eprintln!("error: {message}\n{USAGE}");
            return ExitCode::from(2);
        }
    }

    let (Some(manifest), Some(platform)) = (manifest, platform) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let dir = dir.unwrap_or_else(|| {
        manifest
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    });

    match check_manifest(&manifest, &dir, &platform, &arch, version.as_deref()) {
        Ok(checked) => {
            if let Some(key) = public_key.as_deref().filter(|key| !key.trim().is_empty())
                && let Err(error) = verify_manifest_file_signature(&manifest, key)
            {
                eprintln!("{}: {error}", manifest.display());
                return ExitCode::FAILURE;
            }
            println!(
                "{}: versión {}, {} archivo(s) verificados; {}/{} descargará {}",
                manifest.display(),
                checked.manifest.version,
                checked.verified_files.len(),
                platform.as_str(),
                arch.as_str(),
                checked.selected.url
            );
            ExitCode::SUCCESS
        }
        Err(problems) => {
            eprintln!("{}: manifiesto no válido", manifest.display());
            for problem in problems {
                eprintln!("  - {problem}");
            }
            ExitCode::FAILURE
        }
    }
}
