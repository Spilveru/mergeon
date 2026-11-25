#!/bin/bash
# Lanza la app nativa en un display virtual, espera y captura la pantalla.
# Uso: native/tools/shot.sh <salida.png> [segundos=4] [-- args de la app]
# Las variables de entorno del llamador (MERGEON_*) llegan a la app.
set -euo pipefail
OUT="${1:?salida.png}"; WAIT="${2:-4}"; shift; shift || true
[ "${1:-}" = "--" ] && shift
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${MERGEON_BIN:-$HERE/../target/debug/mergeon}"
mkdir -p "$(dirname "$OUT")"
"$HERE/in-x.sh" bash -c '
  "$1" "${@:4}" 2> >(grep -v "naga::\|blade\|descriptor\|Presentation support" >&2) &
  PID=$!
  sleep "$3"
  scrot -o "$2"
  kill $PID 2>/dev/null || true
' _ "$BIN" "$OUT" "$WAIT" "$@"
