# Empaquetado y CI de la versión nativa

Cómo se construyen, publican y actualizan los instaladores de la versión nativa
(Rust + GPUI). Todo vive en `native/packaging/` y no depende de Node ni de
electron-builder. La app Electron y sus workflows (`ci.yml`, `release.yml`,
`electron-builder.config.cjs`) no se tocan: se siguen publicando igual.

## Qué se publica

Los mismos artefactos y nombres que publica hoy electron-builder, para que el
actualizador (el nativo y el de Electron) los encuentre:

| Plataforma | Artefacto                                                        | Manifiesto         | Se actualiza sola                    |
| ---------- | ---------------------------------------------------------------- | ------------------ | ------------------------------------ |
| Linux      | `Mergeon-<v>.AppImage`                                           | `latest-linux.yml` | sí (variable `APPIMAGE`)             |
| Linux      | `mergeon_<v>_amd64.deb`                                          | `latest-linux.yml` | no (solo avisa)                      |
| Windows    | `Mergeon-Setup-<v>.exe` (NSIS, por usuario)                      | `latest.yml`       | sí                                   |
| macOS      | `Mergeon-<v>-arm64-mac.zip` (o `-mac.zip` / `-universal-mac.zip`) | `latest-mac.yml`   | solo avisa (decisión del actualizador) |

`<v>` es la versión de `[workspace.package]` en `native/Cargo.toml`: la que el
binario lleva dentro (`CARGO_PKG_VERSION`) y la que compara el actualizador.
Los artefactos quedan en `native/dist/`.

Tamaños medidos (0.9.0, cáscara aún sin todas las vistas): AppImage 20,8 MB y
`.deb` 15,1 MB, contra 146,7 MB y 114,5 MB de la versión Electron.

## Estructura

```
native/packaging/
  common.sh                 versión, rutas, cargo build, descargas verificadas
  update-manifest.py        genera latest*.yml (Python estándar)
  manifest-check/           crate: comprueba latest*.yml con el parser del actualizador
  icons/                    íconos derivados de build/icon.png (make-icons.sh)
  linux/   build-linux.sh, build-in-container.sh, Dockerfile, AppRun, .desktop,
           metainfo, deb/{preinst,copyright,lintian-overrides}, smoke-test.sh
  windows/ mergeon.nsi, build-windows.sh, wine-smoke/ (prueba bajo wine)
  macos/   build-macos.sh, Info.plist.in, entitlements.plist
native/crates/mergeon/build.rs   recursos de Windows dentro del .exe (ver abajo)
.github/workflows/ci.yml, release.yml
```

## Linux

```bash
# En la máquina (rápido; el binario exigirá la glibc de esta máquina):
native/packaging/linux/build-linux.sh                 # compila y empaqueta
native/packaging/linux/build-linux.sh --skip-build    # reutiliza target/release
# Para publicar (lo que usa el CI): dentro de Ubuntu 22.04, glibc 2.35
native/packaging/linux/build-in-container.sh
# Prueba en una distribución limpia (instala el .deb, abre la app, captura):
native/packaging/linux/smoke-test.sh ubuntu:24.04
```

En esta máquina compartida: `CARGO_BUILD_FLAGS="-j 2" MERGEON_NICE=15`.

- **glibc**: un binario exige la glibc de donde se compila. Compilado en el
  runner (Ubuntu 26.04) pide `GLIBC_2.43` y no arranca en casi ninguna
  distribución actual; por eso el release compila en el contenedor de
  `linux/Dockerfile` (Ubuntu 22.04 → glibc 2.35: Ubuntu 22.04+, Debian 12+).
  `MERGEON_MAX_GLIBC` hace fallar el build si se supera.
- **AppImage**: `AppRun` + `io.github.spilveru.Mergeon.desktop` + ícono + binario. No lleva
  bibliotecas dentro: xcb, xkbcommon, Vulkan y Wayland son las del sistema (van
  ligadas al servidor gráfico y a los controladores). Si falta alguna, `AppRun`
  lo dice en un diálogo en vez de morir en silencio. `appimagetool` (1.9.1) y el
  runtime (20251108) se descargan una vez a `~/.cache/mergeon-packaging` con
  sha256 fijado; `appimagetool` se extrae y se ejecuta sin FUSE.
