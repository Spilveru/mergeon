#!/usr/bin/env bash
# Construye los artefactos de Linux de la versión nativa, sin Node ni
# electron-builder:
#   dist/Mergeon-<version>.AppImage          (se actualiza sola)
#   dist/mergeon_<version>_<arch>.deb        (no se actualiza sola)
#
# Uso:
#   native/packaging/linux/build-linux.sh [--skip-build] [--bin <ruta>]
#                                         [--appimage] [--deb]
# Sin --appimage ni --deb construye los dos.
#
# Variables:
#   CARGO_BUILD_FLAGS      flags extra para cargo (p. ej. "--locked -j 2")
#   MERGEON_NICE           prioridad de `nice` para la compilación
#   APP_UPDATE_FEED_URL    canal de actualizaciones que queda fijado en el
#                          AppImage (el .deb no se actualiza solo)
#   MERGEON_MAX_GLIBC      si se define (p. ej. 2.35), falla cuando el binario
#                          exige una glibc más nueva
#   SOURCE_DATE_EPOCH      fecha fija para builds reproducibles
#
# El binario exige la glibc de la máquina donde se compila: para publicar hay
# que compilar en una distribución antigua (ver build-in-container.sh).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source-path=SCRIPTDIR source=../common.sh
source "$HERE/../common.sh"

# Versiones fijadas de las herramientas del AppImage (se descargan una vez a
# $TOOLS_CACHE_DIR y se verifican por sha256).
APPIMAGETOOL_VERSION="1.9.1"
APPIMAGE_RUNTIME_VERSION="20251108"
declare -A APPIMAGETOOL_SHA256=(
  [x86_64]="ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0"
  [aarch64]="f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158"
)
declare -A APPIMAGE_RUNTIME_SHA256=(
  [x86_64]="2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d"
  [aarch64]="00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444"
)

SKIP_BUILD=0
BIN=""
WANT_APPIMAGE=0
WANT_DEB=0
while [ $# -gt 0 ]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1 ;;
    --bin) BIN="${2:?--bin necesita una ruta}"; shift ;;
    --appimage) WANT_APPIMAGE=1 ;;
    --deb) WANT_DEB=1 ;;
    -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
    *) die "argumento desconocido: $1" ;;
  esac
  shift
done
if [ "$WANT_APPIMAGE" -eq 0 ] && [ "$WANT_DEB" -eq 0 ]; then
  WANT_APPIMAGE=1
  WANT_DEB=1
fi

[ "$(uname -s)" = "Linux" ] || die "este script solo corre en Linux"
MACHINE="$(uname -m)"
case "$MACHINE" in
  x86_64) DEB_ARCH="amd64" ;;
  aarch64) DEB_ARCH="arm64" ;;
  *) die "arquitectura no soportada: $MACHINE" ;;
esac

VERSION="$(mergeon_version)"
[ -n "$VERSION" ] || die "no se pudo leer la versión de $NATIVE_DIR/Cargo.toml"
EPOCH="${SOURCE_DATE_EPOCH:-$(date +%s)}"
BUILD_DATE="$(date -u -d "@$EPOCH" +%Y-%m-%d)"
ICONS_DIR="$PACKAGING_DIR/icons/hicolor"
ICON_SIZES=(16 24 32 48 64 128 256 512)

# --- binario ----------------------------------------------------------------

if [ -z "$BIN" ]; then
  [ "$SKIP_BUILD" -eq 1 ] || cargo_build_release
  BIN="${CARGO_TARGET_DIR:-$NATIVE_DIR/target}/release/$BINARY_NAME"
fi
[ -x "$BIN" ] || die "no existe el binario $BIN (compila antes o quita --skip-build)"
need objdump readelf awk sort

# glibc mínima que exige el binario: la más alta de sus símbolos versionados.
REQUIRED_GLIBC="$(objdump -T "$BIN" | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' | sed 's/GLIBC_//' | sort -Vu | tail -1)"
log "binario: $BIN ($(du -h "$BIN" | cut -f1)), glibc mínima $REQUIRED_GLIBC"
if [ -n "${MERGEON_MAX_GLIBC:-}" ]; then
  highest="$(printf '%s\n%s\n' "$REQUIRED_GLIBC" "$MERGEON_MAX_GLIBC" | sort -V | tail -1)"
  [ "$highest" = "$MERGEON_MAX_GLIBC" ] \
    || die "el binario exige glibc $REQUIRED_GLIBC (> $MERGEON_MAX_GLIBC): compila en una distribución más antigua"
fi

mkdir -p "$DIST_DIR" "$WORK_DIR"

