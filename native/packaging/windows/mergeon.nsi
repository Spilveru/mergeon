; Instalador de Mergeon (versión nativa) para Windows.
;
; NSIS 3.x, solo con lo que trae la instalación estándar (MUI2, FileFunc,
; LogicLib, nsExec, System): no hay plugins que descargar.
;
; Lo compila native/packaging/windows/build-windows.sh, que pasa:
;   VERSION          versión del producto (la del binario), p. ej. 0.9.1
;   VERSION_NUMERIC  la misma en cuatro números, p. ej. 0.9.1.0
;   APP_EXE          ruta de mergeon.exe ya compilado
;   ICON_FILE        ícono .ico
;   OUT_FILE         instalador a generar (Mergeon-Setup-<versión>.exe)
;
; Contrato con el actualizador (native/crates/mergeon-updater, NsisInstaller),
; el mismo que tenía el instalador de electron-builder:
;   /S            instalación silenciosa (lo interpreta NSIS)
;   --updated     lo lanzó la app para actualizarse: la app se está cerrando,
;                 se espera a que salga y no se tocan los accesos directos
;                 que el usuario haya borrado
;   --force-run   abrir la app al terminar, aunque sea silenciosa
;
; Instala por usuario (sin UAC) en %LOCALAPPDATA%\Programs\mergeon, que es
; donde instalaba la versión Electron, y usa sus mismas claves de registro:
; para Windows es la misma aplicación, así que actualiza encima de ella.

Unicode true
ManifestDPIAware true
RequestExecutionLevel user
SetCompressor /SOLID lzma

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

!ifndef VERSION
  !error "Falta /DVERSION=x.y.z"
!endif
!ifndef VERSION_NUMERIC
  !error "Falta /DVERSION_NUMERIC=x.y.z.0"
!endif
!ifndef APP_EXE
  !error "Falta /DAPP_EXE=<ruta de mergeon.exe>"
!endif
!ifndef ICON_FILE
  !define ICON_FILE "..\icons\mergeon.ico"
!endif
!ifndef OUT_FILE
  !define OUT_FILE "Mergeon-Setup-${VERSION}.exe"
!endif

!define PRODUCT_NAME "Mergeon"
!define PUBLISHER "Spilveru"
!define APP_EXE_NAME "mergeon.exe"
!define UNINSTALLER_NAME "Uninstall ${PRODUCT_NAME}.exe"

; UUID v5 de "io.github.spilveru.Mergeon" en el espacio de nombres de
; electron-builder (50e065bc-3134-11e6-9bab-38c9862bdaf3).
!define APP_GUID "88c73d68-b9b5-584f-bd28-a064bf3d6b1d"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_GUID}"
!define INSTALL_KEY "Software\${APP_GUID}"

; taskkill solo sobre los procesos del usuario actual: en un equipo compartido
; no hay que cerrar el Mergeon de otra sesión. (La prueba bajo wine lo deja
; vacío: su taskkill no admite /fi.)
!ifndef TASKKILL_USER_FILTER
  !define TASKKILL_USER_FILTER '/fi "USERNAME eq %USERNAME%"'
!endif

; Esperas, en pasos de 500 ms.
!define EXIT_WAIT_STEPS 30   ; hasta 15 s para que la app salga sola
!define CLOSE_WAIT_STEPS 10  ; hasta 5 s tras pedirle que cierre

Name "${PRODUCT_NAME}"
OutFile "${OUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\mergeon"
; Una instalación anterior (Electron o nativa) manda sobre la ruta por defecto.
InstallDirRegKey HKCU "${INSTALL_KEY}" "InstallLocation"
BrandingText "${PRODUCT_NAME} ${VERSION}"
ShowInstDetails nevershow
ShowUninstDetails nevershow
AutoCloseWindow true

VIProductVersion "${VERSION_NUMERIC}"
VIFileVersion "${VERSION_NUMERIC}"
VIAddVersionKey "ProductName" "${PRODUCT_NAME}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "${PRODUCT_NAME} Setup"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "LegalCopyright" "Copyright (c) ${PUBLISHER}"
VIAddVersionKey "OriginalFilename" "Mergeon-Setup-${VERSION}.exe"

!define MUI_ICON "${ICON_FILE}"
!define MUI_UNICON "${ICON_FILE}"

; Instalador de un clic: solo la página de progreso.
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "Spanish"

LangString AppRunning ${LANG_ENGLISH} "${PRODUCT_NAME} is running. Click OK to close it and continue."
LangString AppRunning ${LANG_SPANISH} "${PRODUCT_NAME} está abierto. Pulsa Aceptar para cerrarlo y continuar."
LangString AppCannotBeClosed ${LANG_ENGLISH} "${PRODUCT_NAME} could not be closed. Close it manually and click Retry."
LangString AppCannotBeClosed ${LANG_SPANISH} "No se pudo cerrar ${PRODUCT_NAME}. Ciérralo a mano y pulsa Reintentar."
LangString InstallerRunning ${LANG_ENGLISH} "The ${PRODUCT_NAME} installer is already running."
LangString InstallerRunning ${LANG_SPANISH} "El instalador de ${PRODUCT_NAME} ya se está ejecutando."

