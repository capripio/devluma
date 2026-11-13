; Devluma installer hooks (Tauri `bundle.windows.nsis.installerHooks`, ADR 0020).
;
; Reinstalling over an existing Devluma folder offers what to do with it:
;   - Keep everything (the default, and always in silent, passive and updater runs).
;   - Reset Devluma: its settings database, generated configs, logs, certificates and the
;     like move to "previous\<date>" in the same folder (nothing is deleted), so the next
;     start is a first run. Optionally the downloaded tools (bin) are deleted, and the
;     database data (data) is moved aside too.
; Projects in www are never touched.
;
; This file is included before the template's page list, so the page below is the first
; one shown, and only when Devluma data is found in the install folder.

!include LogicLib.nsh
!include nsDialogs.nsh
!include FileFunc.nsh

Var DlReset        ; 1 = reset chosen
Var DlDeleteBin    ; 1 = delete bin (after moving it aside)
Var DlMoveData     ; 1 = move data aside too
Var DlDataDir      ; the folder the choice was made for
Var DlRadioKeep
Var DlRadioReset
Var DlCheckBin
Var DlCheckData
Var DlPrev         ; previous\<stamp>
Var DlFailed       ; name of the item that couldn't be moved

Page custom DlDataPage DlDataPageLeave

Function DlDataPage
  StrCpy $DlReset 0
  StrCpy $DlDeleteBin 0
  StrCpy $DlMoveData 0
  StrCpy $DlDataDir ""
  ; Unattended runs (silent, /P passive, /UPDATE from the updater) always keep everything.
  ${If} ${Silent}
    Abort
  ${EndIf}
  ClearErrors
  ${GetOptions} $CMDLINE "/P" $0
  ${IfNot} ${Errors}
    Abort
  ${EndIf}
  ClearErrors
  ${GetOptions} $CMDLINE "/UPDATE" $0
  ${IfNot} ${Errors}
    Abort
  ${EndIf}
  ${IfNot} ${FileExists} "$INSTDIR\app.db"
    Abort
  ${EndIf}

  !insertmacro MUI_HEADER_TEXT "Your Devluma data" "Devluma is already set up in this folder."
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}

  ${NSD_CreateLabel} 0 0 100% 20u "Found settings, tools and projects in $INSTDIR. What should happen to them?"
  Pop $0
  ${NSD_CreateRadioButton} 0 24u 100% 10u "&Keep everything (recommended)"
  Pop $DlRadioKeep
  ${NSD_CreateRadioButton} 0 38u 100% 10u "&Reset Devluma: start again with the setup wizard"
  Pop $DlRadioReset
  ${NSD_CreateLabel} 12u 50u -12u 26u "Settings, service configs, logs, certificates and backups move to $INSTDIR\previous\<date>. Nothing there is deleted. If you trusted Devluma's local CA, remove it on the SSL page first."
  Pop $0
  ${NSD_CreateCheckbox} 12u 78u -12u 10u "&Delete downloaded tools (bin) to free space; they download again"
  Pop $DlCheckBin
  ${NSD_CreateCheckbox} 12u 90u -12u 10u "&Move database data (data) aside too"
  Pop $DlCheckData
  ${NSD_CreateLabel} 0 106u 100% 20u "Your projects in $INSTDIR\www are never changed."
  Pop $0

  ${NSD_Check} $DlRadioKeep
  EnableWindow $DlCheckBin 0
  EnableWindow $DlCheckData 0
  ${NSD_OnClick} $DlRadioKeep DlDataPageToggle
  ${NSD_OnClick} $DlRadioReset DlDataPageToggle
  nsDialogs::Show
FunctionEnd

Function DlDataPageToggle
  Pop $0
  ${NSD_GetState} $DlRadioReset $1
  ${If} $1 == ${BST_CHECKED}
    EnableWindow $DlCheckBin 1
    EnableWindow $DlCheckData 1
  ${Else}
    EnableWindow $DlCheckBin 0
    EnableWindow $DlCheckData 0
  ${EndIf}
