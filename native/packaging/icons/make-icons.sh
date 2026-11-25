#!/usr/bin/env bash
# Regenera los íconos derivados a partir de icon.png (1024x1024, junto a este script):
#   - hicolor/<n>x<n>.png  para el .deb y el AppImage
#   - mergeon.ico          para mergeon.exe (build.rs) y el instalador NSIS
# Los resultados están versionados: el CI no necesita ImageMagick. Solo hay que
# volver a correr esto si cambia icon.png.
# Requiere ImageMagick (`sudo apt-get install imagemagick`).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOURCE="${1:-$HERE/icon.png}"
[ -f "$SOURCE" ] || { echo "No existe $SOURCE" >&2; exit 1; }

if command -v magick >/dev/null 2>&1; then
  IM=(magick)
elif command -v convert >/dev/null 2>&1; then
  IM=(convert)
else
  echo "Falta ImageMagick (magick/convert)" >&2
  exit 1
fi

mkdir -p "$HERE/hicolor"
for size in 16 24 32 48 64 128 256 512; do
  "${IM[@]}" "$SOURCE" -resize "${size}x${size}" -strip \
    -define png:exclude-chunks=date,time "$HERE/hicolor/${size}x${size}.png"
done

# Windows elige el tamaño según el contexto (lista, barra de tareas, Alt+Tab).
"${IM[@]}" "$SOURCE" -define icon:auto-resize=256,128,64,48,32,24,16 "$HERE/mergeon.ico"

ls -l "$HERE/hicolor" "$HERE/mergeon.ico"
