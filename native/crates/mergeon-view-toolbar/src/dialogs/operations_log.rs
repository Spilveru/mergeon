//! Diálogo "Operation log"
//! (`src/components/repository/OperationsLogDialog.vue`): cada operación git
//! de la sesión con su resultado y, al desplegarla, su mensaje completo.

use std::rc::Rc;

use chrono::Local;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ListAlignment, ListSizingBehavior, ListState, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, TestSupportExt as _,
    Window, div, list, px, relative, rems,
};
use mergeon_app::AppModel;
use mergeon_model::operation_labels::operation_label;
use mergeon_model::operation_types::OperationPhase;
use mergeon_model::operations_log::OperationLogEntry;
use mergeon_ui::dialog::{AppDialog, DialogId, DialogWidth};
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{ActivePaletteExt as _, MergeonStyled as _, Palette, Radius};

use super::{is_open, scroll_list};
use crate::parts::{mono_font, tint};

const TITLE: &str = "Operation log";

/// Lo que deja pasar "Only errors": terminó, y no fue ni bien ni en nada.
pub(crate) fn is_failed_entry(entry: &OperationLogEntry) -> bool {
    entry
        .outcome
        .as_deref()
        .is_some_and(|outcome| !outcome.is_empty() && outcome != "completed" && outcome != "noop")
}

/// `formatTime`: la hora local del último instante conocido de la operación
/// (`toLocaleTimeString()`: `4:33:27 AM`).
pub(crate) fn entry_time_label(entry: &OperationLogEntry) -> String {
    let known = |timestamp: Option<i64>| timestamp.filter(|timestamp| *timestamp != 0);
    let Some(timestamp) = known(entry.finished_at)
        .or(known(entry.started_at))
        .or(known(entry.queued_at))
        .or(known(Some(entry.created_at)))
    else {
        return String::new();
    };

    time_label_in(timestamp, &Local)
}

fn time_label_in<Tz: chrono::TimeZone>(epoch_ms: i64, zone: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    zone.timestamp_millis_opt(epoch_ms)
        .single()
        .map(|time| time.format("%-I:%M:%S %p").to_string())
        .unwrap_or_default()
}

/// `durationLabel`: lo que tardó, en ms hasta el segundo y en segundos después.
pub(crate) fn entry_duration_label(entry: &OperationLogEntry) -> Option<String> {
    let started = entry.started_at.filter(|started| *started != 0)?;
    let finished = entry.finished_at.filter(|finished| *finished != 0)?;
    let elapsed = finished - started;
    if elapsed < 0 {
        return None;
    }

    Some(if elapsed < 1000 {
        format!("{elapsed} ms")
    } else {
        format!("{:.1} s", elapsed as f64 / 1000.)
    })
}

/// `outcomeKey`: el resultado o, si aún no lo hay, si sigue en la cola.
pub(crate) fn entry_outcome_key(entry: &OperationLogEntry) -> String {
    match entry
        .outcome
        .as_deref()
        .filter(|outcome| !outcome.is_empty())
    {
        Some(outcome) => outcome.to_string(),
        None if matches!(
            entry.phase,
            Some(OperationPhase::Running | OperationPhase::Queued)
        ) =>
        {
            "running".to_string()
        }
        None => "unknown".to_string(),
    }
}

/// `OUTCOME_LABELS`.
pub(crate) fn outcome_label(key: &str) -> &str {
    match key {
        "noop" => "no-op",
        "timeout" => "timed out",
        "unknown" => "pending",
        other => other,
    }
}

