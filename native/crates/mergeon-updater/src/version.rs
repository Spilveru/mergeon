//! Semver mínimo, con las reglas de `semver.parse` / `semver.gt` de node-semver
//! que usa electron-updater para decidir si hay una versión nueva.

use std::cmp::Ordering;
use std::fmt;

use crate::error::UpdaterError;

const MAX_LENGTH: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identifier {
    Numeric(u64),
    Alphanumeric(String),
}

impl fmt::Display for Identifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Numeric(value) => write!(f, "{value}"),
            Self::Alphanumeric(value) => f.write_str(value),
        }
    }
}

impl Ord for Identifier {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Numeric(a), Self::Numeric(b)) => a.cmp(b),
            // Un identificador numérico siempre precede a uno alfanumérico.
            (Self::Numeric(_), Self::Alphanumeric(_)) => Ordering::Less,
            (Self::Alphanumeric(_), Self::Numeric(_)) => Ordering::Greater,
            (Self::Alphanumeric(a), Self::Alphanumeric(b)) => a.as_bytes().cmp(b.as_bytes()),
        }
    }
}

impl PartialOrd for Identifier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Versión semver. La igualdad y el orden son los de precedencia: los metadatos
/// de build (`+abc`) se conservan pero no cuentan al comparar.
#[derive(Debug, Clone)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub prerelease: Vec<Identifier>,
    pub build: Vec<String>,
}

impl Version {
    /// Acepta un prefijo `v` y espacios alrededor, como node-semver.
    pub fn parse(input: &str) -> Option<Self> {
        let text = input.trim();
        if text.is_empty() || text.len() > MAX_LENGTH {
            return None;
        }
        let text = text.strip_prefix('v').unwrap_or(text);

        let (rest, build) = match text.split_once('+') {
            Some((rest, build)) => (rest, Some(build)),
            None => (text, None),
        };
        let (core, prerelease) = match rest.split_once('-') {
            Some((core, prerelease)) => (core, Some(prerelease)),
            None => (rest, None),
        };

        let mut numbers = core.split('.');
        let major = parse_numeric(numbers.next()?)?;
        let minor = parse_numeric(numbers.next()?)?;
        let patch = parse_numeric(numbers.next()?)?;
        if numbers.next().is_some() {
            return None;
        }

        let prerelease = match prerelease {
            Some(text) => text
                .split('.')
                .map(parse_prerelease_identifier)
                .collect::<Option<Vec<_>>>()?,
            None => Vec::new(),
        };

        let build = match build {
            Some(text) => text
                .split('.')
                .map(|part| is_identifier(part).then(|| part.to_string()))
                .collect::<Option<Vec<_>>>()?,
            None => Vec::new(),
        };

        Some(Self {
            major,
            minor,
            patch,
            prerelease,
            build,
        })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.prerelease.is_empty()
    }
}

fn parse_numeric(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // Sin ceros a la izquierda: `01.2.3` no es semver.
    if text.len() > 1 && text.starts_with('0') {
        return None;
    }
    text.parse().ok()
}

