//! Barra de estado inferior de Señal (`.status` del diseño): solo lo que no
//! está ya a la vista. El repositorio está en su pestaña y la rama (con lo que
//! lleva de ventaja o retraso) en la barra de herramientas, así que aquí van
//! el upstream, cuándo se habló por última vez con el remoto (clic: Fetch), el
//! PR de la rama en GitHub y, a la derecha, la posición en el historial. La
//! versión de git queda en About.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task,
    TestSupportExt as _, Window, div, px,
};
use mergeon_app::AppModel;
use mergeon_model::format::now_epoch_ms;
use mergeon_model::operations_log::OperationsLog;
use mergeon_ui::style::text;
use mergeon_ui::{ActivePaletteExt as _, IconTone, Lucide};

use crate::parts::mono_font;
use crate::pull_request_indicator::PullRequestIndicator;

/// Operaciones tras las que el repositorio acaba de hablar con su remoto.
const REMOTE_OPERATIONS: [&str; 3] = ["fetch", "pull", "push"];

/// Cada cuánto se repinta "fetched N min ago" mientras haya una hora que
/// mostrar.
const REFRESH_EVERY: Duration = Duration::from_secs(30);

/// Posición del commit seleccionado en el historial cargado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HistoryPosition {
    /// 1-based; `None` sin commit seleccionado.
    pub selected: Option<usize>,
    /// Commits cargados (no los del repositorio entero: el historial se pide
    /// por páginas).
    pub total: usize,
}

/// "1284" → "1,284".
fn group_thousands(value: usize) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// "27 / 1,284 commits", o solo el total si no hay commit seleccionado.
pub fn history_position_label(position: HistoryPosition) -> String {
    let total = group_thousands(position.total);
    let noun = if position.total == 1 {
        "commit"
    } else {
        "commits"
    };
    match position.selected {
        Some(selected) => format!("{} / {total} {noun}", group_thousands(selected)),
        None => format!("{total} {noun}"),
    }
}

/// "fetched 3 min ago" a partir de dos instantes en epoch ms.
pub fn fetched_ago_label(now_ms: i64, fetched_at_ms: i64) -> String {
    let minutes = (now_ms - fetched_at_ms).max(0) / 60_000;
    match minutes {
        0 => "fetched just now".to_string(),
        1..=59 => format!("fetched {minutes} min ago"),
        60..=1439 => format!("fetched {} h ago", minutes / 60),
        _ => format!("fetched {} d ago", minutes / 1440),
    }
}

/// `git version 2.47.1` → `git 2.47.1` (y `2.47.1.windows.1` → `2.47.1`).
pub fn git_version_label(output: &str) -> Option<String> {
    let version = output.trim().strip_prefix("git version ")?.trim();
    let version = version.split(".windows").next().unwrap_or(version);
    let version = version.split_whitespace().next()?;
    (!version.is_empty()).then(|| format!("git {version}"))
}

/// Cuándo terminó bien el último fetch, pull o push de un repositorio según
/// el log de operaciones de la sesión (epoch ms).
pub fn last_remote_sync(log: &OperationsLog, repo_path: &str) -> Option<i64> {
    last_remote_syncs(log, [repo_path]).remove(repo_path)
}

/// Cuándo terminó bien el último fetch, pull o push de cada repositorio de
/// `paths` según el log (epoch ms), en una sola pasada por el log.
fn last_remote_syncs<'a>(
    log: &OperationsLog,
    paths: impl IntoIterator<Item = &'a str>,
) -> HashMap<String, i64> {
    let mut latest: HashMap<String, i64> = paths
        .into_iter()
        .map(|path| (path.to_string(), i64::MIN))
        .collect();
    for entry in &log.entries {
        let Some(path) = entry.repo_path.as_deref() else {
            continue;
        };
        let is_remote = entry
            .operation
            .as_deref()
            .is_some_and(|operation| REMOTE_OPERATIONS.contains(&operation));
        let succeeded = matches!(entry.outcome.as_deref(), Some("completed" | "noop"));
        if let (true, true, Some(finished_at), Some(known)) = (
            is_remote,
            succeeded,
            entry.finished_at,
            latest.get_mut(path),
        ) {
            *known = (*known).max(finished_at);
        }
    }
    latest.retain(|_, at| *at != i64::MIN);
    latest
}

/// Lo que la barra enseña del modelo. La vista solo se repinta cuando cambia
/// (el modelo avisa por cualquier cosa, y el reloj de "fetched N min ago"
/// late cada 30 s aunque el texto siga igual).
#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusLine {
    upstream: Option<String>,
    fetched: Option<String>,
}

