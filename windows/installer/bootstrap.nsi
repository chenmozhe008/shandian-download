Unicode true
Name "${APP_TITLE}"
OutFile "${OUTPUT_NAME}"
Icon "icon.ico"
RequestExecutionLevel user
CRCCheck force
SetCompress off
SilentInstall silent
VIProductVersion "${APP_VERSION}.0"
VIAddVersionKey "ProductName" "Shandian"
VIAddVersionKey "FileDescription" "Shandian Windows Installer"
VIAddVersionKey "FileVersion" "${APP_VERSION}"
VIAddVersionKey "ProductVersion" "${APP_VERSION}"
VIAddVersionKey "LegalCopyright" "© 2026 cgeek"
!include "FileFunc.nsh"

Section
  ClearErrors
  InitPluginsDir
  IfErrors launchError
  SetOutPath "$PLUGINSDIR"
  File "shandian-setup.exe"
  File "WebView2Loader.dll"
  File "payload.zip"
  IfErrors launchError

  ${GetParameters} $0
  ClearErrors
  ExecWait '"$PLUGINSDIR\shandian-setup.exe" --payload "$PLUGINSDIR\payload.zip" $0' $1
  IfErrors launchError
  SetErrorLevel $1
  Goto finished

launchError:
  MessageBox MB_OK|MB_ICONSTOP "闪电安装程序无法启动。请检查临时目录及 Windows 安全软件提示。"
  SetErrorLevel 1
finished:
SectionEnd
