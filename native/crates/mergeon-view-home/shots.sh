#!/bin/bash
# Captura la galería de mergeon-view-home en un display virtual. Cada captura
# es solo la ventana (1440x900) y lleva el número de la pantalla equivalente de
# native/target-shots/reference/ (ver su README.md).
# Uso: crates/mergeon-view-home/shots.sh [dark|light|all] [filtro]
#   filtro: expresión regular sobre el nombre del caso (p. ej. `picker`, `^10`).
# Antes: cargo build -p mergeon-view-home --example home_gallery
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
NATIVE="$HERE/../.."
THEMES="${1:-all}"; [ "$THEMES" = all ] && THEMES="dark light"
FILTER="${2:-.}"
OUT="$NATIVE/target-shots"
BIN="$NATIVE/target/debug/examples/home_gallery"
WORK="${MERGEON_HOME_WORK:-$HOME/.cache/mergeon-home-gallery}"
mkdir -p "$OUT"

# shot <tema> <caso> <segundos> [VAR=valor...]
shot() {
  local theme="$1" name="$2" wait="$3"; shift 3
  echo "$name" | grep -Eq "$FILTER" || return 0
  local out="$OUT/home-$name-$theme.png"
  env MERGEON_THEME="$theme" "$@" "$NATIVE/tools/in-x.sh" bash -c '
    # El puntero fuera de la ventana: nada sale resaltado por tenerlo encima.
    python3 -c "
import ctypes
x11 = ctypes.CDLL(\"libX11.so.6\")
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
x11.XDefaultRootWindow.restype = ctypes.c_ulong
x11.XWarpPointer.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_int, ctypes.c_int, ctypes.c_uint, ctypes.c_uint, ctypes.c_int, ctypes.c_int]
x11.XFlush.argtypes = [ctypes.c_void_p]
d = x11.XOpenDisplay(None)
x11.XWarpPointer(d, 0, x11.XDefaultRootWindow(d), 0, 0, 0, 0, 2, 2)
x11.XFlush(d)
" 2>/dev/null || true
    "$1" 2> >(grep -v "naga::\|blade\|descriptor\|Presentation support\|xinput" >&2) &
    PID=$!
    sleep "$3"
    # Solo la ventana enfocada, sin el marco del gestor de ventanas.
    scrot -o -u "$2" 2>/dev/null || scrot -o "$2"
    kill $PID 2>/dev/null || true
  ' _ "$BIN" "$out" "$wait"
  echo "$out"
}

TOKEN_INPUT="click:720,597"
SEARCH_INPUT="click:720,342"