# Archivos comunes al AppImage y al .deb, bajo un prefijo tipo /usr.
install_usr_tree() {
  local usr="$1"
  install -Dm755 "$BIN" "$usr/bin/$BINARY_NAME"
  install -Dm644 "$HERE/$APP_ID.desktop" "$usr/share/applications/$APP_ID.desktop"
  for size in "${ICON_SIZES[@]}"; do
    install -Dm644 "$ICONS_DIR/${size}x${size}.png" \
      "$usr/share/icons/hicolor/${size}x${size}/apps/$APP_ID.png"
  done
  mkdir -p "$usr/share/metainfo"
  sed -e "s/@VERSION@/$VERSION/" -e "s/@DATE@/$BUILD_DATE/" \
    "$HERE/$APP_ID.metainfo.xml.in" > "$usr/share/metainfo/$APP_ID.metainfo.xml"
  chmod 644 "$usr/share/metainfo/$APP_ID.metainfo.xml"
}

# --- AppImage ---------------------------------------------------------------

build_appimage() {
  local appdir="$WORK_DIR/AppDir"
  local out="$DIST_DIR/$PRODUCT_NAME-$VERSION.AppImage"
  [ "$DEB_ARCH" = "amd64" ] || out="$DIST_DIR/$PRODUCT_NAME-$VERSION-$DEB_ARCH.AppImage"

  rm -rf "$appdir"
  install_usr_tree "$appdir/usr"
  install -Dm755 "$HERE/AppRun" "$appdir/AppRun"
  # appimagetool toma el .desktop y el ícono de la raíz del AppDir.
  {
    cat "$HERE/$APP_ID.desktop"
    echo "X-AppImage-Version=$VERSION"
  } > "$appdir/$APP_ID.desktop"
  cp "$ICONS_DIR/512x512.png" "$appdir/$APP_ID.png"
  ln -s "$APP_ID.png" "$appdir/.DirIcon"
  if [ -n "${APP_UPDATE_FEED_URL:-}" ]; then
    mkdir -p "$appdir/usr/share/mergeon"
    printf '%s\n' "$APP_UPDATE_FEED_URL" > "$appdir/usr/share/mergeon/update-feed-url"
    log "AppImage con canal de actualizaciones: $APP_UPDATE_FEED_URL"
  fi

  local tool="$TOOLS_CACHE_DIR/appimagetool-$APPIMAGETOOL_VERSION-$MACHINE.AppImage"
  local runtime="$TOOLS_CACHE_DIR/appimage-runtime-$APPIMAGE_RUNTIME_VERSION-$MACHINE"
  fetch_verified \
    "https://github.com/AppImage/appimagetool/releases/download/$APPIMAGETOOL_VERSION/appimagetool-$MACHINE.AppImage" \
    "$tool" "${APPIMAGETOOL_SHA256[$MACHINE]}"
  fetch_verified \
    "https://github.com/AppImage/type2-runtime/releases/download/$APPIMAGE_RUNTIME_VERSION/runtime-$MACHINE" \
    "$runtime" "${APPIMAGE_RUNTIME_SHA256[$MACHINE]}"
  # appimagetool es a su vez un AppImage y los runners de CI y los contenedores
  # no tienen FUSE: se extrae una sola vez a la caché y se ejecuta su AppRun
  # (equivale a --appimage-extract-and-run, sin dejar restos en /tmp).
  local tool_dir="$TOOLS_CACHE_DIR/appimagetool-$APPIMAGETOOL_VERSION-$MACHINE"
  if [ ! -x "$tool_dir/AppRun" ]; then
    local scratch
    scratch="$(mktemp -d "$TOOLS_CACHE_DIR/extract.XXXXXX")"
    chmod +x "$tool"
    (cd "$scratch" && "$tool" --appimage-extract >/dev/null)
    rm -rf "$tool_dir"
    mv "$scratch/squashfs-root" "$tool_dir"
    rmdir "$scratch"
  fi

  # --runtime-file evita que descargue el runtime "continuous" en cada build.
  rm -f "$out"
  ARCH="$MACHINE" SOURCE_DATE_EPOCH="$EPOCH" "$tool_dir/AppRun" \
    --no-appstream --runtime-file "$runtime" "$appdir" "$out" >&2
  chmod +x "$out"
  log "AppImage: $out ($(du -h "$out" | cut -f1))"
}

# --- .deb -------------------------------------------------------------------

