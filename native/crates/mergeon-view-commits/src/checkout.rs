//! Destinos posibles al hacer checkout de una fila del historial
//! (`buildCheckoutTargets` de `CommitView.vue`) y lo que el diálogo de checkout
//! necesita para plantearlos.
//!
//! Un commit que lleva una rama encima casi nunca significa "desancla HEAD
//! aquí": lo natural es cambiarse a esa rama.

use mergeon_model::commit_view_utils::{CommitRef, RefType};

/// Prefijos de los destinos de checkout que ofrece el historial.
pub const BRANCH_TARGET_PREFIX: &str = "branch:";
pub const REMOTE_TARGET_PREFIX: &str = "remote:";
pub const DETACHED_TARGET: &str = "detached";

/// Un destino: `{ value, label, shortLabel?, description, confirmLabel, warning? }`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CheckoutTargetOption {
    /// `branch:<rama>`, `remote:<remoto>/<rama>` o `detached`.
    pub value: String,
    pub label: String,
    /// Sin selector el diálogo ya dice "Switch to:", así que basta el nombre.
    pub short_label: Option<String>,
    pub description: String,
    pub confirm_label: String,
    /// Aviso mientras este destino está elegido (HEAD desanclado).
    pub warning: Option<String>,
}

/// Qué hace un destino elegido.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckoutTargetKind {
    /// Cambiarse a una rama local.
    Branch(String),
    /// Crear la rama local que sigue a una remota.
    Remote(String),
    /// Dejar HEAD desanclado en el commit.
    Detached,
}

impl CheckoutTargetKind {
    /// Interpreta el `value` de un destino. Lo que no se entiende es un
    /// checkout del propio commit, como en el original.
    pub fn parse(value: &str) -> Self {
        match value.split_once(':') {
            Some(("branch", reference)) => Self::Branch(reference.to_string()),
            Some(("remote", reference)) => Self::Remote(reference.to_string()),
            _ => Self::Detached,
        }
    }
}

fn build_local_branch_target(name: &str) -> CheckoutTargetOption {
    CheckoutTargetOption {
        value: format!("{BRANCH_TARGET_PREFIX}{name}"),
        label: format!("Switch to '{name}'"),
        short_label: Some(name.to_string()),
        description: "Moves HEAD to the branch and stays on it.".to_string(),
        confirm_label: "Switch branch".to_string(),
        warning: None,
    }
}

/// `buildCheckoutTargets(commit)`.
///
/// `short_hash` es el hash abreviado del commit, `current_branch` la rama en
/// la que está el repositorio y `known_local_branches` sus ramas locales.
pub fn build_checkout_targets(
    refs: &[CommitRef],
    short_hash: &str,
    current_branch: &str,
    known_local_branches: &[String],
) -> Vec<CheckoutTargetOption> {
    let local_branches: Vec<&str> = refs
        .iter()
        .filter(|commit_ref| {
            commit_ref.kind == RefType::Local
                && !commit_ref.name.is_empty()
                && commit_ref.name != current_branch
        })
        .map(|commit_ref| commit_ref.name.as_str())
        .collect();

    let mut targets: Vec<CheckoutTargetOption> = local_branches
        .iter()
        .map(|name| build_local_branch_target(name))
        .collect();

    /*
     * Solo si el commit no tiene ya una rama local encima: teniéndola, crear otra
     * que trackee la remota duplicaría la misma historia.
     *
     * Si el nombre local ya existe en el repo (tu rama está atrasada respecto a
     * la remota, así que vive en otro commit) no hay tracking que crear: se
     * ofrece cambiarse a esa rama local, que es a donde quieres ir.
     */
    if local_branches.is_empty() {
        let mut offered_locals: Vec<&str> = Vec::new();
        for commit_ref in refs {
            if commit_ref.kind != RefType::Remote || commit_ref.name.is_empty() {
                continue;
            }

            let local_name = commit_ref
                .name
                .split_once('/')
                .map_or("", |(_, branch)| branch);
            if local_name.is_empty() || local_name == current_branch {
                continue;
            }

            if known_local_branches.iter().any(|name| name == local_name) {
                if !offered_locals.contains(&local_name) {
                    offered_locals.push(local_name);
                    targets.push(build_local_branch_target(local_name));
                }
                continue;
            }

            targets.push(CheckoutTargetOption {
                value: format!("{REMOTE_TARGET_PREFIX}{}", commit_ref.name),
                label: format!("Create '{local_name}' tracking '{}'", commit_ref.name),
                short_label: None,
                description: "Creates a local branch here and switches to it.".to_string(),
                confirm_label: "Create and Checkout".to_string(),
                warning: None,
            });
        }
    }

    /*
     * El HEAD desanclado solo se ofrece cuando no hay ninguna rama a la que ir:
     * un commit viejo en medio de la historia. Teniendo rama, mezclarlo como
     * segunda opción convertía una acción obvia en una pregunta.
     */
    if targets.is_empty() {
        targets.push(CheckoutTargetOption {
            value: DETACHED_TARGET.to_string(),
            label: format!("Checkout commit {short_hash}"),
            short_label: Some(format!("commit {short_hash}")),
            description: "Leaves HEAD detached on this commit.".to_string(),
            confirm_label: "Checkout commit".to_string(),
            warning: Some(
                "HEAD will be detached: you will not be on a branch until you check one out again. New commits made here are easy to lose."
                    .to_string(),
            ),
        });
    }

    targets
}

