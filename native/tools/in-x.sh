#!/bin/bash
# Corre un comando dentro de un display virtual (Xvfb) con gestor de ventanas.
# GPUI necesita un gestor de ventanas para recibir frames; sin él se queda en el primero.
# Uso: native/tools/in-x.sh <comando> [args...]
exec xvfb-run -a -s "-screen 0 1600x1000x24" bash -c 'openbox >/dev/null 2>&1 & sleep 0.7; "$@"' _ "$@"
