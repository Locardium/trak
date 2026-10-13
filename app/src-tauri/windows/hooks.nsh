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
