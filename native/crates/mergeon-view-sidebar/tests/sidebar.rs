//! Port de `src/components/layout/Sidebar.test.js` y de los tests de sus
//! secciones: el sidebar real en una ventana sin pantalla, sobre un `AppModel`
//! real y repositorios git reales. Donde el original contaba llamadas a un
//! `window.api.git` simulado, aquí se cuentan las lecturas de cada sección que
//! arranca el modelo y se mira el resultado en el repositorio.

mod support;

use std::time::Duration;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, px};
use mergeon_backend::CompareMode;
use mergeon_gitcmd::Git;
use mergeon_model::notify::NotifyType;
use mergeon_model::workspace_tabs::WorkspaceSection;
use mergeon_view_sidebar::logic::{DatePreset, SearchFilterFields};
use mergeon_view_sidebar::{ComparisonRequest, PanelMode, Section, SidebarCommand, SidebarEvent};

use support::{Harness, TestRepo, default_sections, sections};

/// Lecturas de varias secciones a la vez, para comparar antes y después.
fn reads(h: &Harness, cx: &mut TestAppContext) -> [usize; 5] {
    ["status", "branches", "remotes", "tags", "stashes"].map(|section| h.reads(cx, section))
}

const STATUS: usize = 0;
const BRANCHES: usize = 1;
const REMOTES: usize = 2;
const TAGS: usize = 3;
const STASHES: usize = 4;

// ---------------------------------------------------------------------------
// Checkout
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn refreshes_only_branches_after_a_clean_local_checkout_result_with_embedded_status(
    cx: &mut TestAppContext,
) {
    let repo = TestRepo::with_feature();
    let h = Harness::open(&repo, cx).await;

    assert!(h.exists(cx, "branch:feature"));
    let before = reads(&h, cx);

    h.double_click(cx, "branch:feature");
    h.wait_repo(cx, "the checkout", |repository| {
        repository.current_branch.as_deref() == Some("feature")
            && repository.last_mutation_operation.as_deref() == Some("checkout")
    });
    h.settle(cx);

    assert_eq!(repo.current_branch(), "feature");
    let after = reads(&h, cx);
    assert_eq!(after[BRANCHES], before[BRANCHES] + 1);
    assert_eq!(after[TAGS], before[TAGS]);
    assert_eq!(after[STASHES], before[STASHES]);
    // El resultado del checkout ya trae el estado: no se vuelve a leer.
    assert_eq!(after[STATUS], before[STATUS]);
}

#[gpui_kit::test]
async fn shows_a_checkout_dialog_for_dirty_branches_and_can_stash_then_reapply_before_switching(
    cx: &mut TestAppContext,
) {
    let repo = TestRepo::with_feature();
    repo.dirty();
    let h = Harness::open(&repo, cx).await;
    let before = reads(&h, cx);

    h.double_click(cx, "branch:feature");
    h.wait_ui(cx, "the checkout dialog", |window, cx| {
        mergeon_ui::dialog::open_count(window, cx) == 1
    });
    assert_eq!(h.dialog_titles(cx), ["Switch Branch"]);
    // Todavía no se ha tocado nada.
    assert_eq!(repo.current_branch(), "main");

    h.click(cx, "radio-stash");
    h.click(cx, "checkout-submit");
    h.wait_repo(cx, "the checkout", |repository| {
        repository.current_branch.as_deref() == Some("feature")
            && repository.last_mutation_operation.as_deref() == Some("checkout")
    });
    h.settle(cx);

    assert_eq!(repo.current_branch(), "feature");
    // El stash se creó y se devolvió al working tree.
    assert_eq!(repo.stash_count(), 0);
    assert!(
        std::fs::read_to_string(repo.path.join("README.md"))
            .unwrap()
            .contains("local change")
    );
    let after = reads(&h, cx);
    assert_eq!(after[BRANCHES], before[BRANCHES] + 1);
    assert_eq!(after[STASHES], before[STASHES] + 1);
    assert_eq!(after[TAGS], before[TAGS]);
    assert!(h.dialog_titles(cx).is_empty());
}

#[gpui_kit::test]
async fn allows_keeping_local_changes_through_the_checkout_dialog(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    repo.dirty();
    let h = Harness::open(&repo, cx).await;

    h.double_click(cx, "branch:feature");
    h.wait_ui(cx, "the checkout dialog", |window, cx| {
        mergeon_ui::dialog::open_count(window, cx) == 1
    });
    let target = h.sidebar.read_with(cx, |sidebar, cx| {
        sidebar
            .checkout_dialog()
            .map(|dialog| dialog.read(cx).options().target_label.to_string())
    });
    assert_eq!(target.as_deref(), Some("feature"));

    h.click(cx, "checkout-submit");
    h.wait_repo(cx, "the checkout", |repository| {
        repository.current_branch.as_deref() == Some("feature")
    });
    h.settle(cx);

    assert_eq!(repo.current_branch(), "feature");
    assert_eq!(repo.stash_count(), 0);
    assert!(
        std::fs::read_to_string(repo.path.join("README.md"))
            .unwrap()
            .contains("local change")
    );
}

#[gpui_kit::test]
async fn cancelling_the_checkout_dialog_changes_nothing(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    repo.dirty();
    let h = Harness::open(&repo, cx).await;

    h.double_click(cx, "branch:feature");
    h.wait_ui(cx, "the checkout dialog", |window, cx| {
        mergeon_ui::dialog::open_count(window, cx) == 1
    });
    h.click(cx, "checkout-cancel");
    h.settle(cx);

    assert!(h.dialog_titles(cx).is_empty());
    assert_eq!(repo.current_branch(), "main");
    assert!(
        h.sidebar
            .read_with(cx, |sidebar, _| sidebar.checkout_dialog().is_none())
    );
}

#[gpui_kit::test]
async fn blocks_branch_checkout_when_a_git_operation_is_already_in_progress(
    cx: &mut TestAppContext,
) {
    let repo = TestRepo::with_feature();
    // `main` y `feature` cambian la misma línea: el merge queda en conflicto.
    repo.git(&["checkout", "feature"]);
    repo.commit_file("README.md", "# Feature\n", "Feature readme");
    repo.git(&["checkout", "main"]);
    repo.commit_file("README.md", "# Main\n", "Main readme");
    let _ = Git::new(&repo.path).raw(["merge", "feature"]);
    let h = Harness::open(&repo, cx).await;

    h.double_click(cx, "branch:feature");
    h.settle(cx);

    let warnings = h.host.messages(NotifyType::Warning);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].starts_with("Resolve or abort the current merge")
            && warnings[0].ends_with("before switching branches."),
        "{warnings:?}"
    );
    assert_eq!(repo.current_branch(), "main");
    assert!(h.dialog_titles(cx).is_empty());
}

#[gpui_kit::test]
async fn checks_out_remote_branches_through_the_structured_flow_without_reloading_unrelated_sections(
    cx: &mut TestAppContext,
) {
    let repo = TestRepo::with_feature();
    repo.publish_to_origin();
    repo.publish_remote_only_branch("remote-feature");
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "remotes"]), cx).await;
    let before = reads(&h, cx);

    h.double_click(cx, "remote-branch:origin/remote-feature");
    h.wait_ui(cx, "the tracking dialog", |window, cx| {
        mergeon_ui::dialog::open_count(window, cx) == 1
    });

    // El doble clic ya no hace checkout directo: abre el diálogo de tracking.
    assert_eq!(h.dialog_titles(cx), ["Track Remote Branch"]);
    assert!(!repo.branches().contains(&"remote-feature".to_string()));
    let (target, remote) = h.sidebar.read_with(cx, |sidebar, cx| {
        let dialog = sidebar.checkout_dialog().expect("checkout dialog").read(cx);
        (
            dialog.options().target_label.to_string(),
            dialog.options().remote_branch.to_string(),
        )
    });
    assert_eq!(target, "origin/remote-feature");
    assert_eq!(remote, "origin/remote-feature");

    h.click(cx, "checkout-submit");
    h.wait_repo(cx, "the remote checkout", |repository| {
        repository.current_branch.as_deref() == Some("remote-feature")
    });
    h.settle(cx);

    assert_eq!(repo.current_branch(), "remote-feature");
    assert_eq!(
        repo.git(&["rev-parse", "--abbrev-ref", "remote-feature@{upstream}"])
            .trim(),
        "origin/remote-feature"
    );
    let after = reads(&h, cx);
    assert_eq!(after[BRANCHES], before[BRANCHES] + 1);
    assert_eq!(after[REMOTES], before[REMOTES]);
    assert_eq!(after[TAGS], before[TAGS]);
}

/// Ya tienes esa rama en local: doble clic sobre la remota debe llevarte a la
/// local, no negarse.
#[gpui_kit::test]
async fn switches_to_the_existing_local_branch_when_double_clicking_its_remote_counterpart(
    cx: &mut TestAppContext,
) {
    let repo = TestRepo::with_feature();
    repo.publish_to_origin();
    repo.git(&["push", "origin", "feature"]);
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "remotes"]), cx).await;

    h.double_click(cx, "remote-branch:origin/feature");
    h.wait_repo(cx, "the checkout of the local branch", |repository| {
        repository.current_branch.as_deref() == Some("feature")
    });
    h.settle(cx);

    assert_eq!(repo.current_branch(), "feature");
    // No se creó ninguna rama nueva ni se pasó por el diálogo.
    assert_eq!(repo.branches(), ["feature", "main"]);
    assert!(h.dialog_titles(cx).is_empty());
}

#[gpui_kit::test]
async fn double_clicking_the_remote_counterpart_of_the_current_branch_only_says_so(
    cx: &mut TestAppContext,
) {
    let repo = TestRepo::new();
    repo.publish_to_origin();
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "remotes"]), cx).await;

    h.double_click(cx, "remote-branch:origin/main");
    h.settle(cx);

    assert!(
        h.host
            .has_message(NotifyType::Info, "You are already on 'main'.")
    );
    assert!(h.dialog_titles(cx).is_empty());
}

// ---------------------------------------------------------------------------
// Secciones
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn hydrates_sidebar_sections_from_persisted_settings(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    repo.publish_to_origin();
    repo.git(&["tag", "v1.0.0"]);
    let h = Harness::open_with(
        &repo,
        sections(&["branches", "remotes", "tags", "submodules"]),
        cx,
    )
    .await;
    // Las secciones restauradas ya desplegadas cargan sus datos solas.
    h.wait_repo(cx, "the lazy sections", |repository| {
        repository.sidebar_loaded.tags && repository.sidebar_loaded.submodules
    });

    h.sidebar.read_with(cx, |sidebar, cx| {
        assert!(!sidebar.is_section_expanded(Section::Starred, cx));
        assert!(sidebar.is_section_expanded(Section::Tags, cx));
        assert!(!sidebar.is_section_expanded(Section::Stashes, cx));
    });
    assert!(h.exists(cx, "remote:origin"));
    assert!(h.exists(cx, "tag:v1.0.0"));
    assert!(h.exists(cx, "branch:main"));
}

#[gpui_kit::test]
async fn falls_back_to_default_sidebar_sections_when_persisted_settings_are_partial(
    cx: &mut TestAppContext,
) {
    let repo = TestRepo::new();
    repo.publish_to_origin();
    repo.git(&["tag", "v1.0.0"]);
    let h = Harness::open_with(&repo, serde_json::json!({ "remotes": true }), cx).await;
    h.settle(cx);

    h.sidebar.read_with(cx, |sidebar, cx| {
        assert!(sidebar.is_section_expanded(Section::Starred, cx));
        assert!(sidebar.is_section_expanded(Section::Branches, cx));
        assert!(sidebar.is_section_expanded(Section::Remotes, cx));
        assert!(!sidebar.is_section_expanded(Section::Tags, cx));
    });
    assert!(h.exists(cx, "remote:origin"));
    assert!(!h.exists(cx, "tag:v1.0.0"));
}

#[gpui_kit::test]
async fn persists_the_complete_sidebar_section_snapshot_after_toggling_a_section(
    cx: &mut TestAppContext,
) {
    let repo = TestRepo::new();
    let h = Harness::open(&repo, cx).await;

    h.click(cx, "section-header-remotes");
    h.settle(cx);

    let saved = h.model.read_with(cx, |model, _| {
        model.backend().storage().get_settings().sidebar_sections
    });
    assert!(saved.starred && saved.branches && saved.remotes);
    assert!(!saved.tags && !saved.stashes && !saved.submodules);

    // Y vuelve a plegarse.
    h.click(cx, "section-header-remotes");
    h.settle(cx);
    let saved = h.model.read_with(cx, |model, _| {
        model.backend().storage().get_settings().sidebar_sections
    });
    assert!(!saved.remotes);
}

