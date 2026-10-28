; -*- coding: utf-8 -*-
Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "x64.nsh"
!include "WinVer.nsh"
!ifndef PROJECT_ROOT
  !error "PROJECT_ROOT is required"
!endif
!ifndef VERSION
  !error "VERSION is required"
!endif
Name "VIBE ${VERSION}"
OutFile "${PROJECT_ROOT}\dist\VIBE-Setup-${VERSION}.exe"
InstallDir "$LOCALAPPDATA\Programs\VIBE"
RequestExecutionLevel user
SetCompressor /SOLID zlib
SetDatablockOptimize on
VIProductVersion "${VERSION}.0"
VIAddVersionKey /LANG=1033 "ProductName" "VIBE"
VIAddVersionKey /LANG=1033 "FileDescription" "VIBE desktop installer"
VIAddVersionKey /LANG=1033 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=1033 "LegalCopyright" "VIBE"
!define MUI_ICON "${PROJECT_ROOT}\assets\vibe.ico"
!define MUI_UNICON "${PROJECT_ROOT}\assets\vibe.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\VIBE.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start VIBE"
!define MUI_WELCOMEPAGE_TEXT "Install VIBE with offline dictation support.$\r$\n$\r$\nYour conversations and account settings are stored separately and are preserved when updating."
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "Hungarian"

Function .onInit
  ${IfNot} ${AtLeastWin10}
    MessageBox MB_OK|MB_ICONSTOP "VIBE requires Windows 10 or Windows 11."
    Abort
  ${EndIf}
  ${IfNot} ${RunningX64}
    MessageBox MB_OK|MB_ICONSTOP "This installer requires 64-bit Windows."
    Abort
  ${EndIf}
  SetRegView 64
  SetShellVarContext current
  ReadRegStr $0 HKCU "Software\VIBE" "InstallLocation"
  ${If} $0 != ""
    StrCpy $INSTDIR $0
  ${EndIf}
FunctionEnd

!macro NotRunning PREFIX
Function ${PREFIX}EnsureNotRunning
retry:
  IfFileExists "$INSTDIR\VIBE.exe" 0 done
  ClearErrors
  FileOpen $0 "$INSTDIR\VIBE.exe" a
  IfErrors running
  FileClose $0
  Goto done
running:
  MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "Please close VIBE before installing or removing it, then choose Retry. No running sessions were terminated." IDRETRY retry
  SetErrorLevel 2
  Abort
done:
FunctionEnd
!macroend
!insertmacro NotRunning ""
!insertmacro NotRunning "un."

Section "VIBE and offline dictation (required)" Core
  SectionIn RO
  Call EnsureNotRunning
  SetOutPath "$INSTDIR"
  SetOverwrite on
  File /oname=VIBE.exe "${PROJECT_ROOT}\target\release\vibe.exe"
  File "${PROJECT_ROOT}\.tools\runtime\*.dll"
  File /oname=NOTICE-runtime.txt "${PROJECT_ROOT}\.tools\runtime\NOTICE.txt"
  File /oname=NOTICE-OpenRouter.txt "${PROJECT_ROOT}\assets\providers\LICENSE-openrouter.txt"
  SetOutPath "$INSTDIR\speech"
  File "${PROJECT_ROOT}\speech\whisper-cli.exe"
  File "${PROJECT_ROOT}\speech\whisper.dll"
  File "${PROJECT_ROOT}\speech\ggml.dll"
  File "${PROJECT_ROOT}\speech\ggml-base.dll"
  File "${PROJECT_ROOT}\speech\ggml-cpu.dll"
  File "${PROJECT_ROOT}\speech\ggml-small.bin"
  File "${PROJECT_ROOT}\speech\LICENSE-model.txt"
  File "${PROJECT_ROOT}\speech\LICENSE-whisper.txt"
  File "${PROJECT_ROOT}\.tools\runtime\*.dll"
  SetOutPath "$INSTDIR"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateDirectory "$SMPROGRAMS\VIBE"
  CreateShortcut "$SMPROGRAMS\VIBE\VIBE.lnk" "$INSTDIR\VIBE.exe" "" "$INSTDIR\VIBE.exe"
  CreateShortcut "$SMPROGRAMS\VIBE\Uninstall VIBE.lnk" "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\VIBE" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\VIBE" "DisplayName" "VIBE"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\VIBE" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\VIBE" "DisplayIcon" "$INSTDIR\VIBE.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\VIBE" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\VIBE" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\VIBE" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\VIBE" "NoRepair" 1
SectionEnd

Section /o "Desktop shortcut" DesktopShortcut
  CreateShortcut "$DESKTOP\VIBE.lnk" "$INSTDIR\VIBE.exe" "" "$INSTDIR\VIBE.exe"
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  SetRegView 64
  Call un.EnsureNotRunning
  ; Remove owned files only. Never recursively delete INSTDIR or user data.
  Delete "$INSTDIR\VIBE.exe"
  Delete "$INSTDIR\speech\whisper-cli.exe"
  Delete "$INSTDIR\speech\whisper.dll"
  Delete "$INSTDIR\speech\ggml.dll"
  Delete "$INSTDIR\speech\ggml-base.dll"
  Delete "$INSTDIR\speech\ggml-cpu.dll"
  Delete "$INSTDIR\speech\ggml-small.bin"
  Delete "$INSTDIR\speech\LICENSE-model.txt"
  Delete "$INSTDIR\speech\LICENSE-whisper.txt"
  Delete "$INSTDIR\NOTICE-runtime.txt"
  Delete "$INSTDIR\NOTICE-OpenRouter.txt"
  !macro RemoveRuntime NAME
    Delete "$INSTDIR\${NAME}"
    Delete "$INSTDIR\speech\${NAME}"
  !macroend
  !insertmacro RemoveRuntime "concrt140.dll"
  !insertmacro RemoveRuntime "msvcp140.dll"
  !insertmacro RemoveRuntime "msvcp140_1.dll"
  !insertmacro RemoveRuntime "msvcp140_2.dll"
  !insertmacro RemoveRuntime "msvcp140_atomic_wait.dll"
  !insertmacro RemoveRuntime "msvcp140_codecvt_ids.dll"
  !insertmacro RemoveRuntime "vccorlib140.dll"
  !insertmacro RemoveRuntime "vcruntime140.dll"
  !insertmacro RemoveRuntime "vcruntime140_1.dll"
  !insertmacro RemoveRuntime "vcruntime140_threads.dll"
  !insertmacro RemoveRuntime "vcomp140.dll"
  RMDir "$INSTDIR\speech"
  Delete "$SMPROGRAMS\VIBE\VIBE.lnk"
  Delete "$SMPROGRAMS\VIBE\Uninstall VIBE.lnk"
  RMDir "$SMPROGRAMS\VIBE"
  Delete "$DESKTOP\VIBE.lnk"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\VIBE"
  DeleteRegKey HKCU "Software\VIBE"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
SectionEnd
