//! Comprueba un manifiesto de actualización (`latest*.yml`) con el mismo código
//! que usará la app instalada: `mergeon_updater::parse_update_manifest`, la
//! elección del artefacto por plataforma y la verificación del sha512.
//!
//! Lo usa el pipeline de release antes de subir nada (binario
//! `mergeon-manifest-check`) y los tests de `tests/`, que ejecutan el generador
//! `native/packaging/update-manifest.py`.

use std::fs;
use std::path::{Path, PathBuf};

use mergeon_updater::{
    Arch, ArtifactKind, Platform, UpdateFile, UpdateManifest, Version, channel_file_name,
    parse_update_manifest, select_artifact, sha512_base64_of_file, sha512_matches,
    sign_manifest_bytes, signature_file_name, verify_manifest_signature,
};

/// Resultado de una comprobación correcta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub manifest: UpdateManifest,
    /// Artefacto que descargaría el actualizador en esa plataforma.
    pub selected: UpdateFile,
    /// Archivos locales verificados (tamaño y sha512).
    pub verified_files: Vec<PathBuf>,
}

/// Formato que el actualizador instala en cada plataforma (en Linux, el
/// AppImage: es el caso en que `APPIMAGE` está definida).
pub fn artifact_kind_for(platform: &Platform) -> Option<ArtifactKind> {
    match platform {
        Platform::Win32 => Some(ArtifactKind::Nsis),
        Platform::Linux => Some(ArtifactKind::AppImage),
        Platform::Darwin => Some(ArtifactKind::MacZip),
        Platform::Other(_) => None,
    }
}

/// Plataforma a partir de su nombre de Node o del que usan los scripts.
pub fn parse_platform(name: &str) -> Option<Platform> {
    match name {
        "linux" => Some(Platform::Linux),
        "windows" | "win32" => Some(Platform::Win32),
        "mac" | "macos" | "darwin" => Some(Platform::Darwin),
        _ => None,
    }
}

pub fn parse_arch(name: &str) -> Option<Arch> {
    match name {
        "x64" | "x86_64" | "amd64" => Some(Arch::X64),
        "arm64" | "aarch64" => Some(Arch::Arm64),
        _ => None,
    }
}

fn is_remote(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://") || url.starts_with('/')
}

/// Comprueba el manifiesto `manifest_path` contra los artefactos de
/// `artifacts_dir`. Devuelve todos los problemas encontrados, no solo el primero.
pub fn check_manifest(
    manifest_path: &Path,
    artifacts_dir: &Path,
    platform: &Platform,
    arch: &Arch,
    expected_version: Option<&str>,
) -> Result<Checked, Vec<String>> {
    let text = fs::read_to_string(manifest_path).map_err(|error| {
        vec![format!(
            "no se pudo leer {}: {error}",
            manifest_path.display()
        )]
    })?;
    let manifest = parse_update_manifest(&text)
        .map_err(|error| vec![format!("el actualizador rechaza el manifiesto: {error:?}")])?;

    let mut problems = Vec::new();

    let expected_name = channel_file_name("latest", platform, arch);
    let actual_name = manifest_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if actual_name != expected_name {
        problems.push(format!(
            "el actualizador pedirá `{expected_name}` en {}/{}, no `{actual_name}`",
            platform.as_str(),
            arch.as_str()
        ));
    }

    if Version::parse(&manifest.version).is_none() {
        problems.push(format!(
            "`version: {}` no es un semver válido",
            manifest.version
        ));
    }
    if let Some(expected) = expected_version
        && manifest.version != expected
    {
        problems.push(format!(
            "el manifiesto dice {} y se esperaba {expected}",
            manifest.version
        ));
    }
    if manifest.release_date.is_none() {
        problems.push("falta `releaseDate`".to_string());
    }

    let files = match manifest.resolved_files() {
        Ok(files) => files,
        Err(error) => {
            problems.push(format!("sin archivos: {error:?}"));
            return Err(problems);
        }
    };

    let mut verified_files = Vec::new();
    for file in &files {
        let Some(sha512) = file.sha512.as_deref() else {
            problems.push(format!("`{}` no trae sha512", file.url));
            continue;
        };
        if is_remote(&file.url) {
            continue;
        }
        let local = artifacts_dir.join(&file.url);
        let metadata = match fs::metadata(&local) {
            Ok(metadata) if metadata.is_file() => metadata,
            _ => {
                problems.push(format!("no existe el artefacto {}", local.display()));
                continue;
            }
        };
        match file.size {
            Some(size) if size == metadata.len() => {}
            Some(size) => problems.push(format!(
                "`{}`: el manifiesto dice {size} bytes y el archivo tiene {}",
                file.url,
                metadata.len()
            )),
            None => problems.push(format!("`{}` no trae `size`", file.url)),
        }
        match sha512_base64_of_file(&local) {
            Ok(actual) if sha512_matches(sha512, &actual) => verified_files.push(local),
            Ok(actual) => problems.push(format!(
                "`{}`: sha512 del manifiesto {sha512}, del archivo {actual}",
                file.url
            )),
            Err(error) => problems.push(format!("no se pudo leer {}: {error}", local.display())),
        }
    }

    let Some(kind) = artifact_kind_for(platform) else {
        problems.push(format!(
            "el actualizador no instala nada en {}",
            platform.as_str()
        ));
        return Err(problems);
    };
    let selected = match select_artifact(&files, kind, platform, arch) {
        Ok(selected) => selected.clone(),
        Err(error) => {
            problems.push(format!(
                "ningún archivo sirve para {}/{}: {error:?}",
                platform.as_str(),
                arch.as_str()
            ));
            return Err(problems);
        }
    };
    let extension = format!(".{}", kind.extension()).to_ascii_lowercase();
    if !selected.url.to_ascii_lowercase().ends_with(&extension) {
        problems.push(format!(
            "el actualizador elegiría `{}`, que no es un {}",
            selected.url,
            kind.extension()
        ));
    }

    // Los campos antiguos `path` / `sha512` (los lee electron-updater) deben
    // describir el primer archivo.
    match (&manifest.path, files.first()) {
        (Some(path), Some(first)) if *path == first.url => {
            if manifest.sha512 != first.sha512 {
                problems.push("el `sha512` raíz no es el del archivo de `path`".to_string());
            }
        }
        (Some(path), _) => {
            problems.push(format!("`path: {path}` no es el primer archivo de `files`"))
        }
        (None, _) => problems.push("falta `path`".to_string()),
    }

    if problems.is_empty() {
        Ok(Checked {
            manifest,
            selected,
            verified_files,
        })
    } else {
        Err(problems)
    }
}

