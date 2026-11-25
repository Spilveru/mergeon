#!/usr/bin/env bash
# Prueba de humo del instalador de Windows bajo wine, en un contenedor (no
# instala nada en la máquina). Compila mergeon.nsi con un ejecutable de
# relleno (el notepad.exe de wine haciendo de mergeon.exe) y comprueba la
# lógica del instalador: instalación silenciosa, accesos directos, registro,
# cierre de la app en ejecución, --updated, --force-run, limpieza de la
# versión Electron y desinstalación.
#
# NO sustituye a probar en Windows: wine no es Windows y el ejecutable
# instalado no es Mergeon. Sirve para detectar errores del .nsi sin Windows.
#
# Uso: native/packaging/windows/wine-smoke/run.sh
# Requiere Docker (o MERGEON_CONTAINER_ENGINE=podman).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source-path=SCRIPTDIR source=../../common.sh
source "$HERE/../../common.sh"

ENGINE="${MERGEON_CONTAINER_ENGINE:-docker}"
need "$ENGINE"
IMAGE="${MERGEON_WINE_IMAGE:-mergeon-wine-smoke:ubuntu24.04}"

log "imagen de prueba: $IMAGE"
"$ENGINE" build --quiet --tag "$IMAGE" --file "$HERE/Dockerfile" "$HERE" >&2

# --init: xvfb-run no arranca como PID 1 (espera una señal que nunca le llega).
# El repo va de solo lectura: el instalador se genera dentro del contenedor.
"$ENGINE" run --rm --init \
  --volume "$REPO_DIR:$REPO_DIR:ro" \
  "$IMAGE" \
  xvfb-run -a "$HERE/in-container.sh"
