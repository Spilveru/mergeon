# shellcheck shell=bash
# shellcheck disable=SC2034  # las variables las usan los scripts que incluyen este archivo
# Funciones compartidas por los scripts de empaquetado (se incluye con `source`).

PACKAGING_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
NATIVE_DIR="$(cd "$PACKAGING_DIR/.." && pwd)"
REPO_DIR="$(cd "$NATIVE_DIR/.." && pwd)"

# Identidad del producto (FreeDesktop / Flathub / macOS / Windows).
APP_ID="io.github.spilveru.Mergeon"
PRODUCT_NAME="Mergeon"
BINARY_NAME="mergeon"

# Artefactos finales y directorios de trabajo.
DIST_DIR="${MERGEON_DIST_DIR:-$NATIVE_DIR/dist}"
WORK_DIR="${MERGEON_PACKAGING_WORK_DIR:-$NATIVE_DIR/target/packaging}"
TOOLS_CACHE_DIR="${MERGEON_TOOLS_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/mergeon-packaging}"

log() { printf '\033[1m[packaging]\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31m[packaging] error:\033[0m %s\n' "$*" >&2; exit 1; }

need() {
  local missing=()
  for cmd in "$@"; do
    command -v "$cmd" >/dev/null 2>&1 || missing+=("$cmd")
  done
  [ "${#missing[@]}" -eq 0 ] || die "faltan herramientas: ${missing[*]}"
}

# Versión del producto: la de [workspace.package] en native/Cargo.toml, que es
# la que el binario lleva dentro (CARGO_PKG_VERSION) y compara el actualizador.
mergeon_version() {
  awk '
    /^\[/ { section = $0 }
    section == "[workspace.package]" && /^version[[:space:]]*=/ {
      gsub(/^[^"]*"|"[^"]*$/, ""); print; exit
    }
  ' "$NATIVE_DIR/Cargo.toml"
}

# Compila el binario en release. Variables:
#   CARGO_BUILD_FLAGS  flags extra (p. ej. "--locked -j 2")
#   MERGEON_NICE       prioridad para `nice` (p. ej. 15) en máquinas compartidas
# Argumentos: flags extra de cargo (p. ej. --target <triple>).
# shellcheck disable=SC2120
cargo_build_release() {
  need cargo
  # Un solo arreglo, nunca vacío: bash 3.2 (el de macOS) falla al expandir
  # arreglos vacíos con `set -u`.
  local command=(cargo build -p "$BINARY_NAME" --release)
  if [ -n "${MERGEON_NICE:-}" ]; then
    command=(nice -n "$MERGEON_NICE" "${command[@]}")
  fi
  if [ -n "${CARGO_BUILD_FLAGS:-}" ]; then
    # shellcheck disable=SC2206
    command+=($CARGO_BUILD_FLAGS)
  fi
  log "${command[*]} $*"
  (cd "$NATIVE_DIR" && "${command[@]}" "$@")
}

# Descarga $1 a $2 (si no está) y comprueba su sha256 ($3).
fetch_verified() {
  local url="$1" dest="$2" sha256="$3"
  if [ ! -f "$dest" ] || ! echo "$sha256  $dest" | sha256sum --check --status; then
    need curl sha256sum
    mkdir -p "$(dirname "$dest")"
    log "descargando $url"
    curl --fail --location --silent --show-error --retry 3 -o "$dest.part" "$url"
    echo "$sha256  $dest.part" | sha256sum --check --status \
      || die "sha256 inesperado para $url (¿cambió el archivo publicado?)"
    mv "$dest.part" "$dest"
  fi
}