/// Barra de estado del repositorio activo. Sin repositorio no dibuja nada.
///
/// La hora del último fetch / pull / push no la guarda la app: esta vista la
/// recoge del log de operaciones cada vez que cambia el modelo y la conserva
/// en memoria por repositorio abierto (así sobrevive a "Clear" del log; al
/// cerrar el repositorio se olvida). La posición en el historial se la da la
/// cáscara con [`StatusBarView::set_history`].
pub struct StatusBarView {
    model: Entity<AppModel>,
    /// Ruta del repositorio → último fetch/pull/push terminado (epoch ms).
    remote_synced_at: HashMap<String, i64>,
    history: Option<HistoryPosition>,
    /// Lo último que se pintó (o se iba a pintar) del modelo.
    shown: Option<StatusLine>,
    renders: usize,
    /// El PR de la rama actual (`pull_request_indicator.rs`).
    pull_request: Entity<PullRequestIndicator>,
    _observer: Subscription,
    _ticker: Task<()>,
}

impl StatusBarView {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        let observer = cx.observe(&model, |this, _, cx| {
            this.remember_remote_syncs(cx);
            this.notify_if_changed(cx);
        });

        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(REFRESH_EVERY).await;
                let alive = this.update(cx, |this, cx| {
                    if !this.remote_synced_at.is_empty() {
                        this.notify_if_changed(cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });

        let mut this = Self {
            model,
            remote_synced_at: HashMap::new(),
            history: None,
            shown: None,
            renders: 0,
            pull_request: cx.new(PullRequestIndicator::new),
            _observer: observer,
            _ticker: ticker,
        };
        this.remember_remote_syncs(cx);
        this.shown = this.status_line(cx);
        // El observador empieza a escuchar al final de este ciclo: un aviso
        // del modelo que ya estuviera en cola no le llegaría.
        let view = cx.weak_entity();
        cx.defer(move |cx| {
            view.update(cx, |this, cx| {
                this.remember_remote_syncs(cx);
                this.notify_if_changed(cx);
            })
            .ok();
        });
        this
    }

    /// La cáscara pasa aquí la posición en el historial del repositorio
    /// activo (`None` si no la sabe: no se muestra).
    pub fn set_history(&mut self, history: Option<HistoryPosition>, cx: &mut Context<Self>) {
        if self.history != history {
            self.history = history;
            cx.notify();
        }
    }

    /// Hora del último fetch / pull / push que esta vista conoce para un
    /// repositorio (epoch ms).
    pub fn remote_synced_at(&self, repo_path: &str) -> Option<i64> {
        self.remote_synced_at.get(repo_path).copied()
    }

    /// Repositorios de los que la vista guarda la hora del último fetch.
    #[doc(hidden)]
    pub fn remembered_remote_syncs(&self) -> usize {
        self.remote_synced_at.len()
    }

    /// Veces que se pintó (tests).
    #[doc(hidden)]
    pub fn render_count(&self) -> usize {
        self.renders
    }

    /// El indicador del PR de la rama actual.
    pub fn pull_request_indicator(&self) -> &Entity<PullRequestIndicator> {
        &self.pull_request
    }

    fn remember_remote_syncs(&mut self, cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        let open: Vec<&str> = model
            .open_repositories()
            .iter()
            .map(|repository| repository.path.as_str())
            .collect();
        // Los repositorios cerrados se olvidan: si se reabren, el log (si no
        // se vació) vuelve a decir cuándo fue.
        self.remote_synced_at
            .retain(|path, _| open.contains(&path.as_str()));
        for (path, at) in last_remote_syncs(model.operations_log(), open) {
            let known = self.remote_synced_at.entry(path).or_insert(at);
            *known = (*known).max(at);
        }
    }

    fn status_line(&self, cx: &Context<Self>) -> Option<StatusLine> {
        let model = self.model.read(cx);
        let repository = model.active_repository()?;
        let branch_entry = model.active_branch_entry();
        Some(StatusLine {
            upstream: branch_entry
                .and_then(|entry| entry.upstream.clone())
                .filter(|upstream| !upstream.is_empty()),
            fetched: self
                .remote_synced_at(&repository.path)
                .map(|at| fetched_ago_label(now_epoch_ms(), at)),
        })
    }

    fn notify_if_changed(&mut self, cx: &mut Context<Self>) {
        let line = self.status_line(cx);
        if line != self.shown {
            self.shown = line;
            cx.notify();
        }
    }
}

impl StatusBarView {
    /// Clic en "fetched N min ago": otro Fetch.
    fn fetch(&mut self, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| model.handle_fetch(cx));
    }
}