#[gpui_kit::test]
async fn collapsed_sections_load_their_counts_once(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    repo.git(&["tag", "v1.0.0"]);
    let h = Harness::open(&repo, cx).await;
    // Plegadas, tags y stashes se leen igual: su cabecera muestra cuántos hay
    // (antes decía 0 hasta desplegarlas).
    h.wait_repo(cx, "the collapsed sections", |repository| {
        repository.sidebar_loaded.tags
            && repository.sidebar_loaded.stashes
            && repository.sidebar_loaded.submodules
    });
    assert_eq!(h.repo(cx, |repository| repository.tags.clone()), ["v1.0.0"]);
    assert!(!h.exists(cx, "tag:v1.0.0"));
    let loaded = h.reads(cx, "tags");

    h.click(cx, "section-header-tags");
    h.settle(cx);
    assert!(h.exists(cx, "tag:v1.0.0"));
    assert_eq!(h.reads(cx, "tags"), loaded);

    // Plegar y volver a desplegar no relee lo ya cargado.
    h.click(cx, "section-header-tags");
    h.click(cx, "section-header-tags");
    h.settle(cx);
    assert_eq!(h.reads(cx, "tags"), loaded);
}

/// `SidebarBranchSection.test.js`: "renders starred and branch lists and emits
/// branch actions".
#[gpui_kit::test]
async fn stars_and_unstars_branches(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let h = Harness::open(&repo, cx).await;
    assert!(!h.exists(cx, "starred:feature"));

    let starred_reads = h.reads(cx, "starredBranches");
    h.click(cx, "star:feature");
    h.wait_repo(cx, "the starred branch", |repository| {
        repository.starred_branches == ["feature"]
    });
    h.settle(cx);
    assert!(h.exists(cx, "starred:feature"));
    // La escritura ya trae la lista: no se invalida ni se relee la sección
    // (que se vaciaba un instante).
    assert_eq!(h.reads(cx, "starredBranches"), starred_reads);
    // Destacar no navega ni hace checkout.
    assert_eq!(repo.current_branch(), "main");
    assert!(h.events().is_empty());

    // El botón de la fila destacada aparece al pasar el cursor.
    h.hover(cx, "starred:feature");
    h.click(cx, "unstar:feature");
    h.wait_repo(cx, "the branch to lose its star", |repository| {
        repository.starred_branches.is_empty()
    });
    assert!(!h.exists(cx, "starred:feature"));
}

/// `SidebarBranchSection.test.js`: "renders a separate detached HEAD row
/// without changing active branches".
#[gpui_kit::test]
async fn renders_a_separate_detached_head_row(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    repo.git(&["checkout", "--detach", "HEAD"]);
    let h = Harness::open(&repo, cx).await;

    assert!(h.exists(cx, "detached-head"));
    assert!(h.exists(cx, "branch:main"));
    let active = h.repo(cx, |repository| {
        repository
            .branches
            .iter()
            .find(|branch| branch.name == "main")
            .map(|branch| branch.active)
    });
    assert_eq!(active, Some(false));
}

#[gpui_kit::test]
async fn filters_branches_by_name(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let h = Harness::open(&repo, cx).await;

    h.click(cx, "sidebar-filter");
    h.in_window(cx, |window, cx| window.input("FEAT", cx));

    assert_eq!(
        h.sidebar
            .read_with(cx, |sidebar, cx| sidebar.filter_text(cx)),
        "FEAT"
    );
    assert!(h.exists(cx, "branch:feature"));
    assert!(!h.exists(cx, "branch:main"));
}

// ---------------------------------------------------------------------------
// Menús contextuales
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn shows_a_context_menu_for_remote_branches_and_deletes_the_selected_remote_branch(
    cx: &mut TestAppContext,
) {
    let repo = TestRepo::new();
    let origin = repo.publish_to_origin();
    repo.publish_remote_only_branch("remote-feature");
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "remotes"]), cx).await;
    let before = reads(&h, cx);
    h.host.answer_confirm(true);

    let entries = h.sidebar.read_with(cx, |sidebar, cx| {
        sidebar.remote_branch_menu("origin", "remote-feature", cx)
    });
    h.choose_menu_entry(
        cx,
        "remote-branch:origin/remote-feature",
        &entries,
        "Delete 'origin/remote-feature'...",
    );
    h.wait_repo(cx, "the remote branch to go", |repository| {
        repository
            .remotes
            .first()
            .is_some_and(|remote| remote.branches == ["main"])
    });
    h.settle(cx);

    let confirms = h.host.confirms();
    assert_eq!(confirms[0].title.as_deref(), Some("Delete remote branch"));
    assert_eq!(
        confirms[0].description.as_deref(),
        Some("Delete 'origin/remote-feature' from 'origin'?")
    );
    assert_eq!(confirms[0].variant.as_deref(), Some("destructive"));
    assert!(
        !support::git_in(&origin, &["branch", "--format=%(refname:short)"])
            .contains("remote-feature")
    );
    let after = reads(&h, cx);
    assert_eq!(after[REMOTES], before[REMOTES] + 1);
    assert_eq!(after[STATUS], before[STATUS]);
}

#[gpui_kit::test]
async fn sets_up_tracking_and_refreshes_branches_plus_remotes_once(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    repo.publish_to_origin();
    repo.git(&["push", "origin", "feature"]);
    let h = Harness::open(&repo, cx).await;
    let before = reads(&h, cx);

    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.local_branch_menu("feature", cx));
    h.choose_menu_entry(
        cx,
        "branch:feature",
        &entries,
        "Set Upstream to origin/feature",
    );
    h.wait_repo(cx, "the upstream", |repository| {
        repository
            .branches
            .iter()
            .any(|branch| branch.name == "feature" && branch.upstream_set)
    });
    h.settle(cx);

    assert_eq!(
        repo.git(&["rev-parse", "--abbrev-ref", "feature@{upstream}"])
            .trim(),
        "origin/feature"
    );
    let after = reads(&h, cx);
    assert_eq!(after[BRANCHES], before[BRANCHES] + 1);
    assert_eq!(after[REMOTES], before[REMOTES] + 1);
    assert_eq!(after[STATUS], before[STATUS]);
}

#[gpui_kit::test]
async fn does_not_refresh_sections_when_setup_tracking_is_already_configured(
    cx: &mut TestAppContext,
) {
    let repo = TestRepo::new();
    repo.publish_to_origin();
    let h = Harness::open(&repo, cx).await;
    let before = reads(&h, cx);

    h.in_window(cx, |window, cx| {
        h.sidebar.update(cx, |sidebar, cx| {
            sidebar.run_command(
                SidebarCommand::SetupTracking("main".to_string()),
                window,
                cx,
            );
        });
    });
    h.wait(cx, "the tracking answer", |_| {
        !h.host.messages(NotifyType::Info).is_empty()
    });
    h.settle(cx);

    assert_eq!(reads(&h, cx), before);
    assert_eq!(h.host.messages(NotifyType::Info).len(), 1);
    assert!(h.host.messages(NotifyType::Success).is_empty());
}

#[gpui_kit::test]
async fn shows_a_clear_warning_when_setup_tracking_cannot_find_a_matching_remote_branch(
    cx: &mut TestAppContext,
) {
    let repo = TestRepo::with_feature();
    repo.publish_to_origin();
    let h = Harness::open(&repo, cx).await;
    let before = reads(&h, cx);

    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.local_branch_menu("feature", cx));
    h.choose_menu_entry(
        cx,
        "branch:feature",
        &entries,
        "Set Upstream to origin/feature",
    );
    h.wait(cx, "the warning", |_| {
        !h.host.messages(NotifyType::Warning).is_empty()
    });
    h.settle(cx);

    // Solo se relee el estado (un fallo sin estado adjunto lo invalida).
    let mut expected = before;
    expected[STATUS] += 1;
    assert_eq!(reads(&h, cx), expected);
    let warnings = h.host.messages(NotifyType::Warning);
    assert!(warnings[0].contains("origin/feature"), "{warnings:?}");
}

#[gpui_kit::test]
async fn refreshes_status_branches_and_changes_count_after_a_branch_dialog_merge_completes(
    cx: &mut TestAppContext,
) {
    let repo = TestRepo::with_feature();
    let h = Harness::open(&repo, cx).await;
    let before = reads(&h, cx);

    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.local_branch_menu("feature", cx));
    h.choose_menu_entry(cx, "branch:feature", &entries, "Merge into 'main'...");
    assert_eq!(h.dialog_titles(cx), ["Branch Actions: feature → main"]);
    // El chequeo de conflictos termina antes de decidir nada.
    h.wait_app(cx, "the conflict check", |cx| {
        h.sidebar
            .read(cx)
            .branch_action_dialog()
            .map(|dialog| dialog.read(cx).is_checking_conflicts())
            == Some(false)
    });

    h.host.answer_confirm(true);
    h.click(cx, "branch-action-merge");
    h.wait_repo(cx, "the merge", |repository| {
        repository.last_mutation_operation.as_deref() == Some("merge")
    });
    h.settle(cx);

    let confirm = &h.host.confirms()[0];
    assert_eq!(confirm.title.as_deref(), Some("Confirm merge"));
    assert_eq!(
        confirm.description.as_deref(),
        Some("This will switch to 'main' and merge 'feature' into it.")
    );
    assert_eq!(confirm.confirm_text.as_deref(), Some("Merge"));
    assert!(repo.path.join("feature.txt").exists());
    assert!(h.dialog_titles(cx).is_empty());
    assert!(
        h.sidebar
            .read_with(cx, |sidebar, _| sidebar.branch_action_dialog().is_none())
    );
    let after = reads(&h, cx);
    assert_eq!(after[BRANCHES], before[BRANCHES] + 1);
}

#[gpui_kit::test]
async fn the_branch_actions_dialog_compares_branches(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let h = Harness::open(&repo, cx).await;

    h.in_window(cx, |window, cx| {
        h.sidebar.update(cx, |sidebar, cx| {
            sidebar.run_command(
                SidebarCommand::BranchAction {
                    branch: "feature".to_string(),
                    action: mergeon_view_sidebar::BranchAction::Compare,
                },
                window,
                cx,
            );
        });
    });
    h.click(cx, "branch-action-compare");
    h.wait_ui(cx, "the comparison", |window, cx| {
        mergeon_ui::dialog::open_count(window, cx) == 2
    });

    assert_eq!(
        h.dialog_titles(cx),
        ["Branch Actions: feature → main", "Branch Comparison"]
    );
    let comparison = h.sidebar.read_with(cx, |sidebar, cx| {
        sidebar
            .branch_action_dialog()
            .and_then(|dialog| dialog.read(cx).comparison().cloned())
    });
    let comparison = comparison.expect("comparison");
    assert_eq!((comparison.ahead, comparison.behind), (1, 0));
    assert_eq!(comparison.ahead_commits[0].message, "Add feature");

    h.click(cx, "branch-comparison-close");
    assert_eq!(h.dialog_titles(cx), ["Branch Actions: feature → main"]);
    h.click(cx, "branch-action-cancel");
    assert!(h.dialog_titles(cx).is_empty());
}

#[gpui_kit::test]
async fn dropping_a_branch_on_another_opens_branch_actions_between_them(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    repo.git(&["branch", "hotfix"]);
    let h = Harness::open(&repo, cx).await;

    h.in_window(cx, |window, cx| {
        window.drag_to("branch:feature", "branch:hotfix", cx)
    });
    h.wait_ui(cx, "branch actions", |window, cx| {
        mergeon_ui::dialog::open_count(window, cx) == 1
    });

    assert_eq!(h.dialog_titles(cx), ["Branch Actions: feature → hotfix"]);
    // Sin acción preseleccionada.
    let selected = h.sidebar.read_with(cx, |sidebar, cx| {
        sidebar
            .branch_action_dialog()
            .map(|dialog| dialog.read(cx).selected_action())
    });
    assert_eq!(selected, Some(None));
}

#[gpui_kit::test]
async fn renames_a_branch_and_keeps_its_star(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let h = Harness::open(&repo, cx).await;
    h.click(cx, "star:feature");
    h.wait_repo(cx, "the star", |repository| {
        repository.starred_branches == ["feature"]
    });
    h.host.answer_prompt("  topic  ");

    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.local_branch_menu("feature", cx));
    h.choose_menu_entry(cx, "branch:feature", &entries, "Rename 'feature'...");
    h.wait_repo(cx, "the rename", |repository| {
        repository
            .branches
            .iter()
            .any(|branch| branch.name == "topic")
            && repository.starred_branches == ["topic"]
    });
    h.settle(cx);

    let prompt = &h.host.prompts()[0];
    assert_eq!(prompt.title.as_deref(), Some("Rename branch"));
    assert_eq!(prompt.description.as_deref(), Some("Rename 'feature' to:"));
    assert_eq!(prompt.default_value.as_deref(), Some("feature"));
    assert_eq!(repo.branches(), ["main", "topic"]);
    assert!(h.exists(cx, "starred:topic"));
}

