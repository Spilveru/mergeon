//! Selección múltiple del historial: Ctrl/⌘+clic añade o quita un commit,
//! Shift+clic (o Shift+flechas) selecciona el rango desde el ancla.
//!
//! Lógica pura sobre la lista cargada: cada operación cuesta lo que la lista
//! (se hace al pulsar, nunca al pintar), y la pertenencia de una fila se
//! responde con un `HashSet` compartido por `Rc` con la lista.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use mergeon_model::commit_view_utils::CommitRow;

use crate::history_list::row_key;

/// Los commits seleccionados además del principal (el que mueve el teclado y
/// guarda la sesión). Con menos de dos no hay selección múltiple.
#[derive(Debug, Clone, Default)]
pub struct MultiSelection {
    /// Claves ([`row_key`]) seleccionadas, en el orden de la lista (del más
    /// nuevo al más viejo).
    keys: Vec<String>,
    /// Índices de esas claves en la lista cargada, en el mismo orden.
    indices: Rc<Vec<usize>>,
    set: Rc<HashSet<String>>,
    /// Desde dónde se extiende un rango con Shift.
    anchor: Option<String>,
}

/// Índice de cada fila de la lista por su clave.
fn index_by_key(commits: &[CommitRow]) -> HashMap<&str, usize> {
    commits
        .iter()
        .enumerate()
        .map(|(index, commit)| (row_key(commit), index))
        .collect()
}

impl MultiSelection {
    /// Hay al menos dos commits seleccionados.
    pub fn is_multi(&self) -> bool {
        self.keys.len() > 1
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.set.contains(key)
    }

    /// Las claves seleccionadas, en el orden de la lista.
    pub fn keys(&self) -> &[String] {
        &self.keys
    }

    /// Índices en la lista cargada, en su orden.
    pub fn indices(&self) -> &Rc<Vec<usize>> {
        &self.indices
    }

    /// El conjunto que comparte la lista para pintar las filas.
    pub fn set(&self) -> &Rc<HashSet<String>> {
        &self.set
    }

    pub fn anchor(&self) -> Option<&str> {
        self.anchor.as_deref()
    }

    /// Vuelve a la selección simple, con el ancla en `anchor`.
    pub fn collapse(&mut self, anchor: Option<&str>) {
        self.keys.clear();
        self.indices = Rc::new(Vec::new());
        if !self.set.is_empty() {
            self.set = Rc::new(HashSet::new());
        }
        self.anchor = anchor.map(str::to_string);
    }

    fn assign(&mut self, commits: &[CommitRow], mut indices: Vec<usize>) {
        indices.sort_unstable();
        indices.dedup();
        if indices.len() < 2 {
            let anchor = self.anchor.take();
            self.collapse(anchor.as_deref());
            return;
        }
        self.keys = indices
            .iter()
            .map(|index| row_key(&commits[*index]).to_string())
            .collect();
        self.set = Rc::new(self.keys.iter().cloned().collect());
        self.indices = Rc::new(indices);
    }

    /// Ctrl/⌘+clic sobre `key`: lo añade o lo quita. `primary` es el commit
    /// seleccionado hasta ahora (forma parte de la selección aunque no hubiera
    /// múltiple). Devuelve el commit que pasa a ser el principal: el pulsado
    /// si se añadió; si se quitó, el primero que quede.
    pub fn toggle(
        &mut self,
        commits: &[CommitRow],
        primary: Option<&str>,
        key: &str,
    ) -> Option<String> {
        let by_key = index_by_key(commits);
        let index = *by_key.get(key)?;
        let mut indices: Vec<usize> = if self.is_multi() {
            self.indices.to_vec()
        } else {
            primary
                .and_then(|primary| by_key.get(primary).copied())
                .into_iter()
                .collect()
        };

        let next_primary = match indices.iter().position(|selected| *selected == index) {
            // El único seleccionado no se puede quitar: no hay selección vacía.
            Some(_) if indices.len() == 1 => return Some(key.to_string()),
            Some(position) => {
                indices.remove(position);
                let mut remaining = indices.clone();
                remaining.sort_unstable();
                remaining
                    .first()
                    .map(|index| row_key(&commits[*index]).to_string())
            }
            None => {
                indices.push(index);
                Some(key.to_string())
            }
        };
        self.anchor = Some(key.to_string());
        self.assign(commits, indices);
        next_primary
    }