/// Fondo y texto de la píldora de resultado (`.outcome-*`). Un resultado sin
/// receta se queda con el texto del diálogo y sin fondo.
fn outcome_colors(key: &str, palette: &Palette) -> (Option<Hsla>, Option<Hsla>) {
    match key {
        "completed" => (
            Some(tint(palette.success, 0.14)),
            Some(palette.success.into()),
        ),
        "noop" | "running" | "unknown" => (
            Some(palette.sidebar_control_surface.into()),
            Some(palette.muted_foreground.into()),
        ),
        "failed" | "timeout" => (
            Some(tint(palette.destructive, 0.14)),
            Some(palette.destructive.into()),
        ),
        "conflicted" | "blocked" | "cancelled" => (
            Some(tint(palette.warning, 0.16)),
            Some(palette.warning.into()),
        ),
        _ => (None, None),
    }
}

/// Hueco entre filas (`gap: 0.2rem`).
const ROW_GAP: f32 = 0.2;

/// Alto supuesto de una fila plegada mientras no se mide: lo usa la barra de
/// scroll para las que aún no se pintaron.
const ESTIMATED_ROW_HEIGHT: f32 = 37.;

/// Lo que muestra una entrada, escrito una vez al recibirla (hora, duración,
/// etiquetas), no en cada frame.
#[derive(Debug, Clone, PartialEq)]
struct LogRow {
    operation_id: String,
    time: SharedString,
    operation: SharedString,
    repo_name: Option<SharedString>,
    duration: Option<SharedString>,
    outcome_key: String,
    failed: bool,
    /// El mensaje completo, si lo hay.
    message: Option<SharedString>,
    /// Lo que dice la fila, para lectores de pantalla: `Push · demo · failed`.
    label: SharedString,
}