#[gpui_kit::test]
async fn deletes_a_branch_and_optionally_its_upstream(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let origin = repo.publish_to_origin();
    repo.git(&["push", "-u", "origin", "feature"]);
    // Ya fusionada: git no se niega a borrarla.
    repo.git(&["merge", "feature"]);
    let h = Harness::open(&repo, cx).await;
    h.host.answer_confirm_with_option(true, true);

    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.local_branch_menu("feature", cx));
    h.choose_menu_entry(cx, "branch:feature", &entries, "Delete 'feature'...");
    h.wait_repo(cx, "the branch to go", |repository| {
        repository.branches.len() == 1
            && repository.last_mutation_operation.as_deref() == Some("delete-branch")
    });
    h.settle(cx);

    let confirm = &h.host.option_confirms()[0];
    assert_eq!(confirm.title.as_deref(), Some("Delete branch"));
    assert_eq!(
        confirm.option_label.as_deref(),
        Some("Also delete 'origin/feature' on origin")
    );
    // Apagada de fábrica: borrar en el remoto no se deshace solo.
    assert!(!confirm.option_checked);
    assert_eq!(repo.branches(), ["main"]);
    assert!(
        !support::git_in(&origin, &["branch", "--format=%(refname:short)"]).contains("feature")
    );
}

#[gpui_kit::test]
async fn a_cancelled_delete_keeps_the_branch(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let h = Harness::open(&repo, cx).await;
    h.host.answer_confirm(false);

    h.in_window(cx, |window, cx| {
        h.sidebar.update(cx, |sidebar, cx| {
            sidebar.run_command(
                SidebarCommand::DeleteBranch("feature".to_string()),
                window,
                cx,
            );
        });
    });
    h.settle(cx);

    // Sin upstream la confirmación es la sencilla.
    assert_eq!(h.host.confirms().len(), 1);
    assert!(h.host.option_confirms().is_empty());
    assert_eq!(repo.branches(), ["feature", "main"]);
}

#[gpui_kit::test]
async fn copies_the_branch_name(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let h = Harness::open(&repo, cx).await;

    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.local_branch_menu("feature", cx));
    h.choose_menu_entry(cx, "branch:feature", &entries, "Copy Branch Name");

    let copied = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(copied.as_deref(), Some("feature"));
    assert!(
        h.host
            .has_message(NotifyType::Success, "Branch name copied to clipboard")
    );
}

// ---------------------------------------------------------------------------
// Remotos
// ---------------------------------------------------------------------------

/// También `SidebarRemoteSection.test.js`: el botón de la cabecera pide añadir
/// un remoto.
#[gpui_kit::test]
async fn adds_a_remote_through_the_dialog_and_refreshes_remotes_without_reloading_status(
    cx: &mut TestAppContext,
) {
    let repo = TestRepo::new();
    let upstream = repo.bare_repository("upstream");
    let h = Harness::open(&repo, cx).await;
    let before = reads(&h, cx);

    // El botón aparece al pasar por la cabecera.
    h.hover(cx, "section-header-remotes");
    h.click(cx, "add-remote");
    assert_eq!(h.dialog_titles(cx), ["Remote"]);
    // No desplegó la sección: el clic no pasa a la cabecera.
    assert!(!h.sidebar.read_with(cx, |sidebar, cx| {
        sidebar.is_section_expanded(Section::Remotes, cx)
    }));

    let dialog = h
        .sidebar
        .read_with(cx, |sidebar, _| sidebar.add_remote_dialog().cloned())
        .expect("add remote dialog");
    let url = format!("{}/", upstream.to_string_lossy());
    h.in_window(cx, |window, cx| {
        let (name_input, url_input) = {
            let dialog = dialog.read(cx);
            (
                dialog.remote_name_input().clone(),
                dialog.remote_url_input().clone(),
            )
        };
        name_input.update(cx, |state, cx| state.set_value("upstream", window, cx));
        url_input.update(cx, |state, cx| state.set_value(url.clone(), window, cx));
    });
    h.click(cx, "add-remote-submit");
    h.wait_repo(cx, "the new remote", |repository| {
        repository
            .remotes
            .iter()
            .any(|remote| remote.name == "upstream")
            && repository.last_mutation_operation.as_deref() == Some("add-remote")
    });
    h.settle(cx);

    assert_eq!(repo.remotes(), ["upstream"]);
    // La URL se guarda sin la barra final.
    assert_eq!(
        repo.git(&["remote", "get-url", "upstream"]).trim(),
        upstream.to_string_lossy()
    );
    assert!(h.dialog_titles(cx).is_empty());
    assert!(
        h.sidebar
            .read_with(cx, |sidebar, _| sidebar.add_remote_dialog().is_none())
    );
    let after = reads(&h, cx);
    assert_eq!(after[REMOTES], before[REMOTES] + 1);
    assert_eq!(after[STATUS], before[STATUS]);
}

#[gpui_kit::test]
async fn tests_the_connection_of_a_remote_before_adding_it(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    let upstream = repo.bare_repository("upstream");
    let h = Harness::open(&repo, cx).await;

    h.in_window(cx, |window, cx| {
        h.sidebar.update(cx, |sidebar, cx| {
            sidebar.run_command(SidebarCommand::AddRemote, window, cx);
        });
    });
    let dialog = h
        .sidebar
        .read_with(cx, |sidebar, _| sidebar.add_remote_dialog().cloned())
        .expect("add remote dialog");
    // Sin URL no hay nada que probar ni que añadir.
    dialog.read_with(cx, |dialog, cx| {
        assert!(!dialog.is_testable(cx));
        assert!(!dialog.is_submittable(cx));
    });

    let url = upstream.to_string_lossy().into_owned();
    h.in_window(cx, |window, cx| {
        let input = dialog.read(cx).remote_url_input().clone();
        input.update(cx, |state, cx| state.set_value(url, window, cx));
    });
    h.click(cx, "add-remote-test");
    h.wait_app(cx, "the connection test", |cx| {
        dialog.read(cx).connection_result().is_some()
    });

    dialog.read_with(cx, |dialog, _| {
        assert!(
            dialog
                .connection_result()
                .is_some_and(|result| result.success)
        );
        assert!(!dialog.is_testing_connection());
    });
    // Probar no añade nada.
    assert!(repo.remotes().is_empty());

    h.click(cx, "add-remote-cancel");
    assert!(h.dialog_titles(cx).is_empty());
}

#[gpui_kit::test]
async fn renames_edits_and_removes_a_remote_from_its_menu(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    repo.publish_to_origin();
    let other = repo.bare_repository("other");
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "remotes"]), cx).await;

    // Rename
    h.host.answer_prompt("upstream");
    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.remote_menu("origin", cx));
    h.choose_menu_entry(cx, "remote:origin", &entries, "Rename 'origin'…");
    h.wait_repo(cx, "the rename", |repository| {
        repository
            .remotes
            .iter()
            .any(|remote| remote.name == "upstream")
    });
    h.settle(cx);
    assert_eq!(repo.remotes(), ["upstream"]);
    let prompt = &h.host.prompts()[0];
    assert_eq!(prompt.title.as_deref(), Some("Rename 'origin'"));
    assert_eq!(prompt.pattern.as_deref(), Some("^[A-Za-z0-9._-]+$"));

    // Edit URL
    h.host.answer_prompt(&other.to_string_lossy());
    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.remote_menu("upstream", cx));
    h.choose_menu_entry(cx, "remote:upstream", &entries, "Edit URL…");
    h.wait_repo(cx, "the new url", |repository| {
        repository
            .remotes
            .iter()
            .any(|remote| remote.refs.fetch == other.to_string_lossy())
    });
    h.settle(cx);
    assert_eq!(
        repo.git(&["remote", "get-url", "upstream"]).trim(),
        other.to_string_lossy()
    );
    assert_eq!(
        h.host.prompts()[1].title.as_deref(),
        Some("Edit URL of 'upstream'")
    );

    // Remove
    h.host.answer_confirm(true);
    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.remote_menu("upstream", cx));
    h.choose_menu_entry(cx, "remote:upstream", &entries, "Remove 'upstream'…");
    h.wait_repo(cx, "the removal", |repository| {
        repository.remotes.is_empty()
    });
    h.settle(cx);
    assert!(repo.remotes().is_empty());
    assert_eq!(h.host.confirms()[0].title.as_deref(), Some("Remove remote"));
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

/// También `SidebarTagSection.test.js`: el botón de la cabecera pide crear un
/// tag.
#[gpui_kit::test]
async fn reloads_only_tags_after_creating_a_tag(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    repo.git(&["tag", "v1.0.0"]);
    repo.add_bare_remote("origin");
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "tags"]), cx).await;
    h.wait_repo(cx, "the tags", |repository| repository.sidebar_loaded.tags);
    h.settle(cx);
    let before = reads(&h, cx);
    h.host.answer_prompt("v2.0.0");
    h.host.cancel_prompt();

    h.hover(cx, "section-header-tags");
    h.click(cx, "create-tag");
    h.wait_repo(cx, "the new tag", |repository| {
        repository.tags.contains(&"v2.0.0".to_string())
    });
    h.settle(cx);

    let prompts = h.host.prompts();
    assert_eq!(prompts[0].title.as_deref(), Some("Create tag"));
    assert_eq!(prompts[0].confirm_text.as_deref(), Some("Next"));
    assert_eq!(prompts[1].title.as_deref(), Some("Annotate tag (optional)"));
    assert_eq!(prompts[1].cancel_text.as_deref(), Some("Skip"));
    assert!(prompts[1].textarea);
    // Sin anotación el tag es ligero y apunta a HEAD.
    assert_eq!(repo.git(&["cat-file", "-t", "v2.0.0"]).trim(), "commit");
    assert_eq!(repo.rev_parse("v2.0.0"), repo.head());
    // Recién creado, se ofrece publicarlo.
    assert_eq!(h.host.confirms()[0].title.as_deref(), Some("Tag created"));
    assert_eq!(
        h.host.confirms()[0].confirm_text.as_deref(),
        Some("Push to origin")
    );
    let after = reads(&h, cx);
    assert_eq!(after[TAGS], before[TAGS] + 1);
    assert_eq!(after[BRANCHES], before[BRANCHES]);
    assert_eq!(after[STASHES], before[STASHES]);
    // Sigue desplegada: el clic en el botón no llega a la cabecera.
    assert!(h.sidebar.read_with(cx, |sidebar, cx| {
        sidebar.is_section_expanded(Section::Tags, cx)
    }));
}

#[gpui_kit::test]
async fn creates_an_annotated_tag_when_the_optional_message_is_provided(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "tags"]), cx).await;
    h.host.answer_prompt("v3.0.0");
    h.host.answer_prompt("Release candidate");

    h.hover(cx, "section-header-tags");
    h.click(cx, "create-tag");
    h.wait_repo(cx, "the new tag", |repository| {
        repository.tags.contains(&"v3.0.0".to_string())
    });
    h.settle(cx);

    assert_eq!(repo.git(&["cat-file", "-t", "v3.0.0"]).trim(), "tag");
    assert_eq!(
        repo.git(&["tag", "-l", "--format=%(contents:subject)", "v3.0.0"])
            .trim(),
        "Release candidate"
    );
}

/// `SidebarTagSection.test.js`: los botones de la fila piden checkout y borrado.
#[gpui_kit::test]
async fn checks_out_and_deletes_tags_from_their_row(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    repo.git(&["tag", "v1.0.0"]);
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "tags"]), cx).await;
    h.wait_ui(cx, "the tag row", |window, _| {
        window.try_find("tag:v1.0.0").is_some()
    });

    h.host.answer_confirm(true);
    h.click(cx, "checkout-tag:v1.0.0");
    h.wait_repo(cx, "the detached checkout", |repository| {
        repository.current_branch.as_deref() == Some("HEAD")
    });
    h.settle(cx);
    assert_eq!(h.host.confirms()[0].title.as_deref(), Some("Checkout tag"));
    assert_eq!(repo.current_branch(), "HEAD");
    assert!(h.host.has_message(
        NotifyType::Info,
        "Detached HEAD at tag 'v1.0.0'. Create a branch if you want to keep working from here."
    ));
    // El botón de la fila no navega al commit del tag.
    assert!(h.events().is_empty());

    // Sin remotos no se ofrece borrarlo también en uno: confirmación simple.
    h.host.answer_confirm(true);
    h.click(cx, "delete-tag:v1.0.0");
    h.wait_repo(cx, "the tag to go", |repository| repository.tags.is_empty());
    h.settle(cx);
    assert!(h.host.option_confirms().is_empty());
    let confirm = h.host.confirms().last().cloned().expect("confirm");
    assert_eq!(confirm.title.as_deref(), Some("Delete tag"));
    assert_eq!(confirm.description.as_deref(), Some("Delete tag 'v1.0.0'?"));
    assert!(repo.tags().is_empty());
}

