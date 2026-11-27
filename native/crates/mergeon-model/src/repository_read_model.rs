//! De lo que devuelven las lecturas de ramas y remotos a lo que pinta la barra
//! lateral (`src/utils/repositoryReadModel.js`).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Seguimiento de una rama local, tal como viene en `branches[nombre]`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BranchInfo {
    pub tracking: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

/// Resultado de `git:getBranches` (lo que de él se usa aquí). Los nombres de
/// `all` son `main` para las locales y `remotes/origin/main` para las remotas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BranchData {
    pub current: String,
    pub all: Vec<String>,
    pub branches: HashMap<String, BranchInfo>,
}

impl Default for BranchData {
    fn default() -> Self {
        Self {
            current: "HEAD".to_string(),
            all: Vec::new(),
            branches: HashMap::new(),
        }
    }
}

/// Rama local lista para la barra lateral.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchItem {
    pub name: String,
    pub active: bool,
    pub is_remote: bool,
    pub upstream: Option<String>,
    /// Tiene upstream configurado o, al menos, una rama remota con su nombre.
    /// (El original dejaba aquí el nombre del upstream o un booleano; solo se
    /// leía como verdadero/falso.)
    pub has_tracking: bool,
    pub upstream_set: bool,
    pub ahead: u32,
    pub behind: u32,
}

