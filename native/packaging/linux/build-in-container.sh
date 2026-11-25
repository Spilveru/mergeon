#!/usr/bin/env bash
# Construye los artefactos de Linux dentro de un contenedor con una glibc
# antigua (ver Dockerfile), para que corran en distribuciones más viejas que la
# máquina de compilación. Es lo que usa el workflow de release.
#
# Uso:
#   native/packaging/linux/build-in-container.sh [argumentos de build-linux.sh]
#
# Requiere Docker (o Podman con MERGEON_CONTAINER_ENGINE=podman).
#
# Variables:
#   MERGEON_BUILD_IMAGE   nombre de la imagen (mergeon-linux-build:ubuntu22.04)
#   RUST_TOOLCHAIN        toolchain de Rust de la imagen (stable)
#   MERGEON_MAX_GLIBC     glibc máxima admitida en el binario (2.35)
#   MERGEON_CONTAINER_TARGET_DIR
#                         directorio de compilación (native/target/linux-container)
#   CARGO_BUILD_FLAGS, CARGO_BUILD_JOBS, MERGEON_NICE, APP_UPDATE_FEED_URL,
#   MERGEON_UPDATE_PUBLIC_KEY, SOURCE_DATE_EPOCH, MERGEON_GITHUB_*
#                         pasan tal cual a build-linux.sh / cargo
#
# La compilación usa su propio directorio (no el de la máquina: los objetos
# enlazan contra otra glibc) y guarda el registro de cargo en
# $TOOLS_CACHE_DIR/cargo-home, así que las siguientes ejecuciones son
# incrementales.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source-path=SCRIPTDIR source=../common.sh
source "$HERE/../common.sh"

ENGINE="${MERGEON_CONTAINER_ENGINE:-docker}"
need "$ENGINE"
IMAGE="${MERGEON_BUILD_IMAGE:-mergeon-linux-build:ubuntu22.04}"
CONTAINER_TARGET_DIR="${MERGEON_CONTAINER_TARGET_DIR:-$NATIVE_DIR/target/linux-container}"

log "imagen de compilación: $IMAGE"
"$ENGINE" build --quiet \
  --build-arg "RUST_TOOLCHAIN=${RUST_TOOLCHAIN:-stable}" \
  --tag "$IMAGE" --file "$HERE/Dockerfile" "$HERE" >&2

mkdir -p "$TOOLS_CACHE_DIR/cargo-home" "$CONTAINER_TARGET_DIR" "$DIST_DIR"

# Variables opcionales que llegan al contenedor solo si están definidas.
passthrough=()
for name in CARGO_BUILD_FLAGS CARGO_BUILD_JOBS MERGEON_NICE APP_UPDATE_FEED_URL \
  MERGEON_UPDATE_PUBLIC_KEY SOURCE_DATE_EPOCH \
  MERGEON_GITHUB_CLIENT_ID MERGEON_GITHUB_APP_MODE MERGEON_GITHUB_APP_SLUG; do
  if [ -n "${!name:-}" ]; then
    passthrough+=(--env "$name")
  fi
done

# El repo se monta en su misma ruta y el proceso corre con el uid del
# anfitrión: los artefactos quedan en native/dist con el dueño correcto.
# shellcheck disable=SC2016  # $HOME y $@ los expande el bash del contenedor
"$ENGINE" run --rm \
  --user "$(id -u):$(id -g)" \
  --env HOME=/tmp/home \
  --env CARGO_HOME=/cargo-home \
  --env "CARGO_TARGET_DIR=$CONTAINER_TARGET_DIR" \
  --env MERGEON_TOOLS_CACHE_DIR=/tools-cache \
  --env "MERGEON_DIST_DIR=$DIST_DIR" \
  --env "MERGEON_PACKAGING_WORK_DIR=$CONTAINER_TARGET_DIR/packaging" \
  --env "MERGEON_MAX_GLIBC=${MERGEON_MAX_GLIBC:-2.35}" \
  "${passthrough[@]}" \
  --volume "$REPO_DIR:$REPO_DIR" \
  --volume "$DIST_DIR:$DIST_DIR" \
  --volume "$CONTAINER_TARGET_DIR:$CONTAINER_TARGET_DIR" \
  --volume "$TOOLS_CACHE_DIR/cargo-home:/cargo-home" \
  --volume "$TOOLS_CACHE_DIR:/tools-cache" \
  --workdir "$NATIVE_DIR" \
  "$IMAGE" \
  bash -c 'mkdir -p "$HOME" && exec "$@"' _ "$HERE/build-linux.sh" "$@"