Var IsUpdate
Var ForceRun

; --- argumentos ---------------------------------------------------------------

!macro ParseArguments
  StrCpy $IsUpdate 0
  StrCpy $ForceRun 0
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "--updated" $R1
  ${IfNot} ${Errors}
    StrCpy $IsUpdate 1
  ${EndIf}
  ClearErrors
  ${GetOptions} $R0 "--force-run" $R1
  ${IfNot} ${Errors}
    StrCpy $ForceRun 1
  ${EndIf}
  ClearErrors
!macroend

; --- cierre de la app en ejecución -------------------------------------------
;
; Windows no deja sobrescribir un .exe en uso. La señal fiable de que la app
; sigue abierta es esa misma: no se puede abrir su ejecutable para escritura.
; Las funciones se generan dos veces (instalador y desinstalador, prefijo "un.").

!macro DefineCloseRunningApp un
  ; Deja en $R0 un 1 si el ejecutable instalado está en uso, 0 si no.
  Function ${un}IsAppLocked
    StrCpy $R0 0
    IfFileExists "$INSTDIR\${APP_EXE_NAME}" 0 done
    ClearErrors
    FileOpen $R1 "$INSTDIR\${APP_EXE_NAME}" a
    IfErrors locked
    FileClose $R1
    Goto done
    locked:
      StrCpy $R0 1
      ClearErrors
    done:
  FunctionEnd

  ; Espera hasta $R2 pasos de 500 ms a que el ejecutable quede libre.
  ; Deja en $R0 el resultado de IsAppLocked.
  Function ${un}WaitForAppExit
    ${Do}
      Call ${un}IsAppLocked
      ${If} $R0 == 0
        ${ExitDo}
      ${EndIf}
      ${If} $R2 <= 0
        ${ExitDo}
      ${EndIf}
      Sleep 500
      IntOp $R2 $R2 - 1
    ${Loop}
  FunctionEnd

  Function ${un}CloseRunningApp
    Push $R0
    Push $R1
    Push $R2

    ; Actualización: la app lanzó el instalador y está saliendo por su cuenta.
    StrCpy $R2 0
    ${If} $IsUpdate == 1
      StrCpy $R2 ${EXIT_WAIT_STEPS}
    ${EndIf}
    Call ${un}WaitForAppExit
    ${If} $R0 == 0
      Goto closed
    ${EndIf}

    ${If} $IsUpdate != 1
      MessageBox MB_OKCANCEL|MB_ICONEXCLAMATION "$(AppRunning)" /SD IDOK IDOK +2
      Quit
    ${EndIf}

    retry:
      DetailPrint "Closing ${PRODUCT_NAME}..."
      ; Sin /f: taskkill pide a la ventana que cierre (WM_CLOSE).
      nsExec::Exec '"$SYSDIR\cmd.exe" /c taskkill /im "${APP_EXE_NAME}" ${TASKKILL_USER_FILTER}'
      Pop $R1
      StrCpy $R2 ${CLOSE_WAIT_STEPS}
      Call ${un}WaitForAppExit
      ${If} $R0 == 0
        Goto closed
      ${EndIf}

      nsExec::Exec '"$SYSDIR\cmd.exe" /c taskkill /f /im "${APP_EXE_NAME}" ${TASKKILL_USER_FILTER}'
      Pop $R1
      StrCpy $R2 ${CLOSE_WAIT_STEPS}
      Call ${un}WaitForAppExit
      ${If} $R0 == 0
        Goto closed
      ${EndIf}

      ; Sigue en uso (otro usuario, permisos elevados, antivirus...).
      MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$(AppCannotBeClosed)" /SD IDCANCEL IDRETRY retry
      SetErrorLevel 2
      Quit

    closed:
    Pop $R2
    Pop $R1
    Pop $R0
  FunctionEnd
!macroend

!insertmacro DefineCloseRunningApp ""
!insertmacro DefineCloseRunningApp "un."

; --- instalación --------------------------------------------------------------

Function .onInit
  SetShellVarContext current
  !insertmacro ParseArguments

  ; Una sola instancia del instalador a la vez.
  System::Call 'kernel32::CreateMutex(p 0, i 0, t "${APP_GUID}-setup") p .r1 ?e'
  Pop $R0
  ${If} $R0 = 183 ; ERROR_ALREADY_EXISTS
    MessageBox MB_OK|MB_ICONEXCLAMATION "$(InstallerRunning)" /SD IDOK
    Abort
  ${EndIf}
FunctionEnd