pub fn build_branch_items(branch_data: &BranchData) -> Vec<BranchItem> {
    let remote_branches: Vec<&String> = branch_data
        .all
        .iter()
        .filter(|branch_name| branch_name.starts_with("remotes/"))
        .collect();

    branch_data
        .all
        .iter()
        .filter(|branch_name| !branch_name.starts_with("remotes/"))
        .map(|branch_name| {
            let origin_copy = format!("remotes/origin/{branch_name}");
            let any_remote_copy = format!("/{branch_name}");
            let has_remote = remote_branches
                .iter()
                .any(|remote| **remote == origin_copy || remote.ends_with(&any_remote_copy));
            let branch_info = branch_data.branches.get(branch_name);
            let tracking = branch_info
                .and_then(|info| info.tracking.as_deref())
                .filter(|tracking| !tracking.is_empty());

            BranchItem {
                name: branch_name.clone(),
                active: *branch_name == branch_data.current,
                is_remote: false,
                upstream: tracking.map(str::to_string),
                has_tracking: tracking.is_some() || has_remote,
                upstream_set: tracking.is_some(),
                ahead: branch_info.map_or(0, |info| info.ahead),
                behind: branch_info.map_or(0, |info| info.behind),
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteRefs {
    pub fetch: String,
    pub push: String,
}

/// Un remoto, como lo devuelve `git:getRemotes`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteData {
    pub name: String,
    pub refs: RemoteRefs,
}

/// Remoto con las ramas que se conocen de él.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteItem {
    pub name: String,
    pub refs: RemoteRefs,
    /// Nombres sin el prefijo del remoto (`feature/login`, no `origin/feature/login`).
    pub branches: Vec<String>,
}

/// Separa una referencia `<remoto>/<rama>` usando el nombre de remoto
/// configurado más largo que coincida como prefijo (`"{remote}/"`). Si ningún
/// remoto configurado coincide, divide en la primera `/`.
pub fn split_remote_ref<I, S>(reference: &str, remote_names: I) -> Option<(&str, &str)>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let best_len = remote_names
        .into_iter()
        .filter_map(|remote| {
            let remote = remote.as_ref();
            if remote.is_empty() {
                return None;
            }
            reference
                .strip_prefix(remote)
                .filter(|rest| rest.starts_with('/'))
                .map(|_| remote.len())
        })
        .max();
    if let Some(len) = best_len {
        return Some((&reference[..len], &reference[len + 1..]));
    }
    reference.split_once('/')
}

pub fn build_remote_items(
    remotes_data: &[RemoteData],
    branch_data: &BranchData,
) -> Vec<RemoteItem> {
    let mut remote_branches: HashMap<&str, Vec<String>> = HashMap::new();
    let remote_names: Vec<&str> = remotes_data
        .iter()
        .map(|remote| remote.name.as_str())
        .collect();

    for branch_name in &branch_data.all {
        let Some(short_name) = branch_name.strip_prefix("remotes/") else {
            continue;
        };
        let (remote_name, branch_name_only) =
            split_remote_ref(short_name, remote_names.iter().copied()).unwrap_or((short_name, ""));
        remote_branches
            .entry(remote_name)
            .or_default()
            .push(branch_name_only.to_string());
    }

    remotes_data
        .iter()
        .map(|remote| RemoteItem {
            name: remote.name.clone(),
            refs: remote.refs.clone(),
            branches: remote_branches
                .get(remote.name.as_str())
                .cloned()
                .unwrap_or_default(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch_data() -> BranchData {
        serde_json::from_value(serde_json::json!({
            "current": "main",
            "all": [
                "main",
                "feature/login",
                "local-only",
                "shared",
                "remotes/origin/main",
                "remotes/origin/feature/login",
                "remotes/upstream/shared"
            ],
            "branches": {
                "main": { "current": true, "name": "main", "tracking": "origin/main", "ahead": 2, "behind": 1 },
                "feature/login": { "tracking": null, "ahead": 0, "behind": 0 },
                "local-only": { "ahead": 0, "behind": 0 }
            }
        }))
        .unwrap()
    }

    #[test]
    fn builds_local_branch_items_with_their_tracking() {
        let items = build_branch_items(&branch_data());

        assert_eq!(
            items
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            ["main", "feature/login", "local-only", "shared"]
        );
        assert_eq!(
            items[0],
            BranchItem {
                name: "main".to_string(),
                active: true,
                is_remote: false,
                upstream: Some("origin/main".to_string()),
                has_tracking: true,
                upstream_set: true,
                ahead: 2,
                behind: 1,
            }
        );
    }

    #[test]
    fn tells_a_configured_upstream_from_a_remote_branch_that_merely_shares_the_name() {
        let items = build_branch_items(&branch_data());

        // Hay `origin/feature/login`, pero la rama no tiene upstream configurado.
        assert!(items[1].has_tracking);
        assert!(!items[1].upstream_set);
        assert_eq!(items[1].upstream, None);
        // Ni upstream ni rama remota.
        assert!(!items[2].has_tracking);
        // La copia puede estar en cualquier remoto, no solo en origin.
        assert!(items[3].has_tracking);
        assert_eq!((items[3].ahead, items[3].behind), (0, 0));
        assert!(items.iter().all(|item| !item.is_remote));
    }

    #[test]
    fn has_no_branches_without_branch_data() {
        assert_eq!(build_branch_items(&BranchData::default()), []);
        assert_eq!(BranchData::default().current, "HEAD");
    }

    #[test]
    fn groups_remote_branches_under_their_remote() {
        let remotes = vec![
            RemoteData {
                name: "origin".to_string(),
                refs: RemoteRefs {
                    fetch: "https://example.com/demo.git".to_string(),
                    push: "https://example.com/demo.git".to_string(),
                },
            },
            RemoteData {
                name: "upstream".to_string(),
                ..Default::default()
            },
            RemoteData {
                name: "backup".to_string(),
                ..Default::default()
            },
        ];

        let items = build_remote_items(&remotes, &branch_data());

        assert_eq!(items[0].name, "origin");
        assert_eq!(items[0].refs.fetch, "https://example.com/demo.git");
        assert_eq!(items[0].branches, ["main", "feature/login"]);
        assert_eq!(items[1].branches, ["shared"]);
        assert_eq!(items[2].branches, Vec::<String>::new());
        assert_eq!(build_remote_items(&[], &branch_data()), []);
    }

    #[test]
    fn groups_remote_branches_for_remotes_containing_slashes() {
        let remotes = vec![
            RemoteData {
                name: "team".to_string(),
                ..Default::default()
            },
            RemoteData {
                name: "team/origin".to_string(),
                ..Default::default()
            },
        ];
        let branches: BranchData = serde_json::from_value(serde_json::json!({
            "current": "feature/foo",
            "all": [
                "feature/foo",
                "remotes/team/main",
                "remotes/team/origin/main",
                "remotes/team/origin/feature/foo"
            ],
            "branches": {
                "feature/foo": { "tracking": "team/origin/feature/foo", "ahead": 0, "behind": 0 }
            }
        }))
        .unwrap();

        let items = build_remote_items(&remotes, &branches);
        assert_eq!(items[0].name, "team");
        assert_eq!(items[0].branches, ["main"]);
        assert_eq!(items[1].name, "team/origin");
        assert_eq!(items[1].branches, ["main", "feature/foo"]);

        assert_eq!(
            split_remote_ref("team/origin/feature/foo", ["team", "team/origin"]),
            Some(("team/origin", "feature/foo"))
        );
        assert_eq!(
            split_remote_ref("unknown/branch/name", ["team/origin"]),
            Some(("unknown", "branch/name"))
        );
    }

    #[test]
    fn serializes_with_the_field_names_the_sidebar_reads() {
        let items = build_branch_items(&branch_data());

        assert_eq!(
            serde_json::to_value(&items[1]).unwrap(),
            serde_json::json!({
                "name": "feature/login",
                "active": false,
                "isRemote": false,
                "upstream": null,
                "hasTracking": true,
                "upstreamSet": false,
                "ahead": 0,
                "behind": 0
            })
        );
    }
}
