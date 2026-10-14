; Installer hooks (Phase 5.4). Trak keeps its library, db and caches loose in
; the install folder, next to Trak.exe (see src/data_dir.rs), and runs without
; admin rights. The installer is the only part that runs elevated, so this is
; where the folder becomes writable.
;
; SIDs instead of group names so it works on any Windows language:
;   S-1-5-32-545 Users, S-1-5-32-544 Administrators, S-1-5-18 SYSTEM.

!macro NSIS_HOOK_POSTINSTALL
  ; Users may modify the folder and everything created in it later.
  nsExec::ExecToLog 'icacls "$INSTDIR" /grant *S-1-5-32-545:(OI)(CI)M'
  Pop $0
  ; ...except the programs themselves: with the inherited Modify any process
  ; running as the user could replace Trak.exe. Inheritance off, explicit ACL.
  nsExec::ExecToLog 'icacls "$INSTDIR\${MAINBINARYNAME}.exe" /inheritance:r /grant:r *S-1-5-32-544:F *S-1-5-18:F *S-1-5-32-545:RX'
  Pop $0
  nsExec::ExecToLog 'icacls "$INSTDIR\uninstall.exe" /inheritance:r /grant:r *S-1-5-32-544:F *S-1-5-18:F *S-1-5-32-545:RX'
  Pop $0
!macroend

; "Delete the application data" in the uninstaller. Tauri's own handling of
; that checkbox only clears %APPDATA%/%LOCALAPPDATA%\<bundle id>, where Trak
; keeps nothing: its data is here, next to the program. Never on an update
; (the reinstall runs the old uninstaller first).
;
; Only what Trak creates, by name, and the folder only if that leaves it
; empty: someone may have installed into a folder that holds other things.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    Delete "$INSTDIR\db.sqlite"
    Delete "$INSTDIR\db.sqlite-wal"
    Delete "$INSTDIR\db.sqlite-shm"
    Delete "$INSTDIR\perf.log"
    Delete "$INSTDIR\.trak-write-test"
    RMDir /r "$INSTDIR\library"
    RMDir /r "$INSTDIR\covers"
    RMDir /r "$INSTDIR\trash"
    RMDir /r "$INSTDIR\incoming"
    RMDir /r "$INSTDIR\webview"
    RMDir "$INSTDIR"
  ${EndIf}
!macroend
