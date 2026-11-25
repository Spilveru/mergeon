//! Selección múltiple del historial en la vista: Ctrl/⌘+clic, Shift+clic,
//! Shift+flechas y Esc, y el resumen que ocupa el sitio del inspector con
//! dos o más commits ("3 commits selected": lista compacta, totales del rango
//! si es seguido y las acciones en lote).

use std::path::Path;
use std::rc::Rc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, FontWeight, SharedString, WeakEntity, div, px, relative, uniform_list,
};
use mergeon_git_query::RangeStats;
use mergeon_model::commit_view_utils::CommitRow;
use mergeon_ui::{ActivePaletteExt as _, Lucide};
use mergeon_view_diff::code_font_family;

use super::{BatchCommand, CommitsView, CommitsViewEvent};
use crate::history_list::row_key;
use crate::inspector::stat;

/// Alto de una fila de la lista compacta del resumen.
pub const MULTI_ROW_HEIGHT: f32 = 26.;

/// Totales del rango seleccionado, tal como los pinta el resumen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MultiStats {
    /// No es un tramo seguido de una línea de historia: no se cuentan.
    #[default]
    NotContiguous,
    Loading,
    Ready(RangeStats),
    Failed,
}

impl CommitsView {
    /// Los commits seleccionados con Ctrl/⌘ o Shift, en el orden de la lista
    /// (vacío con selección simple).
    pub fn multi_selected_hashes(&self) -> Vec<String> {
        self.multi
            .indices()
            .iter()
            .filter_map(|index| self.commits.get(*index))
            .map(|commit| commit.full_hash.clone())
            .collect()
    }

    /// La comparación abierta con "Compare these two", si sigue viva.
    pub fn compare_view(&self) -> Option<&gpui_kit::Entity<crate::compare_view::CompareView>> {
        self.compare.as_ref().map(|(view, _)| view)
    }

    /// Hay dos o más commits seleccionados.
    pub fn has_multi_selection(&self) -> bool {
        self.multi.is_multi()
    }

    /// Los totales del rango seleccionado.
    pub fn multi_stats(&self) -> MultiStats {
        self.multi_stats
    }

    /// Ctrl/⌘+clic en una fila.
    pub fn handle_history_toggle(&mut self, commit: CommitRow, cx: &mut Context<Self>) {
        let primary = self
            .selected_commit
            .as_ref()
            .map(|commit| row_key(commit).to_string());
        let next = self
            .multi
            .toggle(&self.commits, primary.as_deref(), row_key(&commit));
        self.multi_selection_changed(next, cx);
    }

    /// Shift+clic en una fila.
    pub fn handle_history_extend(&mut self, commit: CommitRow, cx: &mut Context<Self>) {
        let primary = self
            .selected_commit
            .as_ref()
            .map(|commit| row_key(commit).to_string());
        let next = self
            .multi
            .select_range(&self.commits, primary.as_deref(), row_key(&commit));
        self.multi_selection_changed(next, cx);
    }

    /// Shift+↑/↓: `step` −1 (arriba) o +1 (abajo). `false` si no se movió.
    pub(super) fn extend_selection_by(&mut self, step: isize, cx: &mut Context<Self>) -> bool {
        let primary = self
            .selected_commit
            .as_ref()
            .map(|commit| row_key(commit).to_string());
        let Some(next) = self
            .multi
            .extend_by(&self.commits, primary.as_deref(), step)
        else {
            return false;
        };
        self.multi_selection_changed(Some(next), cx);
        true
    }

    /// Esc: vuelve a la selección simple en el commit principal.
    pub fn collapse_multi_selection(&mut self, cx: &mut Context<Self>) {
        if self.multi.is_empty() {
            return;
        }
        let anchor = self
            .selected_commit
            .as_ref()
            .map(|commit| row_key(commit).to_string());
        self.multi.collapse(anchor.as_deref());
        self.multi_stats_changed(cx);
        self.publish(cx);
    }

    /// La selección cambió: `next` pasa a ser el commit principal.
    fn multi_selection_changed(&mut self, next: Option<String>, cx: &mut Context<Self>) {
        let next = next.and_then(|key| {
            self.commits
                .iter()
                .find(|commit| row_key(commit) == key)
                .cloned()
        });
        self.multi_stats_changed(cx);
        match next {
            Some(commit) => {
                if let Some(repo) = self.active_repo(cx) {
                    cx.emit(CommitsViewEvent::CommitPicked {
                        repo_id: repo.id,
                        hash: commit.full_hash.clone(),
                    });
                }
                self.select_commit(Some(commit), false, None, cx).detach();
            }
            None => self.publish(cx),
        }
    }

