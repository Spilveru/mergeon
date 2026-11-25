#!/bin/bash
# Captura la galería de commits en native/target-shots/commits-<caso>-<tema>.png
# (recortada a la ventana de 1440x900, como las de referencia).
# Uso: crates/mergeon-view-commits/shots.sh [dark|light|all] [regex de casos]
# Antes: cargo build -p mergeon-view-commits --example commits_gallery
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
NATIVE="$HERE/../.."
THEMES="${1:-all}"; [ "$THEMES" = all ] && THEMES="dark light"
ONLY="${2:-.}"
OUT="$NATIVE/target-shots"
BIN="$NATIVE/target/debug/examples/commits_gallery"

shot() { # shot <caso> <segundos> [VAR=valor...]
  local name="$1" wait="$2"; shift 2
  [[ "$name" =~ $ONLY ]] || return 0
  for theme in $THEMES; do
    local file="$OUT/commits-$name-$theme.png"
    env MERGEON_THEME="$theme" "$@" MERGEON_BIN="$BIN" \
      "$NATIVE/tools/shot.sh" "$file" "$wait" 2>&1 | grep -v "xinput" || true
    # openbox deja la ventana en (80, 57) del display de 1600x1000.
    convert "$file" -crop 1440x900+80+57 +repage "$file"
  done
}

BEHIND='cd .. && git clone -q origin.git colleague && cd colleague && export GIT_AUTHOR_NAME="Luis Vega" GIT_AUTHOR_EMAIL=luis.vega@example.com GIT_COMMITTER_NAME="Luis Vega" GIT_COMMITTER_EMAIL=luis.vega@example.com && printf "# Contributing\n\nOpen a pull request against \`main\`.\n" > CONTRIBUTING.md && git add -A && GIT_AUTHOR_DATE=2026-09-15T09:20:00+00:00 GIT_COMMITTER_DATE=2026-09-15T09:20:00+00:00 git commit -q -m "Add contributing notes" && sed -i "s/First steps with the project./First steps with the demo project./" docs/guide.md && GIT_AUTHOR_DATE=2026-09-15T11:27:00+00:00 GIT_COMMITTER_DATE=2026-09-15T11:27:00+00:00 git commit -qam "Name the project in the guide" && git push -q origin main && cd ../demo && git fetch -q origin'

