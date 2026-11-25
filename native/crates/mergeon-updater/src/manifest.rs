//! Manifiesto del canal (`latest.yml`, `latest-linux.yml`, `latest-mac.yml`)
//! con el formato que publica electron-builder, y elección del artefacto.

use crate::error::UpdaterError;
use crate::platform::{Arch, Platform};
use crate::yaml::{self, Yaml};

/// Tamaño máximo aceptado para un manifiesto: son unos cientos de bytes.
pub const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateFile {
    /// Nombre de archivo relativo al feed, o URL absoluta.
    pub url: String,
    /// sha512 del archivo en base64.
    pub sha512: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateManifest {
    pub version: String,
    pub files: Vec<UpdateFile>,
    /// Campo antiguo de electron-builder: el archivo principal.
    pub path: Option<String>,
    pub sha512: Option<String>,
    pub release_date: Option<String>,
    pub release_name: Option<String>,
    pub release_notes: Option<String>,
}

impl UpdateManifest {
    /// `getFileList` de electron-updater: `files` si viene, si no el par
    /// `path` + `sha512` de los manifiestos antiguos.
    pub fn resolved_files(&self) -> Result<Vec<UpdateFile>, UpdaterError> {
        if !self.files.is_empty() {
            return Ok(self.files.clone());
        }
        match &self.path {
            Some(path) => Ok(vec![UpdateFile {
                url: path.clone(),
                sha512: self.sha512.clone(),
                size: None,
            }]),
            None => Err(UpdaterError::NoFiles),
        }
    }
}

fn invalid(detail: impl Into<String>) -> UpdaterError {
    UpdaterError::InvalidManifest {
        detail: detail.into(),
    }
}

fn optional_string(root: &Yaml, key: &str) -> Option<String> {
    root.get(key)
        .and_then(Yaml::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn parse_update_manifest(text: &str) -> Result<UpdateManifest, UpdaterError> {
    let root = yaml::parse(text).map_err(invalid)?;
    if !matches!(root, Yaml::Map(_)) {
        return Err(invalid("the manifest is not a YAML mapping"));
    }

    let version = optional_string(&root, "version").ok_or_else(|| invalid("missing `version`"))?;

    let files = match root.get("files") {
        None | Some(Yaml::Null) => Vec::new(),
        Some(Yaml::List(items)) => items
            .iter()
            .map(|item| {
                if !matches!(item, Yaml::Map(_)) {
                    return Err(invalid("`files` entries must be mappings"));
                }
                let url = optional_string(item, "url")
                    .ok_or_else(|| invalid("a `files` entry has no `url`"))?;
                let size = match item.get("size") {
                    None | Some(Yaml::Null) => None,
                    Some(value) => Some(
                        value
                            .as_str()
                            .and_then(|text| text.trim().parse::<u64>().ok())
                            .ok_or_else(|| invalid("a `files` entry has an invalid `size`"))?,
                    ),
                };
                Ok(UpdateFile {
                    url,
                    sha512: optional_string(item, "sha512"),
                    size,
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(invalid("`files` must be a list")),
    };

    Ok(UpdateManifest {
        version,
        files,
        path: optional_string(&root, "path"),
        sha512: optional_string(&root, "sha512"),
        release_date: optional_string(&root, "releaseDate"),
        release_name: optional_string(&root, "releaseName"),
        release_notes: root
            .get("releaseNotes")
            .and_then(Yaml::as_str)
            .map(str::to_string),
    })
}

/// Nombre del manifiesto del canal para la plataforma, con la convención de
/// electron-builder: `latest.yml` (Windows), `latest-mac.yml`,
/// `latest-linux.yml` y `latest-linux-<arch>.yml` cuando no es x64.
pub fn channel_file_name(channel: &str, platform: &Platform, arch: &Arch) -> String {
    let suffix = match platform {
        Platform::Linux => match arch {
            Arch::X64 => "-linux".to_string(),
            other => format!("-linux-{}", other.as_str()),
        },
        Platform::Darwin => "-mac".to_string(),
        _ => String::new(),
    };
    format!("{channel}{suffix}.yml")
}

/// Formato de instalación que el actualizador sabe aplicar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    /// Instalador NSIS de Windows (`.exe`).
    Nsis,
    /// AppImage de Linux.
    AppImage,
    /// Zip de macOS con el bundle `.app`.
    MacZip,
}

impl ArtifactKind {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Nsis => "exe",
            Self::AppImage => "AppImage",
            Self::MacZip => "zip",
        }
    }

    /// Extensiones que nunca valen como alternativa si no aparece la buscada.
    fn excluded_extensions(self) -> &'static [&'static str] {
        match self {
            Self::Nsis => &[],
            Self::AppImage => &["rpm", "deb", "pacman"],
            Self::MacZip => &["pkg", "dmg"],
        }
    }
}

/// Formato instalable en esta plataforma, si lo hay. Linux solo se actualiza
/// solo cuando corre como AppImage (variable `APPIMAGE`); el `.deb` no.
pub fn resolve_artifact_kind(
    platform: &Platform,
    env: &crate::platform::Env,
) -> Option<ArtifactKind> {
    match platform {
        Platform::Win32 => Some(ArtifactKind::Nsis),
        Platform::Linux if env.is_truthy("APPIMAGE") => Some(ArtifactKind::AppImage),
        Platform::Darwin => Some(ArtifactKind::MacZip),
        _ => None,
    }
}

/// Ruta de la URL sin query ni fragmento, en minúsculas.
fn url_path_lower(url: &str) -> String {
    let end = url.find(['?', '#']).unwrap_or(url.len());
    url[..end].to_ascii_lowercase()
}

fn has_extension(file: &UpdateFile, extension: &str) -> bool {
    url_path_lower(&file.url).ends_with(&format!(".{}", extension.to_ascii_lowercase()))
}

/// `findFile` de electron-updater más el filtro de arquitectura de `MacUpdater`.
pub fn select_artifact<'a>(
    files: &'a [UpdateFile],
    kind: ArtifactKind,
    platform: &Platform,
    arch: &Arch,
) -> Result<&'a UpdateFile, UpdaterError> {
    if files.is_empty() {
        return Err(UpdaterError::NoFiles);
    }

    let mut candidates: Vec<&UpdateFile> = files.iter().collect();

    if *platform == Platform::Darwin {
        // `latest-mac.yml` lista los zip de las dos arquitecturas: en Apple
        // Silicon se prefieren los arm64 si existen; en Intel se descartan.
        let is_arm64 = |file: &&UpdateFile| url_path_lower(&file.url).contains("arm64");
        if *arch == Arch::Arm64 && candidates.iter().any(is_arm64) {
            candidates.retain(is_arm64);
        } else {
            candidates.retain(|file| !is_arm64(file));
        }
    }

    let wanted = kind.extension();
    if let Some(found) = candidates
        .iter()
        .copied()
        .find(|file| has_extension(file, wanted))
    {
        return Ok(found);
    }

    let no_suitable = || UpdaterError::NoSuitableArtifact {
        wanted: wanted.to_string(),
    };

    let excluded = kind.excluded_extensions();
    if excluded.is_empty() {
        return candidates.first().copied().ok_or_else(no_suitable);
    }
    candidates
        .into_iter()
        .find(|file| {
            !excluded
                .iter()
                .any(|extension| has_extension(file, extension))
        })
        .ok_or_else(no_suitable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Env;

    const WINDOWS_MANIFEST: &str = "version: 0.9.1\nfiles:\n  - url: Mergeon-Setup-0.9.1.exe\n    sha512: VkJl1gDqcBHYbYhMb0HRI3iINmr3wZtvqjKb1Wn1nvFYy0E0p5pYFGbhzsQdEu5BFs3wBKi1+TrbRPLpNDY3hg==\n    size: 84513291\npath: Mergeon-Setup-0.9.1.exe\nsha512: VkJl1gDqcBHYbYhMb0HRI3iINmr3wZtvqjKb1Wn1nvFYy0E0p5pYFGbhzsQdEu5BFs3wBKi1+TrbRPLpNDY3hg==\nreleaseDate: '2026-09-30T18:21:04.512Z'\n";

    const LINUX_MANIFEST: &str = "version: 0.9.1\nfiles:\n  - url: Mergeon-0.9.1.AppImage\n    sha512: AAAA\n    size: 120\n    blockMapSize: 12\n  - url: mergeon_0.9.1_amd64.deb\n    sha512: BBBB\n    size: 90\npath: Mergeon-0.9.1.AppImage\nsha512: AAAA\nreleaseDate: '2026-09-30T18:21:04.512Z'\n";

    fn file(url: &str) -> UpdateFile {
        UpdateFile {
            url: url.to_string(),
            sha512: Some("hash".to_string()),
            size: None,
        }
    }

    #[test]
    fn parses_the_manifest_electron_builder_publishes() {
        let manifest = parse_update_manifest(WINDOWS_MANIFEST).unwrap();

        assert_eq!(manifest.version, "0.9.1");
        assert_eq!(
            manifest.files,
            vec![UpdateFile {
                url: "Mergeon-Setup-0.9.1.exe".to_string(),
                sha512: Some("VkJl1gDqcBHYbYhMb0HRI3iINmr3wZtvqjKb1Wn1nvFYy0E0p5pYFGbhzsQdEu5BFs3wBKi1+TrbRPLpNDY3hg==".to_string()),
                size: Some(84_513_291),
            }]
        );
        assert_eq!(manifest.path.as_deref(), Some("Mergeon-Setup-0.9.1.exe"));
        assert_eq!(manifest.sha512, manifest.files[0].sha512);
        assert_eq!(
            manifest.release_date.as_deref(),
            Some("2026-09-30T18:21:04.512Z")
        );
        assert_eq!(manifest.release_notes, None);
    }

    #[test]
    fn parses_the_manifest_generate_mac_update_metadata_writes() {
        // Mismo texto que arma scripts/generate-mac-update-metadata.mjs.
        let text = [
            "version: 0.9.1",
            "files:",
            "  - url: Mergeon-0.9.1-arm64-mac.zip",
            "    sha512: Q0FGRQ==",
            "    size: 98765",
            "path: Mergeon-0.9.1-arm64-mac.zip",
            "sha512: Q0FGRQ==",
            "releaseDate: '2026-09-30T18:21:04.512Z'",
        ]
        .join("\n")
            + "\n";

        let manifest = parse_update_manifest(&text).unwrap();
        assert_eq!(manifest.version, "0.9.1");
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].url, "Mergeon-0.9.1-arm64-mac.zip");
        assert_eq!(manifest.files[0].size, Some(98_765));
    }

    #[test]
    fn keeps_release_notes_and_ignores_unknown_fields() {
        let text = "version: 1.0.0\nreleaseName: Otoño\nreleaseNotes: |\n  - Fix A\n  - Fix B\nstagingPercentage: 50\nfiles:\n  - url: a.exe\n    sha512: x\n    isAdminRightsRequired: true\n";
        let manifest = parse_update_manifest(text).unwrap();

        assert_eq!(manifest.release_name.as_deref(), Some("Otoño"));
        assert_eq!(
            manifest.release_notes.as_deref(),
            Some("- Fix A\n- Fix B\n")
        );
        assert_eq!(manifest.files[0].size, None);
    }

    #[test]
    fn falls_back_to_the_legacy_path_field() {
        let manifest =
            parse_update_manifest("version: 1.0.0\npath: old.exe\nsha512: abc\n").unwrap();
        assert!(manifest.files.is_empty());
        assert_eq!(
            manifest.resolved_files().unwrap(),
            vec![UpdateFile {
                url: "old.exe".to_string(),
                sha512: Some("abc".to_string()),
                size: None,
            }]
        );

        let empty = parse_update_manifest("version: 1.0.0\n").unwrap();
        assert_eq!(empty.resolved_files(), Err(UpdaterError::NoFiles));
    }

    #[test]
    fn rejects_invalid_manifests() {
        for text in [
            "",
            "<!DOCTYPE html>\n<html><body>Not found</body></html>\n",
            "just a string\n",
            "files:\n  - url: a.exe\n",
            "version: 1.0.0\nfiles: nope\n",
            "version: 1.0.0\nfiles:\n  - a.exe\n",
            "version: 1.0.0\nfiles:\n  - sha512: abc\n",
            "version: 1.0.0\nfiles:\n  - url: a.exe\n    size: big\n",
            "version: 'unterminated\n",
            "- version: 1.0.0\n",
        ] {
            assert!(
                matches!(
                    parse_update_manifest(text),
                    Err(UpdaterError::InvalidManifest { .. })
                ),
                "debería rechazar {text:?}"
            );
        }
    }

    #[test]
    fn names_the_channel_file_per_platform() {
        assert_eq!(
            channel_file_name("latest", &Platform::Win32, &Arch::X64),
            "latest.yml"
        );
        assert_eq!(
            channel_file_name("latest", &Platform::Win32, &Arch::Arm64),
            "latest.yml"
        );
        assert_eq!(
            channel_file_name("latest", &Platform::Linux, &Arch::X64),
            "latest-linux.yml"
        );
        assert_eq!(
            channel_file_name("latest", &Platform::Linux, &Arch::Arm64),
            "latest-linux-arm64.yml"
        );
        assert_eq!(
            channel_file_name("latest", &Platform::Darwin, &Arch::Arm64),
            "latest-mac.yml"
        );
        assert_eq!(
            channel_file_name("beta", &Platform::Darwin, &Arch::X64),
            "beta-mac.yml"
        );
    }

    #[test]
    fn resolves_the_installable_format_per_platform() {
        let appimage = Env::from_pairs([("APPIMAGE", "/opt/Mergeon.AppImage")]);
        assert_eq!(
            resolve_artifact_kind(&Platform::Win32, &Env::new()),
            Some(ArtifactKind::Nsis)
        );
        assert_eq!(
            resolve_artifact_kind(&Platform::Linux, &appimage),
            Some(ArtifactKind::AppImage)
        );
        // El .deb no se actualiza solo.
        assert_eq!(resolve_artifact_kind(&Platform::Linux, &Env::new()), None);
        assert_eq!(
            resolve_artifact_kind(&Platform::Darwin, &Env::new()),
            Some(ArtifactKind::MacZip)
        );
        assert_eq!(
            resolve_artifact_kind(&Platform::Other("freebsd".to_string()), &Env::new()),
            None
        );
    }

    #[test]
    fn selects_the_artifact_for_each_platform() {
        let windows = parse_update_manifest(WINDOWS_MANIFEST).unwrap();
        assert_eq!(
            select_artifact(
                &windows.files,
                ArtifactKind::Nsis,
                &Platform::Win32,
                &Arch::X64
            )
            .unwrap()
            .url,
            "Mergeon-Setup-0.9.1.exe"
        );

        // El AppImage se elige aunque el .deb esté en el mismo manifiesto.
        let linux = parse_update_manifest(LINUX_MANIFEST).unwrap();
        let reversed: Vec<UpdateFile> = linux.files.iter().rev().cloned().collect();
        for files in [&linux.files, &reversed] {
            assert_eq!(
                select_artifact(files, ArtifactKind::AppImage, &Platform::Linux, &Arch::X64)
                    .unwrap()
                    .url,
                "Mergeon-0.9.1.AppImage"
            );
        }
    }

    #[test]
    fn matches_extensions_ignoring_case_and_query() {
        let files = [file(
            "https://cdn.example.com/Mergeon-Setup.EXE?token=abc.zip",
        )];
        assert_eq!(
            select_artifact(&files, ArtifactKind::Nsis, &Platform::Win32, &Arch::X64)
                .unwrap()
                .url,
            files[0].url
        );
    }

    #[test]
    fn never_falls_back_to_a_package_the_installer_cannot_apply() {
        let only_deb = [file("mergeon_0.9.1_amd64.deb"), file("mergeon-0.9.1.rpm")];
        assert_eq!(
            select_artifact(
                &only_deb,
                ArtifactKind::AppImage,
                &Platform::Linux,
                &Arch::X64
            ),
            Err(UpdaterError::NoSuitableArtifact {
                wanted: "AppImage".to_string()
            })
        );

        let only_dmg = [file("Mergeon-0.9.1.dmg")];
        assert!(
            select_artifact(
                &only_dmg,
                ArtifactKind::MacZip,
                &Platform::Darwin,
                &Arch::X64
            )
            .is_err()
        );

        assert_eq!(
            select_artifact(&[], ArtifactKind::Nsis, &Platform::Win32, &Arch::X64),
            Err(UpdaterError::NoFiles)
        );

        // Windows conserva el criterio de electron-updater: sin `.exe`, el primero.
        let odd = [file("Mergeon-Setup.msi")];
        assert_eq!(
            select_artifact(&odd, ArtifactKind::Nsis, &Platform::Win32, &Arch::X64)
                .unwrap()
                .url,
            "Mergeon-Setup.msi"
        );
    }

    #[test]
    fn picks_the_mac_zip_for_the_running_architecture() {
        let files = [
            file("Mergeon-0.9.1-mac.zip"),
            file("Mergeon-0.9.1-arm64-mac.zip"),
            file("Mergeon-0.9.1.dmg"),
        ];
        assert_eq!(
            select_artifact(
                &files,
                ArtifactKind::MacZip,
                &Platform::Darwin,
                &Arch::Arm64
            )
            .unwrap()
            .url,
            "Mergeon-0.9.1-arm64-mac.zip"
        );
        assert_eq!(
            select_artifact(&files, ArtifactKind::MacZip, &Platform::Darwin, &Arch::X64)
                .unwrap()
                .url,
            "Mergeon-0.9.1-mac.zip"
        );

        // Sin zip arm64 publicado, Apple Silicon usa el de Intel (Rosetta).
        let intel_only = [file("Mergeon-0.9.1-mac.zip")];
        assert_eq!(
            select_artifact(
                &intel_only,
                ArtifactKind::MacZip,
                &Platform::Darwin,
                &Arch::Arm64
            )
            .unwrap()
            .url,
            "Mergeon-0.9.1-mac.zip"
        );

        // Y un Intel nunca recibe el zip arm64.
        let arm_only = [file("Mergeon-0.9.1-arm64-mac.zip")];
        assert!(
            select_artifact(
                &arm_only,
                ArtifactKind::MacZip,
                &Platform::Darwin,
                &Arch::X64
            )
            .is_err()
        );
    }
}
