//! Indicador del PR de la rama actual en la barra de estado:
//! `#123 · ✓ 5/5 checks` (✗ si alguno falla, ● si faltan), con tooltip y
//! clic que abre el PR en el navegador. Sin PR (o sin cuenta de GitHub) no
//! pinta nada. Los datos los mantiene `AppModel`
//! (`mergeon_app::pull_requests`).

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Context, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, TestSupportExt as _, Window, div, px,
};
use mergeon_app::{AppModel, PullRequestItem};
use mergeon_github::{CheckState, ReviewDecision};
use mergeon_ui::{ActivePaletteExt as _, IconTone, Lucide};

/// Lo que enseña el indicador.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Shown {
    number: u64,
    url: String,
    checks: Option<(CheckState, String)>,
    tooltip: String,
}

/// "Approved", "Changes requested"... para el tooltip.
fn review_text(item: &PullRequestItem) -> Option<&'static str> {
    match item.review?.decision {
        ReviewDecision::Approved => Some("Approved"),
        ReviewDecision::ChangesRequested => Some("Changes requested"),
        ReviewDecision::Pending => Some("Review pending"),
        ReviewDecision::None => None,
    }
}

/// El texto del indicador sin íconos: `#123 · 5/5 checks`.
pub fn pull_request_indicator_label(item: &PullRequestItem) -> String {
    match item.checks.filter(|checks| checks.total > 0) {
        Some(checks) => format!("#{} · {}", item.pull.number, checks.label()),
        None => format!("#{}", item.pull.number),
    }
}

/// El tooltip: título, estado de los checks, revisión y qué hace el clic.
pub fn pull_request_indicator_tooltip(item: &PullRequestItem) -> String {
    let mut lines = vec![format!("#{} {}", item.pull.number, item.pull.title)];
    if let Some(checks) = item.checks.filter(|checks| checks.total > 0) {
        lines.push(match checks.state() {
            Some(CheckState::Failure) => {
                format!("{} failing of {}", checks.failed, checks.total)
            }
            Some(CheckState::Pending) => {
                format!("{} pending of {}", checks.pending, checks.total)
            }
            _ if checks.total == 1 => "The check passed".to_string(),
            _ => format!("All {} checks passed", checks.total),
        });
    } else {
        lines.push("No checks".to_string());
    }
    if let Some(review) = review_text(item) {
        lines.push(review.to_string());
    }
    lines.push("Click to open on GitHub".to_string());
    lines.join("\n")
}

fn state_style(state: CheckState, cx: &App) -> (Lucide, Hsla) {
    let palette = cx.palette();
    match state {
        CheckState::Success => (Lucide::CircleCheck, palette.success_foreground.into()),
        CheckState::Failure => (Lucide::CircleX, palette.destructive_foreground.into()),
        CheckState::Pending => (Lucide::CircleDot, palette.warning_foreground.into()),
    }
}

type SourceKey = (Option<String>, Option<String>, Option<u64>);

/// El indicador; lo pinta [`crate::StatusBarView`].
pub struct PullRequestIndicator {
    model: Entity<AppModel>,
    /// De qué se sacó `shown` (repositorio, rama, generación de sus PRs).
    source: SourceKey,
    shown: Option<Shown>,
    renders: usize,
    _observer: Subscription,
}

impl PullRequestIndicator {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        let observer = cx.observe(&model, |this, model, cx| {
            if !model.read(cx).is_watching_pull_requests() {
                model.update(cx, |model, cx| model.watch_pull_requests(cx));
            }
            this.sync_shown(cx);
        });
        model.update(cx, |model, cx| model.watch_pull_requests(cx));
        let source = Self::source_key(model.read(cx));
        let mut this = Self {
            model,
            source,
            shown: None,
            renders: 0,
            _observer: observer,
        };
        this.shown = this.read_shown(cx);
        // El observador empieza a escuchar al final de este ciclo: un aviso
        // del modelo que ya estuviera en cola no le llegaría.
        let view = cx.weak_entity();
        cx.defer(move |cx| {
            view.update(cx, |this, cx| this.sync_shown(cx)).ok();
        });
        this
    }

    /// Recoge lo que enseña si cambió el estado de los PRs, el repositorio o
    /// la rama (lo barato primero), y repinta si cambió el texto.
    fn sync_shown(&mut self, cx: &mut Context<Self>) {
        let key = Self::source_key(self.model.read(cx));
        if key == self.source {
            return;
        }
        self.source = key;
        let shown = self.read_shown(cx);
        if shown != self.shown {
            self.shown = shown;
            cx.notify();
        }
    }

    fn source_key(model: &AppModel) -> SourceKey {
        let repository = model.active_repository();
        (
            repository.map(|repository| repository.id.clone()),
            repository.and_then(|repository| repository.current_branch.clone()),
            model.active_pull_requests().map(|state| state.generation),
        )
    }

    /// Veces que se pintó (tests).
    #[doc(hidden)]
    pub fn render_count(&self) -> usize {
        self.renders
    }

    /// El texto que enseña ahora, sin íconos (`None`: no se ve).
    pub fn label(&self, cx: &App) -> Option<String> {
        self.model
            .read(cx)
            .active_branch_pull_request()
            .map(pull_request_indicator_label)
    }

    fn read_shown(&self, cx: &App) -> Option<Shown> {
        let item = self.model.read(cx).active_branch_pull_request()?;
        Some(Shown {
            number: item.pull.number,
            url: item.pull.html_url.clone(),
            checks: item
                .checks
                .filter(|checks| checks.total > 0)
                .and_then(|checks| Some((checks.state()?, checks.label()))),
            tooltip: pull_request_indicator_tooltip(item),
        })
    }
}