    /// Shift+clic sobre `key`: el rango de la lista entre el ancla (o el
    /// principal, si no hay ancla) y `key`. El ancla no se mueve; el pulsado
    /// pasa a ser el principal.
    pub fn select_range(
        &mut self,
        commits: &[CommitRow],
        primary: Option<&str>,
        key: &str,
    ) -> Option<String> {
        let by_key = index_by_key(commits);
        let index = *by_key.get(key)?;
        let anchor = self
            .anchor
            .clone()
            .filter(|anchor| by_key.contains_key(anchor.as_str()))
            .or_else(|| primary.map(str::to_string))
            .filter(|anchor| by_key.contains_key(anchor.as_str()))
            .unwrap_or_else(|| key.to_string());
        let anchor_index = by_key[anchor.as_str()];
        let (start, end) = (anchor_index.min(index), anchor_index.max(index));
        self.anchor = Some(anchor);
        self.assign(commits, (start..=end).collect());
        Some(key.to_string())
    }

    /// Shift+↑/↓: mueve el principal una fila (`step` −1 o +1) y selecciona
    /// el rango desde el ancla. `None` si ya está en el borde.
    pub fn extend_by(
        &mut self,
        commits: &[CommitRow],
        primary: Option<&str>,
        step: isize,
    ) -> Option<String> {
        let primary = primary?;
        let index = commits
            .iter()
            .position(|commit| row_key(commit) == primary)?;
        let next = index.checked_add_signed(step)?;
        let next_key = row_key(commits.get(next)?).to_string();
        if self.anchor.is_none() || !self.is_multi() {
            self.anchor = Some(primary.to_string());
        }
        self.select_range(commits, Some(primary), &next_key)
    }

    /// Tras recargar el historial: se quedan los commits que siguen en la
    /// lista (un rebase pudo reescribirlos) y se recalculan sus índices.
    pub fn retain_loaded(&mut self, commits: &[CommitRow]) {
        if self.keys.is_empty() {
            return;
        }
        let by_key = index_by_key(commits);
        let indices: Vec<usize> = self
            .keys
            .iter()
            .filter_map(|key| by_key.get(key.as_str()).copied())
            .collect();
        if indices.as_slice() == self.indices.as_slice() {
            return;
        }
        self.assign(commits, indices);
    }

    /// Los commits seleccionados del más viejo al más nuevo (el orden de un
    /// cherry-pick), por su hash completo.
    pub fn oldest_first(&self, commits: &[CommitRow]) -> Vec<String> {
        self.indices
            .iter()
            .rev()
            .filter_map(|index| commits.get(*index))
            .map(|commit| commit.full_hash.clone())
            .collect()
    }

    /// Del más nuevo al más viejo (el orden de un revert).
    pub fn newest_first(&self, commits: &[CommitRow]) -> Vec<String> {
        let mut hashes = self.oldest_first(commits);
        hashes.reverse();
        hashes
    }