- **`.deb`**: binario en `/usr/bin/mergeon`, `.desktop`, íconos hicolor,
  metainfo AppStream, página de manual. `Depends` = bibliotecas enlazadas
  (calculadas con `dpkg-shlibdeps`) + `libvulkan1` (se carga con `dlopen`) +
  `git`. `Recommends` = Wayland/EGL (también `dlopen`), `mesa-vulkan-drivers`,
  `xdg-desktop-portal`, `xdg-utils`. `Suggests` = `gnome-keyring`.
  El paquete se llama `mergeon`, igual que el de Electron: lo reemplaza. El
  `preinst` retira la alternativa `/usr/bin/mergeon -> /opt/Mergeon/mergeon`
  que dejaba aquel.
- El `.desktop` se llama como el `app_id` de la ventana (`io.github.spilveru.Mergeon`) y
  declara `StartupWMClass=io.github.spilveru.Mergeon`.

## Windows

```bash
# Git Bash, con Rust (toolchain MSVC) y NSIS 3 instalados:
native/packaging/windows/build-windows.sh
# Desde Linux, con un mergeon.exe ya compilado (o para validar el .nsi):
native/packaging/windows/build-windows.sh --exe ruta/mergeon.exe
# Prueba de la lógica del instalador bajo wine, en un contenedor:
native/packaging/windows/wine-smoke/run.sh
```

`mergeon.nsi` (NSIS 3, sin plugins externos):

- Instala por usuario, sin UAC, en `%LOCALAPPDATA%\Programs\mergeon`, con
  accesos directos en el menú Inicio y el escritorio, desinstalador y entrada
  en "Aplicaciones instaladas".
- Registra la aplicación con el GUID `88c73d68-b9b5-584f-bd28-a064bf3d6b1d` (UUID v5 de
  `io.github.spilveru.Mergeon`), borrando archivos de Electron huérfanos si existieran (solo nombres
  conocidos y solo si existe `resources\app.asar`).
- Argumentos: `/S` (silencioso), `--updated` (lo lanzó la app: espera hasta 15 s
  a que salga y no recrea accesos directos borrados) y `--force-run` (abre la
  app al terminar). Es lo que envía `NsisInstaller` del actualizador:
  `--updated /S --force-run`. La app se relanza sin argumentos.
- Antes de sobrescribir comprueba que `mergeon.exe` no está en uso (intenta
  abrirlo para escritura); si lo está, pide cerrar, luego `taskkill` y por
  último `taskkill /f`, solo sobre los procesos del usuario actual.

`native/crates/mergeon/build.rs` (con la dependencia de build `winresource`)
incrusta en `mergeon.exe` el ícono (`packaging/icons/mergeon.ico`) y los
metadatos de versión y, en release, marca el subsistema GUI para que no se abra
una consola (equivale a `#![windows_subsystem = "windows"]`, sin tocar
`main.rs`). En Linux y macOS no hace nada. GPUI ya incrusta el manifiesto de la
aplicación; `build.rs` no añade otro.

## macOS

```bash
native/packaging/macos/build-macos.sh                    # arquitectura de la máquina
native/packaging/macos/build-macos.sh --arch universal   # arm64 + x86_64 (lipo)
```

Arma `Mergeon.app` (`Info.plist`, `packaging/icons/icon.icns`, binario), firma y crea el
zip con `ditto --keepParent`, que deja `Mergeon.app` en la raíz del zip.

- **Arquitectura**: por defecto una sola (arm64 en `macos-latest`), como publica
  publicaba Electron. `universal` duplica el tiempo de compilación en el runner
  más caro; se elige con la variable `MAC_ARCH`. Los nombres siguen la regla
  del actualizador: los zip con `arm64` solo los toma Apple Silicon.
- **Firma**: sin secretos, ad-hoc (`codesign --sign -`). Con `CSC_LINK` (`.p12`
  en base64 o ruta) y `CSC_KEY_PASSWORD`, Developer ID con hardened runtime.
  Con `APPLE_ID`, `APPLE_APP_SPECIFIC_PASSWORD` y `APPLE_TEAM_ID`, además
  notariza (`notarytool`) y grapa el ticket antes de crear el zip.

## Manifiestos de actualización

```bash
native/packaging/update-manifest.py --platform linux --version 0.9.1 --dir native/dist
cargo run -p mergeon-manifest-check -- native/dist/latest-linux.yml --platform linux --version 0.9.1
```

`update-manifest.py` escribe el formato de electron-builder (`version`,
`files[url, sha512 base64, size]`, `path`, `sha512`, `releaseDate`). En Linux
el AppImage va primero y el `.deb` después; `arm64` va a
`latest-linux-arm64.yml`. `mergeon-manifest-check` lee el resultado con
`mergeon_updater::parse_update_manifest`, elige el artefacto como lo haría la
app (`select_artifact`) y verifica tamaño y sha512 contra los archivos. Sus
tests (`cargo test -p mergeon-manifest-check`) ejecutan el generador; el
release lo corre antes de subir nada.

