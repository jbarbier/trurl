; Trurl installer hooks (Tauri NSIS). See docs/USB-BUNDLE.md.
;
; Install: if a "models" folder sits next to the installer (the USB package),
; copy every .gguf in it to the learner's models folder. Each file is copied to
; "<name>.part" and renamed only when complete, so pulling the stick out midway
; never leaves a half model that looks whole. Files already there with the same
; size are skipped. Trurl itself checks bundled models against their SHA-256 on
; first launch.
;
; Never runs during an update (/UPDATE): updates replace the app, not models.
;
; Uninstall: the models are deleted only if the learner ticks Tauri's own
; "Delete the application data" box (never during an update, never in a silent
; uninstall). No extra question, so an interactive reinstall over an older
; version never asks about models.

!include "FileFunc.nsh"

!define TRURL_DATA "$LOCALAPPDATA\CyberiadLabs\Trurl"
!define TRURL_MODELS "$LOCALAPPDATA\CyberiadLabs\Trurl\models"

!macro NSIS_HOOK_POSTINSTALL
  ${If} $UpdateMode <> 1
  ${AndIf} ${FileExists} "$EXEDIR\models\*.gguf"
    CreateDirectory "${TRURL_MODELS}"
    DetailPrint "Copying models from $EXEDIR\models (this can take a few minutes)"
    ClearErrors
    FindFirst $R0 $R1 "$EXEDIR\models\*.gguf"
    ${DoWhile} $R1 != ""
      ${GetSize} "$EXEDIR\models" "/M=$R1 /S=0K /G=0" $R2 $R3 $R4
      StrCpy $R6 "-1"
      ${If} ${FileExists} "${TRURL_MODELS}\$R1"
        ${GetSize} "${TRURL_MODELS}" "/M=$R1 /S=0K /G=0" $R6 $R3 $R4
      ${EndIf}
      ${If} $R6 == $R2
        DetailPrint "Already there: $R1"
      ${Else}
        DetailPrint "Copying $R1"
        Delete "${TRURL_MODELS}\$R1.part"
        ClearErrors
        CopyFiles "$EXEDIR\models\$R1" "${TRURL_MODELS}\$R1.part"
        ${If} ${Errors}
          Delete "${TRURL_MODELS}\$R1.part"
          MessageBox MB_OK|MB_ICONEXCLAMATION "Trurl could not copy $R1. Check that the USB stick is still plugged in and that the computer has enough free space. You can import the model later from Trurl's Models screen." /SD IDOK
        ${Else}
          Delete "${TRURL_MODELS}\$R1"
          ClearErrors
          Rename "${TRURL_MODELS}\$R1.part" "${TRURL_MODELS}\$R1"
          ${If} ${Errors}
            Delete "${TRURL_MODELS}\$R1.part"
            MessageBox MB_OK|MB_ICONEXCLAMATION "Trurl could not finish copying $R1. Close Trurl if it is open, then run this installer again." /SD IDOK
          ${EndIf}
        ${EndIf}
      ${EndIf}
      FindNext $R0 $R1
    ${Loop}
    FindClose $R0
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
  ${AndIf} $DeleteAppDataCheckboxState = 1
    RMDir /r "${TRURL_DATA}"
  ${EndIf}
!macroend