FunctionEnd

Function DlDataPageLeave
  ${NSD_GetState} $DlRadioReset $0
  ${If} $0 == ${BST_CHECKED}
    StrCpy $DlReset 1
    StrCpy $DlDataDir $INSTDIR
    ${NSD_GetState} $DlCheckBin $0
    ${IfThen} $0 == ${BST_CHECKED} ${|} StrCpy $DlDeleteBin 1 ${|}
    ${NSD_GetState} $DlCheckData $0
    ${IfThen} $0 == ${BST_CHECKED} ${|} StrCpy $DlMoveData 1 ${|}
  ${EndIf}
FunctionEnd

; Moves one top-level item into $DlPrev, unless an earlier move failed.
!macro DlMoveAside NAME
  ${If} $DlFailed == ""
  ${AndIf} ${FileExists} "$DlDataDir\${NAME}"
    ClearErrors
    Rename "$DlDataDir\${NAME}" "$DlPrev\${NAME}"
    ${If} ${Errors}
      StrCpy $DlFailed "${NAME}"
    ${EndIf}
  ${EndIf}
!macroend

; Puts one item back after a failed reset.
!macro DlPutBack NAME
  ${If} ${FileExists} "$DlPrev\${NAME}"
  ${AndIfNot} ${FileExists} "$DlDataDir\${NAME}"
    Rename "$DlPrev\${NAME}" "$DlDataDir\${NAME}"
  ${EndIf}
!macroend

!macro DlEachItem MACRO
  !insertmacro ${MACRO} "app.db"
  !insertmacro ${MACRO} "app.db-wal"
  !insertmacro ${MACRO} "app.db-shm"
  !insertmacro ${MACRO} "backups"
  !insertmacro ${MACRO} "services"
  !insertmacro ${MACRO} "certificates"
  !insertmacro ${MACRO} "tunnels"
  !insertmacro ${MACRO} "config"
  !insertmacro ${MACRO} "catalog"
  !insertmacro ${MACRO} "diagnostics"
  !insertmacro ${MACRO} "logs"
  !insertmacro ${MACRO} "composer"
  !insertmacro ${MACRO} "staging"
  !insertmacro ${MACRO} "tmp"
  !insertmacro ${MACRO} "webview"
  !insertmacro ${MACRO} "bin"
  ${If} $DlMoveData = 1
    !insertmacro ${MACRO} "data"
  ${EndIf}
!macroend

; The reset itself: every item is moved (renames on one volume), or none is. A locked
; file (a tool still running) makes the whole reset roll back and the install stop.
!macro DlApplyReset
  ${If} $DlReset = 1
  ${AndIf} $DlDataDir == $INSTDIR
    ${GetTime} "" "L" $0 $1 $2 $3 $4 $5 $6
    StrCpy $DlPrev "$DlDataDir\previous\$2$1$0-$4$5$6"
    StrCpy $DlFailed ""
    CreateDirectory "$DlPrev"
    !insertmacro DlEachItem DlMoveAside
    ${If} $DlFailed != ""
      !insertmacro DlEachItem DlPutBack
      RMDir "$DlPrev"
      RMDir "$DlDataDir\previous"
      MessageBox MB_ICONSTOP|MB_OK "Devluma couldn't move $DlFailed aside, so nothing was reset. Quit Devluma and stop its services (or restart Windows), then run setup again." /SD IDOK
      Abort
    ${EndIf}
    ${If} $DlDeleteBin = 1
      DetailPrint "Deleting downloaded tools from $DlPrev\bin"
      RMDir /r "$DlPrev\bin"
    ${EndIf}
    DetailPrint "Devluma's previous settings are in $DlPrev"
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  ${If} $DlReset = 1
    ; Nothing may hold these files while they move.
    !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  ${EndIf}
  !insertmacro DlApplyReset
!macroend