## CI

`ci.yml` ("Quality"), en cada push y PR (salvo cambios solo en `.md`):

- `linux`: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets
  --locked -- -D warnings` y `tools/in-x.sh cargo test --workspace --locked`
  (Xvfb + openbox). Corre en `ubuntu-latest` por defecto (configurable vía
  `CI_RUNNER`; los PRs de forks externos corren siempre en `ubuntu-latest`).
- `cross`: `cargo build -p mergeon --locked` en `macos-latest` y tests en
  `windows-latest`, solo en pull requests y a mano; `CI_CROSS=always` lo corre
  también en cada push.

`release.yml` ("Release"), con tags `vX.Y.Z` o a mano: `prepare` (crea el
borrador) → `linux`, `windows`, `macos` (construyen, generan, firman y
verifican su manifiesto, suben al borrador) → `publish` (comprueba los
artefactos y publica como "latest"). El tag tiene que coincidir con la versión
de `native/Cargo.toml`.

| Variable de repositorio     | Efecto                                                                                                      |
| --------------------------- | ----------------------------------------------------------------------------------------------------------- |
| `CI_RUNNER`                 | runner de los jobs Linux: `ubuntu-latest` por defecto; `self-hosted` u otra etiqueta de runner personalizada |
| `CI_CROSS=always`           | builds de comprobación en Windows y macOS también en cada push                                              |
| `RELEASE_WINDOWS=false`     | el release no construye Windows                                                                             |
| `RELEASE_MAC=true`          | el release construye macOS                                                                                  |
| `MAC_ARCH`                  | `arm64` (por defecto), `x86_64` o `universal`                                                               |
| `RELEASE_PRERELEASE=true`   | publica como prerelease en vez de "latest" (las apps instaladas no se actualizan)                           |
| `CARGO_JOBS`                | límite de procesos de compilación                                                                           |
| `APP_UPDATE_FEED_URL`       | canal HTTPS de actualizaciones fijado al compilar                                                           |
| `MERGEON_UPDATE_PUBLIC_KEY` | clave pública Ed25519 (32 bytes en base64) fijada al compilar para verificar `latest*.yml.sig`              |

Secretos:
- `MERGEON_UPDATE_SIGNING_KEY`: clave privada Ed25519 (PKCS#8 en base64) para
  firmar los manifiestos `latest*.yml` (`cargo run -p mergeon-manifest-check -- keygen`).
- Opcionales (solo macOS): `CSC_LINK`, `CSC_KEY_PASSWORD`, `APPLE_ID`,
  `APPLE_APP_SPECIFIC_PASSWORD`, `APPLE_TEAM_ID`.

Los runners efímeros (como `ubuntu-latest`) instalan en el propio job los
paquetes que falten y cachean la compilación con `actions/cache`. Los secretos
de firma de macOS se usan únicamente en `macos-latest`.

En el runner propio hacen falta `docker` (el usuario del runner en el grupo
`docker`), `gh`, `python3`, Rust y los paquetes `-dev` que lista el paso "Check
runner prerequisites". La compilación se guarda en `~/.cache/mergeon-ci/` (se
puede borrar).

**"Latest" por defecto.** La app instalada se actualiza desde
`releases/latest`, así que cada release se publica como "latest". Las
instalaciones de la app Electron (v0.9.0 y anteriores) también leen ese canal:
la de Windows y el AppImage intentarían actualizarse al instalador nativo, algo
que no se ha probado; lo seguro es reinstalarlas a mano.

## Enlace con el auto-update

- El actualizador pide `<feed>/latest*.yml`, elige el artefacto y lo descarga
  de la misma carpeta: publicar una versión es dejar en el feed los artefactos y
  su manifiesto (los manifiestos, al final).
- El feed es el `APP_UPDATE_FEED_URL` del entorno al ejecutar o, si no está, el
  que se fijó al compilar (`option_env!`, ver abajo), así que vale para todos
  los formatos si se compiló con la variable. El repositorio es privado y el
  feed de GitHub queda desactivado en `UpdaterConfig::from_env`. El AppImage
  además la exporta desde `AppRun` si se empaquetó con `APP_UPDATE_FEED_URL`.

## Verificado

En esta máquina (Ubuntu 26.04, sin GPU), con el binario de la cáscara actual:

- AppImage y `.deb` construidos de verdad en el contenedor (glibc mínima 2.35).
- El AppImage arranca en el display virtual, con FUSE y con
  `APPIMAGE_EXTRACT_AND_RUN=1`; `APPIMAGE` llega al proceso y la ventana tiene
  `WM_CLASS` `com.mergeon.app`. Capturas en `native/target-shots/packaging/`.
- Actualización de punta a punta: el AppImage 0.9.0 contra un feed local con un
  `latest-linux.yml` 0.9.1 generado por `update-manifest.py` pidió el
  manifiesto, eligió el AppImage, lo descargó y validó su sha512.
- `.deb`: `dpkg-deb --info/--contents` correctos; `lintian --pedantic` sin
  errores ni avisos (un `embedded-library freetype` con override justificado).
- `smoke-test.sh ubuntu:24.04`: apt instala el `.deb` con sus dependencias, la
  app abre y dibuja, el AppImage también, y el `.deb` reemplaza limpiamente al
  de Electron 0.9.0 real (`/opt/Mergeon`, alternativa y `.desktop` retirados).
- `mergeon.nsi` compila con `makensis -WX` (3.10). `wine-smoke/run.sh` pasó
  todas sus comprobaciones con un ejecutable de relleno.
- Manifiestos: 8 tests de `mergeon-manifest-check`; el formato coincide con los
  `latest*.yml` del release Electron 0.9.0.
- Workflows validados con `actionlint` 1.7.12; scripts con `shellcheck`.

## Pendiente y no verificado

**Windows (nada probado en Windows real):**

- Compilar `mergeon.exe` (MSVC, `fxc.exe`) y que `build.rs` incruste el ícono y
  los metadatos; que no se abra consola.
- El instalador con el binario real: la prueba bajo wine usa `notepad.exe` como
  relleno y sin el filtro `/fi "USERNAME eq ..."` de `taskkill`, que wine no
  admite. Falta probar la actualización desde la app y la instalación encima
  de la versión Electron en un Windows de verdad.
- `build-windows.sh` en Git Bash (conversión de rutas con `cygpath`, opciones
  `/D` de `makensis`). Solo se ejecutó en Linux.
- Sin firma de código: SmartScreen avisará, igual que hoy.

**macOS (nada probado en un Mac):**

- `build-macos.sh` solo se ejecutó con `--layout-only` en Linux (estructura del
  bundle, `Info.plist` válido, `Mergeon.app` en la raíz del zip). No se
  probaron `lipo`, `codesign`, el llavero temporal, `notarytool`, `ditto` ni
  la compatibilidad con bash 3.2.
- Que la app firmada ad-hoc abra, y el paso "Ensure Metal toolchain".

**Linux:**

- Con el Vulkan por software de Mesa 22/23 (contenedores de Ubuntu 22.04 y
  Debian 12) la app instala y abre la ventana pero queda **en negro**; con Mesa
  25 (Ubuntu 24.04, Debian 13) dibuja bien. No se pudo probar con una GPU real
  en esas distribuciones: es de GPUI/wgpu, no del paquete, pero decide si
  22.04 y Debian 12 son realmente soportables.
- Wayland: todo se probó sobre X11 (Xvfb).
- arm64: los scripts lo contemplan pero no se construyó.
- Instalar la actualización descargada (reemplazo del AppImage y relanzado)
  desde la interfaz: se verificó hasta "descargada y verificada".

**CI:**

- `ci.yml` exige que fmt, clippy y los tests de todo el workspace pasen.
- `windows-latest` y `macos-latest` son de pago en un repositorio privado y la
  facturación de Actions estaba bloqueada: si sigue así, esos jobs no arrancan
  (hay variables para apagarlos).
- El runner propio necesita `docker`.

**Para los dueños de los crates:**

- `mergeon-updater`: hecho. `UpdaterConfig::from_env` / `for_current_process`
  usan `option_env!("APP_UPDATE_FEED_URL")` (`BUILT_IN_UPDATE_FEED_URL`) cuando
  la variable no está (o está en blanco) al ejecutar. Los workflows ya la pasan
  al compilar, así que el `.exe`, el `.deb` y el `.app` también llevan canal.
- `crates/mergeon`: `build.rs` y la dependencia `winresource` son nuevos;
  `main.rs` trata cualquier argumento como ruta de repositorio, por eso el
  instalador relanza la app sin argumentos.
- Cuando la versión nativa reemplace a la Electron: pasar a tags `vX.Y.Z` y a
  "latest", y decidir si Windows se construye en `windows-latest` o con
  compilación cruzada desde Linux (no explorada).
