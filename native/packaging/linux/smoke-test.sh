#!/usr/bin/env bash
# Prueba de humo de los artefactos de Linux en una distribución limpia, dentro
# de un contenedor: instala el .deb con apt (así se comprueba que sus
# dependencias bastan), abre la app en un display virtual, guarda una captura y
# hace lo mismo con el AppImage (sin FUSE: APPIMAGE_EXTRACT_AND_RUN).
#
# Uso:
#   native/packaging/linux/smoke-test.sh [imagen] [directorio de capturas]
#     imagen      ubuntu:22.04 por defecto (Debian/Ubuntu; hace falta apt)
#     capturas    native/target-shots/packaging por defecto
#
# Opcional: ELECTRON_DEB=<ruta al .deb de la versión Electron> instala primero
# ese paquete y comprueba que el nativo lo reemplaza limpiamente.
#
# Requiere Docker (o MERGEON_CONTAINER_ENGINE=podman) y los artefactos de la
# versión actual en native/dist. Sin GPU: usa el Vulkan por software de Mesa.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source-path=SCRIPTDIR source=../common.sh
source "$HERE/../common.sh"

IMAGE="${1:-ubuntu:22.04}"
SHOTS="${2:-$NATIVE_DIR/target-shots/packaging}"
ENGINE="${MERGEON_CONTAINER_ENGINE:-docker}"
need "$ENGINE"

VERSION="$(mergeon_version)"
case "$(uname -m)" in
  x86_64) DEB="${BINARY_NAME}_${VERSION}_amd64.deb"; APPIMAGE="$PRODUCT_NAME-$VERSION.AppImage" ;;
  aarch64) DEB="${BINARY_NAME}_${VERSION}_arm64.deb"; APPIMAGE="$PRODUCT_NAME-$VERSION-arm64.AppImage" ;;
  *) die "arquitectura no soportada: $(uname -m)" ;;
esac
[ -f "$DIST_DIR/$DEB" ] || die "no existe $DIST_DIR/$DEB"
[ -f "$DIST_DIR/$APPIMAGE" ] || die "no existe $DIST_DIR/$APPIMAGE"

mkdir -p "$SHOTS"
LABEL="$(printf '%s' "$IMAGE" | tr -c 'A-Za-z0-9.\n' '-')"
mounts=(--volume "$DIST_DIR:/dist:ro" --volume "$SHOTS:/shots" --volume "$HERE:/smoke:ro")
if [ -n "${ELECTRON_DEB:-}" ]; then
  [ -f "$ELECTRON_DEB" ] || die "no existe $ELECTRON_DEB"
  mounts+=(--volume "$(cd "$(dirname "$ELECTRON_DEB")" && pwd)/$(basename "$ELECTRON_DEB"):/electron.deb:ro")
fi

log "prueba de humo en $IMAGE"
# --init: xvfb-run no arranca como PID 1.
"$ENGINE" run --rm --init "${mounts[@]}" \
  --env "DEB=$DEB" --env "APPIMAGE_FILE=$APPIMAGE" --env "LABEL=$LABEL" \
  --env "HOST_UID=$(id -u)" --env "HOST_GID=$(id -g)" \
  "$IMAGE" bash /smoke/smoke-test-in-container.sh
