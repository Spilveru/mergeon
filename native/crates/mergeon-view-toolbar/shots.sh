#!/bin/bash
# Captura la galería de la barra (examples/toolbar_gallery.rs) en un display
# virtual, en los mismos estados que las capturas de referencia de la app
# Electron (native/target-shots/reference/). Salida:
# native/target-shots/toolbar-<caso>-<tema>.png (pantalla completa de 1600×1000;
# la ventana de 1440×900 empieza en 80,57).
# Uso: crates/mergeon-view-toolbar/shots.sh [dark|light|all] [filtro]
# Antes: cargo build -p mergeon-view-toolbar --example toolbar_gallery
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
NATIVE="$HERE/../.."
THEMES="${1:-all}"; [ "$THEMES" = all ] && THEMES="dark light"
FILTER="${2:-}"
OUT="$NATIVE/target-shots"
BIN="$NATIVE/target/debug/examples/toolbar_gallery"
# La galería muere por señal y no llega a borrar su repositorio temporal: se
# le da un directorio propio y se limpia al terminar.
export TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

shot() { # shot <tema> <caso> <segundos> [VAR=valor...]
  local theme="$1" name="$2" wait="$3"; shift 3
  [ -n "$FILTER" ] && [[ "$name" != *"$FILTER"* ]] && return 0
  # Sin tubería: un push detenido en su hook sobrevive a la galería y la
  # mantendría abierta hasta que el hook termine.
  env MERGEON_THEME="$theme" "$@" MERGEON_BIN="$BIN" \
    "$NATIVE/tools/shot.sh" "$OUT/toolbar-$name-$theme.png" "$wait" >/dev/null 2>&1 || true
}

for theme in $THEMES; do
  # Barra (201, 601, 604, 606) y su disposición estrecha.
  shot "$theme" default 4
  shot "$theme" detached 4 MERGEON_TOOLBAR_STATE=detached
  shot "$theme" empty-repository 4 MERGEON_TOOLBAR_STATE=empty
  shot "$theme" no-repository 3 MERGEON_TOOLBAR_STATE=none
  shot "$theme" compact 4 MERGEON_TOOLBAR_WIDTH=860
  shot "$theme" head-tooltip 5 "MERGEON_TOOLBAR_INPUT=move:890,60;move:892,61;wait:1200"
  # Operación de secuencia (501, 502, 507, 508, 512, 514, 515).
  shot "$theme" merge 4 MERGEON_TOOLBAR_STATE=merge
  shot "$theme" merge-popover 5 MERGEON_TOOLBAR_STATE=merge "MERGEON_TOOLBAR_INPUT=move:934,60;move:935,61;wait:600"
  shot "$theme" merge-partial 4 MERGEON_TOOLBAR_STATE=merge-partial
  shot "$theme" merge-ready 4 MERGEON_TOOLBAR_STATE=merge-ready
  shot "$theme" rebase 4 MERGEON_TOOLBAR_STATE=rebase
  shot "$theme" cherry-pick 4 MERGEON_TOOLBAR_STATE=cherry-pick
  shot "$theme" revert 4 MERGEON_TOOLBAR_STATE=revert
  shot "$theme" confirm-abort 5 MERGEON_TOOLBAR_STATE=merge "MERGEON_TOOLBAR_INPUT=click:1400,110"
  # Cola de mutaciones (520, 521, 522).
  shot "$theme" push-running 5 MERGEON_TOOLBAR_STATE=push-running
  shot "$theme" queue 6 MERGEON_TOOLBAR_STATE=queue
  shot "$theme" queue-popover 7 MERGEON_TOOLBAR_STATE=queue "MERGEON_TOOLBAR_INPUT=move:938,60;move:939,61;wait:600"
  # Diálogos (400–408, 449–451, 541–543).
  shot "$theme" dialog-fetch 5 MERGEON_TOOLBAR_OPEN=fetch
  shot "$theme" dialog-fetch-single-remote 5 MERGEON_TOOLBAR_OPEN=fetch "MERGEON_TOOLBAR_INPUT=click:561,481"
  shot "$theme" dialog-pull 5 MERGEON_TOOLBAR_OPEN=pull
  shot "$theme" dialog-push 5 MERGEON_TOOLBAR_OPEN=push
  shot "$theme" dialog-push-branch-select 5 MERGEON_TOOLBAR_OPEN=push "MERGEON_TOOLBAR_INPUT=click:790,410"
  shot "$theme" prompt-create-stash 5 MERGEON_TOOLBAR_OPEN=stash "MERGEON_TOOLBAR_INPUT=type:WIP: half-finished search filters"
  shot "$theme" dialog-create-branch 5 MERGEON_TOOLBAR_OPEN=new-branch
  shot "$theme" dialog-create-branch-filled 6 MERGEON_TOOLBAR_OPEN=new-branch "MERGEON_TOOLBAR_INPUT=type:feature/payments;click:708,527"
  shot "$theme" dialog-create-branch-clean 5 MERGEON_TOOLBAR_STATE=detached MERGEON_TOOLBAR_OPEN=new-branch
  shot "$theme" dialog-operations-log-empty 5 MERGEON_TOOLBAR_OPEN=operations-log
  shot "$theme" dialog-operations-log 9 MERGEON_TOOLBAR_STATE=log MERGEON_TOOLBAR_OPEN=operations-log
  shot "$theme" dialog-operations-log-expanded 9 MERGEON_TOOLBAR_STATE=log MERGEON_TOOLBAR_OPEN=operations-log "MERGEON_TOOLBAR_INPUT=click:600,472"
  shot "$theme" dialog-operations-log-only-errors 9 MERGEON_TOOLBAR_STATE=log MERGEON_TOOLBAR_OPEN=operations-log "MERGEON_TOOLBAR_INPUT=click:370,396"
  shot "$theme" dialog-reflog 5 MERGEON_TOOLBAR_OPEN=reflog
  shot "$theme" confirm-reflog-reset 6 MERGEON_TOOLBAR_OPEN=reflog "MERGEON_TOOLBAR_INPUT=click:1051,301"
  shot "$theme" prompt-reflog-create-branch 6 MERGEON_TOOLBAR_OPEN=reflog "MERGEON_TOOLBAR_INPUT=click:1025,301"
done
echo listo
