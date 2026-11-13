; Test harness for src-tauri/windows/hooks.nsh: runs the reset against /D=<folder>.
; Options: /BIN (delete bin), /DATA (move data too). Run silently with /S.
Unicode true
Name "hooks harness"
OutFile "harness.exe"
RequestExecutionLevel user
InstallDir "$TEMP\unused"
!define MAINBINARYNAME devluma
!define PRODUCTNAME Devluma
!include MUI2.nsh
!macro CheckIfAppIsRunning EXE NAME
!macroend
!include "${HOOKS}"
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_LANGUAGE English

Section
  StrCpy $DlReset 1
  StrCpy $DlDataDir $INSTDIR
  StrCpy $DlDeleteBin 0
  StrCpy $DlMoveData 0
  ClearErrors
  ${GetOptions} $CMDLINE "/BIN" $0
  ${IfNot} ${Errors}
    StrCpy $DlDeleteBin 1
  ${EndIf}
  ClearErrors
  ${GetOptions} $CMDLINE "/DATA" $0
  ${IfNot} ${Errors}
    StrCpy $DlMoveData 1
  ${EndIf}
  !insertmacro NSIS_HOOK_PREINSTALL
  FileOpen $9 "$INSTDIR\harness-done.txt" w
  FileWrite $9 "$DlPrev"
  FileClose $9
SectionEnd