    /// Tras recargar el historial: se queda lo que sigue en la lista.
    pub(super) fn multi_commits_changed(&mut self, cx: &mut Context<Self>) {
        let before = self.multi.set().clone();
        self.multi.retain_loaded(&self.commits);
        if !Rc::ptr_eq(&before, self.multi.set()) {
            self.multi_stats_changed(cx);
        }
    }

    /// Relee los totales si la selección es un rango seguido (una sola
    /// lectura `git diff --shortstat` en segundo plano; la última gana).
    fn multi_stats_changed(&mut self, cx: &mut Context<Self>) {
        let range = self
            .multi
            .is_multi()
            .then(|| self.multi.contiguous_range(&self.commits))
            .flatten();
        if range == self.multi_stats_range && self.multi.is_multi() {
            return;
        }
        self.multi_stats_range = range.clone();
        self.multi_stats_task = None;
        let (Some((from, to)), Some(repo)) = (range, self.active_repo(cx)) else {
            self.multi_stats = MultiStats::NotContiguous;
            return;
        };
        self.multi_stats = MultiStats::Loading;
        let read = cx.background_spawn(async move {
            mergeon_git_query::get_range_stats(Path::new(&repo.path), &from, &to)
        });
        self.multi_stats_task = Some(cx.spawn(async move |this, cx| {
            let stats = read.await;
            let _ = this.update(cx, |this, cx| {
                this.multi_stats = match stats {
                    Ok(stats) => MultiStats::Ready(stats),
                    Err(error) => {
                        log::debug!("{error}");
                        MultiStats::Failed
                    }
                };
                this.multi_stats_task = None;
                cx.notify();
            });
        }));
    }

    /// El resumen de la selección múltiple, en el sitio del inspector.
    pub(super) fn render_multi_summary(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = *cx.palette();
        let mono = code_font_family(cx);
        let count = self.multi.len();
        let entity = cx.weak_entity();

        let batch = |id: &'static str, icon: Lucide, label: String, command: BatchCommand| {
            let entity = entity.clone();
            mergeon_ui::button(id)
                .outline()
                .xs()
                .icon(icon)
                .label(label)
                .on_click(move |_, window, cx| {
                    let _ = entity.update(cx, |view, cx| {
                        view.run_batch_command(command, window, cx);
                    });
                })
        };
        let actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .child(
                batch(
                    "multi-cherry-pick",
                    Lucide::Cherry,
                    format!("Cherry-pick {count}"),
                    BatchCommand::CherryPick,
                )
                .tooltip("Apply these commits on the current branch, oldest first"),
            )
            .child(
                batch(
                    "multi-revert",
                    Lucide::RotateCcw,
                    format!("Revert {count}"),
                    BatchCommand::Revert,
                )
                .tooltip("Undo these commits, one revert commit each"),
            )
            .when(count == 2, |actions| {
                actions.child(
                    batch(
                        "multi-compare",
                        Lucide::GitCompareArrows,
                        "Compare".to_string(),
                        BatchCommand::CompareTwo,
                    )
                    .tooltip("Compare these two commits"),
                )
            })
            .child(
                batch(
                    "multi-copy-shas",
                    Lucide::Copy,
                    "Copy SHAs".to_string(),
                    BatchCommand::CopyShas,
                )
                .tooltip("Copy the full SHAs, oldest first"),
            );

