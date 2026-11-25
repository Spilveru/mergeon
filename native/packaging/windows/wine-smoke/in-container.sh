#!/usr/bin/env bash
# Corre dentro del contenedor de run.sh, con un display virtual.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source-path=SCRIPTDIR source=../../common.sh
source "$HERE/../../common.sh"

export HOME=/tmp/home WINEPREFIX=/tmp/prefix WINEDEBUG=-all
# Sin los diálogos de instalación de Mono y Gecko del primer arranque.
export WINEDLLOVERRIDES="mscoree,mshtml="
mkdir -p "$HOME"

VERSION="$(mergeon_version)"
DIST=/tmp/dist
SETUP="$DIST/Mergeon-Setup-$VERSION.exe"
FAKE_APP=/usr/lib/x86_64-linux-gnu/wine/x86_64-windows/notepad.exe
GUID="4ee5e9c0-8f0d-5cf7-b3ba-58d6a51df30e"
UNINSTALL_KEY="HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\$GUID"

FAILED=0
pass() { printf '  ok    %s\n' "$*"; }
fail() { printf '  FALLA %s\n' "$*"; FAILED=1; }
check() { # check <descripción> <comando...>
  local description="$1"; shift
  if "$@" >/dev/null 2>&1; then pass "$description"; else fail "$description"; fi
}
# result <código> <mensaje si 0> [mensaje si no]
result() {
  if [ "$1" -eq 0 ]; then pass "$2"; else fail "${3:-$2}"; fi
}
# expect <valor> <patrón glob> <descripción>
expect() {
  # shellcheck disable=SC2053  # el patrón va sin comillas a propósito
  if [[ "$1" == $2 ]]; then pass "$3 = $1"; else fail "$3 = '$1' (se esperaba $2)"; fi
}
check_not() {
  local description="$1"; shift
  if "$@" >/dev/null 2>&1; then fail "$description"; else pass "$description"; fi
}
app_pids() { wine tasklist 2>/dev/null | tr -d '\r' | awk 'tolower($1) == "mergeon.exe" { print $2 }'; }
# shellcheck disable=SC2329  # se invoca a través de wait_until / check
app_running() { [ -n "$(app_pids)" ]; }
reg_value() { wine reg query "$1" /v "$2" 2>/dev/null | tr -d '\r' | awk -v name="$2" '$1 == name { $1 = ""; $2 = ""; sub(/^ +/, ""); print }'; }
wait_until() { # wait_until <segundos> <comando...>
  local seconds="$1"; shift
  for _ in $(seq "$((seconds * 2))"); do
    if "$@" >/dev/null 2>&1; then return 0; fi
    sleep 0.5
  done
  return 1
}

echo "== preparación"
timeout 180 wineboot -i >/dev/null 2>&1
timeout 60 wineserver -w
echo "  $(wine --version)"
MERGEON_DIST_DIR="$DIST" MAKENSIS_EXTRA_ARGS="-DTASKKILL_USER_FILTER=" \
  "$HERE/../build-windows.sh" --exe "$FAKE_APP" 2>&1 | sed 's/^/  /'
[ -f "$SETUP" ] || { echo "no se generó el instalador"; exit 1; }

USER_DIR="$WINEPREFIX/drive_c/users/$(id -un)"
INSTDIR="$USER_DIR/AppData/Local/Programs/mergeon"
START_MENU="$USER_DIR/AppData/Roaming/Microsoft/Windows/Start Menu/Programs/Mergeon.lnk"
DESKTOP="$USER_DIR/Desktop/Mergeon.lnk"

echo "== 1. instalación silenciosa (/S)"
timeout 120 wine "$SETUP" /S
result $? "el instalador termina con código 0"
check "mergeon.exe instalado en %LOCALAPPDATA%\\Programs\\mergeon" test -f "$INSTDIR/mergeon.exe"
check "el ejecutable instalado es el empaquetado" cmp -s "$INSTDIR/mergeon.exe" "$FAKE_APP"
check "desinstalador presente" test -f "$INSTDIR/Uninstall Mergeon.exe"
check "acceso directo en el menú Inicio" test -f "$START_MENU"
check "acceso directo en el escritorio" test -f "$DESKTOP"
expect "$(reg_value "$UNINSTALL_KEY" DisplayVersion)" "$VERSION" "registro: DisplayVersion"
expect "$(reg_value "$UNINSTALL_KEY" DisplayName)" "Mergeon $VERSION" "registro: DisplayName"
expect "$(reg_value "$UNINSTALL_KEY" QuietUninstallString)" '*Uninstall\ Mergeon.exe\"\ /currentuser\ /S' "registro: QuietUninstallString"
expect "$(reg_value "HKCU\\Software\\$GUID" InstallLocation)" '*Programs\\mergeon' "registro: InstallLocation"
sleep 2
check_not "silencioso sin --force-run: no abre la app" app_running

