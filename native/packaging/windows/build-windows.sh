#!/usr/bin/env bash
# Construye el instalador de Windows de la versión nativa:
#   dist/Mergeon-Setup-<version>.exe   (NSIS, por usuario, sin UAC)
#
# Uso (Git Bash en Windows; es el `shell: bash` de GitHub Actions):
#   native/packaging/windows/build-windows.sh [--skip-build] [--exe <ruta>]
#
#   --skip-build   usa el mergeon.exe ya compilado en target/release
#   --exe <ruta>   empaqueta ese ejecutable (también sirve en Linux, donde
#                  makensis existe: así se valida el .nsi sin Windows)
#
# Requisitos en Windows: Rust con el toolchain MSVC (Visual Studio Build
# Tools + Windows SDK, que aporta rc.exe para el ícono del .exe) y NSIS 3
# (`choco install nsis`; los runners windows-latest de GitHub ya lo traen).
#
# Variables: CARGO_BUILD_FLAGS (p. ej. "--locked"), MAKENSIS (ruta de makensis),
# MAKENSIS_EXTRA_ARGS (opciones extra para makensis; las usa la prueba bajo wine).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source-path=SCRIPTDIR source=../common.sh
source "$HERE/../common.sh"

SKIP_BUILD=0
EXE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1 ;;
    --exe) EXE="${2:?--exe necesita una ruta}"; shift ;;
    -h|--help) sed -n '2,17p' "$0"; exit 0 ;;
    *) die "argumento desconocido: $1" ;;
  esac
  shift
done

VERSION="$(mergeon_version)"
[ -n "$VERSION" ] || die "no se pudo leer la versión de $NATIVE_DIR/Cargo.toml"
# VIProductVersion solo admite cuatro números: 0.9.1-beta.2 -> 0.9.1.0
VERSION_NUMERIC="$(printf '%s' "$VERSION" | sed -E 's/^([0-9]+)\.([0-9]+)\.([0-9]+).*$/\1.\2.\3.0/')"
[[ "$VERSION_NUMERIC" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.0$ ]] || die "versión no numérica: $VERSION"

case "${OSTYPE:-}" in
  msys*|cygwin*|win32*) ON_WINDOWS=1 ;;
  *) ON_WINDOWS=0 ;;
esac

if [ -z "$EXE" ]; then
  [ "$ON_WINDOWS" -eq 1 ] || die "fuera de Windows hay que pasar --exe <mergeon.exe>"
  [ "$SKIP_BUILD" -eq 1 ] || cargo_build_release
  EXE="${CARGO_TARGET_DIR:-$NATIVE_DIR/target}/release/$BINARY_NAME.exe"
fi
[ -f "$EXE" ] || die "no existe $EXE"

# makensis: el del PATH o la instalación estándar de NSIS.
MAKENSIS="${MAKENSIS:-}"
if [ -z "$MAKENSIS" ]; then
  if command -v makensis >/dev/null 2>&1; then
    MAKENSIS="$(command -v makensis)"
  else
    for candidate in \
      "/c/Program Files (x86)/NSIS/makensis.exe" \
      "/c/Program Files/NSIS/makensis.exe" \
      "/c/ProgramData/chocolatey/bin/makensis.exe"; do
      if [ -x "$candidate" ]; then
        MAKENSIS="$candidate"
        break
      fi
    done
  fi
fi
[ -n "$MAKENSIS" ] || die "no se encontró makensis (instala NSIS 3: choco install nsis / apt-get install nsis)"

mkdir -p "$DIST_DIR"
OUT="$DIST_DIR/$PRODUCT_NAME-Setup-$VERSION.exe"
ICON="$PACKAGING_DIR/icons/mergeon.ico"
SCRIPT="$HERE/mergeon.nsi"
rm -f "$OUT"

# En Windows makensis quiere rutas de Windows y opciones con "/"; Git Bash
# convertiría esas opciones en rutas si no se le dice que no toque los argumentos.
native_path() {
  if [ "$ON_WINDOWS" -eq 1 ] && command -v cygpath >/dev/null 2>&1; then
    cygpath -w "$1"
  else
    printf '%s' "$1"
  fi
}
if [ "$ON_WINDOWS" -eq 1 ]; then
  OPT="/"
  export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'
else
  OPT="-"
fi

EXTRA_ARGS=()
if [ -n "${MAKENSIS_EXTRA_ARGS:-}" ]; then
  # shellcheck disable=SC2206
  EXTRA_ARGS=($MAKENSIS_EXTRA_ARGS)
fi

log "makensis: $MAKENSIS"
"$MAKENSIS" "${OPT}V2" "${OPT}WX" \
  ${EXTRA_ARGS[@]+"${EXTRA_ARGS[@]}"} \
  "${OPT}DVERSION=$VERSION" \
  "${OPT}DVERSION_NUMERIC=$VERSION_NUMERIC" \
  "${OPT}DAPP_EXE=$(native_path "$EXE")" \
  "${OPT}DICON_FILE=$(native_path "$ICON")" \
  "${OPT}DOUT_FILE=$(native_path "$OUT")" \
  "$(native_path "$SCRIPT")" >&2

[ -f "$OUT" ] || die "makensis no generó $OUT"
log "instalador: $OUT ($(du -h "$OUT" | cut -f1))"