fn is_identifier(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn parse_prerelease_identifier(text: &str) -> Option<Identifier> {
    if !is_identifier(text) {
        return None;
    }
    if text.bytes().all(|byte| byte.is_ascii_digit()) {
        return parse_numeric(text).map(Identifier::Numeric);
    }
    Some(Identifier::Alphanumeric(text.to_string()))
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
            .then_with(|| {
                match (self.prerelease.is_empty(), other.prerelease.is_empty()) {
                    (true, true) => Ordering::Equal,
                    // 1.0.0 es mayor que 1.0.0-beta.
                    (true, false) => Ordering::Greater,
                    (false, true) => Ordering::Less,
                    // `Vec` compara elemento a elemento y, si uno es prefijo del
                    // otro, gana el más largo: justo la regla de semver.
                    (false, false) => self.prerelease.cmp(&other.prerelease),
                }
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Version {}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.prerelease.is_empty() {
            let parts: Vec<String> = self.prerelease.iter().map(ToString::to_string).collect();
            write!(f, "-{}", parts.join("."))?;
        }
        if !self.build.is_empty() {
            write!(f, "+{}", self.build.join("."))?;
        }
        Ok(())
    }
}

/// Compara dos versiones semver. `None` si alguna no es válida.
pub fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    Some(Version::parse(a)?.cmp(&Version::parse(b)?))
}

/// `isUpdateAvailable` de electron-updater sin `allowDowngrade`: hay
/// actualización solo si la versión publicada es estrictamente mayor.
pub fn is_update_available(current: &str, latest: &str) -> Result<bool, UpdaterError> {
    let latest_version = Version::parse(latest).ok_or_else(|| UpdaterError::InvalidVersion {
        version: latest.to_string(),
    })?;
    let current_version =
        Version::parse(current).ok_or_else(|| UpdaterError::InvalidCurrentVersion {
            version: current.to_string(),
        })?;

    Ok(latest_version > current_version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|| panic!("{text} debería ser semver"))
    }

    #[test]
    fn parses_core_prerelease_and_build() {
        let parsed = version(" v1.2.3-beta.4+build.5 ");
        assert_eq!((parsed.major, parsed.minor, parsed.patch), (1, 2, 3));
        assert_eq!(
            parsed.prerelease,
            vec![
                Identifier::Alphanumeric("beta".to_string()),
                Identifier::Numeric(4)
            ]
        );
        assert_eq!(parsed.build, vec!["build".to_string(), "5".to_string()]);
        assert!(parsed.is_prerelease());
        assert_eq!(parsed.to_string(), "1.2.3-beta.4+build.5");
        assert_eq!(version("0.9.0").to_string(), "0.9.0");
        // Un prerelease puede llevar guiones dentro.
        assert_eq!(
            version("1.0.0-x-y-z.--").prerelease,
            vec![
                Identifier::Alphanumeric("x-y-z".to_string()),
                Identifier::Alphanumeric("--".to_string())
            ]
        );
    }

    #[test]
    fn rejects_what_node_semver_rejects() {
        for invalid in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.02.3",
            "1.2.03",
            "1.2.x",
            "1.2.3-",
            "1.2.3-beta..1",
            "1.2.3-01",
            "1.2.3+",
            "1.2.3-béta",
            "latest",
            "99999999999999999999999.0.0",
        ] {
            assert!(
                Version::parse(invalid).is_none(),
                "{invalid:?} no es semver"
            );
        }
        assert!(Version::parse(&format!("1.0.0-{}", "a".repeat(300))).is_none());
    }

    #[test]
    fn orders_versions_by_semver_precedence() {
        // El orden de ejemplo de la especificación (apartado 11).
        let ordered = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.1.0",
            "1.10.0",
            "2.0.0",
        ];
        for pair in ordered.windows(2) {
            assert!(
                version(pair[0]) < version(pair[1]),
                "{} < {}",
                pair[0],
                pair[1]
            );
            assert_eq!(compare_versions(pair[1], pair[0]), Some(Ordering::Greater));
        }
        // Numérico contra numérico se compara por valor, no como texto.
        assert!(version("0.9.0") < version("0.10.0"));
        assert!(version("1.0.0-2") < version("1.0.0-11"));
        // Numérico precede a alfanumérico.
        assert!(version("1.0.0-9") < version("1.0.0-a"));
    }

    #[test]
    fn build_metadata_does_not_affect_precedence() {
        assert_eq!(version("1.0.0+a"), version("1.0.0+b"));
        assert_eq!(
            compare_versions("1.0.0+linux", "v1.0.0"),
            Some(Ordering::Equal)
        );
        assert_eq!(compare_versions("1.0.0", "nope"), None);
    }

    #[test]
    fn reports_an_update_only_for_a_strictly_newer_version() {
        assert_eq!(is_update_available("0.9.0", "0.9.1"), Ok(true));
        assert_eq!(is_update_available("0.9.0", "0.9.0"), Ok(false));
        // Sin allowDowngrade: una versión publicada más vieja no es una actualización.
        assert_eq!(is_update_available("0.9.0", "0.8.5"), Ok(false));
        assert_eq!(is_update_available("1.0.0-beta.1", "1.0.0"), Ok(true));
        assert_eq!(is_update_available("1.0.0", "1.0.1-beta.1"), Ok(true));
        assert_eq!(is_update_available("1.0.0", "1.0.0-rc.1"), Ok(false));
        assert_eq!(
            is_update_available("1.0.0-beta.1", "1.0.0-beta.2"),
            Ok(true)
        );
    }

    #[test]
    fn invalid_versions_are_errors_not_updates() {
        assert_eq!(
            is_update_available("0.9.0", "nightly"),
            Err(UpdaterError::InvalidVersion {
                version: "nightly".to_string()
            })
        );
        assert_eq!(
            is_update_available("dev", "1.0.0"),
            Err(UpdaterError::InvalidCurrentVersion {
                version: "dev".to_string()
            })
        );
    }
}