echo "== 2. actualización con la app abierta (--updated /S --force-run)"
rm -f "$DESKTOP"                      # el usuario quitó el acceso del escritorio
wine "$INSTDIR/mergeon.exe" >/dev/null 2>&1 &
wait_until 20 app_running
result $? "app en ejecución antes de actualizar"
old_pids="$(app_pids | sort | tr '\n' ' ')"
timeout 180 wine "$SETUP" --updated /S --force-run
result $? "el instalador termina con código 0"
wait_until 20 app_running
result $? "--force-run: la app vuelve a abrirse"
new_pids="$(app_pids | sort | tr '\n' ' ')"
if [ "$new_pids" != "$old_pids" ] && [ "$(echo "$new_pids" | wc -w)" -eq 1 ]; then
  pass "la instancia anterior se cerró y hay una sola nueva (pid antes: $old_pids ahora: $new_pids)"
else
  fail "instancias tras actualizar: antes '$old_pids', ahora '$new_pids'"
fi
check_not "--updated no recrea el acceso del escritorio borrado" test -f "$DESKTOP"
check "--updated conserva el acceso del menú Inicio" test -f "$START_MENU"
wine taskkill /f /im mergeon.exe >/dev/null 2>&1
wait_until 10 bash -c '! wine tasklist 2>/dev/null | grep -qi mergeon.exe'

echo "== 3. instalación encima de la versión Electron"
mkdir -p "$INSTDIR/resources" "$INSTDIR/locales"
touch "$INSTDIR/resources/app.asar" "$INSTDIR/resources/app-update.yml" "$INSTDIR/locales/en-US.pak" \
  "$INSTDIR/ffmpeg.dll" "$INSTDIR/icudtl.dat" "$INSTDIR/resources.pak" "$INSTDIR/LICENSES.chromium.html" \
  "$INSTDIR/notas-del-usuario.txt"
timeout 120 wine "$SETUP" /S
result $? "el instalador termina con código 0"
check_not "se quitan resources\\ de Electron" test -e "$INSTDIR/resources"
check_not "se quitan locales\\ de Electron" test -e "$INSTDIR/locales"
check_not "se quitan las DLL y .pak de Electron" bash -c "ls '$INSTDIR' | grep -Eq '\\.(dll|pak|dat|html)$'"
check "no se toca un archivo ajeno de la carpeta" test -f "$INSTDIR/notas-del-usuario.txt"
check "instalación normal: recrea el acceso del escritorio" test -f "$DESKTOP"

echo "== 4. desinstalación silenciosa"
rm -f "$INSTDIR/notas-del-usuario.txt"
timeout 120 wine "$INSTDIR/Uninstall Mergeon.exe" /S
# El desinstalador se copia a %TEMP% y sigue desde ahí: hay que esperarlo.
wait_until 30 bash -c "! test -e '$INSTDIR'"
result $? "carpeta de instalación eliminada" "queda $INSTDIR: $(find "$INSTDIR" -mindepth 1 2>/dev/null | tr '\n' ' ')"
check_not "acceso directo del menú Inicio eliminado" test -e "$START_MENU"
check_not "acceso directo del escritorio eliminado" test -e "$DESKTOP"
check_not "clave de desinstalación eliminada" wine reg query "$UNINSTALL_KEY"
check_not "clave de instalación eliminada" wine reg query "HKCU\\Software\\$GUID"

wineserver -k >/dev/null 2>&1
echo
if [ "$FAILED" -eq 0 ]; then echo "RESULTADO: todo correcto"; else echo "RESULTADO: hay fallos"; fi
exit "$FAILED"