# Dependencias de las bibliotecas enlazadas (NEEDED), con la versión mínima
# que calcula dpkg-shlibdeps cuando está disponible.
shlib_depends() {
  local binary="$1" scratch
  if command -v dpkg-shlibdeps >/dev/null 2>&1; then
    scratch="$(mktemp -d)"
    mkdir -p "$scratch/debian"
    printf 'Source: mergeon\n\nPackage: mergeon\nArchitecture: any\n' > "$scratch/debian/control"
    if (cd "$scratch" && dpkg-shlibdeps -O "$binary" 2>/dev/null) > "$scratch/out"; then
      sed -n 's/^shlibs:Depends=//p' "$scratch/out"
      rm -rf "$scratch"
      return
    fi
    rm -rf "$scratch"
  fi
  # Sin dpkg-dev: los paquetes que aportan las NEEDED del binario.
  echo "libc6 (>= $REQUIRED_GLIBC), libgcc-s1, libxcb1, libxkbcommon0, libxkbcommon-x11-0"
}

build_deb() {
  need dpkg-deb gzip
  local root="$WORK_DIR/deb"
  local out="$DIST_DIR/${BINARY_NAME}_${VERSION}_${DEB_ARCH}.deb"
  local doc="$root/usr/share/doc/$BINARY_NAME"

  rm -rf "$root"
  install_usr_tree "$root/usr"
  install -Dm644 "$HERE/deb/copyright" "$doc/copyright"
  install -Dm644 "$HERE/deb/lintian-overrides" "$root/usr/share/lintian/overrides/$BINARY_NAME"
  install -Dm644 "$HERE/mergeon.1" "$root/usr/share/man/man1/$BINARY_NAME.1"
  gzip -9n "$root/usr/share/man/man1/$BINARY_NAME.1"
  mkdir -p "$doc"
  cat > "$doc/changelog" <<CHANGELOG
mergeon ($VERSION) stable; urgency=medium

  * Mergeon $VERSION. Release notes:
    https://github.com/Spilveru/mergeon/releases

 -- Spilveru <spilveru@users.noreply.github.com>  $(date -u -R -d "@$EPOCH")
CHANGELOG
  gzip -9n "$doc/changelog"
  chmod 644 "$doc/changelog.gz"

  local shlibs installed_size
  shlibs="$(shlib_depends "$root/usr/bin/$BINARY_NAME")"
  installed_size="$(du -sk --apparent-size "$root/usr" | cut -f1)"

  mkdir -p "$root/DEBIAN"
  # Depends: NEEDED del binario + lo que carga con dlopen y no puede faltar
  # (el cargador de Vulkan) + git, que la app ejecuta para todo.
  # Recommends: Wayland y EGL (también dlopen; sin ellos la app usa X11),
  # un controlador de Vulkan, el portal para los diálogos de archivos y
  # xdg-open. Suggests: un servicio de secretos para guardar la sesión de GitHub.
  cat > "$root/DEBIAN/control" <<CONTROL
Package: $BINARY_NAME
Version: $VERSION
Architecture: $DEB_ARCH
Maintainer: Spilveru <spilveru@users.noreply.github.com>
Installed-Size: $installed_size
Depends: $shlibs, libvulkan1, git
Recommends: libwayland-client0, libwayland-egl1, libegl1, mesa-vulkan-drivers, xdg-desktop-portal, xdg-utils
Suggests: gnome-keyring
Section: vcs
Priority: optional
Homepage: https://github.com/Spilveru/mergeon
Description: desktop Git client
 Mergeon is a graphical Git client: commit history with graph, branches,
 remotes, tags, stashes and submodules; staging by file or by hunk; merge,
 rebase, cherry-pick and conflict resolution; and GitHub sign-in to clone and
 open your repositories.
 .
 This package does not update itself: install a newer .deb to upgrade.
CONTROL
  install -m755 "$HERE/deb/preinst" "$root/DEBIAN/preinst"
  (cd "$root" && find usr -type f -print0 | LC_ALL=C sort -z | xargs -0 md5sum) > "$root/DEBIAN/md5sums"
  chmod 644 "$root/DEBIAN/control" "$root/DEBIAN/md5sums"
  find "$root" -type d -exec chmod 755 {} +
  find "$root" -exec touch -h -d "@$EPOCH" {} +

  rm -f "$out"
  # xz: lo entienden todos los dpkg (zstd no, antes de Debian 12).
  SOURCE_DATE_EPOCH="$EPOCH" dpkg-deb --root-owner-group -Zxz --build "$root" "$out" >&2
  log ".deb: $out ($(du -h "$out" | cut -f1))"
}

[ "$WANT_APPIMAGE" -eq 0 ] || build_appimage
[ "$WANT_DEB" -eq 0 ] || build_deb
log "listo: artefactos en $DIST_DIR"
