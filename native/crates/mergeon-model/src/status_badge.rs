//! El estado de un archivo, traducido a las props de un badge
//! (`src/lib/ui/statusBadge.js`).
//!
//! Cubre los códigos de un commit (M, A, D, R, C) y también los que solo salen en
//! el working tree: sin seguir (`?`, `??`) y en conflicto (`U`). Los cambios
//! locales los pintaban con CSS propio, y por eso la misma "M" salía con la letra
//! de otro color que en el detalle de un commit.
//!
//! Sin seguir se muestra como añadido: para quien mira, un archivo nuevo es un
//! archivo nuevo. El conflicto es lo único que no va sólido, porque pide atención
//! de otra manera: no es un cambio, es algo que resolver.

use serde::{Deserialize, Serialize};

/// Variante de color del badge (las de `variants.js` que usa el estado).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BadgeVariant {
    Default,
    Secondary,
    Success,
    Warning,
    Danger,
}

impl BadgeVariant {
    pub fn as_str(self) -> &'static str {
        match self {
            BadgeVariant::Default => "default",
            BadgeVariant::Secondary => "secondary",
            BadgeVariant::Success => "success",
            BadgeVariant::Warning => "warning",
            BadgeVariant::Danger => "danger",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusBadge {
    pub variant: BadgeVariant,
    pub solid: bool,
}

pub fn status_badge(status: &str) -> StatusBadge {
    let variant = match status {
        "M" => BadgeVariant::Warning,
        "A" | "?" | "??" => BadgeVariant::Success,
        "D" | "U" => BadgeVariant::Danger,
        "R" | "C" => BadgeVariant::Default,
        _ => BadgeVariant::Secondary,
    };

    StatusBadge {
        variant,
        solid: matches!(status, "M" | "A" | "D" | "?" | "??"),
    }
}

/// Letra que se pinta dentro del badge; un código desconocido se muestra tal cual.
pub fn status_label(status: &str) -> &str {
    match status {
        "M" => "M",
        "A" | "?" | "??" => "+",
        "D" => "D",
        "R" => "R",
        "C" => "C",
        "U" => "!",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn badge(variant: BadgeVariant, solid: bool) -> StatusBadge {
        StatusBadge { variant, solid }
    }

    #[test]
    fn gives_modified_added_and_deleted_their_own_solid_colour() {
        assert_eq!(status_badge("M"), badge(BadgeVariant::Warning, true));
        assert_eq!(status_badge("A"), badge(BadgeVariant::Success, true));
        assert_eq!(status_badge("D"), badge(BadgeVariant::Danger, true));
    }

    // Para quien mira, un archivo sin seguir es un archivo nuevo.
    #[test]
    fn shows_an_untracked_file_as_added() {
        assert_eq!(status_badge("?"), status_badge("A"));
        assert_eq!(status_badge("??"), status_badge("A"));
        assert_eq!(status_label("??"), "+");
    }

    // Un conflicto no es un cambio, es algo que resolver: no va sólido.
    #[test]
    fn sets_a_conflict_apart_from_a_deletion() {
        assert_eq!(status_badge("U"), badge(BadgeVariant::Danger, false));
        assert_eq!(status_label("U"), "!");
    }

    #[test]
    fn falls_back_instead_of_leaving_a_hole_for_an_unknown_code() {
        assert_eq!(status_badge("X"), badge(BadgeVariant::Secondary, false));
        assert_eq!(status_label("X"), "X");
    }

    #[test]
    fn renames_and_copies_stay_neutral() {
        assert_eq!(status_badge("R"), badge(BadgeVariant::Default, false));
        assert_eq!(status_badge("C"), badge(BadgeVariant::Default, false));
        assert_eq!(status_label("R"), "R");
        assert_eq!(
            serde_json::to_value(status_badge("M")).unwrap(),
            serde_json::json!({ "variant": "warning", "solid": true })
        );
    }
}
