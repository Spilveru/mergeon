//! Bloques y líneas sueltas del diff a la vista: preparar, quitar del stage y
//! descartar (`handleApplyHunk` y lo que se le añadió). Descartar no se puede
//! deshacer, así que pregunta antes.

use gpui_kit::Context;
use mergeon_backend::ApplyPatchOptions;
use mergeon_model::dialogs::ConfirmOptions;
use mergeon_model::patch::extract_hunk_patch;
use mergeon_view_diff::{HunkRequest, LinesRequest, PatchAction};

use super::ChangesView;
use crate::logic::BusyAction;

/// Qué parte del diff va en el parche: cambia los textos de la confirmación y
/// de los errores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatchScope {
    Hunk,
    Lines(usize),
}

impl PatchScope {
    fn no_patch_message(self) -> &'static str {
        match self {
            Self::Hunk => "Could not build a patch for that block",
            Self::Lines(_) => "Could not build a patch for the selected lines",
        }
    }

    fn confirm(self, path: &str) -> ConfirmOptions {
        let (title, description) = match self {
            Self::Hunk => ("Discard hunk", format!("Discard this hunk in {path}?")),
            Self::Lines(count) => (
                "Discard lines",
                format!(
                    "Discard {count} selected line{} in {path}?",
                    if count == 1 { "" } else { "s" }
                ),
            ),
        };
        ConfirmOptions {
            title: Some(title.to_string()),
            description: Some(description),
            confirm_text: Some("Discard".to_string()),
            cancel_text: Some("Cancel".to_string()),
            variant: Some("destructive".to_string()),
            ..Default::default()
        }
    }
}

impl ChangesView {
    /// `handleApplyHunk`: prepara (o saca del índice) solo el bloque `index`
    /// del diff a la vista.
    pub fn handle_apply_hunk(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(action) = self.hunk_action(cx) else {
            return;
        };
        self.apply_hunk_patch(index, action.primary(), cx);
    }

    /// Clic en "Stage hunk" / "Unstage hunk" / "Discard hunk". El parche sale
    /// del diff que tiene la vista, el mismo que pinta el visor.
    pub fn handle_hunk_request(&mut self, request: &HunkRequest, cx: &mut Context<Self>) {
        self.apply_hunk_patch(request.index, request.action, cx);
    }

    /// Clic en "Stage lines" / "Unstage lines" / "Discard lines" (o su tecla).
    pub fn handle_lines_request(&mut self, request: &LinesRequest, cx: &mut Context<Self>) {
        self.apply_patch(
            request.action,
            request.patch.clone(),
            PatchScope::Lines(request.lines.len()),
            cx,
        );
    }

    fn apply_hunk_patch(&mut self, index: usize, action: PatchAction, cx: &mut Context<Self>) {
        let patch = extract_hunk_patch(&self.file_diff, index);
        self.apply_patch(action, patch, PatchScope::Hunk, cx);
    }

    fn apply_patch(
        &mut self,
        action: PatchAction,
        patch: Option<String>,
        scope: PatchScope,
        cx: &mut Context<Self>,
    ) {
        // Solo lo que se ofrece sobre este diff: lo preparado no se descarta.
        let offered = self
            .hunk_action(cx)
            .is_some_and(|hunk_action| hunk_action.patch_actions().contains(&action));
        if !offered {
            return;
        }
        let (Some(file), Some(patch), Some(repo_path)) =
            (self.selected_file.clone(), patch, self.repo_path(cx))
        else {
            self.model.update(cx, |model, cx| {
                model.notify_error(scope.no_patch_message(), cx);
            });
            return;
        };

        if !action.touches_worktree() {
            self.run_patch(action, repo_path, file.path, patch, cx);
            return;
        }

        let confirmed = self
            .model
            .update(cx, |model, cx| model.confirm(scope.confirm(&file.path), cx));
        cx.spawn(async move |this, cx| {
            if !confirmed.await {
                return;
            }
            this.update(cx, |this, cx| {
                this.run_patch(action, repo_path, file.path, patch, cx);
            })
            .ok();
        })
        .detach();
    }

    fn run_patch(
        &mut self,
        action: PatchAction,
        repo_path: String,
        file_path: String,
        patch: String,
        cx: &mut Context<Self>,
    ) {
        let busy = match action {
            PatchAction::Stage => BusyAction::StageHunk,
            PatchAction::Unstage => BusyAction::UnstageHunk,
            PatchAction::Discard => BusyAction::DiscardHunk,
        };
        let model = self.model.clone();
        self.run_action(busy, cx, async move |this, cx| {
            let options = ApplyPatchOptions {
                reverse: action.is_reverse(),
                worktree: action.touches_worktree(),
                file_path: Some(file_path.clone()),
            };
            let result = model
                .update(cx, |model, cx| {
                    model.apply_patch(&repo_path, &patch, options, cx)
                })
                .await;
            Self::apply_operation_result(&this, &result, Some(file_path), true, cx).await;
        });
    }
}