# Las coordenadas de los pasos son de la ventana de 1440x900 de la galería.
shot history 6
shot commit-selected 6 "MERGEON_COMMITS_STEPS=select:Clarify the login"
shot commit-many-files 6 "MERGEON_COMMITS_STEPS=select:Initial commit"
shot merge-selected 6 "MERGEON_COMMITS_STEPS=select:Merge branch"
shot summary-expanded 6 "MERGEON_COMMITS_STEPS=select:Merge branch;expand"
shot file-diff 7 "MERGEON_COMMITS_STEPS=select:Clarify the login;wait:600;file:src/auth/login.js;wait:900"
shot file-diff-split 7 "MERGEON_COMMITS_STEPS=select:Clarify the login;wait:600;file:src/auth/login.js;wait:600;split"
shot file-list-hidden 7 "MERGEON_COMMITS_STEPS=select:Clarify the login;wait:600;file:src/auth/login.js;wait:600;files:off"
shot file-menu 8 "MERGEON_COMMITS_STEPS=select:Clarify the login;wait:600;file:src/auth/login.js;wait:900;rclick:230,197"
shot whitespace 7 MERGEON_COMMITS_FIXTURE=website "MERGEON_COMMITS_STEPS=select:Wrap the nav;wait:600;file:index.html;wait:900"
shot ignore-spaces 8 MERGEON_COMMITS_FIXTURE=website "MERGEON_COMMITS_STEPS=select:Wrap the nav;wait:600;file:index.html;wait:900;ignore-spaces;wait:900"
shot ignore-spaces-split 8 MERGEON_COMMITS_FIXTURE=website "MERGEON_COMMITS_STEPS=select:Wrap the nav;wait:600;file:index.html;wait:900;ignore-spaces;wait:600;split;wait:600"
shot repo-tree 7 "MERGEON_COMMITS_STEPS=select:Reword the startup;wait:600;repo-tree;wait:900"
shot repo-tree-file 8 "MERGEON_COMMITS_STEPS=select:Reword the startup;wait:600;repo-tree;wait:900;tree:src;wait:600;tree:src/app.js;wait:900"
shot file-tabs 8 "MERGEON_COMMITS_STEPS=select:Initial commit;wait:600;file:README.md;wait:600;commits;wait:400;file:styles/main.css;wait:900"
shot binary-file 8 MERGEON_COMMITS_FIXTURE=website "MERGEON_COMMITS_STEPS=repo-tree;wait:900;tree:assets;wait:600;tree:assets/logo.png;wait:900"
shot branch-viewing 6 "MERGEON_COMMITS_STEPS=view:feature/search;wait:900"
shot context-menu 7 "MERGEON_COMMITS_STEPS=select:Clarify the login;wait:400;row-rclick:Add logout;wait:600"
shot checkout-commit 7 "MERGEON_COMMITS_STEPS=row-dclick:Add logout;wait:1200;move:700,880"
shot checkout-choice 7 "MERGEON_COMMITS_SETUP=git branch feature/search-v2 feature/search" "MERGEON_COMMITS_STEPS=row-dclick:Search ignoring case;wait:1200;move:700,880"
shot confirm-reset-mixed 6 "MERGEON_COMMITS_STEPS=command:reset-mixed:Explain usage;wait:600"
shot confirm-reset-hard 6 "MERGEON_COMMITS_STEPS=command:reset-hard:Explain usage;wait:600"
shot confirm-squash 6 "MERGEON_COMMITS_STEPS=command:squash:Expose the version;wait:600"
shot prompt-reword 6 "MERGEON_COMMITS_STEPS=command:reword:Expose the version;wait:600"
shot confirm-drop 6 "MERGEON_COMMITS_STEPS=command:drop:Expose the version;wait:600"
# El rango atraviesa el merge de feature/login: aviso en vez de diálogo (531).
shot rebase-blocked 6 "MERGEON_COMMITS_STEPS=command:rebase:Style the buttons;wait:600"
# Con un stash guardado ya no se niega (el original sí: ver docs/view-commits.md).
shot rebase-with-stash 6 "MERGEON_COMMITS_STEPS=command:rebase:Explain usage;wait:600"
shot interactive-rebase 6 MERGEON_COMMITS_FIXTURE=api "MERGEON_COMMITS_STEPS=command:rebase:Bump version;wait:600"
shot interactive-rebase-edited 7 MERGEON_COMMITS_FIXTURE=api "MERGEON_COMMITS_STEPS=command:rebase:Bump version;wait:600;rebase-action:1:squash;rebase-action:2:reword;rebase-message:2:Return a sample user from /users;rebase-action:3:drop;wait:400"
shot revert-completed 6 MERGEON_COMMITS_FIXTURE=website "MERGEON_COMMITS_STEPS=command:revert:Tighten the hero;wait:1500"
shot cherry-pick-completed 6 MERGEON_COMMITS_FIXTURE=website "MERGEON_COMMITS_STEPS=command:cherry-pick:Try the new typeface;wait:1500"
shot ahead-behind 7 MERGEON_HISTORY_WIDTH=700 "MERGEON_COMMITS_SETUP=$BEHIND"
shot to-pull-selected 7 MERGEON_HISTORY_WIDTH=700 "MERGEON_COMMITS_SETUP=$BEHIND" "MERGEON_COMMITS_STEPS=select:Name the project;wait:600"
shot empty-repository 6 MERGEON_COMMITS_FIXTURE=empty
# shot.sh mata la galería antes de que borre su fixture temporal.
rm -rf "${TMPDIR:-/tmp}"/mergeon-commits-gallery-*
echo listo
