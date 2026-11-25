//! Resumen de un repositorio para el inicio
//! (`src/composables/useRepositorySummary.js`): se guarda por ruta y dos
//! peticiones simultáneas de la misma ruta comparten la lectura.
//!
//! El guardado se revalida con la huella de HEAD y las ramas
//! (`get_head_fingerprint`, sin lanzar git): mientras no cambie se sirve sin
//! leer nada; si cambió se relee, y el recuento de commits y las fechas (lo
//! caro: `rev-list --count` recorre todo el historial) se reaprovechan si el
//! OID de HEAD sigue siendo el mismo (`refresh_repository_summary`).

use std::collections::HashMap;
use std::path::Path;

use gpui_kit::{Context, Task};
use mergeon_backend::RepositorySummary;

use crate::AppModel;
use crate::tasks::{InFlightMap, settled};

/// Tope de resúmenes guardados. Cada uno lleva el README entero, y se
/// guardaba uno por cada repositorio que se miraba en el inicio. Al pasarse
/// se olvida el que hace más que no se pide.
pub(crate) const MAX_SUMMARIES: usize = 32;

#[derive(Debug, Default)]
pub(crate) struct RepositorySummaries {
    pub cache: HashMap<String, RepositorySummary>,
    /// Huella de HEAD y las ramas de cada ruta cuando se leyó su resumen.
    pub head_fingerprints: HashMap<String, String>,
    pub loading_path: Option<String>,
    pub requests: InFlightMap,
    /// Cuándo se pidió por última vez cada resumen (reloj propio).
    used_at: HashMap<String, u64>,
    clock: u64,
}

impl RepositorySummaries {
    fn touch(&mut self, path: &str) {
        self.clock += 1;
        self.used_at.insert(path.to_string(), self.clock);
    }

    fn insert(&mut self, path: String, summary: RepositorySummary) {
        self.cache.insert(path, summary);
        while self.cache.len() > MAX_SUMMARIES {
            let oldest = self
                .cache
                .keys()
                .min_by_key(|path| self.used_at.get(*path).copied().unwrap_or(0))
                .cloned();
            let Some(oldest) = oldest else { break };
            self.remove(&oldest);
        }
    }

    pub(crate) fn remove(&mut self, path: &str) -> Option<RepositorySummary> {
        self.head_fingerprints.remove(path);
        self.used_at.remove(path);
        self.cache.remove(path)
    }
}

fn summary_requests(model: &mut AppModel) -> &mut InFlightMap {
    &mut model.summaries.requests
}

/// `buildInvalidSummary`: lo que se muestra de una ruta cuyo resumen no se
/// pudo leer.
pub fn build_invalid_summary(repo_path: &str) -> RepositorySummary {
    let name = repo_path
        .rsplit(['\\', '/'])
        .next()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(repo_path);

    RepositorySummary {
        success: true,
        repo_path: repo_path.to_string(),
        name: name.to_string(),
        is_valid: false,
        invalid_reason: None,
        current_branch: None,
        changes_count: 0,
        remotes: Vec::new(),
        commit_count: 0,
        initial_commit_date: None,
        last_commit_date: None,
        repository_size: 0,
        readme_path: None,
        readme_content: None,
        head_oid: None,
    }
}

impl AppModel {
    /// `summaryCache`: resúmenes ya leídos, por ruta.
    pub fn summary_cache(&self) -> &HashMap<String, RepositorySummary> {
        &self.summaries.cache
    }

    /// Resumen ya leído de una ruta.
    pub fn repository_summary(&self, repo_path: &str) -> Option<&RepositorySummary> {
        self.summaries.cache.get(repo_path)
    }

    /// `loadingSummaryPath`: la ruta cuyo resumen se está leyendo.
    pub fn loading_summary_path(&self) -> Option<&str> {
        self.summaries.loading_path.as_deref()
    }

    /// `loadRepositorySummary`: devuelve el resumen guardado si HEAD y las
    /// ramas no cambiaron desde que se leyó; si no, lo (re)lee. Si la lectura
    /// falla se guarda (y devuelve) el resumen de una ruta no válida. Solo la
    /// primera lectura de una ruta marca `loading_summary_path`: al revalidar
    /// se sigue mostrando el guardado hasta que llega el nuevo.
    pub fn load_repository_summary(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<Option<RepositorySummary>> {
        if repo_path.is_empty() {
            return Task::ready(None);
        }

        let path = repo_path.to_string();
        self.summaries.touch(&path);
        let shared = self.run_shared(
            summary_requests,
            path.clone(),
            false,
            cx,
            move |this, cx| {
                let previous = this.summaries.cache.get(&path).cloned();
                let known_fingerprint = this.summaries.head_fingerprints.get(&path).cloned();
                if previous.is_none() {
                    this.summaries.loading_path = Some(path.clone());
                    cx.notify();
                }

                let read = this.blocking(cx, {
                    let path = path.clone();
                    move |backend| {
                        // Antes de leer: si algo cambia durante la lectura, la
                        // próxima vez la huella ya no coincide y se relee.
                        let fingerprint = mergeon_runtime::get_head_fingerprint(Path::new(&path));
                        if let Some(previous) = previous.as_ref()
                            && fingerprint.is_some()
                            && fingerprint == known_fingerprint
                        {
                            return (previous.clone(), fingerprint);
                        }
                        let summary = backend
                            .git()
                            .refresh_repository_summary(&path, previous.as_ref())
                            .unwrap_or_else(|_| build_invalid_summary(&path));
                        (summary, fingerprint)
                    }
                });
                cx.spawn(async move |this, cx| {
                    let (summary, fingerprint) = read.await;
                    this.update(cx, |this, cx| {
                        this.summaries.insert(path.clone(), summary.clone());
                        match fingerprint {
                            Some(fingerprint) => {
                                this.summaries
                                    .head_fingerprints
                                    .insert(path.clone(), fingerprint);
                            }
                            None => {
                                this.summaries.head_fingerprints.remove(&path);
                            }
                        }
                        if this.summaries.loading_path.as_deref() == Some(path.as_str()) {
                            this.summaries.loading_path = None;
                        }
                        cx.notify();
                    })
                    .ok();
                    summary
                })
            },
        );

        cx.foreground_executor()
            .spawn(async move { settled(shared).await })
    }

    /// `clearRepositorySummary`.
    pub fn clear_repository_summary(&mut self, repo_path: &str, cx: &mut Context<Self>) {
        if self.summaries.remove(repo_path).is_some() {
            cx.notify();
        }
    }

    /// `retainRepositorySummaries`.
    pub fn retain_repository_summaries(
        &mut self,
        cx: &mut Context<Self>,
        mut predicate: impl FnMut(&str, &RepositorySummary) -> bool,
    ) {
        let summaries = &mut self.summaries;
        summaries
            .cache
            .retain(|repo_path, summary| predicate(repo_path, summary));
        summaries
            .head_fingerprints
            .retain(|repo_path, _| summaries.cache.contains_key(repo_path));
        summaries
            .used_at
            .retain(|repo_path, _| summaries.cache.contains_key(repo_path));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_an_unreadable_path_after_its_last_segment() {
        assert_eq!(build_invalid_summary("/home/dev/demo").name, "demo");
        assert_eq!(build_invalid_summary("C:\\work\\demo").name, "demo");
        // Sin último tramo se usa la ruta entera.
        assert_eq!(build_invalid_summary("/home/dev/").name, "/home/dev/");

        let summary = build_invalid_summary("/home/dev/demo");
        assert!(summary.success);
        assert!(!summary.is_valid);
        assert_eq!(summary.repo_path, "/home/dev/demo");
    }
}