#[gpui_kit::test]
async fn tag_actions_use_the_repository_remote_instead_of_origin(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    let upstream = repo.add_bare_remote("upstream");
    repo.git(&["tag", "v1.0.0"]);
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "tags"]), cx).await;
    h.wait_ui(cx, "the tag row", |window, _| {
        window.try_find("tag:v1.0.0").is_some()
    });

    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.tag_menu("v1.0.0", cx));
    h.choose_menu_entry(cx, "tag:v1.0.0", &entries, "Push 'v1.0.0' to upstream");
    h.wait(cx, "the tag on upstream", |_| {
        support::git_in(&upstream, &["tag"]).contains("v1.0.0")
    });
    h.settle(cx);

    h.host.answer_confirm_with_option(true, true);
    h.click(cx, "delete-tag:v1.0.0");
    h.wait_repo(cx, "the tag to go", |repository| repository.tags.is_empty());
    h.wait(cx, "the tag to leave upstream", |_| {
        support::git_in(&upstream, &["tag"]).trim().is_empty()
    });
    h.settle(cx);
    assert_eq!(
        h.host.option_confirms()[0].option_label.as_deref(),
        Some("Also delete it on upstream")
    );
}

#[gpui_kit::test]
async fn a_new_tag_is_not_offered_for_publishing_without_remotes(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "tags"]), cx).await;
    h.host.answer_prompt("v2.0.0");
    h.host.cancel_prompt();

    h.hover(cx, "section-header-tags");
    h.click(cx, "create-tag");
    h.wait_repo(cx, "the new tag", |repository| {
        repository.tags.contains(&"v2.0.0".to_string())
    });
    h.settle(cx);
    assert!(h.host.confirms().is_empty());
}

#[gpui_kit::test]
async fn shows_the_details_of_an_annotated_tag(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    repo.git(&["tag", "-a", "v1.0.0", "-m", "First stable release"]);
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "tags"]), cx).await;
    h.wait_ui(cx, "the tag row", |window, _| {
        window.try_find("tag:v1.0.0").is_some()
    });

    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.tag_menu("v1.0.0", cx));
    h.choose_menu_entry(cx, "tag:v1.0.0", &entries, "View details…");
    assert_eq!(h.dialog_titles(cx), ["v1.0.0"]);
    let dialog = h
        .sidebar
        .read_with(cx, |sidebar, _| sidebar.tag_details_dialog().cloned())
        .expect("tag details");
    h.wait_app(cx, "the tag details", |cx| !dialog.read(cx).is_loading());

    dialog.read_with(cx, |dialog, _| {
        let details = dialog.details().expect("details");
        assert!(details.is_annotated);
        assert_eq!(details.target_hash, repo.head());
        assert_eq!(details.tagger_name.as_deref(), Some("Test User"));
        assert_eq!(details.message.trim(), "First stable release");
    });
}

// ---------------------------------------------------------------------------
// Stashes
// ---------------------------------------------------------------------------

fn stash_repo() -> (TestRepo, String) {
    let repo = TestRepo::new();
    repo.dirty();
    repo.git(&["stash", "push", "-m", "WIP"]);
    let hash = repo.rev_parse("stash@{0}");
    (repo, hash)
}

#[gpui_kit::test]
async fn reloads_only_stashes_after_dropping_a_stash(cx: &mut TestAppContext) {
    let (repo, hash) = stash_repo();
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "stashes"]), cx).await;
    h.wait_ui(cx, "the stash row", |window, _| {
        window.try_find(format!("stash:{hash}")).is_some()
    });
    h.settle(cx);
    let before = reads(&h, cx);
    h.host.answer_confirm(true);

    h.click(cx, &format!("stash-drop:{hash}"));
    h.wait_repo(cx, "the stash to go", |repository| {
        repository.stashes.is_empty()
    });
    h.settle(cx);

    let confirm = &h.host.confirms()[0];
    assert_eq!(confirm.title.as_deref(), Some("Drop stash"));
    assert_eq!(
        confirm.description,
        Some(format!("Drop {hash}? This cannot be undone."))
    );
    assert_eq!(confirm.variant.as_deref(), Some("destructive"));
    assert_eq!(repo.stash_count(), 0);
    let after = reads(&h, cx);
    assert_eq!(after[STASHES], before[STASHES] + 1);
    assert_eq!(after[TAGS], before[TAGS]);
    // El botón no cuenta como clic en la fila.
    assert!(h.events().is_empty());
}

#[gpui_kit::test]
async fn creates_a_branch_from_stash_and_refreshes_branches_plus_stashes(cx: &mut TestAppContext) {
    let (repo, hash) = stash_repo();
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "stashes"]), cx).await;
    h.wait_ui(cx, "the stash row", |window, _| {
        window.try_find(format!("stash:{hash}")).is_some()
    });
    h.settle(cx);
    let before = reads(&h, cx);
    h.host.answer_prompt("recover-work");

    h.click(cx, &format!("stash-branch:{hash}"));
    h.wait_repo(cx, "the new branch", |repository| {
        repository.current_branch.as_deref() == Some("recover-work")
            && repository.stashes.is_empty()
    });
    h.settle(cx);

    assert_eq!(
        h.host.prompts()[0].title.as_deref(),
        Some("Create branch from stash")
    );
    assert_eq!(repo.current_branch(), "recover-work");
    assert_eq!(repo.stash_count(), 0);
    let after = reads(&h, cx);
    assert_eq!(after[BRANCHES], before[BRANCHES] + 1);
    assert_eq!(after[STASHES], before[STASHES] + 1);
}

/// Clic derecho en un stash: el mismo menú que sus botones (QA 0.12.0).
#[gpui_kit::test]
async fn applies_a_stash_from_its_context_menu(cx: &mut TestAppContext) {
    let (repo, hash) = stash_repo();
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "stashes"]), cx).await;
    h.wait_ui(cx, "the stash row", |window, _| {
        window.try_find(format!("stash:{hash}")).is_some()
    });

    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.stash_menu(&hash, cx));
    h.choose_menu_entry(cx, &format!("stash:{hash}"), &entries, "Apply stash");
    h.wait_repo(cx, "the applied changes", |repository| {
        repository.last_mutation_operation.as_deref() == Some("apply-stash")
    });
    h.settle(cx);
    assert_eq!(repo.stash_count(), 1);
    assert!(
        std::fs::read_to_string(repo.path.join("README.md"))
            .unwrap()
            .contains("local change")
    );
}

/// `SidebarStashSection.test.js`: aplicar, sacar y descartar desde la fila.
#[gpui_kit::test]
async fn applies_and_pops_stashes_from_their_row(cx: &mut TestAppContext) {
    let (repo, hash) = stash_repo();
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "stashes"]), cx).await;
    h.wait_ui(cx, "the stash row", |window, _| {
        window.try_find(format!("stash:{hash}")).is_some()
    });

    // Apply deja el stash en la lista.
    h.click(cx, &format!("stash-apply:{hash}"));
    h.wait_repo(cx, "the applied changes", |repository| {
        repository.last_mutation_operation.as_deref() == Some("apply-stash")
    });
    h.settle(cx);
    assert_eq!(repo.stash_count(), 1);
    assert!(
        std::fs::read_to_string(repo.path.join("README.md"))
            .unwrap()
            .contains("local change")
    );

    // Pop pregunta antes y, sin confirmar, no hace nada.
    h.host.answer_confirm(false);
    h.click(cx, &format!("stash-pop:{hash}"));
    h.settle(cx);
    assert_eq!(h.host.confirms()[0].title.as_deref(), Some("Pop stash"));
    assert_eq!(repo.stash_count(), 1);

    repo.git(&["checkout", "--", "README.md"]);
    h.host.answer_confirm(true);
    h.click(cx, &format!("stash-pop:{hash}"));
    h.wait_repo(cx, "the popped stash", |repository| {
        repository.stashes.is_empty()
    });
    assert_eq!(repo.stash_count(), 0);
}

#[gpui_kit::test]
async fn clicking_a_stash_asks_for_its_diff(cx: &mut TestAppContext) {
    let (repo, hash) = stash_repo();
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "stashes"]), cx).await;
    h.wait_ui(cx, "the stash row", |window, _| {
        window.try_find(format!("stash:{hash}")).is_some()
    });

    h.click(cx, &format!("stash:{hash}"));

    match h.events().as_slice() {
        [SidebarEvent::OpenStashDiff { repo_path, stash }] => {
            assert_eq!(repo_path, &repo.id());
            assert_eq!(stash.hash, hash);
        }
        other => panic!("unexpected events: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Navegación al historial
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn navigates_the_history_to_a_branch_tip_on_single_click_keeping_ref_name(
    cx: &mut TestAppContext,
) {
    let repo = TestRepo::with_feature();
    let h = Harness::open(&repo, cx).await;
    h.model.update(cx, |model, cx| {
        model.handle_change_view(WorkspaceSection::Changes, cx);
    });

    h.click(cx, "branch:feature");
    h.wait(cx, "the selection request", |_| !h.events().is_empty());

    let tip = repo.rev_parse("feature");
    match h.events().as_slice() {
        [SidebarEvent::CommitSelectionRequested { repo_id, request }] => {
            assert_eq!(repo_id, &h.repo_id);
            assert_eq!(request.hash, tip);
            assert_eq!(request.ref_name.as_deref(), Some("feature"));
        }
        other => panic!("unexpected events: {other:?}"),
    }
    // `emit('change-view', 'commits')`.
    assert_eq!(
        h.model.read_with(cx, |model, _| model.active_view()),
        WorkspaceSection::Commits
    );
    h.sidebar.read_with(cx, |sidebar, _| {
        assert_eq!(sidebar.viewed_ref(&h.repo_id), Some("feature"));
    });
    // Ver una rama no cambia la rama activa.
    assert_eq!(repo.current_branch(), "main");

    // La vista de commits retira la marca cuando el usuario elige otro commit.
    h.sidebar.update(cx, |sidebar, cx| {
        sidebar.clear_viewed_ref(&h.repo_id, cx);
    });
    h.sidebar.read_with(cx, |sidebar, _| {
        assert_eq!(sidebar.viewed_ref(&h.repo_id), None);
        assert_eq!(
            sidebar
                .commit_selection_request(&h.repo_id)
                .map(|request| request.hash.clone()),
            Some(tip.clone())
        );
    });
}

#[gpui_kit::test]
async fn navigates_to_a_remote_branch_tip_when_clicking_it_under_its_remote(
    cx: &mut TestAppContext,
) {
    let repo = TestRepo::with_feature();
    repo.publish_to_origin();
    repo.publish_remote_only_branch("remote-feature");
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "remotes"]), cx).await;

    h.click(cx, "remote-branch:origin/remote-feature");
    h.wait(cx, "the selection request", |_| !h.events().is_empty());

    match h.events().as_slice() {
        [SidebarEvent::CommitSelectionRequested { request, .. }] => {
            assert_eq!(request.hash, repo.rev_parse("origin/remote-feature"));
            assert_eq!(request.ref_name.as_deref(), Some("origin/remote-feature"));
        }
        other => panic!("unexpected events: {other:?}"),
    }
}

#[gpui_kit::test]
async fn navigates_to_the_commit_of_a_tag_without_marking_a_ref(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    repo.git(&["tag", "-a", "v1.0.0", "-m", "Release", "feature"]);
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "tags"]), cx).await;
    h.wait_ui(cx, "the tag row", |window, _| {
        window.try_find("tag:v1.0.0").is_some()
    });

    h.click(cx, "tag:v1.0.0");
    h.wait(cx, "the selection request", |_| !h.events().is_empty());

    match h.events().as_slice() {
        [SidebarEvent::CommitSelectionRequested { request, .. }] => {
            // El commit al que apunta el tag anotado, no el objeto tag.
            assert_eq!(request.hash, repo.rev_parse("feature"));
            assert_eq!(request.ref_name, None);
        }
        other => panic!("unexpected events: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Búsqueda de commits
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn opens_commit_search_mode_searches_server_side_with_debounce_and_requests_commit_focus(
    cx: &mut TestAppContext,
) {
    let repo = TestRepo::new();
    repo.commit_file("login.txt", "fix\n", "Fix login crash");
    let dashboard = repo.commit_file("dashboard.txt", "search\n", "Add dashboard search");
    let h = Harness::open(&repo, cx).await;
    let before = reads(&h, cx);

    h.click(cx, "sidebar-show-search");
    h.settle(cx);

    // Abrir el panel con la query vacía no dispara ninguna búsqueda.
    h.sidebar.read_with(cx, |sidebar, cx| {
        assert_eq!(sidebar.panel_mode(), PanelMode::Search);
        assert!(sidebar.is_search_panel_visible(cx));
        assert!(sidebar.search_results().is_empty());
        assert!(!sidebar.is_search_loading());
    });
    assert!(h.exists(cx, "commit-search-panel"));
    assert!(!h.exists(cx, "section-header-branches"));

    h.click(cx, "commit-search-input");
    h.in_window(cx, |window, cx| window.input("dashboard", cx));
    // Hasta que pasa la pausa de tecleo no se busca.
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert!(
        h.sidebar
            .read_with(cx, |sidebar, _| sidebar.search_results().is_empty())
    );

    cx.executor().advance_clock(Duration::from_millis(150));
    h.wait_app(cx, "the search results", |cx| {
        !h.sidebar.read(cx).search_results().is_empty()
    });
    let results = h
        .sidebar
        .read_with(cx, |sidebar, _| sidebar.search_results().to_vec());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].message, "Add dashboard search");
    assert_eq!(results[0].full_hash, dashboard);
    assert_eq!(results[0].author, "Test User");

    // Con la búsqueda a la vista, Refresh repite la búsqueda y no relee secciones.
    h.click(cx, "sidebar-refresh");
    h.settle(cx);
    assert_eq!(reads(&h, cx), before);
    assert_eq!(
        h.sidebar
            .read_with(cx, |sidebar, _| sidebar.search_results().len()),
        1
    );

    h.click(cx, &format!("commit-result:{dashboard}"));
    match h.events().as_slice() {
        [SidebarEvent::CommitSelectionRequested { request, .. }] => {
            assert_eq!(request.hash, dashboard);
            assert_eq!(request.ref_name, None);
        }
        other => panic!("unexpected events: {other:?}"),
    }

    // Volver al panel del repositorio conserva la búsqueda escrita.
    h.click(cx, "sidebar-show-repository");
    assert!(h.exists(cx, "section-header-branches"));
    assert_eq!(
        h.sidebar.read_with(cx, |sidebar, cx| sidebar
            .search_input()
            .read(cx)
            .value()
            .to_string()),
        "dashboard"
    );
}

