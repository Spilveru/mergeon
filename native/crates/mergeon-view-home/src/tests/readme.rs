//! El README del resumen: HTML incrustado, badges y la lista virtual.

use gpui_kit::{ListOffset, TestAppContext, px};

use super::support::{TestApp, Workspace, repo_row, seeded_store};
use crate::HomeView;

const KANEO: &str = r#"<p align="center">
  <a href="https://kaneo.app">
    <img src="https://assets.kaneo.app/logo.png" alt="Kaneo's logo" width="300" />
  </a>
</p>

<div align="center">

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/usekaneo/kaneo/actions/workflows/ci.yml/badge.svg)](https://github.com/usekaneo/kaneo/actions)

</div>

```bash
pnpm install
```
"#;

#[gpui_kit::test]
async fn badges_are_pills_that_open_their_link(cx: &mut TestAppContext) {
    let workspace = Workspace::new();
    let open = workspace.repository("work/kaneo", Some(KANEO));
    let app = TestApp::with_store(seeded_store(&[], &[]), cx);
    app.start(cx);
    app.open_repository(&open, cx);
    app.show(cx, HomeView::new);

    assert!(app.exists(cx, "readme-panel"));
    // Fila 0: el logo; fila 1: los dos badges en una sola fila; fila 2: el
    // bloque de código.
    assert!(app.exists(cx, "readme-0-img-0"));
    assert!(app.exists(cx, "readme-1-img-0"));
    assert!(app.exists(cx, "readme-1-img-1"));
    assert!(app.exists(cx, "readme-2-code"));
    let first = app.find(cx, "readme-1-img-0").expect("badge License");
    let second = app.find(cx, "readme-1-img-1").expect("badge CI");
    assert_eq!(
        first.bounds().origin.y,
        second.bounds().origin.y,
        "los badges van en la misma fila"
    );

    // Un enlace relativo no sale de la app; uno http(s) sí.
    app.click(cx, "readme-1-img-0");
    assert!(app.launcher.launched_args().is_empty());
    app.click(cx, "readme-1-img-1");
    app.wait_for(cx, "abrir el enlace del badge", |_| {
        app.launcher
            .launched_args()
            .iter()
            .any(|arg| arg == "https://github.com/usekaneo/kaneo/actions")
    });
}

#[gpui_kit::test]
async fn a_long_readme_is_parsed_once_and_painted_on_demand(cx: &mut TestAppContext) {
    let long: String = (0..400)
        .map(|index| format!("```\nblock {index}\n```\n\n"))
        .collect();
    let workspace = Workspace::new();
    let first = workspace.repository("work/long", Some(&long));
    let second = workspace.repository("work/short", Some("# Short\n"));
    let app = TestApp::with_store(seeded_store(&[&second], &[]), cx);
    app.start(cx);
    app.open_repository(&first, cx);
    let home = app.show(cx, HomeView::new);
    home.read_with(cx, |home, _| {
        assert_eq!(home.selected_repo_path(), Some(first.as_str()))
    });

    // Solo se pinta lo visible.
    assert!(app.exists(cx, "readme-0-code"));
    assert!(!app.exists(cx, "readme-399-code"));

    // Los frames siguientes no vuelven a interpretar el README.
    let parsed = home.read_with(cx, |home, _| {
        home.readme_document()
            .expect("README interpretado")
            .parsed_ptr()
    });
    app.in_window(cx, |window, _| window.refresh());
    app.settle(cx);
    assert!(app.exists(cx, "readme-0-code"));
    home.read_with(cx, |home, _| {
        assert_eq!(
            home.readme_document().map(|doc| doc.parsed_ptr()),
            Some(parsed)
        )
    });

    // Al final de la lista se pinta el último bloque.
    home.read_with(cx, |home, _| {
        home.readme_document()
            .expect("README interpretado")
            .list_state()
            .scroll_to(ListOffset {
                item_ix: 399,
                offset_in_item: px(0.),
            })
    });
    app.in_window(cx, |window, _| window.refresh());
    assert!(app.exists(cx, "readme-399-code"));
    assert!(!app.exists(cx, "readme-0-code"));

    // Otro repositorio, otro README: se interpreta el nuevo.
    app.click(cx, repo_row(&second));
    assert!(!app.exists(cx, "readme-1-code"));
    home.read_with(cx, |home, _| {
        assert_ne!(
            home.readme_document().map(|doc| doc.parsed_ptr()),
            Some(parsed)
        )
    });
}
