#!/bin/bash
# Lanza la app nativa en un display virtual y ejecuta una secuencia de pasos
# con xdotool, capturando pantallas. Coordenadas de pantalla (1600x1000; la
# ventana de 1440x900 queda en x+80, y+57 bajo openbox).
# Uso: native/tools/drive.sh <dir-salida> <repo> "paso;paso;..."
#   pasos: click:X,Y  jclick:X,Y (con 3px de temblor)  dclick:X,Y  rclick:X,Y  key:combo  type:texto  wheel:n (rueda abajo, n pasos)  wait:segundos  shot:nombre
set -euo pipefail
OUT="$1"; REPO="$2"; STEPS="$3"
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${MERGEON_BIN:-$HERE/../target/debug/mergeon}"
mkdir -p "$OUT"
"$HERE/in-x.sh" bash -c '
  "$1" "$2" 2>"$3/app.log" &
  PID=$!
  sleep 7
  IFS=";" read -ra STEPS <<< "$4"
  for step in "${STEPS[@]}"; do
    kind="${step%%:*}"; arg="${step#*:}"
    case "$kind" in
      click) xdotool mousemove ${arg/,/ } click 1 ;;
      dclick) xdotool mousemove ${arg/,/ } click --repeat 2 --delay 80 1 ;;
      rclick) xdotool mousemove ${arg/,/ } click 3 ;;
      jclick) # clic de mano real: se mueve 3px con el botón apretado
        xdotool mousemove ${arg/,/ } mousedown 1; sleep 0.05
        xdotool mousemove_relative 2 1; sleep 0.05; xdotool mousemove_relative 1 0; sleep 0.05
        xdotool mouseup 1 ;;
      move) xdotool mousemove ${arg/,/ } ;;
      key) xdotool key "$arg" ;;
      type) xdotool type --delay 30 "$arg" ;;
      wheel) xdotool click --repeat "$arg" --delay 15 5 ;; # n pasos de rueda hacia abajo
      wait) sleep "$arg" ;;
      shot) sleep 0.8; scrot -o "$3/$arg.png" ;;
    esac
    sleep 0.4
  done
  kill $PID 2>/dev/null || true
' _ "$BIN" "$REPO" "$OUT" "$STEPS"