#[gpui_kit::test]
async fn searches_by_author_and_by_hash(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    let fix = repo.commit_file("login.txt", "fix\n", "Fix login crash");
    let h = Harness::open(&repo, cx).await;

    h.in_window(cx, |window, cx| {
        h.sidebar.update(cx, |sidebar, cx| {
            sidebar.show_search_panel(window, cx);
            sidebar.set_search_query("author:nobody", window, cx);
        });
    });
    h.wait_app(cx, "the author search", |cx| {
        !h.sidebar.read(cx).is_search_loading()
    });
    assert!(
        h.sidebar
            .read_with(cx, |sidebar, _| sidebar.search_results().is_empty())
    );

    let short = fix[..8].to_string();
    h.in_window(cx, |window, cx| {
        h.sidebar.update(cx, |sidebar, cx| {
            sidebar.set_search_query(&short, window, cx);
        });
    });
    h.wait_app(cx, "the hash search", |cx| {
        !h.sidebar.read(cx).search_results().is_empty()
    });
    let results = h
        .sidebar
        .read_with(cx, |sidebar, _| sidebar.search_results().to_vec());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].full_hash, fix);
}

#[gpui_kit::test]
async fn the_focus_commit_search_shortcut_opens_the_search_panel(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    let h = Harness::open(&repo, cx).await;
    h.model.update(cx, |model, cx| {
        model.handle_change_view(WorkspaceSection::Changes, cx);
    });
    cx.run_until_parked();

    h.model.update(cx, |_, cx| {
        cx.emit(mergeon_app::AppEvent::FocusCommitSearch)
    });
    cx.run_until_parked();

    h.sidebar.read_with(cx, |sidebar, cx| {
        assert_eq!(sidebar.panel_mode(), PanelMode::Search);
        assert!(sidebar.is_search_panel_visible(cx));
    });
    assert_eq!(
        h.model.read_with(cx, |model, _| model.active_view()),
        WorkspaceSection::Commits
    );

    // Salir de la vista de commits devuelve el panel del repositorio.
    h.model.update(cx, |model, cx| {
        model.handle_change_view(WorkspaceSection::Changes, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        h.sidebar.read_with(cx, |sidebar, _| sidebar.panel_mode()),
        PanelMode::Repository
    );
}

// ---------------------------------------------------------------------------
// Refresco y ancho
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn refresh_rereads_every_section(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    let h = Harness::open(&repo, cx).await;
    let before = reads(&h, cx);
    // Algo cambia fuera de la app.
    repo.git(&["branch", "outside"]);
    repo.git(&["tag", "v9"]);

    h.click(cx, "sidebar-refresh");
    h.wait_repo(cx, "the manual refresh", |repository| {
        repository.last_mutation_operation.as_deref() == Some("manual-refresh")
    });
    h.settle(cx);

    let after = reads(&h, cx);
    assert_eq!(after[BRANCHES], before[BRANCHES] + 1);
    assert_eq!(after[REMOTES], before[REMOTES] + 1);
    assert_eq!(after[TAGS], before[TAGS] + 1);
    assert_eq!(after[STASHES], before[STASHES] + 1);
    assert!(h.exists(cx, "branch:outside"));
    assert_eq!(h.repo(cx, |repository| repository.tags.clone()), ["v9"]);
    assert!(
        !h.sidebar
            .read_with(cx, |sidebar, cx| sidebar.is_refreshing(cx))
    );
}

#[gpui_kit::test]
async fn dragging_the_resizer_changes_and_saves_the_sidebar_width(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    let h = Harness::open(&repo, cx).await;
    let width = |cx: &mut TestAppContext| {
        h.model
            .read_with(cx, |model, _| model.sidebar_width().width)
    };
    assert_eq!(width(cx), 260.0);
    h.in_window(cx, |window, _| {
        assert_eq!(window.find("sidebar").bounds().size.width, px(260.));
    });

    h.in_window(cx, |window, cx| {
        let handle = window.find("sidebar-resizer").bounds().center();
        window.drag(handle, gpui_kit::point(px(340.), handle.y), cx);
    });
    h.settle(cx);

    assert_eq!(width(cx), 340.0);
    assert!(
        !h.model
            .read_with(cx, |model, _| model.sidebar_width().resizing)
    );
    h.in_window(cx, |window, _| {
        assert_eq!(window.find("sidebar").bounds().size.width, px(340.));
    });
    let saved = h.model.read_with(cx, |model, _| {
        model.backend().storage().get_settings().sidebar_width
    });
    assert_eq!(saved, Some(340.0));

    // Nunca más estrecho que su mínimo.
    h.in_window(cx, |window, cx| {
        let handle = window.find("sidebar-resizer").bounds().center();
        window.drag(handle, gpui_kit::point(px(20.), handle.y), cx);
    });
    h.settle(cx);
    assert_eq!(width(cx), 200.0);

    // Doble clic: vuelve al ancho inicial.
    h.double_click(cx, "sidebar-resizer");
    h.settle(cx);
    assert_eq!(width(cx), 260.0);
}

#[gpui_kit::test]
async fn without_a_repository_the_sidebar_only_shows_its_navigation(cx: &mut TestAppContext) {
    let h = Harness::mount(Some(default_sections()), cx);

    assert!(h.exists(cx, "sidebar-show-repository"));
    assert!(!h.exists(cx, "section-header-branches"));
    // Sin repositorio las órdenes no hacen nada.
    h.in_window(cx, |window, cx| {
        h.sidebar.update(cx, |sidebar, cx| {
            sidebar.run_command(SidebarCommand::CreateTag, window, cx);
            sidebar.refresh(cx);
        });
    });
    assert!(h.host.prompts().is_empty());
}

// ---------------------------------------------------------------------------
// Operaciones de rama con upstream
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn pushes_pulls_and_fast_forwards_the_current_branch_from_its_menu(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    let origin = repo.publish_to_origin();
    let local_commit = repo.commit_file("local.txt", "local\n", "Local commit");
    let h = Harness::open(&repo, cx).await;
    assert_eq!(
        h.repo(cx, |repository| repository.branches[0].ahead),
        1,
        "main is one commit ahead"
    );

    // Push
    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.local_branch_menu("main", cx));
    h.choose_menu_entry(cx, "branch:main", &entries, "Push 'main' to 'origin'");
    h.wait_repo(cx, "the push", |repository| {
        repository.last_mutation_operation.as_deref() == Some("push")
            && repository.branches[0].ahead == 0
    });
    h.settle(cx);
    assert_eq!(
        support::git_in(&origin, &["rev-parse", "main"]).trim(),
        local_commit
    );

    // Alguien publica un commit: la rama queda por detrás.
    let colleague = repo.root().join("colleague");
    support::git_in(
        &repo.root(),
        &[
            "clone",
            &origin.to_string_lossy(),
            &colleague.to_string_lossy(),
        ],
    );
    for (key, value) in [("user.name", "Colleague"), ("user.email", "c@example.com")] {
        support::git_in(&colleague, &["config", key, value]);
    }
    std::fs::write(colleague.join("theirs.txt"), "theirs\n").unwrap();
    support::git_in(&colleague, &["add", "."]);
    support::git_in(&colleague, &["commit", "-m", "Their commit"]);
    support::git_in(&colleague, &["push", "origin", "main"]);
    let their_commit = support::git_in(&colleague, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    repo.git(&["fetch", "origin"]);
    h.click(cx, "sidebar-refresh");
    h.wait_repo(cx, "the branch to be behind", |repository| {
        repository.branches[0].behind == 1
    });
    h.settle(cx);

    // Fast-forward
    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.local_branch_menu("main", cx));
    h.choose_menu_entry(cx, "branch:main", &entries, "Fast-Forward to 'origin/main'");
    h.wait_repo(cx, "the fast-forward", |repository| {
        repository.branches[0].behind == 0
    });
    h.settle(cx);
    assert_eq!(repo.head(), their_commit);

    // Pull (ya al día): relee y registra la mutación igualmente.
    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.local_branch_menu("main", cx));
    h.choose_menu_entry(cx, "branch:main", &entries, "Pull 'origin/main'");
    h.wait_repo(cx, "the pull", |repository| {
        repository.last_mutation_operation.as_deref() == Some("pull")
    });
    h.settle(cx);
    assert_eq!(repo.head(), their_commit);
}

#[gpui_kit::test]
async fn a_conflicting_merge_closes_branch_actions_and_records_the_conflict(
    cx: &mut TestAppContext,
) {
    let repo = TestRepo::with_feature();
    repo.git(&["checkout", "feature"]);
    repo.commit_file("README.md", "# Feature\n", "Feature readme");
    repo.git(&["checkout", "main"]);
    repo.commit_file("README.md", "# Main\n", "Main readme");
    let h = Harness::open(&repo, cx).await;

    h.in_window(cx, |window, cx| {
        h.sidebar.update(cx, |sidebar, cx| {
            sidebar.run_command(
                SidebarCommand::BranchAction {
                    branch: "feature".to_string(),
                    action: mergeon_view_sidebar::BranchAction::Merge,
                },
                window,
                cx,
            );
        });
    });
    h.host.answer_confirm(true);
    h.click(cx, "branch-action-merge");
    h.wait_repo(cx, "the conflicted merge", |repository| {
        repository.last_mutation_operation.as_deref() == Some("merge-conflict")
    });
    h.settle(cx);

    assert!(h.dialog_titles(cx).is_empty());
    assert!(repo.path.join(".git/MERGE_HEAD").exists());
    assert!(h.repo(cx, |repository| repository.conflict_state.in_conflict));
}

#[gpui_kit::test]
async fn rebases_the_current_branch_from_branch_actions(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    repo.commit_file("main.txt", "main\n", "Main moves on");
    let h = Harness::open(&repo, cx).await;

    h.in_window(cx, |window, cx| {
        h.sidebar.update(cx, |sidebar, cx| {
            sidebar.run_command(
                SidebarCommand::BranchAction {
                    branch: "feature".to_string(),
                    action: mergeon_view_sidebar::BranchAction::Rebase,
                },
                window,
                cx,
            );
        });
    });
    // Sin confirmar no pasa nada y el diálogo sigue abierto.
    h.host.answer_confirm(false);
    h.click(cx, "branch-action-rebase");
    h.settle(cx);
    assert_eq!(h.dialog_titles(cx), ["Branch Actions: feature → main"]);
    assert_eq!(
        h.host.confirms()[0].description.as_deref(),
        Some("This will rebase 'main' onto 'feature'.")
    );

    h.host.answer_confirm(true);
    h.click(cx, "branch-action-rebase");
    h.wait_repo(cx, "the rebase", |repository| {
        repository.last_mutation_operation.as_deref() == Some("rebase")
    });
    h.settle(cx);

    assert!(h.dialog_titles(cx).is_empty());
    // `main` quedó encima de `feature`.
    assert_eq!(repo.rev_parse("main~1"), repo.rev_parse("feature"));
}

