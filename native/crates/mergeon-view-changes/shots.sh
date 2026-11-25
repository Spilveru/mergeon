#!/bin/bash
# Captura la galería de cambios locales (examples/changes_gallery.rs) en un
# display virtual, en los mismos estados que las capturas de referencia de la
# app Electron (native/target-shots/reference/, 301–320, 445, 503–508, 513).
# Salida: native/target-shots/changes-<NNN>-<caso>-<tema>.png, recortada a la
# ventana (1440×900, la misma geometría que las de referencia).
# Uso: crates/mergeon-view-changes/shots.sh [dark|light|all] [filtro]
# Antes: cargo build -p mergeon-view-changes --example changes_gallery
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
NATIVE="$HERE/../.."
THEMES="${1:-all}"; [ "$THEMES" = all ] && THEMES="dark light"
FILTER="${2:-}"
OUT="$NATIVE/target-shots"
BIN="$NATIVE/target/debug/examples/changes_gallery"
# La galería muere por señal y no llega a borrar su repositorio temporal: se
# le da un directorio propio y se limpia al terminar.
export TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

shot() { # shot <tema> <caso> <segundos> [VAR=valor...]
  local theme="$1" name="$2" wait="$3"; shift 3
  [ -n "$FILTER" ] && [[ "$name" != *"$FILTER"* ]] && return 0
  local file="$OUT/changes-$name-$theme.png"
  env MERGEON_THEME="$theme" "$@" MERGEON_BIN="$BIN" \
    "$NATIVE/tools/shot.sh" "$file" "$wait" >/dev/null 2>&1 || true
  # La ventana de 1440×900 queda centrada en la pantalla de 1600×1000.
  [ -f "$file" ] && convert "$file" -crop 1440x900+80+57 +repage "$file"
}

WORKING_APP=MERGEON_CHANGES_MODE=file:working:src/app.js
HUNK="MERGEON_CHANGES_INPUT=move:600,219;move:602,220;wait:400"

for theme in $THEMES; do
  # Lista y caja de commit (301–304, 316–320).
  shot "$theme" 301-list 5
  shot "$theme" 302-commit-message 6 "MERGEON_CHANGES_INPUT=click:900,170;type:Describe the startup mode;key:shift-enter;key:shift-enter;type:Log the NODE_ENV next to the start date."
  shot "$theme" 303-push-after-commit 6 "MERGEON_CHANGES_INPUT=click:900,170;type:Describe the startup mode;key:shift-enter;key:shift-enter;type:Log the NODE_ENV next to the start date.;click:407,238"
  shot "$theme" 304-amend 5 "MERGEON_CHANGES_INPUT=click:565,238"
  shot "$theme" 316-confirm-discard-all 5 "MERGEON_CHANGES_INPUT=click:351,143;move:700,700;wait:400"
  shot "$theme" 317-partially-staged 5 MERGEON_CHANGES_STATE=stage-app "MERGEON_CHANGES_INPUT=click:200,594;move:700,700"
  shot "$theme" 318-all-staged 5 MERGEON_CHANGES_STATE=all-staged
  shot "$theme" 319-clean 5 MERGEON_CHANGES_STATE=clean
  shot "$theme" 320-amend-pushed-warning 5 MERGEON_CHANGES_STATE=all-staged,pushed "MERGEON_CHANGES_INPUT=click:200,645;click:565,238"
  # Diff de una pestaña de archivo (305–310).
  shot "$theme" 305-diff-unstaged 6 "$WORKING_APP" "$HUNK"
  shot "$theme" 306-diff-split 6 "$WORKING_APP" MERGEON_CHANGES_STATE=split
  shot "$theme" 307-diff-ignore-spaces 6 "$WORKING_APP" MERGEON_CHANGES_STATE=ignore-spaces
  shot "$theme" 308-diff-staged 6 MERGEON_CHANGES_MODE=file:index:README.md "$HUNK"
  shot "$theme" 309-diff-untracked 6 MERGEON_CHANGES_MODE=file:working:notes.txt
  shot "$theme" 310-diff-deleted 6 MERGEON_CHANGES_MODE=file:working:docs/old.md
  # Menús contextuales y lo que abren (311–315).
  shot "$theme" 311-menu-modified-file 5 "MERGEON_CHANGES_INPUT=click:200,204;rclick:213,256;move:217,260;wait:400"
  shot "$theme" 312-menu-untracked-file 5 "MERGEON_CHANGES_INPUT=click:200,204;rclick:213,282;move:217,286;wait:400"
  shot "$theme" 313-menu-staged-file 5 "MERGEON_CHANGES_INPUT=rclick:213,620;move:217,624;wait:400"
  shot "$theme" 314-dialog-blame 6 MERGEON_CHANGES_OPEN=blame:src/app.js
  shot "$theme" 315-confirm-discard-file 6 "MERGEON_CHANGES_INPUT=rclick:213,256;wait:400;click:290,302;wait:400"
  # Stash (445).
  shot "$theme" 445-dialog-stash-diff 6 MERGEON_CHANGES_OPEN=stash
  # Conflictos (503–508, 513): el hueco de arriba incluye el banner.
  shot "$theme" 503-conflicts 5 MERGEON_CHANGES_STATE=merge MERGEON_CHANGES_TOP=175
  shot "$theme" 504-conflict-resolver 6 MERGEON_CHANGES_STATE=merge MERGEON_CHANGES_TOP=206 "$WORKING_APP"
  shot "$theme" 505-conflict-resolver-block-resolved 6 MERGEON_CHANGES_STATE=merge MERGEON_CHANGES_TOP=206 "$WORKING_APP" "MERGEON_CHANGES_INPUT=click:872,315"
  shot "$theme" 506-conflict-resolver-ready 7 MERGEON_CHANGES_STATE=merge MERGEON_CHANGES_TOP=206 "$WORKING_APP" "MERGEON_CHANGES_INPUT=click:872,315;wait:400;click:1100,507"
  shot "$theme" 507-merge-partially-resolved 5 MERGEON_CHANGES_STATE=merge-partial MERGEON_CHANGES_TOP=175
  shot "$theme" 513-rebase-conflict-resolver 6 MERGEON_CHANGES_STATE=rebase MERGEON_CHANGES_TOP=206 "$WORKING_APP"
done
