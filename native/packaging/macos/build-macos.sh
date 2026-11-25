#!/usr/bin/env bash
# Construye el bundle y el zip de macOS de la versión nativa:
#   dist/Mergeon-<version>-arm64-mac.zip       (--arch arm64, Apple Silicon)
#   dist/Mergeon-<version>-mac.zip             (--arch x86_64, Intel)
#   dist/Mergeon-<version>-universal-mac.zip   (--arch universal, los dos)
# El zip lleva `Mergeon.app` en la raíz: es lo que espera el actualizador.
#
# Uso:
#   native/packaging/macos/build-macos.sh [--arch arm64|x86_64|universal]
#                                         [--skip-build] [--layout-only]
#
#   --arch         por defecto, la arquitectura de la máquina
#   --skip-build   usa los binarios ya compilados en target/<triple>/release
#   --layout-only  arma el bundle y el zip sin herramientas de Apple (sin lipo,
#                  codesign ni ditto; --bin <ruta> da el binario). Solo sirve
#                  para probar este script fuera de macOS: NO es publicable.
#
# Firma (mismas variables que usaba electron-builder):
#   sin variables                      firma ad-hoc (`codesign --sign -`): la app
#                                      abre tras "clic derecho > Abrir"
#   CSC_LINK + CSC_KEY_PASSWORD        certificado "Developer ID Application"
#                                      (.p12 en base64 o ruta a un archivo)
#   MACOS_SIGN_IDENTITY                identidad ya presente en el llavero
#   APPLE_ID + APPLE_APP_SPECIFIC_PASSWORD + APPLE_TEAM_ID
#                                      notariza y grapa el ticket (requiere
#                                      firma Developer ID)
#
# Otras variables: CARGO_BUILD_FLAGS, MACOSX_DEPLOYMENT_TARGET (10.15.7).
# Compatible con bash 3.2 (el de macOS).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source-path=SCRIPTDIR source=../common.sh
source "$HERE/../common.sh"

ARCH=""
SKIP_BUILD=0
LAYOUT_ONLY=0
LAYOUT_BIN=""
while [ $# -gt 0 ]; do
  case "$1" in
    --arch) ARCH="${2:?--arch necesita un valor}"; shift ;;
    --skip-build) SKIP_BUILD=1 ;;
    --layout-only) LAYOUT_ONLY=1 ;;
    --bin) LAYOUT_BIN="${2:?--bin necesita una ruta}"; shift ;;
    -h|--help) sed -n '2,29p' "$0"; exit 0 ;;
    *) die "argumento desconocido: $1" ;;
  esac
  shift
done

if [ "$LAYOUT_ONLY" -eq 0 ] && [ "$(uname -s)" != "Darwin" ]; then
  die "este script solo corre en macOS (o con --layout-only para probar la estructura)"
fi

if [ -z "$ARCH" ]; then
  case "$(uname -m)" in
    arm64|aarch64) ARCH="arm64" ;;
    *) ARCH="x86_64" ;;
  esac
fi
case "$ARCH" in
  arm64) TRIPLES=(aarch64-apple-darwin); SUFFIX="-arm64-mac" ;;
  x86_64) TRIPLES=(x86_64-apple-darwin); SUFFIX="-mac" ;;
  universal) TRIPLES=(aarch64-apple-darwin x86_64-apple-darwin); SUFFIX="-universal-mac" ;;
  *) die "arquitectura desconocida: $ARCH (arm64, x86_64 o universal)" ;;
esac

VERSION="$(mergeon_version)"
[ -n "$VERSION" ] || die "no se pudo leer la versión de $NATIVE_DIR/Cargo.toml"
# La mínima que soporta GPUI; también fija la que declara el binario.
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-10.15.7}"

TARGET_DIR="${CARGO_TARGET_DIR:-$NATIVE_DIR/target}"
STAGE="$WORK_DIR/macos"
APP="$STAGE/$PRODUCT_NAME.app"
ZIP="$DIST_DIR/$PRODUCT_NAME-$VERSION$SUFFIX.zip"
ICNS="$REPO_DIR/native/packaging/icons/icon.icns"
[ -f "$ICNS" ] || die "no existe $ICNS"

# --- binario ----------------------------------------------------------------

rm -rf "$STAGE"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$DIST_DIR"

if [ "$LAYOUT_ONLY" -eq 1 ]; then
  [ -n "$LAYOUT_BIN" ] || die "--layout-only necesita --bin <ruta>"
  cp "$LAYOUT_BIN" "$APP/Contents/MacOS/$BINARY_NAME"
else
  BUILT=()
  for triple in "${TRIPLES[@]}"; do
    if [ "$SKIP_BUILD" -eq 0 ]; then
      if command -v rustup >/dev/null 2>&1; then
        rustup target add "$triple" >&2
      fi
      cargo_build_release --target "$triple"
    fi
    binary="$TARGET_DIR/$triple/release/$BINARY_NAME"
    [ -x "$binary" ] || die "no existe $binary"
    BUILT+=("$binary")
  done
  if [ "$ARCH" = "universal" ]; then
    need lipo
    lipo -create -output "$APP/Contents/MacOS/$BINARY_NAME" "${BUILT[@]}"
    lipo -info "$APP/Contents/MacOS/$BINARY_NAME" >&2
  else
    cp "${BUILT[0]}" "$APP/Contents/MacOS/$BINARY_NAME"
  fi