#[gpui_kit::test]
async fn dropping_a_branch_on_another_rebases_the_drop_target_not_head(cx: &mut TestAppContext) {
    // `feature` se arrastra sobre `hotfix` estando en `main`: se rebasa
    // `hotfix` sobre `feature` y `main` no se toca.
    let repo = TestRepo::with_feature();
    repo.git(&["checkout", "-b", "hotfix"]);
    repo.commit_file("hotfix.txt", "hotfix\n", "Hotfix");
    repo.git(&["checkout", "main"]);
    repo.commit_file("main.txt", "main\n", "Main moves on");
    let main_before = repo.rev_parse("main");
    let h = Harness::open(&repo, cx).await;

    h.in_window(cx, |window, cx| {
        window.drag_to("branch:feature", "branch:hotfix", cx)
    });
    h.wait_ui(cx, "branch actions", |window, cx| {
        mergeon_ui::dialog::open_count(window, cx) == 1
    });
    h.wait_app(cx, "the conflict check", |cx| {
        h.sidebar
            .read(cx)
            .branch_action_dialog()
            .map(|dialog| dialog.read(cx).is_checking_conflicts())
            == Some(false)
    });
    h.host.answer_confirm(true);
    h.click(cx, "branch-action-rebase");
    h.wait_repo(cx, "the rebase", |repository| {
        repository.last_mutation_operation.as_deref() == Some("rebase")
    });
    h.settle(cx);

    assert_eq!(
        h.host.confirms()[0].description.as_deref(),
        Some("This will rebase 'hotfix' onto 'feature'.")
    );
    assert!(h.dialog_titles(cx).is_empty());
    assert_eq!(repo.current_branch(), "hotfix");
    assert_eq!(repo.rev_parse("hotfix~1"), repo.rev_parse("feature"));
    assert_eq!(repo.rev_parse("main"), main_before);
    assert_eq!(
        h.repo(cx, |repository| repository.current_branch.clone()),
        Some("hotfix".to_string())
    );
}

// ---------------------------------------------------------------------------
// Submódulos
// ---------------------------------------------------------------------------

fn submodule_repo() -> TestRepo {
    let repo = TestRepo::new();
    let library = repo.root().join("library");
    std::fs::create_dir(&library).unwrap();
    support::git_in(&library, &["init", "-b", "main"]);
    for (key, value) in [
        ("user.name", "Test User"),
        ("user.email", "test@example.com"),
    ] {
        support::git_in(&library, &["config", key, value]);
    }
    std::fs::write(library.join("lib.txt"), "lib\n").unwrap();
    support::git_in(&library, &["add", "."]);
    support::git_in(&library, &["commit", "-m", "Library"]);
    repo.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        &library.to_string_lossy(),
        "vendor/library",
    ]);
    repo.git(&["commit", "-m", "Add submodule"]);
    repo.git(&["submodule", "deinit", "-f", "vendor/library"]);
    repo
}

#[gpui_kit::test]
async fn lists_submodules_and_refuses_to_open_an_uninitialized_one(cx: &mut TestAppContext) {
    let repo = submodule_repo();
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "submodules"]), cx).await;
    h.wait_ui(cx, "the submodule row", |window, _| {
        window.try_find("submodule:vendor/library").is_some()
    });

    // Sin inicializar solo se ofrece inicializarlo.
    assert!(h.exists(cx, "submodule-update:vendor/library"));
    assert!(!h.exists(cx, "submodule-open:vendor/library"));

    h.double_click(cx, "submodule:vendor/library");
    h.settle(cx);
    assert!(h.host.has_message(
        NotifyType::Warning,
        "Initialize the submodule before opening it."
    ));
    assert_eq!(
        h.model
            .read_with(cx, |model, _| model.open_repositories().len()),
        1
    );
}

// ---------------------------------------------------------------------------
// Repositorios enormes
// ---------------------------------------------------------------------------

/// Muchas ramas locales, ramas de `origin` y tags.
fn huge_repo(branches: usize, tags: usize) -> TestRepo {
    let repo = TestRepo::new();
    repo.git(&["remote", "add", "origin", "/nonexistent/origin.git"]);
    repo.create_refs(
        (0..branches)
            .flat_map(|ix| {
                [
                    format!("refs/heads/feature/b-{ix:05}"),
                    format!("refs/remotes/origin/r-{ix:05}"),
                ]
            })
            .chain((0..tags).map(|ix| format!("refs/tags/t-{ix:05}"))),
    );
    repo
}

#[gpui_kit::test]
async fn builds_only_the_visible_rows_of_huge_sections(cx: &mut TestAppContext) {
    let repo = huge_repo(2_000, 2_000);
    let h = Harness::open_with(
        &repo,
        sections(&["starred", "branches", "remotes", "tags"]),
        cx,
    )
    .await;
    h.wait_repo(cx, "every ref", |repo| {
        repo.branches.len() == 2_001
            && repo.tags.len() == 2_000
            && repo
                .remotes
                .first()
                .is_some_and(|remote| remote.branches.len() == 2_000)
    });

    // Arriba se ven las primeras ramas; ni las últimas ni nada de Remotes o
    // Tags (miles de filas más abajo) se construye.
    assert!(h.exists(cx, "branch:feature/b-00000"));
    assert!(!h.exists(cx, "branch:feature/b-01999"));
    assert!(!h.exists(cx, "remote-branch:origin/r-00000"));
    assert!(!h.exists(cx, "tag:t-00000"));
    assert!(h.exists(cx, "section-header-branches"));

    // Las filas virtuales siguen respondiendo al clic.
    h.click(cx, "branch:feature/b-00001");
    h.wait_app(cx, "the click on a virtual row", |cx| {
        h.sidebar
            .read(cx)
            .viewed_ref(&h.repo_id)
            .is_some_and(|reference| reference == "feature/b-00001")
    });

    // Al bajar hasta el final aparecen los últimos tags y dejan de construirse
    // las primeras ramas.
    h.in_window(cx, |window, cx| {
        window.scroll(
            "sidebar",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-1_000_000.))),
            cx,
        );
    });
    assert!(h.exists(cx, "tag:t-01999"));
    assert!(!h.exists(cx, "tag:t-00000"));
    assert!(!h.exists(cx, "branch:feature/b-00000"));
    // La fila virtual sigue teniendo su menú contextual.
    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.tag_menu("t-01999", cx));
    assert!(!entries.is_empty());
    h.right_click(cx, "tag:t-01999");
    assert!(h.exists(cx, "popup-menu"));
}

#[gpui_kit::test]
async fn reuses_the_repository_data_until_it_changes(cx: &mut TestAppContext) {
    let repo = huge_repo(300, 300);
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "tags"]), cx).await;
    h.wait_repo(cx, "every ref", |repo| {
        repo.branches.len() == 301 && repo.tags.len() == 300
    });
    h.settle(cx);
    let builds = |h: &Harness, cx: &mut TestAppContext| {
        h.sidebar.read_with(cx, |sidebar, _| {
            (sidebar.data_builds(), sidebar.filter_builds())
        })
    };
    let before = builds(&h, cx);

    // Frames sin cambios (hover, redibujos) y notificaciones del modelo que no
    // tocan las ramas no copian ni filtran nada.
    h.hover(cx, "branch:feature/b-00002");
    h.hover(cx, "branch:feature/b-00003");
    h.model.update(cx, |_, cx| cx.notify());
    h.in_window(cx, |_, _| {});
    assert_eq!(builds(&h, cx), before);

    // Cambiar el filtro solo vuelve a filtrar.
    h.in_window(cx, |window, cx| {
        h.sidebar.update(cx, |sidebar, cx| {
            sidebar.set_filter_text("B-0002", window, cx);
        });
    });
    h.in_window(cx, |_, _| {});
    assert_eq!(builds(&h, cx), (before.0, before.1 + 1));
    assert!(h.exists(cx, "branch:feature/b-00020"));
    assert!(!h.exists(cx, "branch:feature/b-00001"));
    h.in_window(cx, |_, _| {});
    assert_eq!(builds(&h, cx), (before.0, before.1 + 1));

    // Una rama nueva sí se copia (y se filtra) otra vez.
    repo.git(&["branch", "feature/b-00029x"]);
    h.click(cx, "sidebar-refresh");
    h.wait_ui(cx, "the new branch", |window, _| {
        window.try_find("branch:feature/b-00029x").is_some()
    });
    let after = builds(&h, cx);
    assert!(after.0 > before.0, "{after:?} vs {before:?}");
    assert!(after.1 > before.1 + 1, "{after:?} vs {before:?}");
}

// ---------------------------------------------------------------------------
// Comparación navegable
// ---------------------------------------------------------------------------

fn comparison_requests(h: &Harness) -> Vec<ComparisonRequest> {
    h.events()
        .into_iter()
        .filter_map(|event| match event {
            SidebarEvent::OpenComparison(request) => Some(request),
            _ => None,
        })
        .collect()
}

#[gpui_kit::test]
async fn compare_with_the_current_branch_asks_for_the_comparison(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let h = Harness::open(&repo, cx).await;

    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.local_branch_menu("feature", cx));
    h.choose_menu_entry(cx, "branch:feature", &entries, "Compare with 'main'...");

    let requests = comparison_requests(&h);
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(
        (request.base.as_str(), request.head.as_str()),
        ("main", "feature")
    );
    assert_eq!(request.mode, CompareMode::SinceDivergence);
    assert_eq!(request.repo_path, repo.id());
    // Solo lee: no abre ningún diálogo propio.
    assert!(h.dialog_titles(cx).is_empty());
}

#[gpui_kit::test]
async fn branch_comparison_browses_the_changes(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let h = Harness::open(&repo, cx).await;

    h.in_window(cx, |window, cx| {
        h.sidebar.update(cx, |sidebar, cx| {
            sidebar.run_command(
                SidebarCommand::BranchAction {
                    branch: "feature".to_string(),
                    action: mergeon_view_sidebar::BranchAction::Compare,
                },
                window,
                cx,
            );
        });
    });
    h.click(cx, "branch-action-compare");
    h.wait_ui(cx, "the comparison", |window, cx| {
        mergeon_ui::dialog::open_count(window, cx) == 2
    });

    h.click(cx, "branch-comparison-browse");
    assert!(h.dialog_titles(cx).is_empty());
    let requests = comparison_requests(&h);
    assert_eq!(requests.len(), 1);
    assert_eq!(
        (requests[0].base.as_str(), requests[0].head.as_str()),
        ("main", "feature")
    );
}

// ---------------------------------------------------------------------------
// Filtros de la búsqueda de commits
// ---------------------------------------------------------------------------

/// main: docs y src; feature: un commit que añade `render_body` en src.
fn filtered_repo() -> TestRepo {
    let repo = TestRepo::new();
    for dir in ["docs", "src"] {
        std::fs::create_dir_all(repo.path.join(dir)).expect("directory");
    }
    repo.commit_file("docs/guide.md", "guide\n", "Write the guide");
    repo.commit_file("src/parser.rs", "fn parse_header() {}\n", "Add the parser");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.commit_file("src/render.rs", "fn render_body() {}\n", "Add the renderer");
    repo.git(&["checkout", "-q", "main"]);
    repo
}

fn search(h: &Harness, cx: &mut TestAppContext, query: &str) -> Vec<String> {
    h.in_window(cx, |window, cx| {
        h.sidebar.update(cx, |sidebar, cx| {
            sidebar.show_search_panel(window, cx);
            sidebar.set_search_query(query, window, cx);
        });
    });
    wait_search(h, cx)
}

fn wait_search(h: &Harness, cx: &mut TestAppContext) -> Vec<String> {
    h.wait_app(cx, "the search", |cx| {
        !h.sidebar.read(cx).is_search_loading()
    });
    h.sidebar.read_with(cx, |sidebar, _| {
        sidebar
            .search_results()
            .iter()
            .map(|result| result.message.clone())
            .collect()
    })
}

#[gpui_kit::test]
async fn filters_the_search_with_the_quick_syntax(cx: &mut TestAppContext) {
    let repo = filtered_repo();
    let h = Harness::open(&repo, cx).await;

    assert_eq!(
        search(&h, cx, "add path:src branch:main"),
        ["Add the parser"]
    );
    assert!(h.exists(cx, "search-chip:path"));
    assert!(h.exists(cx, "search-chip:branch"));
    assert_eq!(
        search(&h, cx, "path:src"),
        ["Add the renderer", "Add the parser"]
    );
    assert_eq!(search(&h, cx, "code:render_body"), ["Add the renderer"]);
    assert_eq!(search(&h, cx, "regex:parse_[a-z]+"), ["Add the parser"]);
    assert_eq!(
        search(&h, cx, "after:1d"),
        [
            "Add the renderer",
            "Add the parser",
            "Write the guide",
            "Initial commit"
        ]
    );
    assert!(search(&h, cx, "before:2001-01-01").is_empty());

    // Una rama que no existe se dice en el panel, no como aviso.
    assert!(search(&h, cx, "branch:nope").is_empty());
    assert_eq!(
        h.sidebar
            .read_with(cx, |sidebar, _| sidebar.search_error().map(str::to_string)),
        Some("Failed to search commits: Unknown branch or revision 'nope'.".to_string())
    );
    assert!(h.exists(cx, "commit-search-error"));
    assert!(h.host.messages(NotifyType::Error).is_empty());

    // La × del chip quita el filtro del cuadro.
    search(&h, cx, "add branch:main path:src");
    h.click(cx, "search-chip-clear:branch");
    assert_eq!(wait_search(&h, cx), ["Add the renderer", "Add the parser"]);
    assert_eq!(
        h.sidebar.read_with(cx, |sidebar, cx| sidebar
            .search_input()
            .read(cx)
            .value()
            .to_string()),
        "add path:src"
    );
}