/// El destino de un doble clic en la etiqueta de una rama del historial: esa
/// rama y no otra. Local: cambiarse a ella. Remota: cambiarse a la local con
/// su nombre si ya existe o, si no, crear la que la sigue (como el doble clic
/// en el sidebar). `None` si no hay nada que hacer (la rama actual, un tag o
/// HEAD desanclado).
pub fn ref_checkout_target(
    name: &str,
    kind: RefType,
    current_branch: &str,
    known_local_branches: &[String],
) -> Option<CheckoutTargetOption> {
    match kind {
        RefType::Local if !name.is_empty() && name != current_branch => {
            Some(build_local_branch_target(name))
        }
        RefType::Remote => {
            let local_name = name.split_once('/').map_or("", |(_, branch)| branch);
            if local_name.is_empty() || local_name == current_branch {
                return None;
            }
            if known_local_branches.iter().any(|known| known == local_name) {
                return Some(build_local_branch_target(local_name));
            }
            Some(CheckoutTargetOption {
                value: format!("{REMOTE_TARGET_PREFIX}{name}"),
                label: format!("Create '{local_name}' tracking '{name}'"),
                short_label: None,
                description: "Creates a local branch here and switches to it.".to_string(),
                confirm_label: "Create and Checkout".to_string(),
                warning: None,
            })
        }
        _ => None,
    }
}

/// Lo que el diálogo de checkout (`CheckoutBranchDialog`) tiene que plantear:
/// sus props, tal como las calcula `CommitView.vue`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CheckoutDialogRequest {
    /// Hash completo del commit sobre el que se pidió el checkout.
    pub commit_hash: String,
    /// `target-options`.
    pub targets: Vec<CheckoutTargetOption>,
    pub has_local_changes: bool,
}

impl CheckoutDialogRequest {
    fn only_target(&self) -> Option<&CheckoutTargetOption> {
        match self.targets.as_slice() {
            [target] => Some(target),
            _ => None,
        }
    }

    /// `checkoutTargetLabel`: con un solo destino el diálogo no pinta
    /// selector, así que necesita la etiqueta suelta.
    pub fn target_label(&self) -> &str {
        self.only_target()
            .map(|target| target.short_label.as_deref().unwrap_or(&target.label))
            .unwrap_or("HEAD")
    }

    /// `checkoutRemoteBranchTarget`: si el único destino es trackear una rama
    /// remota, el diálogo es exactamente el mismo que sale al doble clic sobre
    /// esa rama en el sidebar. Vacío en los demás casos.
    pub fn remote_branch(&self) -> &str {
        self.only_target()
            .and_then(|target| target.value.strip_prefix(REMOTE_TARGET_PREFIX))
            .unwrap_or("")
    }

    /// `isDetachedOnlyCheckout`.
    pub fn is_detached_only(&self) -> bool {
        self.only_target()
            .is_some_and(|target| target.value == DETACHED_TARGET)
    }

