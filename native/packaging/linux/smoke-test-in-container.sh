#!/usr/bin/env bash
# Corre dentro del contenedor de smoke-test.sh (como root, en una imagen
# Debian/Ubuntu limpia). Variables: DEB, APPIMAGE_FILE, LABEL, HOST_UID, HOST_GID.
set -uo pipefail
export DEBIAN_FRONTEND=noninteractive

# Segunda entrada, ya dentro del display virtual (ver launch): abre el comando
# $2, espera, cuenta sus ventanas y captura la pantalla en $3.
if [ "${1:-}" = "--in-display" ]; then
  openbox >/dev/null 2>&1 &
  sleep 1
  "$2" >/tmp/app.log 2>&1 &
  app=$!
  sleep 12
  xwininfo -root -tree 2>/dev/null | grep -c '"Mergeon"' > /tmp/windows
  scrot -o "$3"
  # Colores distintos en el interior de la ventana: 1 = no se dibujó nada.
  convert "$3" -crop 1300x800+150+100 +repage -format '%k' info: > /tmp/colors 2>/dev/null
  kill "$app" 2>/dev/null
  pkill -x mergeon 2>/dev/null
  exit 0
fi

FAILED=0
pass() { printf '  ok    %s\n' "$*"; }
fail() { printf '  FALLA %s\n' "$*"; FAILED=1; }

# shellcheck disable=SC1091
echo "== $(. /etc/os-release && echo "$PRETTY_NAME"), glibc $(ldd --version | head -1 | grep -oE '[0-9.]+$')"
apt-get update -qq
# Herramientas de la prueba (no son dependencias de la app).
apt-get install -y -qq --no-install-recommends \
  xvfb xauth openbox scrot x11-utils imagemagick file procps >/dev/null

if [ -f /electron.deb ]; then
  echo "== versión Electron instalada antes"
  apt-get install -y -qq /electron.deb >/dev/null 2>&1
  if [ -L /usr/bin/mergeon ]; then
    pass "/usr/bin/mergeon es el enlace de la versión Electron -> $(readlink -f /usr/bin/mergeon)"
  else
    fail "la versión Electron no dejó su enlace en /usr/bin/mergeon"
  fi
fi

if [ -f /electron.deb ]; then
  # En la prueba las dos versiones tienen el mismo número y apt no "actualiza"
  # a la misma versión: se reemplaza con dpkg y apt completa las dependencias.
  echo "== reemplazo de la versión Electron por el .deb nativo"
  dpkg -i "/dist/$DEB" >/tmp/apt.log 2>&1
  if apt-get install -f -y -qq >>/tmp/apt.log 2>&1; then
    pass "dpkg reemplazó el paquete y apt completó las dependencias"
  else
    fail "no se pudo reemplazar el paquete"
    tail -20 /tmp/apt.log
  fi
else
  echo "== instalación del .deb con apt"
  if apt-get install -y -qq "/dist/$DEB" >/tmp/apt.log 2>&1; then
    pass "apt instaló el paquete y sus dependencias"
  else
    fail "apt no pudo instalar el paquete"
    tail -20 /tmp/apt.log
  fi
fi
status="$(dpkg-query -W -f '${Status} ${Version}' mergeon 2>/dev/null)"
case "$status" in
  "install ok installed "*) pass "dpkg: $status" ;;
  *) fail "dpkg: $status" ;;
esac
if [ -f /usr/bin/mergeon ] && [ ! -L /usr/bin/mergeon ] && file /usr/bin/mergeon | grep -q ELF; then
  pass "/usr/bin/mergeon es el binario nativo"
else
  fail "/usr/bin/mergeon no es el binario nativo: $(ls -l /usr/bin/mergeon 2>&1)"
fi
missing="$(ldd /usr/bin/mergeon | grep 'not found')"
if [ -z "$missing" ]; then
  pass "todas las bibliotecas enlazadas se resuelven"
else
  fail "faltan bibliotecas: $missing"
fi
if [ -f /usr/share/applications/io.github.spilveru.Mergeon.desktop ]; then
  pass ".desktop instalado"
else
  fail "falta el .desktop"
fi
if [ -f /electron.deb ]; then
  if [ ! -e /opt/Mergeon ]; then pass "se retiró /opt/Mergeon"; else fail "queda /opt/Mergeon"; fi
  if [ ! -e /usr/share/applications/mergeon.desktop ]; then
    pass "se retiró el .desktop de Electron"
  else
    fail "queda mergeon.desktop"
  fi
  if update-alternatives --query mergeon >/dev/null 2>&1; then
    fail "queda la alternativa mergeon"
  else
    pass "se retiró la alternativa de /usr/bin/mergeon"
  fi
fi

# Abre el comando $1 y comprueba que aparece su ventana y que dibuja algo.
# $2 = captura, $3 = nombre para los mensajes.
launch() {
  rm -f /tmp/windows /tmp/colors
  xvfb-run -a -s "-screen 0 1600x1000x24" bash "$0" --in-display "$1" "$2"
  chown "$HOST_UID:$HOST_GID" "$2" 2>/dev/null
  if [ "$(cat /tmp/windows 2>/dev/null || echo 0)" -ge 1 ]; then
    pass "$3: ventana \"Mergeon\" abierta (captura: $(basename "$2"))"
  else
    fail "$3: no apareció la ventana"
    grep -v 'naga::\|blade\|descriptor' /tmp/app.log | tail -15
    return
  fi
  local colors
  colors="$(cat /tmp/colors 2>/dev/null || echo 0)"
  if [ "${colors:-0}" -gt 16 ]; then
    pass "$3: la ventana dibuja la interfaz ($colors colores)"
  else
    # Visto con el Vulkan por software (lavapipe) de Mesa 22/23: la ventana
    # abre pero queda en negro. Con Mesa 25 dibuja bien.
    fail "$3: la ventana está vacía ($colors colores; Mesa $(dpkg-query -W -f '${Version}' mesa-vulkan-drivers 2>/dev/null))"
  fi
}

echo "== la app instalada por el .deb abre una ventana"
launch /usr/bin/mergeon "/shots/deb-$LABEL.png" ".deb"

echo "== el AppImage abre una ventana (sin FUSE)"
cp "/dist/$APPIMAGE_FILE" /tmp/Mergeon.AppImage
chmod +x /tmp/Mergeon.AppImage
export APPIMAGE_EXTRACT_AND_RUN=1
launch /tmp/Mergeon.AppImage "/shots/appimage-$LABEL.png" "AppImage"

echo "== desinstalación"
if apt-get remove -y -qq mergeon >/dev/null 2>&1 && [ ! -e /usr/bin/mergeon ]; then
  pass "apt remove deja el sistema sin /usr/bin/mergeon"
else
  fail "la desinstalación dejó restos"
fi

echo
if [ "$FAILED" -eq 0 ]; then echo "RESULTADO: todo correcto"; else echo "RESULTADO: hay fallos"; fi
exit "$FAILED"