/// Firma `manifest_path` con `private_key_b64` (Ed25519 PKCS#8 o seed en base64)
/// y escribe `<manifest_path>.sig`.
pub fn sign_manifest_file(manifest_path: &Path, private_key_b64: &str) -> Result<PathBuf, String> {
    let bytes = fs::read(manifest_path)
        .map_err(|error| format!("no se pudo leer {}: {error}", manifest_path.display()))?;
    let signature = sign_manifest_bytes(&bytes, private_key_b64)?;
    let file_name = manifest_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| format!("ruta inválida: {}", manifest_path.display()))?;
    let sig_path = manifest_path.with_file_name(signature_file_name(&file_name));
    fs::write(&sig_path, format!("{signature}\n"))
        .map_err(|error| format!("no se pudo escribir {}: {error}", sig_path.display()))?;
    Ok(sig_path)
}

/// Verifica que `<manifest_path>.sig` sea una firma Ed25519 válida bajo `public_key_b64`.
pub fn verify_manifest_file_signature(
    manifest_path: &Path,
    public_key_b64: &str,
) -> Result<PathBuf, String> {
    let bytes = fs::read(manifest_path)
        .map_err(|error| format!("no se pudo leer {}: {error}", manifest_path.display()))?;
    let file_name = manifest_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| format!("ruta inválida: {}", manifest_path.display()))?;
    let sig_path = manifest_path.with_file_name(signature_file_name(&file_name));
    let signature = fs::read_to_string(&sig_path)
        .map_err(|error| format!("no se pudo leer {}: {error}", sig_path.display()))?;
    verify_manifest_signature(&bytes, &signature, public_key_b64)
        .map_err(|error| format!("firma inválida en {}: {error:?}", sig_path.display()))?;
    Ok(sig_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mergeon_updater::generate_signing_keypair;

    #[test]
    fn signs_and_verifies_a_manifest_file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("latest-linux.yml");
        fs::write(&manifest, "version: 0.14.0\n").unwrap();

        let (priv_key, pub_key) = generate_signing_keypair().unwrap();
        let sig_path = sign_manifest_file(&manifest, &priv_key).unwrap();
        assert_eq!(sig_path, dir.path().join("latest-linux.yml.sig"));
        verify_manifest_file_signature(&manifest, &pub_key).unwrap();

        // Alterar el manifiesto rompe la verificación.
        fs::write(&manifest, "version: 0.14.1\n").unwrap();
        assert!(verify_manifest_file_signature(&manifest, &pub_key).is_err());
    }
}