; Archivos de la versión Electron que quedarían huérfanos al instalar encima.
; Solo se tocan si la carpeta es una instalación de Electron (resources\app.asar)
; y solo nombres conocidos: nunca se borra la carpeta entera.
Function RemoveElectronFiles
  IfFileExists "$INSTDIR\resources\app.asar" 0 done
    DetailPrint "Removing the previous (Electron) version..."
    RMDir /r "$INSTDIR\resources"
    RMDir /r "$INSTDIR\locales"
    Delete "$INSTDIR\chrome_100_percent.pak"
    Delete "$INSTDIR\chrome_200_percent.pak"
    Delete "$INSTDIR\resources.pak"
    Delete "$INSTDIR\icudtl.dat"
    Delete "$INSTDIR\snapshot_blob.bin"
    Delete "$INSTDIR\v8_context_snapshot.bin"
    Delete "$INSTDIR\d3dcompiler_47.dll"
    Delete "$INSTDIR\dxcompiler.dll"
    Delete "$INSTDIR\dxil.dll"
    Delete "$INSTDIR\ffmpeg.dll"
    Delete "$INSTDIR\libEGL.dll"
    Delete "$INSTDIR\libGLESv2.dll"
    Delete "$INSTDIR\vk_swiftshader.dll"
    Delete "$INSTDIR\vk_swiftshader_icd.json"
    Delete "$INSTDIR\vulkan-1.dll"
    Delete "$INSTDIR\LICENSE.electron.txt"
    Delete "$INSTDIR\LICENSES.chromium.html"
  done:
FunctionEnd

Section "${PRODUCT_NAME}" SecMain
  SectionIn RO
  SetShellVarContext current

  Call CloseRunningApp
  Call RemoveElectronFiles

  SetOutPath "$INSTDIR"
  SetOverwrite on
  File "/oname=${APP_EXE_NAME}" "${APP_EXE}"
  WriteUninstaller "$INSTDIR\${UNINSTALLER_NAME}"

  ; Accesos directos. En una actualización solo se refrescan los que existen:
  ; si el usuario quitó el del escritorio, no vuelve a aparecer.
  ${If} $IsUpdate == 1
    ${If} ${FileExists} "$SMPROGRAMS\${PRODUCT_NAME}.lnk"
      CreateShortCut "$SMPROGRAMS\${PRODUCT_NAME}.lnk" "$INSTDIR\${APP_EXE_NAME}" "" "$INSTDIR\${APP_EXE_NAME}" 0
    ${EndIf}
    ${If} ${FileExists} "$DESKTOP\${PRODUCT_NAME}.lnk"
      CreateShortCut "$DESKTOP\${PRODUCT_NAME}.lnk" "$INSTDIR\${APP_EXE_NAME}" "" "$INSTDIR\${APP_EXE_NAME}" 0
    ${EndIf}
  ${Else}
    CreateShortCut "$SMPROGRAMS\${PRODUCT_NAME}.lnk" "$INSTDIR\${APP_EXE_NAME}" "" "$INSTDIR\${APP_EXE_NAME}" 0
    CreateShortCut "$DESKTOP\${PRODUCT_NAME}.lnk" "$INSTDIR\${APP_EXE_NAME}" "" "$INSTDIR\${APP_EXE_NAME}" 0
  ${EndIf}

  ; Registro: ubicación (para la próxima actualización) y "Aplicaciones instaladas".
  WriteRegStr HKCU "${INSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${PRODUCT_NAME} ${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\${APP_EXE_NAME},0"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\${UNINSTALLER_NAME}" /currentuser'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\${UNINSTALLER_NAME}" /currentuser /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" "$0"
SectionEnd

; Como el instalador de un clic de electron-builder: al terminar abre la app,
; salvo en modo silencioso sin --force-run. Sin argumentos: la app trata cada
; argumento como la ruta de un repositorio.
Function .onInstSuccess
  ${If} $ForceRun == 1
  ${OrIfNot} ${Silent}
    SetOutPath "$INSTDIR"
    Exec '"$INSTDIR\${APP_EXE_NAME}"'
  ${EndIf}
FunctionEnd

; --- desinstalación -----------------------------------------------------------

Function un.onInit
  SetShellVarContext current
  !insertmacro ParseArguments
FunctionEnd

Section "Uninstall"
  SetShellVarContext current
  Call un.CloseRunningApp

  Delete "$INSTDIR\${APP_EXE_NAME}"
  Delete "$INSTDIR\${UNINSTALLER_NAME}"
  ; Sin /r: la carpeta solo se quita si quedó vacía.
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${PRODUCT_NAME}.lnk"
  Delete "$DESKTOP\${PRODUCT_NAME}.lnk"

  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  DeleteRegKey HKCU "${INSTALL_KEY}"

  ; Descargas del actualizador. La configuración del usuario
  ; (%APPDATA%\Mergeon) se conserva.
  RMDir /r "$LOCALAPPDATA\mergeon-updater"
SectionEnd