impl Render for PullRequestIndicator {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        // Lo leído al cambiar los PRs, el repositorio o la rama: pintar no
        // lee el modelo, que repintaría la ventana con cada aviso suyo.
        let Some(shown) = self.shown.clone() else {
            return div().into_any_element();
        };
        let palette = *cx.palette();
        let url = shown.url.clone();
        let mut label = format!("#{}", shown.number);
        if let Some((_, checks)) = &shown.checks {
            label.push_str(" · ");
            label.push_str(checks);
        }

        div()
            .id("status-pull-request")
            .test_support()
            .aria_label(SharedString::from(label))
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .hover(|style| style.text_color(palette.text_1))
            .tooltip(mergeon_ui::tooltip(shown.tooltip.clone()))
            .on_click(cx.listener(move |this, _, _, cx| {
                let url = url.clone();
                this.model
                    .update(cx, |model, cx| model.open_external(&url, cx))
                    .detach();
            }))
            .child(
                mergeon_ui::icon(Lucide::GitPullRequest)
                    .size(px(11.))
                    .tone(IconTone::Current),
            )
            .child(
                div()
                    .text_color(palette.text_2)
                    .font_weight(FontWeight::MEDIUM)
                    .child(format!("#{}", shown.number)),
            )
            .when_some(shown.checks, |this, (state, checks)| {
                let (icon, color) = state_style(state, cx);
                this.child(div().child("·")).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .text_color(color)
                        .child(mergeon_ui::icon(icon).size(px(11.)).tone(IconTone::Current))
                        .child(checks),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mergeon_github::{CheckSummary, GitHubPullRequest, ReviewSummary};

    fn item(checks: Option<CheckSummary>) -> PullRequestItem {
        PullRequestItem {
            pull: GitHubPullRequest {
                number: 123,
                title: "Round VAT per line".into(),
                html_url: "https://github.com/acme/web/pull/123".into(),
                draft: false,
                author: Some("ana".into()),
                author_avatar_url: None,
                head_ref: "fix/vat".into(),
                head_sha: "abc".into(),
                head_repository: Some("acme/web".into()),
                head_owner: Some("acme".into()),
                base_ref: "main".into(),
                same_repository: true,
                updated_at: None,
                requested_reviewers: 0,
            },
            checks,
            review: Some(ReviewSummary {
                decision: ReviewDecision::Approved,
                approvals: 1,
                changes_requested: 0,
            }),
        }
    }

    #[test]
    fn says_the_number_and_how_many_checks_passed() {
        let green = item(Some(CheckSummary {
            total: 5,
            passed: 5,
            failed: 0,
            pending: 0,
        }));
        assert_eq!(pull_request_indicator_label(&green), "#123 · 5/5 checks");
        assert_eq!(
            pull_request_indicator_tooltip(&green),
            "#123 Round VAT per line\nAll 5 checks passed\nApproved\nClick to open on GitHub"
        );

        let failing = item(Some(CheckSummary {
            total: 5,
            passed: 3,
            failed: 1,
            pending: 1,
        }));
        assert_eq!(pull_request_indicator_label(&failing), "#123 · 3/5 checks");
        assert!(pull_request_indicator_tooltip(&failing).contains("1 failing of 5"));

        let none = item(None);
        assert_eq!(pull_request_indicator_label(&none), "#123");
        assert!(pull_request_indicator_tooltip(&none).contains("No checks"));
    }
}