#[gpui_kit::test]
async fn filters_the_search_from_more_filters(cx: &mut TestAppContext) {
    let repo = filtered_repo();
    let h = Harness::open(&repo, cx).await;
    search(&h, cx, "");

    h.click(cx, "commit-search-filters");
    assert!(h.exists(cx, "commit-search-fields"));
    assert!(!h.exists(cx, "search-field-after"));
    h.click(cx, "tab-custom");
    assert!(h.exists(cx, "search-field-after"));

    h.in_window(cx, |window, cx| {
        h.sidebar.update(cx, |sidebar, cx| {
            sidebar.set_search_filters(
                SearchFilterFields {
                    path: "src".into(),
                    code: "(parse|render)_".into(),
                    code_regex: true,
                    ..Default::default()
                },
                window,
                cx,
            );
        });
    });
    // Sin texto en el cuadro, los filtros solos ya buscan.
    assert_eq!(wait_search(&h, cx), ["Add the renderer", "Add the parser"]);
    let options = h
        .sidebar
        .read_with(cx, |sidebar, _| sidebar.search_options().cloned())
        .unwrap();
    assert_eq!(options.path.as_deref(), Some("src"));
    assert!(options.code_regex);

    h.click(cx, "search-code-regex");
    // `-S` literal: el paréntesis no casa con nada.
    assert!(wait_search(&h, cx).is_empty());

    h.click(cx, "search-chip-clear:code");
    assert_eq!(wait_search(&h, cx), ["Add the renderer", "Add the parser"]);
    h.click(cx, "tab-7d");
    assert_eq!(wait_search(&h, cx).len(), 2);
    assert!(h.exists(cx, "search-chip:date"));
    h.click(cx, "search-chip-clear:date");
    wait_search(&h, cx);
    assert_eq!(
        h.sidebar
            .read_with(cx, |sidebar, cx| sidebar.search_filter_fields(cx).date),
        DatePreset::AnyTime
    );
}

#[gpui_kit::test]
async fn the_search_pages_with_load_more(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    let mut stream = String::new();
    for index in 0..130 {
        let message = format!("change {index:03}");
        stream.push_str(&format!(
            "commit refs/heads/main\ncommitter T <t@e> {} +0000\ndata {}\n{message}\n",
            1_900_000_000 + index * 60,
            message.len()
        ));
        if index == 0 {
            stream.push_str("from refs/heads/main^0\n");
        }
        stream.push_str(&format!(
            "M 644 inline src/file.txt\ndata 4\n{index:03}\n\n"
        ));
    }
    git_stdin(&repo, &["fast-import", "--quiet"], &stream);
    repo.git(&["reset", "--hard", "-q", "main"]);
    let h = Harness::open(&repo, cx).await;

    assert_eq!(search(&h, cx, "path:src").len(), 100);
    assert!(
        h.sidebar
            .read_with(cx, |sidebar, _| sidebar.search_has_more())
    );
    h.click(cx, "commit-search-load-more");
    h.wait_app(cx, "the next page", |cx| {
        h.sidebar.read(cx).search_results().len() > 100
    });
    let results = wait_search(&h, cx);
    assert_eq!(results.len(), 130);
    assert_eq!(results[0], "change 129");
    assert_eq!(results[129], "change 000");
    assert!(
        !h.sidebar
            .read_with(cx, |sidebar, _| sidebar.search_has_more())
    );
}

fn git_stdin(repo: &TestRepo, args: &[&str], stdin: &str) {
    Git::new(&repo.path)
        .raw_with_stdin(args, stdin.as_bytes())
        .expect("git with stdin");
}

// ---------------------------------------------------------------------------
// Clean up merged branches
// ---------------------------------------------------------------------------

/// `main` con `merged` (merge commit), `ff` (fast-forward), `squashed`
/// (squash-merge), `wip` (sin integrar) y `develop` (protegida).
fn repository_with_merged_branches() -> TestRepo {
    let repo = TestRepo::new();
    repo.git(&["checkout", "-q", "-b", "merged"]);
    repo.commit_file("merged.txt", "merged\n", "Merged work");
    repo.git(&["checkout", "-q", "main"]);
    repo.git(&["merge", "-q", "--no-ff", "-m", "Merge merged", "merged"]);
    repo.git(&["checkout", "-q", "-b", "ff"]);
    repo.commit_file("ff.txt", "ff\n", "Fast-forward work");
    repo.git(&["checkout", "-q", "main"]);
    repo.git(&["merge", "-q", "--ff-only", "ff"]);
    repo.git(&["checkout", "-q", "-b", "squashed"]);
    repo.commit_file("squash.txt", "squash\n", "Squash work");
    repo.git(&["checkout", "-q", "main"]);
    repo.git(&["merge", "-q", "--squash", "squashed"]);
    repo.git(&["commit", "-q", "-m", "Squashed work"]);
    repo.git(&["checkout", "-q", "-b", "wip"]);
    repo.commit_file("wip.txt", "wip\n", "Work in progress");
    repo.git(&["checkout", "-q", "main"]);
    repo.git(&["branch", "develop"]);
    repo
}

fn cleanup_dialog(
    h: &Harness,
    cx: &mut TestAppContext,
) -> gpui_kit::Entity<mergeon_view_sidebar::dialogs::CleanupBranchesDialog> {
    let dialog = h
        .sidebar
        .read_with(cx, |sidebar, _| sidebar.cleanup_branches_dialog().cloned())
        .expect("cleanup dialog");
    h.wait_app(cx, "the merged branches", |cx| {
        !dialog.read(cx).is_loading()
    });
    dialog
}

#[gpui_kit::test]
async fn cleans_up_merged_branches_from_the_branches_section_menu(cx: &mut TestAppContext) {
    let repo = repository_with_merged_branches();
    let h = Harness::open(&repo, cx).await;
    h.click(cx, "star:merged");
    h.wait_repo(cx, "the starred branch", |repository| {
        repository.starred_branches == ["merged"]
    });

    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.branches_section_menu(cx));
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.label())
            .collect::<Vec<_>>(),
        ["Clean up merged branches…"]
    );
    assert!(!entries[0].is_disabled());
    h.choose_menu_entry(
        cx,
        "section-header-branches",
        &entries,
        "Clean up merged branches…",
    );
    assert_eq!(h.dialog_titles(cx), ["Clean up merged branches"]);

    let dialog = cleanup_dialog(&h, cx);
    dialog.read_with(cx, |dialog, _| {
        assert_eq!(dialog.base(), Some("main"));
        let mut names: Vec<&str> = dialog
            .candidates()
            .iter()
            .map(|candidate| candidate.name.as_str())
            .collect();
        names.sort_unstable();
        // Ni `main` (base), ni `develop` (protegida), ni `wip` (sin integrar).
        assert_eq!(names, ["ff", "merged", "squashed"]);
        // Las integradas llegan marcadas; la de squash, no.
        let mut selected = dialog.selected_names();
        selected.sort_unstable();
        assert_eq!(selected, ["ff", "merged"]);
        assert!(!dialog.delete_remote());
    });
    assert!(h.exists(cx, "cleanup-row:squashed"));

    // La de squash se marca a mano y se borra con `-D`.
    h.click(cx, "cleanup-check:squashed");
    let request = dialog
        .read_with(cx, |dialog, _| dialog.request())
        .expect("request");
    assert_eq!(request.base, "main");
    assert_eq!(request.branches.len(), 3);
    assert!(
        request
            .branches
            .iter()
            .any(|branch| branch.name == "squashed" && branch.force)
    );

    h.click(cx, "cleanup-submit");
    h.wait_repo(cx, "the cleanup", |repository| {
        repository.last_mutation_operation.as_deref() == Some("cleanup-branches")
    });
    h.settle(cx);

    let mut branches = repo.branches();
    branches.sort_unstable();
    assert_eq!(branches, ["develop", "main", "wip"]);
    assert!(h.dialog_titles(cx).is_empty());
    assert!(
        h.host
            .messages(NotifyType::Success)
            .iter()
            .any(|message| message.contains("Deleted 3 branches.")),
        "{:?}",
        h.host.messages(NotifyType::Success)
    );
    // La destacada borrada deja de estarlo y la operación queda en el log.
    h.wait_repo(cx, "the starred cleanup", |repository| {
        repository.starred_branches.is_empty()
    });
    h.wait(cx, "the log entry", |model| {
        model.operations_log().entries.iter().any(|entry| {
            entry.operation.as_deref() == Some("cleanup-branches")
                && entry.outcome.as_deref() == Some("completed")
        })
    });
}

#[gpui_kit::test]
async fn unmerged_branches_with_a_gone_upstream_need_an_explicit_force(cx: &mut TestAppContext) {
    let repo = repository_with_merged_branches();
    let origin = repo.publish_to_origin();
    repo.git(&["push", "-q", "-u", "origin", "wip"]);
    repo.git(&["push", "-q", "-u", "origin", "ff"]);
    // El PR de `wip` se cerró y su rama se borró en el remoto.
    repo.git(&["push", "-q", "origin", "--delete", "wip"]);
    repo.git(&["fetch", "-q", "--prune", "origin"]);
    let h = Harness::open(&repo, cx).await;

    h.in_window(cx, |window, cx| {
        h.sidebar.update(cx, |sidebar, cx| {
            sidebar.run_command(SidebarCommand::CleanupMergedBranches, window, cx);
        });
    });
    let dialog = cleanup_dialog(&h, cx);
    let wip = dialog.read_with(cx, |dialog, _| {
        dialog
            .candidates()
            .iter()
            .find(|candidate| candidate.name == "wip")
            .cloned()
    });
    let wip = wip.expect("wip is listed: its upstream is gone");
    assert!(wip.upstream_gone);
    assert_eq!(wip.unmerged_commits, 1);

    // Sin "Allow deleting…" no se puede marcar.
    h.click(cx, "cleanup-check:wip");
    assert!(
        !dialog
            .read_with(cx, |dialog, _| dialog.selected_names())
            .contains(&"wip".to_string())
    );
    h.click(cx, "cleanup-force");
    h.click(cx, "cleanup-check:wip");
    h.click(cx, "cleanup-delete-remote");
    let request = dialog
        .read_with(cx, |dialog, _| dialog.request())
        .expect("request");
    assert!(request.delete_remote);
    assert!(
        request
            .branches
            .iter()
            .any(|branch| branch.name == "wip" && branch.force)
    );

    h.click(cx, "cleanup-submit");
    h.wait_repo(cx, "the cleanup", |repository| {
        repository.last_mutation_operation.as_deref() == Some("cleanup-branches")
    });
    h.settle(cx);
    assert!(!repo.branches().contains(&"wip".to_string()));
    assert!(!repo.branches().contains(&"ff".to_string()));
    // `ff` seguía publicada: con "Also delete remote branches" también se va.
    let remote = Git::new(&origin)
        .raw(["for-each-ref", "--format=%(refname:short)", "refs/heads"])
        .unwrap();
    assert_eq!(remote.trim(), "main");
}

// ---------------------------------------------------------------------------
// 0.13.1: errores del sidebar y pulido
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn pushes_a_branch_to_its_renamed_upstream_branch(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    let origin = repo.publish_to_origin();
    // `topic` sigue a `origin/remote-name`, no a `origin/topic`.
    repo.git(&["checkout", "-b", "topic"]);
    repo.commit_file("topic.txt", "topic\n", "Topic");
    repo.git(&["push", "-u", "origin", "topic:remote-name"]);
    let local_commit = repo.commit_file("topic2.txt", "more\n", "More topic");
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "remotes"]), cx).await;
    h.settle(cx);
    let remotes_before = h.reads(cx, "remotes");

    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.local_branch_menu("topic", cx));
    h.choose_menu_entry(cx, "branch:topic", &entries, "Push 'topic' to 'origin'");
    h.wait_repo(cx, "the push", |repository| {
        repository.last_mutation_operation.as_deref() == Some("push")
    });
    h.settle(cx);

    assert_eq!(
        support::git_in(&origin, &["rev-parse", "remote-name"]).trim(),
        local_commit
    );
    assert!(
        Git::new(&origin)
            .raw(["rev-parse", "--verify", "refs/heads/topic"])
            .is_err(),
        "the push must not create origin/topic"
    );
    // El push mueve `origin/remote-name`: también se relee Remotes.
    assert!(h.reads(cx, "remotes") > remotes_before);
}