for theme in $THEMES; do
  # --- 0xx · Home, lanzador y GitHub ---------------------------------------
  shot "$theme" 001-home-empty 4
  shot "$theme" 002-open-repository-launcher 4 MERGEON_HOME_OPEN=launcher
  shot "$theme" 003-confirm-no-github-accounts 5 MERGEON_HOME_OPEN=launcher \
    "MERGEON_GALLERY_INPUT=id:launcher-github"
  shot "$theme" 004-github-account-dialog 4 MERGEON_HOME_OPEN=account
  shot "$theme" 005-github-account-dialog-token 6 MERGEON_HOME_OPEN=account \
    "MERGEON_GALLERY_INPUT=id:github-token-toggle;$TOKEN_INPUT;type:ghp_exampleexampleexample"
  shot "$theme" 006-github-account-dialog-error 7 MERGEON_HOME_OPEN=account \
    "MERGEON_GALLERY_INPUT=id:github-token-toggle;$TOKEN_INPUT;type:ghp_exampleexampleexample;id:github-token-login"
  shot "$theme" 007-github-account-dialog-device-code 6 MERGEON_HOME_OPEN=account \
    "MERGEON_GALLERY_INPUT=id:github-token-toggle;id:github-device-login"
  shot "$theme" 008-confirm-not-a-git-directory 5 MERGEON_HOME_OPEN=launcher \
    "MERGEON_HOME_PICK=$WORK/plain" "MERGEON_GALLERY_INPUT=id:launcher-local"

  shot "$theme" 010-home-recent-repositories 5 MERGEON_HOME_STATE=recents
  shot "$theme" 011-home-repository-statistics 5 MERGEON_HOME_STATE=recents MERGEON_HOME_TAB=statistics
  shot "$theme" 012-home-repository-without-readme 5 MERGEON_HOME_STATE=recents MERGEON_HOME_SELECT=acme-portal
  shot "$theme" 013-home-repository-unavailable 5 MERGEON_HOME_STATE=recents MERGEON_HOME_SELECT=old-prototype
  shot "$theme" 014-home-group-collapsed 5 MERGEON_HOME_STATE=recents MERGEON_HOME_COLLAPSE=clients
  shot "$theme" 015-confirm-clear-recent 5 MERGEON_HOME_STATE=recents \
    "MERGEON_GALLERY_INPUT=id:clear-recent"
  shot "$theme" 016-home-readme-markdown 5 MERGEON_HOME_STATE=recents MERGEON_HOME_SELECT=api-server
  shot "$theme" 017-home-repository-open 6 MERGEON_HOME_STATE=open

  shot "$theme" 020-github-clone-picker 6 MERGEON_HOME_STATE=recents MERGEON_HOME_OPEN=picker
  shot "$theme" 021-github-clone-picker-selected 7 MERGEON_HOME_STATE=recents MERGEON_HOME_OPEN=picker \
    "MERGEON_GALLERY_INPUT=wait:600;id:github-picker-browse;scroll:github-picker-results,-350;id:github-repo:6"
  shot "$theme" 022-github-clone-picker-search 7 MERGEON_HOME_STATE=recents MERGEON_HOME_OPEN=picker \
    "MERGEON_GALLERY_INPUT=wait:600;id:github-picker-browse;scroll:github-picker-results,-350;id:github-repo:6;$SEARCH_INPUT;type:acme"
  shot "$theme" 023-github-clone-picker-account-select 7 MERGEON_HOME_STATE=recents MERGEON_HOME_OPEN=picker \
    "MERGEON_GALLERY_INPUT=wait:600;id:github-picker-browse;scroll:github-picker-results,-350;id:github-repo:6;id:github-picker-account"
  shot "$theme" 024-github-clone-picker-install-required 6 MERGEON_HOME_STATE=recents MERGEON_HOME_OPEN=picker \
    MERGEON_HOME_ACCOUNT=acme-bot "MERGEON_GALLERY_INPUT=wait:600;id:github-picker-browse"
  shot "$theme" 025-github-clone-picker-empty 6 MERGEON_HOME_STATE=recents MERGEON_HOME_OPEN=picker \
    MERGEON_HOME_ACCOUNT=luis-vega "MERGEON_GALLERY_INPUT=wait:600;id:github-picker-browse"
  shot "$theme" 026-github-clone-picker-cloning 8 MERGEON_HOME_STATE=recents MERGEON_HOME_OPEN=picker \
    "MERGEON_GALLERY_INPUT=wait:600;id:github-picker-browse;scroll:github-picker-results,-252;id:github-repo:3;id:github-picker-clone"

  # --- 1xx · Settings --------------------------------------------------------
  S="MERGEON_HOME_VIEW=settings"
  shot "$theme" 101-settings-appearance 4 $S MERGEON_SETTINGS_SECTION=appearance
  shot "$theme" 102-settings-git-no-repository 4 $S MERGEON_SETTINGS_SECTION=git
  shot "$theme" 103-settings-accounts-empty 4 $S MERGEON_SETTINGS_SECTION=accounts
  shot "$theme" 104-settings-help-about 4 $S MERGEON_SETTINGS_SECTION=help MERGEON_UPDATER=unsupported
  shot "$theme" 105-settings-help-shortcuts 5 $S MERGEON_SETTINGS_SECTION=help MERGEON_UPDATER=unsupported \
    "MERGEON_GALLERY_INPUT=scroll:settings-body,-2000"
  shot "$theme" 106-settings-accounts 5 $S MERGEON_HOME_STATE=recents MERGEON_SETTINGS_SECTION=accounts
  shot "$theme" 107-confirm-remove-account 6 $S MERGEON_HOME_STATE=recents MERGEON_SETTINGS_SECTION=accounts \
    "MERGEON_GALLERY_INPUT=id:github-account-remove:ana-torres"
  shot "$theme" 108-settings-git 6 $S MERGEON_HOME_STATE=open MERGEON_SETTINGS_SECTION=git
  shot "$theme" 109-settings-git-repository-scope 7 $S MERGEON_HOME_STATE=open MERGEON_SETTINGS_SECTION=git \
    "MERGEON_GALLERY_INPUT=wait:800;click:343,246"
  shot "$theme" 110-settings-git-interval-select 7 $S MERGEON_HOME_STATE=open MERGEON_SETTINGS_SECTION=git \
    "MERGEON_GALLERY_INPUT=wait:800;id:prefs-auto-fetch;id:prefs-auto-fetch-interval"
  for state in available downloading ready error uptodate; do
    case $state in
      available) n=111; name=update-available ;;
      downloading) n=112; name=update-downloading ;;
      ready) n=113; name=update-ready ;;
      error) n=114; name=update-error ;;
      uptodate) n=115; name=up-to-date ;;
    esac
    shot "$theme" "$n-settings-help-$name" 5 $S MERGEON_HOME_STATE=open MERGEON_SETTINGS_SECTION=help MERGEON_UPDATER=$state
  done
done
echo listo