    /// Si la selección es un tramo seguido de una sola línea de historia (cada
    /// commit es el primer padre del de encima), el padre del más viejo y el
    /// más nuevo: `git diff <padre> <más nuevo>` resume el rango entero.
    pub fn contiguous_range(&self, commits: &[CommitRow]) -> Option<(String, String)> {
        let indices = self.indices.as_slice();
        let (first, last) = (*indices.first()?, *indices.last()?);
        if last - first + 1 != indices.len() {
            return None;
        }
        let first_parent =
            |commit: &CommitRow| commit.parents.split_whitespace().next().map(str::to_string);
        for pair in indices.windows(2) {
            let (newer, older) = (commits.get(pair[0])?, commits.get(pair[1])?);
            if first_parent(newer).as_deref() != Some(older.full_hash.as_str()) {
                return None;
            }
        }
        let oldest = commits.get(last)?;
        let newest = commits.get(first)?;
        Some((first_parent(oldest)?, newest.full_hash.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Una línea `c4 → c3 → c2 → c1` (la lista va del más nuevo al más viejo).
    fn line() -> Vec<CommitRow> {
        let hashes = ["c4", "c3", "c2", "c1"];
        hashes
            .iter()
            .enumerate()
            .map(|(index, hash)| CommitRow {
                hash: hash.to_string(),
                full_hash: hash.to_string(),
                parents: hashes.get(index + 1).copied().unwrap_or("c0").to_string(),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn ctrl_click_adds_and_removes_commits() {
        let commits = line();
        let mut selection = MultiSelection::default();

        assert_eq!(
            selection.toggle(&commits, Some("c4"), "c2").as_deref(),
            Some("c2")
        );
        assert!(selection.is_multi());
        assert_eq!(selection.keys(), ["c4", "c2"]);

        selection.toggle(&commits, Some("c2"), "c1");
        assert_eq!(selection.keys(), ["c4", "c2", "c1"]);

        // Quitar el principal deja como principal el primero que queda.
        assert_eq!(
            selection.toggle(&commits, Some("c1"), "c1").as_deref(),
            Some("c4")
        );
        assert_eq!(selection.keys(), ["c4", "c2"]);

        // Con uno solo vuelve a la selección simple.
        assert_eq!(
            selection.toggle(&commits, Some("c4"), "c4").as_deref(),
            Some("c2")
        );
        assert!(!selection.is_multi());
        assert!(selection.is_empty());
    }

    #[test]
    fn ctrl_click_on_the_only_selected_commit_keeps_it() {
        let commits = line();
        let mut selection = MultiSelection::default();
        assert_eq!(
            selection.toggle(&commits, Some("c3"), "c3").as_deref(),
            Some("c3")
        );
        assert!(!selection.is_multi());
    }

    #[test]
    fn shift_click_selects_the_range_from_the_anchor() {
        let commits = line();
        let mut selection = MultiSelection::default();
        selection.collapse(Some("c4"));

        selection.select_range(&commits, Some("c4"), "c2");
        assert_eq!(selection.keys(), ["c4", "c3", "c2"]);
        // El ancla no se mueve: otro Shift+clic rehace el rango desde ella.
        selection.select_range(&commits, Some("c2"), "c3");
        assert_eq!(selection.keys(), ["c4", "c3"]);
        assert_eq!(selection.anchor(), Some("c4"));
    }

    #[test]
    fn shift_arrows_extend_from_the_selected_commit() {
        let commits = line();
        let mut selection = MultiSelection::default();
        selection.collapse(Some("c3"));

        assert_eq!(
            selection.extend_by(&commits, Some("c3"), 1).as_deref(),
            Some("c2")
        );
        assert_eq!(selection.keys(), ["c3", "c2"]);
        assert_eq!(
            selection.extend_by(&commits, Some("c2"), 1).as_deref(),
            Some("c1")
        );
        assert_eq!(selection.keys(), ["c3", "c2", "c1"]);
        // Volver hacia el ancla encoge el rango.
        selection.extend_by(&commits, Some("c1"), -1);
        assert_eq!(selection.keys(), ["c3", "c2"]);
        // En el borde no hay a dónde ir.
        assert_eq!(selection.extend_by(&commits, Some("c4"), -1), None);
    }

    #[test]
    fn orders_hashes_for_cherry_pick_and_revert() {
        let commits = line();
        let mut selection = MultiSelection::default();
        selection.toggle(&commits, Some("c4"), "c2");
        assert_eq!(selection.oldest_first(&commits), ["c2", "c4"]);
        assert_eq!(selection.newest_first(&commits), ["c4", "c2"]);
    }

    #[test]
    fn a_run_of_one_line_of_history_is_a_contiguous_range() {
        let commits = line();
        let mut selection = MultiSelection::default();
        selection.collapse(Some("c3"));
        selection.select_range(&commits, Some("c3"), "c1");
        assert_eq!(
            selection.contiguous_range(&commits),
            Some(("c0".to_string(), "c3".to_string()))
        );

        // Con un hueco ya no lo es.
        let mut gapped = MultiSelection::default();
        gapped.toggle(&commits, Some("c4"), "c2");
        assert_eq!(gapped.contiguous_range(&commits), None);
    }

    #[test]
    fn keeps_only_the_commits_that_survive_a_reload() {
        let mut commits = line();
        let mut selection = MultiSelection::default();
        selection.collapse(Some("c4"));
        selection.select_range(&commits, Some("c4"), "c2");

        commits.remove(1); // `c3` desaparece
        selection.retain_loaded(&commits);
        assert_eq!(selection.keys(), ["c4", "c2"]);
        assert_eq!(selection.indices().as_slice(), [0, 1]);

        commits.remove(0);
        selection.retain_loaded(&commits);
        assert!(!selection.is_multi());
    }
}