        let title = format!("{count} commits selected");
        let header = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(px(10.))
            .px(px(16.))
            .pt(px(14.))
            .pb(px(12.))
            .border_b_1()
            .border_color(palette.border_hair)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .id("multi-selection-title")
                            .test_support()
                            .aria_label(SharedString::from(title.clone()))
                            .flex_1()
                            .min_w_0()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(palette.text_1)
                            .child(title),
                    )
                    .child(
                        mergeon_ui::button("multi-clear")
                            .ghost()
                            .xs()
                            .icon(Lucide::X)
                            .label("Clear")
                            .tooltip("Back to a single commit (Esc)")
                            .on_click({
                                let entity = entity.clone();
                                move |_, _, cx| {
                                    let _ = entity.update(cx, |view, cx| {
                                        view.collapse_multi_selection(cx);
                                    });
                                }
                            }),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(palette.text_3)
                    .child("Ctrl/\u{2318}-click adds or removes, Shift-click selects a range."),
            )
            .child(actions);

        let blank = matches!(self.multi_stats, MultiStats::Loading);
        let note = match self.multi_stats {
            MultiStats::NotContiguous => {
                Some("Not one contiguous run of history: no combined totals.")
            }
            MultiStats::Failed => Some("Could not count the changes of this range."),
            MultiStats::Loading | MultiStats::Ready(_) => None,
        };
        let stats = match self.multi_stats {
            MultiStats::Ready(stats) => Some(stats),
            MultiStats::Loading => Some(RangeStats::default()),
            _ => None,
        };
        let value = |text: String| if blank { "\u{2013}".to_string() } else { text };
        let totals = div()
            .id("multi-stats")
            .test_support()
            .flex()
            .flex_shrink_0()
            .items_stretch()
            .border_b_1()
            .border_color(palette.border_hair)
            .when_some(stats, |row, stats| {
                row.child(stat(
                    "multi-stat-files",
                    value(stats.files.to_string()),
                    if stats.files == 1 { "FILE" } else { "FILES" },
                    palette.text_1.into(),
                    cx,
                ))
                .child(stat(
                    "multi-stat-additions",
                    value(format!("+{}", stats.insertions)),
                    "LINES",
                    palette.success_foreground.into(),
                    cx,
                ))
                .child(stat(
                    "multi-stat-deletions",
                    value(format!("\u{2212}{}", stats.deletions)),
                    "LINES",
                    palette.destructive_foreground.into(),
                    cx,
                ))
            })
            .when_some(note, |row, note| {
                row.child(
                    div()
                        .id("multi-stats-note")
                        .test_support()
                        .aria_label(note)
                        .px(px(16.))
                        .py(px(10.))
                        .text_size(px(12.))
                        .text_color(palette.text_3)
                        .child(note),
                )
            });

        let rows = self.multi.indices().clone();
        let commits = self.commits.clone();
        let view: WeakEntity<Self> = entity;
        let list = uniform_list("multi-commit-list", rows.len(), move |range, _, cx| {
            rows[range.start..range.end.min(rows.len())]
                .iter()
                .filter_map(|index| commits.get(*index))
                .map(|commit| multi_row(commit, &view, cx))
                .collect()
        })
        .track_scroll(&self.multi_scroll)
        .size_full();

        div()
            .id("multi-selection")
            .test_support()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(palette.card)
            .line_height(relative(1.5))
            .child(header)
            .child(totals)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .py(px(4.))
                    .font_family(mono)
                    .child(list)
                    .vertical_scrollbar(&self.multi_scroll),
            )
            .into_any_element()
    }
}

/// Una fila de la lista compacta: hash corto, asunto, autor y fecha. Un
/// clic vuelve a la selección simple en ese commit.
fn multi_row(commit: &CommitRow, view: &WeakEntity<CommitsView>, cx: &App) -> AnyElement {
    let palette = cx.palette();
    let select = commit.clone();
    let view = view.clone();
    let hash = if commit.hash.is_empty() {
        commit.full_hash.chars().take(7).collect()
    } else {
        commit.hash.clone()
    };
    div()
        .id(SharedString::from(format!(
            "multi-commit:{}",
            commit.full_hash
        )))
        .test_support()
        .flex()
        .items_center()
        .gap(px(10.))
        .w_full()
        .h(px(MULTI_ROW_HEIGHT))
        .px(px(16.))
        .min_w_0()
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|style| style.bg(palette.bg_hover))
        .tooltip(mergeon_ui::tooltip("Show only this commit"))
        .on_click(move |_, _, cx| {
            let _ = view.update(cx, |view, cx| {
                view.collapse_multi_selection(cx);
                view.handle_history_select(select.clone(), cx);
            });
        })
        .child(
            div()
                .flex_shrink_0()
                .w(px(60.))
                .text_color(palette.text_2)
                .child(hash),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(palette.text_1)
                .child(commit.message.clone()),
        )
        .child(
            div()
                .flex_shrink_0()
                .max_w(px(140.))
                .truncate()
                .text_color(palette.text_3)
                .child(commit.author.clone()),
        )
        .child(
            div()
                .flex_shrink_0()
                .w(px(80.))
                .truncate()
                .text_color(palette.text_3)
                .child(crate::history_list::compact_date(commit)),
        )
        .into_any_element()
}