impl Render for StatusBarView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        // Lo leído al avisar el modelo (o el reloj del "fetched ago"): pintar
        // no lee el modelo, que repintaría la ventana con cada aviso suyo.
        let line = self.shown.clone();
        let Some(StatusLine { upstream, fetched }) = line else {
            return div().into_any_element();
        };

        let palette = *cx.palette();
        div()
            .id("status-bar")
            .test_support()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(12.))
            .w_full()
            .h(px(24.))
            .px(px(8.))
            .overflow_hidden()
            .bg(palette.bg_sunken)
            .border_t_1()
            .border_color(palette.border_hair)
            .font_family(mono_font(cx))
            .text_size(text::CAPTION)
            .text_color(palette.text_3)
            .whitespace_nowrap()
            .when_some(upstream, |bar, upstream| {
                let upstream = SharedString::from(upstream);
                bar.child(
                    div()
                        .id("status-upstream")
                        .test_support()
                        .aria_label(upstream.clone())
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(4.))
                        .px(px(4.))
                        .tooltip(mergeon_ui::tooltip("Upstream of the current branch"))
                        .child(
                            mergeon_ui::icon(Lucide::Globe)
                                .size(px(12.))
                                .tone(IconTone::Muted),
                        )
                        .child(upstream),
                )
            })
            .when_some(fetched, |bar, fetched| {
                let fetched = SharedString::from(fetched);
                let hover = palette.bg_hover;
                let text_1 = palette.text_1;
                bar.child(
                    div()
                        .id("status-fetched")
                        .test_support()
                        .aria_label(fetched.clone())
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(4.))
                        .h(px(20.))
                        .px(px(4.))
                        .rounded(mergeon_ui::Radius::Xs.px(cx))
                        .cursor_pointer()
                        .hover(move |el| el.bg(hover).text_color(text_1))
                        .tooltip(mergeon_ui::tooltip("Fetch now"))
                        .child(
                            mergeon_ui::icon(Lucide::RefreshCw)
                                .size(px(12.))
                                .tone(IconTone::Current),
                        )
                        .child(fetched)
                        .on_click(cx.listener(|this, _, _, cx| this.fetch(cx))),
                )
            })
            .child(self.pull_request.clone())
            .child(div().flex_1())
            .when_some(self.history, |bar, history| {
                let label = SharedString::from(history_position_label(history));
                bar.child(
                    div()
                        .id("status-history")
                        .test_support()
                        .aria_label(label.clone())
                        .flex_shrink_0()
                        .child(label),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mergeon_model::operations_log::OperationLogEntry;

    #[test]
    fn counts_the_history_position_with_thousands_separators() {
        let position = HistoryPosition {
            selected: Some(27),
            total: 1284,
        };
        assert_eq!(history_position_label(position), "27 / 1,284 commits");
        assert_eq!(
            history_position_label(HistoryPosition {
                selected: None,
                total: 12,
            }),
            "12 commits"
        );
        assert_eq!(
            history_position_label(HistoryPosition {
                selected: Some(1),
                total: 1,
            }),
            "1 / 1 commit"
        );
        assert_eq!(group_thousands(1_234_567), "1,234,567");
        assert_eq!(group_thousands(999), "999");
    }

    #[test]
    fn tells_how_long_ago_the_last_fetch_was() {
        let now = 10_000_000_000;
        assert_eq!(fetched_ago_label(now, now - 20_000), "fetched just now");
        assert_eq!(
            fetched_ago_label(now, now - 3 * 60_000),
            "fetched 3 min ago"
        );
        assert_eq!(
            fetched_ago_label(now, now - 2 * 3_600_000),
            "fetched 2 h ago"
        );
        assert_eq!(
            fetched_ago_label(now, now - 3 * 86_400_000),
            "fetched 3 d ago"
        );
        // Un reloj que retrocede no da tiempos negativos.
        assert_eq!(fetched_ago_label(now, now + 60_000), "fetched just now");
    }

    #[test]
    fn reads_the_git_version() {
        assert_eq!(
            git_version_label("git version 2.47.1\n").as_deref(),
            Some("git 2.47.1")
        );
        assert_eq!(
            git_version_label("git version 2.47.1.windows.2").as_deref(),
            Some("git 2.47.1")
        );
        assert_eq!(
            git_version_label("git version 2.39.5 (Apple Git-154)").as_deref(),
            Some("git 2.39.5")
        );
        assert_eq!(git_version_label("command not found"), None);
    }

    fn entry(operation: &str, path: &str, outcome: &str, finished_at: i64) -> OperationLogEntry {
        OperationLogEntry {
            operation_id: format!("{operation}-{finished_at}"),
            repo_path: Some(path.to_string()),
            operation: Some(operation.to_string()),
            outcome: Some(outcome.to_string()),
            finished_at: Some(finished_at),
            ..Default::default()
        }
    }

    #[test]
    fn the_last_remote_sync_is_the_latest_successful_fetch_pull_or_push_of_that_repository() {
        let log = OperationsLog {
            entries: vec![
                entry("fetch", "/other", "completed", 900),
                entry("commit", "/repo", "completed", 800),
                entry("push", "/repo", "failed", 700),
                entry("pull", "/repo", "noop", 500),
                entry("fetch", "/repo", "completed", 300),
            ],
            unseen_errors: 0,
        };
        assert_eq!(last_remote_sync(&log, "/repo"), Some(500));
        assert_eq!(last_remote_sync(&log, "/other"), Some(900));
        assert_eq!(last_remote_sync(&log, "/nowhere"), None);

        // Todos los repositorios de una pasada, solo los pedidos.
        let all = last_remote_syncs(&log, ["/repo", "/other", "/nowhere"]);
        assert_eq!(all.len(), 2);
        assert_eq!(all.get("/repo"), Some(&500));
        assert_eq!(all.get("/other"), Some(&900));
        assert!(!last_remote_syncs(&log, ["/repo"]).contains_key("/other"));
    }
}