#[gpui_kit::test]
async fn pull_and_add_remote_go_to_the_operations_log(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    repo.publish_to_origin();
    let upstream = repo.bare_repository("upstream");
    let h = Harness::open(&repo, cx).await;
    let logged = |h: &Harness, cx: &mut TestAppContext, operation: &str| {
        h.model.read_with(cx, |model, _| {
            model.operations_log().entries.iter().any(|entry| {
                entry.operation.as_deref() == Some(operation) && entry.outcome.is_some()
            })
        })
    };

    let entries = h
        .sidebar
        .read_with(cx, |sidebar, cx| sidebar.local_branch_menu("main", cx));
    h.choose_menu_entry(cx, "branch:main", &entries, "Pull 'origin/main'");
    h.wait_repo(cx, "the pull", |repository| {
        repository.last_mutation_operation.as_deref() == Some("pull")
    });
    h.settle(cx);
    assert!(logged(&h, cx, "pull"));

    h.in_window(cx, |window, cx| {
        h.sidebar.update(cx, |sidebar, cx| {
            sidebar.run_command(SidebarCommand::AddRemote, window, cx);
        });
    });
    let dialog = h
        .sidebar
        .read_with(cx, |sidebar, _| sidebar.add_remote_dialog().cloned())
        .expect("add remote dialog");
    // `origin` ya existe: el nombre no se propone.
    assert_eq!(
        dialog.read_with(cx, |dialog, cx| dialog
            .remote_name_input()
            .read(cx)
            .value()
            .to_string()),
        ""
    );
    let url = upstream.to_string_lossy().into_owned();
    h.in_window(cx, |window, cx| {
        let (name, input) = {
            let dialog = dialog.read(cx);
            (
                dialog.remote_name_input().clone(),
                dialog.remote_url_input().clone(),
            )
        };
        name.update(cx, |state, cx| state.set_value("upstream", window, cx));
        input.update(cx, |state, cx| state.set_value(url, window, cx));
    });
    h.click(cx, "add-remote-submit");
    h.wait_repo(cx, "the new remote", |repository| {
        repository.last_mutation_operation.as_deref() == Some("add-remote")
    });
    h.settle(cx);
    assert!(logged(&h, cx, "add-remote"));
}

#[gpui_kit::test]
async fn a_stale_connection_test_does_not_overwrite_a_newer_one(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    let h = Harness::open(&repo, cx).await;
    h.in_window(cx, |window, cx| {
        h.sidebar.update(cx, |sidebar, cx| {
            sidebar.run_command(SidebarCommand::AddRemote, window, cx);
        });
    });
    let dialog = h
        .sidebar
        .read_with(cx, |sidebar, _| sidebar.add_remote_dialog().cloned())
        .expect("add remote dialog");
    // Sin `origin`, se propone.
    assert_eq!(
        dialog.read_with(cx, |dialog, cx| dialog
            .remote_name_input()
            .read(cx)
            .value()
            .to_string()),
        "origin"
    );
    let result = |success: bool| mergeon_backend::RemoteConnectionTest {
        success,
        message: if success { "ok" } else { "old" }.to_string(),
    };
    dialog.update(cx, |dialog, cx| {
        let first = dialog.begin_connection_test(cx);
        let second = dialog.begin_connection_test(cx);
        assert!(!dialog.finish_connection_test(first, result(false), cx));
        assert!(dialog.is_testing_connection());
        assert!(dialog.finish_connection_test(second, result(true), cx));
        assert!(
            dialog
                .connection_result()
                .is_some_and(|result| result.success)
        );
        // Otra URL: el resultado anterior ya no vale.
        dialog.url_changed(cx);
        assert!(dialog.connection_result().is_none());
    });
}

#[gpui_kit::test]
async fn switching_repositories_dismisses_every_sidebar_dialog(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let other = TestRepo::new();
    let h = Harness::open(&repo, cx).await;

    h.in_window(cx, |window, cx| {
        h.sidebar.update(cx, |sidebar, cx| {
            sidebar.run_command(SidebarCommand::AddRemote, window, cx);
            sidebar.run_command(SidebarCommand::ViewTagDetails("v0".to_string()), window, cx);
        });
    });
    h.in_window(cx, |window, cx| {
        h.sidebar.update(cx, |sidebar, cx| {
            sidebar.set_filter_text("feat", window, cx);
            sidebar.toggle_search_filters(cx);
        });
    });
    assert!(!h.dialog_titles(cx).is_empty());

    h.open_repository(&other, cx).await;
    h.settle(cx);

    assert!(h.dialog_titles(cx).is_empty(), "{:?}", h.dialog_titles(cx));
    h.sidebar.read_with(cx, |sidebar, cx| {
        assert!(sidebar.add_remote_dialog().is_none());
        assert!(sidebar.tag_details_dialog().is_none());
        assert!(sidebar.filter_text(cx).is_empty());
        assert!(!sidebar.is_search_filters_open());
    });
}

#[gpui_kit::test]
async fn a_starred_branch_that_no_longer_exists_is_not_checked_out(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let h = Harness::open(&repo, cx).await;
    h.click(cx, "star:feature");
    h.wait_repo(cx, "the star", |repository| {
        repository.starred_branches == ["feature"]
    });
    repo.git(&["branch", "-D", "feature"]);
    h.click(cx, "sidebar-refresh");
    h.wait_repo(cx, "the branch to go", |repository| {
        !repository
            .branches
            .iter()
            .any(|branch| branch.name == "feature")
    });
    h.settle(cx);
    assert!(h.exists(cx, "starred:feature"));

    h.double_click(cx, "starred:feature");
    h.settle(cx);
    assert_eq!(repo.current_branch(), "main");
    assert!(h.host.messages(NotifyType::Error).is_empty());
    assert!(h.events().is_empty());
}

#[gpui_kit::test]
async fn row_and_header_actions_are_disabled_during_an_operation(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    repo.git(&["tag", "v1.0.0"]);
    repo.dirty();
    repo.git(&["stash", "push", "-m", "WIP"]);
    let stash = repo.rev_parse("stash@{0}");
    repo.git(&["checkout", "feature"]);
    repo.commit_file("README.md", "# Feature\n", "Feature readme");
    repo.git(&["checkout", "main"]);
    repo.commit_file("README.md", "# Main\n", "Main readme");
    // Un merge con conflictos deja una operación a medias.
    let _ = Git::new(&repo.path).raw(["merge", "feature"]);
    let h = Harness::open_with(
        &repo,
        sections(&["starred", "branches", "tags", "stashes"]),
        cx,
    )
    .await;
    h.wait_repo(cx, "the merge in progress", |repository| {
        repository
            .operation_state
            .as_ref()
            .is_some_and(|state| state.in_progress)
    });
    h.settle(cx);
    let operation = h.repo(cx, |repository| repository.last_mutation_operation.clone());

    h.click(cx, &format!("stash-apply:{stash}"));
    h.click(cx, "delete-tag:v1.0.0");
    h.hover(cx, "section-header-branches");
    h.click(cx, "cleanup-branches");
    h.settle(cx);

    assert_eq!(repo.stash_count(), 1);
    assert_eq!(repo.tags(), ["v1.0.0"]);
    assert!(h.host.confirms().is_empty());
    assert!(h.host.option_confirms().is_empty());
    assert!(h.dialog_titles(cx).is_empty());
    assert_eq!(
        h.repo(cx, |repository| repository.last_mutation_operation.clone()),
        operation
    );
}

#[gpui_kit::test]
async fn escape_and_clear_filter_empty_the_branch_filter(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let h = Harness::open(&repo, cx).await;
    let filter = |h: &Harness, cx: &mut TestAppContext| {
        h.sidebar
            .read_with(cx, |sidebar, cx| sidebar.filter_text(cx))
    };

    h.click(cx, "sidebar-filter");
    h.in_window(cx, |window, cx| window.input("nothing-matches", cx));
    assert!(h.exists(cx, "clear-branch-filter"));
    h.click(cx, "clear-branch-filter");
    assert_eq!(filter(&h, cx), "");
    assert!(h.exists(cx, "branch:main"));

    h.click(cx, "sidebar-filter");
    h.in_window(cx, |window, cx| window.input("feat", cx));
    assert_eq!(filter(&h, cx), "feat");
    h.in_window(cx, |window, cx| window.press("escape", cx));
    assert_eq!(filter(&h, cx), "");
}

#[gpui_kit::test]
async fn escape_clears_the_commit_search_and_then_leaves_it(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let h = Harness::open(&repo, cx).await;
    h.click(cx, "sidebar-show-search");
    h.click(cx, "commit-search-input");
    h.in_window(cx, |window, cx| window.input("Add", cx));
    h.in_window(cx, |window, cx| window.press("escape", cx));
    h.sidebar.read_with(cx, |sidebar, cx| {
        assert_eq!(sidebar.search_input().read(cx).value().to_string(), "");
        assert_eq!(sidebar.panel_mode(), PanelMode::Search);
    });
    h.in_window(cx, |window, cx| window.press("escape", cx));
    assert_eq!(
        h.sidebar.read_with(cx, |sidebar, _| sidebar.panel_mode()),
        PanelMode::Repository
    );
}

#[gpui_kit::test]
async fn a_navigation_overtaken_by_the_search_panel_is_dropped(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let h = Harness::open(&repo, cx).await;

    h.in_window(cx, |window, cx| {
        h.sidebar.update(cx, |sidebar, cx| {
            sidebar.view_branch("feature", cx);
            sidebar.show_search_panel(window, cx);
        });
    });
    h.settle(cx);

    assert!(
        !h.events()
            .iter()
            .any(|event| matches!(event, SidebarEvent::CommitSelectionRequested { .. }))
    );
    assert_eq!(
        h.sidebar.read_with(cx, |sidebar, _| sidebar
            .viewed_ref(&h.repo_id)
            .map(str::to_string)),
        None
    );
}

#[gpui_kit::test]
async fn unrelated_model_changes_do_not_notify_the_sidebar(cx: &mut TestAppContext) {
    let repo = TestRepo::with_feature();
    let h = Harness::open(&repo, cx).await;
    h.settle(cx);
    // Un frame al día: lo que el modelo cambió ya está pintado.
    h.in_window(cx, |_, _| {});
    let notified = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let _observer = cx.update(|cx| {
        let notified = notified.clone();
        cx.observe(&h.sidebar, move |_, _| notified.set(notified.get() + 1))
    });

    // Un aviso del modelo que no toca nada de lo que pinta el sidebar.
    h.model.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert_eq!(notified.get(), 0);

    // Un cambio de lo que sí pinta (una rama destacada) lo repinta.
    h.click(cx, "star:feature");
    h.wait_repo(cx, "the star", |repository| {
        repository.starred_branches == ["feature"]
    });
    assert!(notified.get() > 0);
}

#[gpui_kit::test]
async fn right_click_hides_row_tooltips_until_the_pointer_moves(cx: &mut TestAppContext) {
    let (repo, hash) = stash_repo();
    let h = Harness::open_with(&repo, sections(&["starred", "branches", "stashes"]), cx).await;
    h.wait_ui(cx, "the stash row", |window, _| {
        window.try_find(format!("stash:{hash}")).is_some()
    });
    let tooltips = |h: &Harness, cx: &mut TestAppContext| {
        h.sidebar
            .read_with(cx, |sidebar, _| sidebar.row_tooltips_enabled())
    };
    assert!(tooltips(&h, cx));

    h.right_click(cx, &format!("stash:{hash}"));
    assert!(!tooltips(&h, cx));

    h.hover(cx, "sidebar-show-search");
    assert!(tooltips(&h, cx));
}

// ---------------------------------------------------------------------------
// Repintados
// ---------------------------------------------------------------------------

fn sidebar_renders(h: &Harness, cx: &mut TestAppContext) -> (usize, usize) {
    h.sidebar.read_with(cx, |sidebar, cx| {
        (
            sidebar.render_count(),
            sidebar.pull_requests_section().read(cx).render_count(),
        )
    })
}

/// El sidebar (y su sección de PRs) pinta desde lo que recogió del modelo,
/// sin leerlo al pintar: un aviso del modelo que no le toca (un toast, un
/// `notify` cualquiera) no lo repinta; uno que sí (plegar una sección,
/// cambiar el ancho) sí.
#[gpui_kit::test]
async fn unrelated_model_notifications_do_not_repaint_the_sidebar(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    let h = Harness::open(&repo, cx).await;
    h.settle(cx);

    let before = sidebar_renders(&h, cx);
    for index in 0..3 {
        h.model.update(cx, |model, cx| {
            model.notify_info(&format!("Something else happened {index}"), cx);
            cx.notify();
        });
        cx.run_until_parked();
    }
    assert_eq!(sidebar_renders(&h, cx), before);

    h.sidebar
        .update(cx, |sidebar, cx| sidebar.toggle_section(Section::Tags, cx));
    h.settle(cx);
    let toggled = sidebar_renders(&h, cx);
    assert!(toggled.0 > before.0, "{toggled:?} vs {before:?}");

    h.model.update(cx, |model, cx| {
        model.nudge_sidebar_width(16.0, 1200.0, cx).detach();
    });
    cx.run_until_parked();
    let resized = sidebar_renders(&h, cx);
    assert!(resized.0 > toggled.0, "{resized:?} vs {toggled:?}");

    // Y, sin más cambios, sigue quieto.
    h.model.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert_eq!(sidebar_renders(&h, cx).0, resized.0);
}