    /// `checkoutDialogTitle`. Vacío = los textos por defecto del diálogo.
    pub fn title(&self) -> &'static str {
        if self.is_detached_only() {
            "Checkout commit"
        } else {
            ""
        }
    }

    /// `checkoutDialogDescription`.
    pub fn description(&self) -> &'static str {
        if self.is_detached_only() {
            "This commit has no branch on it — HEAD will move here on its own"
        } else if self.targets.len() > 1 {
            "Choose which branch HEAD should end up on"
        } else {
            ""
        }
    }
}

/// Lo que el diálogo "Create Branch" tiene que plantear al crear una rama
/// desde un commit del historial.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CreateBranchDialogRequest {
    /// Hash completo del commit (`base-reference`).
    pub base_reference: String,
    /// Hash abreviado (`base-label`).
    pub base_label: String,
    pub has_local_changes: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use mergeon_model::commit_view_utils::parse_refs;

    fn targets(refs: &str, known: &[&str]) -> Vec<CheckoutTargetOption> {
        let known: Vec<String> = known.iter().map(|name| name.to_string()).collect();
        build_checkout_targets(&parse_refs(refs), "2222222", "main", &known)
    }

    fn request(refs: &str, known: &[&str]) -> CheckoutDialogRequest {
        CheckoutDialogRequest {
            commit_hash: "2222222222222222".to_string(),
            targets: targets(refs, known),
            has_local_changes: false,
        }
    }

    // Un commit sin refs solo admite HEAD desanclado: el diálogo lo dice y el
    // botón confirma exactamente eso.
    #[test]
    fn a_commit_without_refs_only_offers_a_detached_checkout() {
        let request = request("", &[]);

        assert_eq!(request.targets.len(), 1);
        assert_eq!(request.targets[0].value, DETACHED_TARGET);
        assert_eq!(request.targets[0].label, "Checkout commit 2222222");
        assert_eq!(request.targets[0].confirm_label, "Checkout commit");
        assert!(request.targets[0].warning.is_some());
        assert!(request.is_detached_only());
        assert_eq!(request.title(), "Checkout commit");
        assert_eq!(
            request.description(),
            "This commit has no branch on it — HEAD will move here on its own"
        );
        assert_eq!(request.target_label(), "commit 2222222");
        assert_eq!(request.remote_branch(), "");
    }

    #[test]
    fn the_tip_of_a_local_branch_switches_to_that_branch() {
        let request = request("feature", &[]);

        assert_eq!(request.targets.len(), 1);
        assert_eq!(request.targets[0].value, "branch:feature");
        assert_eq!(request.targets[0].label, "Switch to 'feature'");
        assert_eq!(request.targets[0].confirm_label, "Switch branch");
        assert_eq!(request.target_label(), "feature");
        // Los textos por defecto del diálogo ya son los correctos.
        assert_eq!((request.title(), request.description()), ("", ""));
        assert!(!request.is_detached_only());
    }

    // La rama en la que ya estás no es un destino.
    #[test]
    fn the_current_branch_is_not_a_target() {
        let targets = targets("HEAD -> main, feature", &[]);

        let values: Vec<&str> = targets.iter().map(|target| target.value.as_str()).collect();
        assert_eq!(values, ["branch:feature"]);
    }

    #[test]
    fn two_local_branches_become_a_choice() {
        let request = request("refs/heads/feature, refs/heads/hotfix", &[]);

        let values: Vec<&str> = request
            .targets
            .iter()
            .map(|target| target.value.as_str())
            .collect();
        assert_eq!(values, ["branch:feature", "branch:hotfix"]);
        assert_eq!(
            request.description(),
            "Choose which branch HEAD should end up on"
        );
        assert_eq!(request.target_label(), "HEAD");
    }

    // Solo con rama remota encima: se ofrece crear la local que la trackea, en
    // vez de dejar HEAD suelto sobre el commit.
    #[test]
    fn a_remote_only_commit_offers_a_tracking_branch() {
        let request = request("origin/feature", &[]);

        assert_eq!(request.targets.len(), 1);
        assert_eq!(request.targets[0].value, "remote:origin/feature");
        assert_eq!(
            request.targets[0].label,
            "Create 'feature' tracking 'origin/feature'"
        );
        assert_eq!(request.targets[0].confirm_label, "Create and Checkout");
        assert_eq!(request.remote_branch(), "origin/feature");
        assert_eq!((request.title(), request.description()), ("", ""));
    }

    // Tu rama local existe pero está atrasada, así que vive en otro commit: en
    // el commit de origin/x no hay tracking que crear, lo que quieres es ir a
    // tu rama.
    #[test]
    fn an_existing_local_branch_wins_over_tracking_its_remote_again() {
        let targets = targets("refs/remotes/origin/feature", &["main", "feature"]);

        let values: Vec<&str> = targets.iter().map(|target| target.value.as_str()).collect();
        assert_eq!(values, ["branch:feature"]);
    }

    // Teniendo ya una rama local encima no se ofrece además trackear la remota.
    #[test]
    fn a_local_branch_hides_the_remote_copy() {
        let targets = targets("refs/heads/feature, refs/remotes/origin/other", &[]);

        let values: Vec<&str> = targets.iter().map(|target| target.value.as_str()).collect();
        assert_eq!(values, ["branch:feature"]);
    }

    // `origin/main` cuando estás en main no lleva a ninguna parte.
    #[test]
    fn the_remote_copy_of_the_current_branch_falls_back_to_detached() {
        let targets = targets("refs/remotes/origin/main", &["main"]);

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].value, DETACHED_TARGET);
    }

    #[test]
    fn a_double_click_on_a_branch_label_targets_that_branch() {
        let known = ["main".to_string(), "feature".to_string()];
        let local = ref_checkout_target("feature", RefType::Local, "main", &known).unwrap();
        assert_eq!(local.value, "branch:feature");

        // La rama actual, un tag o HEAD desanclado: nada.
        assert_eq!(
            ref_checkout_target("main", RefType::Local, "main", &known),
            None
        );
        assert_eq!(
            ref_checkout_target("v1.0", RefType::Tag, "main", &known),
            None
        );
        assert_eq!(
            ref_checkout_target("HEAD", RefType::Head, "main", &known),
            None
        );

        // Remota con local del mismo nombre: cambiarse a la local.
        let remote = ref_checkout_target("origin/feature", RefType::Remote, "main", &known);
        assert_eq!(
            remote.map(|target| target.value).as_deref(),
            Some("branch:feature")
        );
        // Sin local: crear la que la sigue.
        let tracking = ref_checkout_target("origin/fix", RefType::Remote, "main", &known).unwrap();
        assert_eq!(tracking.value, "remote:origin/fix");
        assert_eq!(tracking.label, "Create 'fix' tracking 'origin/fix'");
        // La copia remota de la rama actual: nada.
        assert_eq!(
            ref_checkout_target("origin/main", RefType::Remote, "main", &known),
            None
        );
    }

    #[test]
    fn deduplicates_local_branch_targets_across_multiple_remotes() {
        let refs = vec![
            CommitRef {
                name: "origin/main".to_string(),
                kind: RefType::Remote,
                is_head: false,
                color: String::new(),
            },
            CommitRef {
                name: "web/main".to_string(),
                kind: RefType::Remote,
                is_head: false,
                color: String::new(),
            },
            CommitRef {
                name: "origin/release/1.x".to_string(),
                kind: RefType::Remote,
                is_head: false,
                color: String::new(),
            },
        ];
        let known = vec!["main".to_string()];
        let targets = build_checkout_targets(&refs, "topic", "ea819ba", &known);
        let labels: Vec<&str> = targets.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Switch to 'main'",
                "Create 'release/1.x' tracking 'origin/release/1.x'",
            ]
        );
    }

    #[test]
    fn reads_what_a_chosen_target_does() {
        assert_eq!(
            CheckoutTargetKind::parse("branch:feature/login"),
            CheckoutTargetKind::Branch("feature/login".to_string())
        );
        assert_eq!(
            CheckoutTargetKind::parse("remote:origin/release/1.x"),
            CheckoutTargetKind::Remote("origin/release/1.x".to_string())
        );
        assert_eq!(
            CheckoutTargetKind::parse("detached"),
            CheckoutTargetKind::Detached
        );
    }
}
