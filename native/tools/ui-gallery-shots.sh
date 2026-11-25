#!/bin/bash
# Captura la galería del sistema de diseño (cargo run -p mergeon-ui --example gallery)
# en un display virtual: páginas, diálogos y toasts abiertos, y estados que piden
# interacción. Salida: native/target-shots/ui-gallery-<tema>-<nombre>.png, más
# ui-gallery-<tema>.png (la página de controles).
# Uso: native/tools/ui-gallery-shots.sh [dark|light|all] [pages|overlays|states|all]
# Antes: cargo build -p mergeon-ui --example gallery
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
THEMES="${1:-all}"; [ "$THEMES" = all ] && THEMES="dark light"
WHAT="${2:-all}"
OUT="$HERE/../target-shots"
BIN="$HERE/../target/debug/examples/gallery"

shot() { # shot <tema> <nombre> [VAR=valor...]
  local theme="$1" name="$2"; shift 2
  env MERGEON_THEME="$theme" "$@" MERGEON_BIN="$BIN" \
    "$HERE/shot.sh" "$OUT/ui-gallery-$theme-$name.png" 3 2>&1 | grep -v "xinput" || true
}

for theme in $THEMES; do
  if [ "$WHAT" = pages ] || [ "$WHAT" = all ]; then
    for page in controls forms feedback overlays; do
      shot "$theme" "$page" MERGEON_GALLERY_PAGE=$page
    done
    cp "$OUT/ui-gallery-$theme-controls.png" "$OUT/ui-gallery-$theme.png"
  fi
  if [ "$WHAT" = overlays ] || [ "$WHAT" = all ]; then
    for open in dialog dialog-long confirm confirm-destructive confirm-option prompt \
      prompt-textarea stacked toasts toast-action toast-path toast-success toast-warning details; do
      shot "$theme" "open-$open" MERGEON_GALLERY_OPEN=$open
    done
    # El error de validación aparece al confirmar.
    shot "$theme" open-prompt-error MERGEON_GALLERY_OPEN=prompt-error "MERGEON_GALLERY_INPUT=key:enter"
  fi
  if [ "$WHAT" = states ] || [ "$WHAT" = all ]; then
    # Coordenadas de la ventana de 1440x900 de la galería.
    shot "$theme" state-context-menu MERGEON_GALLERY_PAGE=overlays "MERGEON_GALLERY_INPUT=rclick:560,151"
    shot "$theme" state-dropdown MERGEON_GALLERY_PAGE=overlays "MERGEON_GALLERY_INPUT=click:703,151"
    shot "$theme" state-hover-card MERGEON_GALLERY_PAGE=overlays "MERGEON_GALLERY_INPUT=move:70,420;move:75,421"
    shot "$theme" state-tooltip MERGEON_GALLERY_PAGE=overlays "MERGEON_GALLERY_INPUT=move:148,420;move:150,421"
    shot "$theme" state-select-open MERGEON_GALLERY_PAGE=forms "MERGEON_GALLERY_INPUT=click:1190,174"
    shot "$theme" state-input-focus MERGEON_GALLERY_PAGE=forms "MERGEON_GALLERY_INPUT=click:240,174"
    shot "$theme" state-button-focus MERGEON_GALLERY_PAGE=controls "MERGEON_GALLERY_INPUT=focus:8"
    shot "$theme" state-forms-focus MERGEON_GALLERY_PAGE=forms "MERGEON_GALLERY_INPUT=focus:12"
    shot "$theme" state-button-hover MERGEON_GALLERY_PAGE=controls "MERGEON_GALLERY_INPUT=move:250,224;move:252,225"
    shot "$theme" state-toast-details MERGEON_GALLERY_OPEN=toast-error "MERGEON_GALLERY_INPUT=click:1150,120"
  fi
done
echo listo