impl LogRow {
    fn new(entry: &OperationLogEntry) -> Self {
        let operation: SharedString =
            operation_label(entry.operation.as_deref().unwrap_or_default()).into();
        let outcome_key = entry_outcome_key(entry);
        let repo_name = entry.repo_name.clone().filter(|name| !name.is_empty());
        let label: SharedString = [
            Some(operation.to_string()),
            repo_name.clone(),
            Some(outcome_label(&outcome_key).to_string()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
        .into();
        Self {
            operation_id: entry.operation_id.clone(),
            time: entry_time_label(entry).into(),
            operation,
            repo_name: repo_name.map(SharedString::from),
            duration: entry_duration_label(entry).map(SharedString::from),
            failed: is_failed_entry(entry),
            outcome_key,
            message: entry
                .message
                .clone()
                .filter(|message| !message.is_empty())
                .map(SharedString::from),
            label,
        }
    }
}

/// Contenido del diálogo: qué filtro hay y qué entrada está desplegada.
pub(crate) struct OperationsLogDialog {
    model: Entity<AppModel>,
    only_errors: bool,
    expanded_id: Option<String>,
    /// Las entradas tal como llegaron la última vez, para no rehacer nada si
    /// el modelo cambió por otra cosa.
    source: Vec<OperationLogEntry>,
    rows: Rc<[LogRow]>,
    /// Las filas que deja ver el filtro.
    visible: Rc<[usize]>,
    list: ListState,
    /// La lista no cabe y hace scroll (lo que se vio en el último frame).
    overflowing: bool,
    _observer: Subscription,
}

impl OperationsLogDialog {
    fn new(cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        // Al abrir, los errores acumulados quedan vistos (se apaga el punto rojo).
        model.update(cx, |model, cx| model.mark_operations_seen(cx));
        let observer = cx.observe(&model, |this, model, cx| {
            if this.source != model.read(cx).operations_log().entries {
                this.entries_changed(cx);
                cx.notify();
            }
        });
        let mut this = Self {
            model,
            only_errors: false,
            expanded_id: None,
            source: Vec::new(),
            rows: Rc::from([]),
            visible: Rc::from([]),
            list: ListState::new(0, ListAlignment::Top, px(440.)),
            overflowing: false,
            _observer: observer,
        };
        this.entries_changed(cx);
        this
    }

    /// Rehace las filas a partir del log del modelo.
    fn entries_changed(&mut self, cx: &mut Context<Self>) {
        self.source = self.model.read(cx).operations_log().entries.clone();
        let previous = std::mem::replace(
            &mut self.rows,
            self.source.iter().map(LogRow::new).collect(),
        );
        self.refilter(&previous);
    }

    /// Recalcula qué filas se ven y lo cuenta a la lista virtual. Lo que no
    /// cambió al principio y al final conserva su medida, y el scroll sigue
    /// sobre la misma fila (las entradas nuevas llegan arriba).
    fn refilter(&mut self, previous_rows: &[LogRow]) {
        let visible: Rc<[usize]> = (0..self.rows.len())
            .filter(|ix| !self.only_errors || self.rows[*ix].failed)
            .collect();
        let old: Vec<&LogRow> = self.visible.iter().map(|ix| &previous_rows[*ix]).collect();
        let new: Vec<&LogRow> = visible.iter().map(|ix| &self.rows[*ix]).collect();
        self.visible = visible;

        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        if prefix == old.len() && prefix == new.len() {
            return;
        }
        if prefix == 0 && suffix == 0 {
            self.list.reset(new.len());
        } else {
            self.list
                .splice(prefix..old.len() - suffix, new.len() - prefix - suffix);
        }
        // Las que aún no se midieron cuentan como filas plegadas.
        self.list
            .clone()
            .with_uniform_item_height(px(ESTIMATED_ROW_HEIGHT));
    }

    /// `toggleExpanded`.
    fn toggle_expanded(&mut self, operation_id: &str, cx: &mut Context<Self>) {
        let previous = self.expanded_id.take();
        if previous.as_deref() != Some(operation_id) {
            self.expanded_id = Some(operation_id.to_string());
        }
        // Las filas que cambian de alto se vuelven a medir.
        for id in [previous.as_deref(), self.expanded_id.as_deref()]
            .into_iter()
            .flatten()
        {
            if let Some(ix) = self
                .visible
                .iter()
                .position(|row| self.rows[*row].operation_id == id)
            {
                self.list.remeasure_items(ix..ix + 1);
            }
        }
        cx.notify();
    }

    fn render_item(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let palette = *cx.palette();
        let Some(row) = self.visible.get(ix).and_then(|row| self.rows.get(*row)) else {
            return div().into_any_element();
        };
        let row = row.clone();
        let last = ix + 1 == self.visible.len();
        div()
            .when(!last, |this| this.pb(rems(ROW_GAP)))
            .child(self.render_row(&row, &palette, cx))
            .into_any_element()
    }

    fn render_row(
        &self,
        entry: &LogRow,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let expanded = self.expanded_id.as_deref() == Some(entry.operation_id.as_str());
        let (outcome_background, outcome_color) = outcome_colors(&entry.outcome_key, palette);
        let message = entry.message.clone().filter(|_| expanded);
        let operation_id = entry.operation_id.clone();
        let mono = mono_font(cx);
        let hover = palette.sidebar_row_hover;

        div()
            .id(SharedString::from(format!("ops-log-row-{operation_id}")))
            .test_support()
            .aria_label(entry.label.clone())
            .aria_expanded(expanded)
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(rems(0.3))
            .w_full()
            .border_1()
            .radius(Radius::Lg, cx)
            .py(rems(0.4))
            .px(rems(0.55))
            .cursor_pointer()
            .map(|this| {
                if expanded {
                    this.border_color(with_alpha(palette.border, 0.8))
                        .bg(with_alpha(palette.card, 0.6))
                } else {
                    this.border_color(gpui_kit::transparent_black())
                        .hover(move |style| style.bg(hover))
                }
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_expanded(&operation_id, cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rems(0.55))
                    .w_full()
                    .min_w_0()
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_family(mono.clone())
                            .text_size(rems(0.71))
                            .text_color(palette.muted_foreground)
                            .child(entry.time.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .whitespace_nowrap()
                            .text_size(rems(0.8125))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(palette.foreground)
                            .child(entry.operation.clone()),
                    )
                    .when_some(entry.repo_name.clone(), |this, repo_name| {
                        this.child(
                            div()
                                .truncate()
                                .text_size(rems(0.75))
                                .text_color(palette.muted_foreground)
                                .child(repo_name),
                        )
                    })
                    .child(div().flex_1())
                    .when_some(entry.duration.clone(), |this, duration| {
                        this.child(
                            div()
                                .flex_shrink_0()
                                .text_size(rems(0.71))
                                .text_color(palette.muted_foreground)
                                .child(duration),
                        )
                    })
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(rems(0.71))
                            .font_weight(FontWeight::MEDIUM)
                            .py(rems(0.08))
                            .px(rems(0.45))
                            .rounded_full()
                            .when_some(outcome_background, |this, color| this.bg(color))
                            .when_some(outcome_color, |this, color| this.text_color(color))
                            .child(SharedString::from(
                                outcome_label(&entry.outcome_key).to_string(),
                            )),
                    ),
            )
            .when_some(message, |this, message| {
                this.child(
                    div()
                        .id(SharedString::from(format!(
                            "ops-log-message-{}",
                            entry.operation_id
                        )))
                        .test_support()
                        .aria_label(message.clone())
                        .w_full()
                        .font_family(mono)
                        .text_size(rems(0.73))
                        .line_height(relative(1.45))
                        .text_color(palette.foreground)
                        .py(rems(0.35))
                        .px(rems(0.45))
                        .radius(Radius::Md, cx)
                        .bg(with_alpha(palette.background, 0.7))
                        .child(message),
                )
            })
    }
}

impl Render for OperationsLogDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let has_entries = !self.rows.is_empty();
        let only_errors = self.only_errors;
        let model = self.model.clone();

        div()
            .flex()
            .flex_col()
            .gap(rems(0.6))
            .min_h(px(200.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(rems(0.6))
                    .child(
                        mergeon_ui::checkbox("ops-log-only-errors")
                            .label("Only errors")
                            .checked(only_errors)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.only_errors = *checked;
                                let rows = this.rows.clone();
                                this.refilter(&rows);
                                cx.notify();
                            })),
                    )
                    .child(
                        mergeon_ui::button("ops-log-clear")
                            .ghost()
                            .sm()
                            .label("Clear")
                            .disabled(!has_entries)
                            .on_click(move |_, _, cx| {
                                model.update(cx, |model, cx| model.clear_operations_log(cx));
                            }),
                    ),
            )
            .map(|this| {
                if self.visible.is_empty() {
                    this.child(
                        div()
                            .id("ops-log-empty")
                            .test_support()
                            .aria_label(if only_errors {
                                "No failed operations this session."
                            } else {
                                "No operations yet."
                            })
                            .flex()
                            .justify_center()
                            .py(rems(1.5))
                            .px(rems(0.5))
                            .text_size(rems(0.8125))
                            .text_color(palette.muted_foreground)
                            .child(if only_errors {
                                "No failed operations this session."
                            } else {
                                "No operations yet."
                            }),
                    )
                } else {
                    // Lista virtual: solo se construyen las filas a la vista.
                    let rows = list(
                        self.list.clone(),
                        cx.processor(|this, ix, _, cx| this.render_item(ix, cx)),
                    )
                    .with_sizing_behavior(ListSizingBehavior::Infer);
                    let view = cx.entity();
                    this.child(scroll_list(
                        "ops-log-list",
                        rows,
                        &self.list,
                        440.,
                        self.overflowing,
                        move |overflowing, window, cx| {
                            window.defer(cx, move |_, cx| {
                                view.update(cx, |view, cx| {
                                    view.overflowing = overflowing;
                                    cx.notify();
                                });
                            });
                        },
                    ))
                }
            })
    }
}

