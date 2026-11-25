#!/bin/bash
# Captura la galería del sidebar en native/target-shots/sidebar-<caso>-<tema>.png.
# Uso: crates/mergeon-view-sidebar/shots.sh [dark|light|all] [regex de casos]
# Antes: cargo build -p mergeon-view-sidebar --example sidebar_gallery
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
NATIVE="$HERE/../.."
THEMES="${1:-all}"; [ "$THEMES" = all ] && THEMES="dark light"
ONLY="${2:-.}"
OUT="$NATIVE/target-shots"
BIN="$NATIVE/target/debug/examples/sidebar_gallery"

shot() { # shot <caso> <segundos> [VAR=valor...]
  local name="$1" wait="$2"; shift 2
  [[ "$name" =~ $ONLY ]] || return 0
  for theme in $THEMES; do
    env MERGEON_THEME="$theme" "$@" MERGEON_BIN="$BIN" \
      "$NATIVE/tools/shot.sh" "$OUT/sidebar-$name-$theme.png" "$wait" 2>&1 | grep -v "xinput\|fixture listo" || true
  done
}

# Las coordenadas de los pasos son de la ventana de 1440x900 de la galería.
shot all 4
shot default 4 MERGEON_SIDEBAR_SECTIONS=default
shot collapsed 4 MERGEON_SIDEBAR_SECTIONS=none
shot filter 5 "MERGEON_SIDEBAR_STEPS=filter:feature"
shot viewing 5 "MERGEON_SIDEBAR_STEPS=view:feature/search"
shot stash-hover 5 "MERGEON_SIDEBAR_STEPS=move:150,683;move:152,684"
shot tag-hover 5 "MERGEON_SIDEBAR_STEPS=move:150,594;move:152,595"
shot branch-tooltip 6 "MERGEON_SIDEBAR_STEPS=move:150,345;move:152,346;wait:1200"
shot search-empty 5 "MERGEON_SIDEBAR_STEPS=panel:search"
shot search-results 6 "MERGEON_SIDEBAR_STEPS=search:login;wait:900;click:160,321"
shot search-author 6 "MERGEON_SIDEBAR_STEPS=search:author:Marta;wait:900"
shot search-no-results 6 "MERGEON_SIDEBAR_STEPS=search:zzz-nothing;wait:900"
shot menu-local-branch 5 "MERGEON_SIDEBAR_STEPS=move:150,345;rclick:150,345"
shot menu-current-branch 5 "MERGEON_SIDEBAR_STEPS=move:150,397;rclick:150,397"
shot menu-remote-branch 5 "MERGEON_SIDEBAR_STEPS=move:160,533;rclick:160,533"
shot menu-tag 5 "MERGEON_SIDEBAR_STEPS=move:140,594;rclick:140,594"
shot menu-remote 5 "MERGEON_SIDEBAR_STEPS=move:120,455;rclick:120,455"
shot dialog-checkout 6 "MERGEON_SIDEBAR_STEPS=checkout:hotfix/crash;wait:900"
shot dialog-track-remote 6 "MERGEON_SIDEBAR_STEPS=checkout-remote:origin/release/1.x;wait:900"
shot dialog-branch-actions-merge 6 "MERGEON_SIDEBAR_STEPS=branch-action:merge:feature/search;wait:1200"
shot dialog-branch-actions-merge-squash 7 "MERGEON_SIDEBAR_STEPS=branch-action:merge:feature/search;wait:900;click:699,606;wait:500;click:700,690;type:Bring search helpers into main"
shot confirm-merge 7 "MERGEON_SIDEBAR_STEPS=branch-action:merge:feature/search;wait:900;click:600,400;wait:600"
shot dialog-branch-actions-rebase 6 "MERGEON_SIDEBAR_STEPS=branch-action:rebase:feature/search;wait:1200"
shot dialog-branch-actions-compare 6 "MERGEON_SIDEBAR_STEPS=branch-action:compare:feature/search;wait:1200"
shot dialog-branch-comparison 7 "MERGEON_SIDEBAR_STEPS=branch-action:compare:feature/search;wait:900;click:870,550;wait:900"
shot dialog-branch-actions-drag-drop 6 "MERGEON_SIDEBAR_STEPS=drop:feature/search:hotfix/crash;wait:1200"
shot prompt-rename-branch 6 "MERGEON_SIDEBAR_STEPS=rename-branch:feature/search;wait:600"
shot confirm-delete-branch 6 "MERGEON_SIDEBAR_STEPS=delete-branch:hotfix/crash;wait:600"
shot confirm-delete-branch-with-option 6 "MERGEON_SIDEBAR_STEPS=delete-branch:feature/login;wait:600"
shot confirm-delete-remote-branch 6 "MERGEON_SIDEBAR_STEPS=delete-remote-branch:origin/release/1.x;wait:600"
shot dialog-add-remote 6 "MERGEON_SIDEBAR_STEPS=add-remote;wait:600"
shot dialog-create-branch 6 "MERGEON_SIDEBAR_STEPS=new-branch:feature/search;wait:900"
shot prompt-edit-remote-url 6 "MERGEON_SIDEBAR_STEPS=edit-remote-url:origin;wait:600"
shot prompt-rename-remote 6 "MERGEON_SIDEBAR_STEPS=rename-remote:origin;wait:600"
shot confirm-remove-remote 6 "MERGEON_SIDEBAR_STEPS=remove-remote:origin;wait:600"
shot dialog-tag-details 6 "MERGEON_SIDEBAR_STEPS=tag-details:v1.0.0;wait:900"
shot confirm-checkout-tag 6 "MERGEON_SIDEBAR_STEPS=checkout-tag:v1.0.0;wait:600"
shot confirm-delete-tag 6 "MERGEON_SIDEBAR_STEPS=delete-tag:v1.0.0;wait:600"
shot prompt-create-tag 6 "MERGEON_SIDEBAR_STEPS=create-tag;wait:600"
shot prompt-branch-from-stash 6 "MERGEON_SIDEBAR_STEPS=stash-branch;wait:600"
shot confirm-pop-stash 6 "MERGEON_SIDEBAR_STEPS=stash-pop;wait:600"
shot confirm-drop-stash 6 "MERGEON_SIDEBAR_STEPS=stash-drop;wait:600"
shot ahead-behind 5 MERGEON_SIDEBAR_FIXTURE=behind
shot detached-head 5 MERGEON_SIDEBAR_FIXTURE=detached
shot submodules 6 MERGEON_SIDEBAR_FIXTURE=submodules MERGEON_SIDEBAR_SECTIONS=starred,branches,remotes,submodules "MERGEON_SIDEBAR_STEPS=move:150,545;move:152,546"
shot empty-repository 5 MERGEON_SIDEBAR_FIXTURE=empty
echo listo