fi
chmod 755 "$APP/Contents/MacOS/$BINARY_NAME"

# --- bundle -----------------------------------------------------------------

sed -e "s/@VERSION@/$VERSION/g" -e "s/@MIN_MACOS@/$MACOSX_DEPLOYMENT_TARGET/g" \
  "$HERE/Info.plist.in" > "$APP/Contents/Info.plist"
printf 'APPL????' > "$APP/Contents/PkgInfo"
cp "$ICNS" "$APP/Contents/Resources/icon.icns"

if [ "$LAYOUT_ONLY" -eq 1 ]; then
  need zip
  rm -f "$ZIP"
  (cd "$STAGE" && zip -q -r -y "$ZIP" "$PRODUCT_NAME.app")
  log "zip de prueba (sin firmar, NO publicable): $ZIP"
  exit 0
fi

need codesign ditto plutil
plutil -lint "$APP/Contents/Info.plist" >&2

# --- firma ------------------------------------------------------------------

KEYCHAIN=""
cleanup() {
  if [ -n "$KEYCHAIN" ]; then
    security delete-keychain "$KEYCHAIN" >/dev/null 2>&1 || true
  fi
  rm -f "$STAGE/certificate.p12" "$STAGE/notarize.zip"
}
trap cleanup EXIT

IDENTITY="${MACOS_SIGN_IDENTITY:-}"
if [ -z "$IDENTITY" ] && [ -n "${CSC_LINK:-}" ]; then
  # Certificado de CI: va a un llavero temporal que se borra al terminar.
  need security
  if [ -f "$CSC_LINK" ]; then
    cp "$CSC_LINK" "$STAGE/certificate.p12"
  else
    printf '%s' "$CSC_LINK" | base64 --decode > "$STAGE/certificate.p12"
  fi
  KEYCHAIN="$STAGE/signing.keychain-db"
  KEYCHAIN_PASSWORD="$(uuidgen)"
  security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
  security set-keychain-settings -lut 21600 "$KEYCHAIN"
  security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
  security import "$STAGE/certificate.p12" -k "$KEYCHAIN" -P "${CSC_KEY_PASSWORD:-}" \
    -T /usr/bin/codesign >/dev/null
  security set-key-partition-list -S apple-tool:,apple: -s -k "$KEYCHAIN_PASSWORD" "$KEYCHAIN" >/dev/null
  # El llavero temporal tiene que estar en la lista de búsqueda de codesign.
  # shellcheck disable=SC2046
  security list-keychains -d user -s "$KEYCHAIN" $(security list-keychains -d user | tr -d '"')
  IDENTITY="$(security find-identity -v -p codesigning "$KEYCHAIN" \
    | awk -F'"' '/Developer ID Application/ { print $2; exit }')"
  [ -n "$IDENTITY" ] || die "el certificado de CSC_LINK no es un 'Developer ID Application'"
fi

if [ -n "$IDENTITY" ]; then
  log "firmando con: $IDENTITY"
  codesign --force --timestamp --options runtime \
    --entitlements "$HERE/entitlements.plist" --sign "$IDENTITY" "$APP"
else
  # Sin certificado de Apple: firma ad-hoc. Un binario arm64 sin firma no
  # arranca; con ad-hoc abre tras "clic derecho > Abrir". Sin hardened runtime.
  log "firma ad-hoc (sin certificado Developer ID)"
  codesign --force --sign - "$APP"
fi
codesign --verify --deep --strict --verbose=2 "$APP" >&2

# --- notarización -----------------------------------------------------------

if [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_APP_SPECIFIC_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]; then
  [ -n "$IDENTITY" ] || die "la notarización necesita firma Developer ID (CSC_LINK o MACOS_SIGN_IDENTITY)"
  log "notarizando (puede tardar varios minutos)"
  ditto -c -k --keepParent "$APP" "$STAGE/notarize.zip"
  xcrun notarytool submit "$STAGE/notarize.zip" --wait \
    --apple-id "$APPLE_ID" --password "$APPLE_APP_SPECIFIC_PASSWORD" --team-id "$APPLE_TEAM_ID" >&2
  xcrun stapler staple "$APP" >&2
else
  log "sin notarizar (faltan APPLE_ID / APPLE_APP_SPECIFIC_PASSWORD / APPLE_TEAM_ID)"
fi

# --- zip --------------------------------------------------------------------

# ditto conserva permisos, enlaces y la firma; --keepParent deja Mergeon.app
# en la raíz del zip. Se crea después de grapar el ticket de notarización.
rm -f "$ZIP"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"
log "zip: $ZIP ($(du -h "$ZIP" | cut -f1))"