/// Abre el log de operaciones de la sesión y marca sus errores como vistos.
/// Devuelve `None` si ya estaba abierto.
pub fn open_operations_log(window: &mut Window, cx: &mut App) -> Option<DialogId> {
    if is_open(TITLE, window, cx) {
        return None;
    }

    let content = cx.new(OperationsLogDialog::new);
    Some(
        AppDialog::new()
            .title(TITLE)
            .description("Every git operation from this session, with full messages.")
            .width(DialogWidth::Xl3)
            .body(move |_, _| content.clone())
            .open(window, cx),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn entry(outcome: Option<&str>, phase: Option<OperationPhase>) -> OperationLogEntry {
        OperationLogEntry {
            operation_id: "op-1".to_string(),
            outcome: outcome.map(str::to_string),
            phase,
            ..Default::default()
        }
    }

    #[test]
    fn only_errors_keeps_what_did_not_end_well() {
        assert!(is_failed_entry(&entry(Some("failed"), None)));
        assert!(is_failed_entry(&entry(Some("cancelled"), None)));
        assert!(is_failed_entry(&entry(Some("blocked"), None)));
        assert!(!is_failed_entry(&entry(Some("completed"), None)));
        assert!(!is_failed_entry(&entry(Some("noop"), None)));
        // Una operación todavía sin resultado no es un error.
        assert!(!is_failed_entry(&entry(
            None,
            Some(OperationPhase::Running)
        )));
    }

    #[test]
    fn an_entry_without_outcome_reads_as_running_or_pending() {
        let running = entry(None, Some(OperationPhase::Running));
        let queued = entry(None, Some(OperationPhase::Queued));
        let unknown = entry(None, None);

        assert_eq!(outcome_label(&entry_outcome_key(&running)), "running");
        assert_eq!(outcome_label(&entry_outcome_key(&queued)), "running");
        assert_eq!(outcome_label(&entry_outcome_key(&unknown)), "pending");
    }

    #[test]
    fn outcomes_keep_their_name_except_the_reworded_ones() {
        assert_eq!(outcome_label("completed"), "completed");
        assert_eq!(outcome_label("noop"), "no-op");
        assert_eq!(outcome_label("timeout"), "timed out");
        assert_eq!(outcome_label("conflicted"), "conflicted");
        assert_eq!(outcome_label("something-new"), "something-new");
    }

    #[test]
    fn durations_switch_from_milliseconds_to_seconds() {
        let timed = |started, finished| OperationLogEntry {
            started_at: Some(started),
            finished_at: Some(finished),
            ..Default::default()
        };

        assert_eq!(
            entry_duration_label(&timed(1_000, 1_044)).as_deref(),
            Some("44 ms")
        );
        assert_eq!(
            entry_duration_label(&timed(1_000, 4_600)).as_deref(),
            Some("3.6 s")
        );
        assert_eq!(entry_duration_label(&timed(2_000, 1_000)), None);
        assert_eq!(
            entry_duration_label(&OperationLogEntry {
                finished_at: Some(5),
                ..Default::default()
            }),
            None
        );
    }

    #[test]
    fn the_time_is_the_local_time_of_day_with_seconds() {
        let utc = FixedOffset::east_opt(0).expect("utc");
        // 2026-10-02T04:33:27Z
        assert_eq!(time_label_in(1_790_915_607_000, &utc), "4:33:27 AM");
        // 2026-10-02T16:05:09Z
        assert_eq!(time_label_in(1_790_957_109_000, &utc), "4:05:09 PM");
    }

    #[test]
    fn the_time_shown_is_the_latest_known_instant() {
        let finished = OperationLogEntry {
            created_at: 1_000,
            queued_at: Some(2_000),
            started_at: Some(3_000),
            finished_at: Some(4_000),
            ..Default::default()
        };
        let queued = OperationLogEntry {
            created_at: 1_000,
            queued_at: Some(2_000),
            ..Default::default()
        };

        assert_eq!(entry_time_label(&finished), time_label_in(4_000, &Local));
        assert_eq!(entry_time_label(&queued), time_label_in(2_000, &Local));
        assert_eq!(entry_time_label(&OperationLogEntry::default()), "");
    }
}
