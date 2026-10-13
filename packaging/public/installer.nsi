Unicode true
!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"
!ifndef PACKAGE_ROOT
  !error "PACKAGE_ROOT is required"
!endif
!ifndef SETUP_OUT
  !error "SETUP_OUT is required"
!endif
!ifndef VERSION
  !define VERSION "0.3.0"
!endif
Name "ClipForge ${VERSION}"
OutFile "${SETUP_OUT}"
InstallDir "$PROFILE\ClipForge"
RequestExecutionLevel user
SetCompressor zlib
ShowInstDetails show
ShowUninstDetails show
Var FixtureMode
Var GuardReason
!macro CheckNoLink Relative
  Push "${Relative}"
  !ifdef CHECK_UNINSTALL
    Call un.CheckDestination
  !else
    Call CheckDestination
  !endif
!macroend
!macro DefineDestinationCheck Prefix
Function ${Prefix}CheckDestination
  Pop $R8
  System::Call 'kernel32::GetFileAttributesW(w "$INSTDIR\$R8") i.r0'
  ${If} $0 != -1
    IntOp $R6 $0 & 0x400
    ${If} $R6 != 0
      MessageBox MB_ICONSTOP "A linked application file or folder was found. Restore a regular ClipForge installation before updating or uninstalling." /SD IDOK
      SetErrorLevel 4
      Abort
    ${EndIf}
    IntOp $R6 $0 & 0x10
    ${If} $R6 == 0
      ; Hard links do not carry a reparse flag. Reject a shared file before
      ; writing so an update cannot alter another name outside the workspace.
      System::Call 'kernel32::CreateFileW(w "$INSTDIR\$R8", i 0, i 7, p 0, i 3, i 0, p 0) p.r7'
      ${If} $7 != -1
        System::Alloc 52
        Pop $5
        ${If} $5 != 0
          System::Call 'kernel32::GetFileInformationByHandle(p r7, p r5) i.r6'
          ${If} $6 != 0
            IntOp $R7 $5 + 40
            System::Call '*$R7(i.r6)'
            ${If} $6 > 1
              StrCpy $R6 1
            ${EndIf}
          ${EndIf}
          System::Free $5
        ${EndIf}
        System::Call 'kernel32::CloseHandle(p r7)'
      ${EndIf}
      ${If} $R6 == 1
        MessageBox MB_ICONSTOP "An application file has multiple linked locations. Restore a regular ClipForge installation before updating or uninstalling." /SD IDOK
        SetErrorLevel 4
        Abort
      ${EndIf}
    ${EndIf}
  ${EndIf}
FunctionEnd
!macroend
!insertmacro DefineDestinationCheck ""
!insertmacro DefineDestinationCheck "un."
!define MUI_ABORTWARNING
!ifdef NATIVE_ICON
  !define MUI_ICON "${NATIVE_ICON}"
  !define MUI_UNICON "${NATIVE_ICON}"
!endif
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${PACKAGE_ROOT}\LICENSE.txt"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function .onInit
  StrCpy $FixtureMode 0
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/FIXTURE" $R1
  IfErrors +2
    StrCpy $FixtureMode 1
FunctionEnd

Function .onVerifyInstDir
  StrCpy $GuardReason "root"
  StrCpy $R4 "$INSTDIR" 4
  ${If} $R4 == "\\?\"
    StrCpy $INSTDIR "$INSTDIR" "" 4
  ${EndIf}
  ; Installer workspaces are absolute local drive folders, not UNC/device paths.
  StrCpy $R4 "$INSTDIR" 2 1
  StrCmp "$R4" ":\" 0 unsafe
  ; NSIS GetFullPathName rejects nonexistent folders. The Windows API normalizes
  ; the selected new path without requiring an existing installation.
  System::Call 'kernel32::GetFullPathNameW(w "$INSTDIR", i ${NSIS_MAX_STRLEN}, w .r4, p 0) i.r0'
  IntCmp $0 0 unsafe unsafe
  IntCmp $0 ${NSIS_MAX_STRLEN} unsafe 0 unsafe
  StrCpy $INSTDIR "$4"
  ${GetRoot} "$INSTDIR" $R0
  StrCmp "$INSTDIR" "$R0" unsafe
  StrCmp "$INSTDIR" "$R0\" unsafe
  StrCpy $GuardReason "appdata"
  System::Call 'shlwapi::PathIsPrefixW(w "$APPDATA", w "$INSTDIR") i.r0'
  IntCmp $0 0 +2
    Goto unsafe
  StrCpy $GuardReason "localappdata"
  System::Call 'shlwapi::PathIsPrefixW(w "$LOCALAPPDATA", w "$INSTDIR") i.r0'
  IntCmp $0 0 +2
    Goto unsafe
  StrCpy $GuardReason "windows"
  System::Call 'shlwapi::PathIsPrefixW(w "$WINDIR", w "$INSTDIR") i.r0'
  IntCmp $0 0 +2
    Goto unsafe
  StrCpy $GuardReason "programfiles"
  System::Call 'shlwapi::PathIsPrefixW(w "$PROGRAMFILES", w "$INSTDIR") i.r0'
  IntCmp $0 0 +2
    Goto unsafe
  StrCpy $GuardReason "programfiles64"
  System::Call 'shlwapi::PathIsPrefixW(w "$PROGRAMFILES64", w "$INSTDIR") i.r0'
  IntCmp $0 0 +2
    Goto unsafe
  StrCpy $R3 "$INSTDIR"
  ancestors:
    StrCpy $GuardReason "linked ancestor"
    System::Call 'kernel32::GetFileAttributesW(w "$R3") i.r0'
    IntCmp $0 -1 parent
    IntOp $R2 $0 & 0x400
    IntCmp $R2 0 parent
      Goto unsafe
    parent:
    ${GetParent} "$R3" $R2
    StrCmp "$R3" "$R2" checkcontents
    StrCmp "$R2" "" checkcontents
    StrCpy $R3 "$R2"
    Goto ancestors
  checkcontents:
    StrCpy $GuardReason "nonempty unrelated folder"
    IfFileExists "$INSTDIR\clipforge-portable.json" done
    ClearErrors
    FindFirst $R1 $R2 "$INSTDIR\*"
    IfErrors done
    scan:
      StrCmp "$R2" "." next
      StrCmp "$R2" ".." next
      FindClose $R1
      Goto unsafe
    next:
      ClearErrors
      FindNext $R1 $R2
      IfErrors empty
      Goto scan
    empty:
      FindClose $R1
  done:
    Return
  unsafe:
    StrCmp $FixtureMode 1 0 nofixturelog
      ReadEnvStr $8 TEMP
      System::Call 'shlwapi::PathIsPrefixW(w "J:\CS2ClipStudio\temp", w "$8") i.r0'
      IntCmp $0 1 0 nofixturelog nofixturelog
      FileOpen $9 "$8\installer-guard.txt" a
      FileSeek $9 0 END
      FileWrite $9 "$GuardReason: $INSTDIR; ancestor=$R3; attrs=$0$\r$\n"
      FileClose $9
    nofixturelog:
    MessageBox MB_ICONSTOP "Choose an empty private writable folder outside AppData and system folders, or an existing ClipForge workspace. Linked folders and drive roots are unsupported." /SD IDOK
    SetErrorLevel 2
    Abort
FunctionEnd

Section "ClipForge"
  Call .onVerifyInstDir
  !include "${CHECK_MANIFEST}"
  IfFileExists "$INSTDIR\ClipForge.exe" 0 writable
    ; Never overwrite a running/locked native app or alter the process owning it.
    System::Call 'kernel32::CreateFileW(w "$INSTDIR\ClipForge.exe", i 0x40000000, i 7, p 0, i 3, i 0, p 0) p.r0'
    IntCmp $0 -1 appbusy
    System::Call 'kernel32::CloseHandle(p r0)'
  writable:
  SetOutPath "$INSTDIR"
  IfErrors appbusy
  File /r "${PACKAGE_ROOT}\*.*"
  IfErrors appbusy
  WriteUninstaller "$INSTDIR\Uninstall-ClipForge.exe"
  StrCmp $FixtureMode 1 registrationsdone
    SetShellVarContext current
    CreateDirectory "$SMPROGRAMS\ClipForge"
    CreateShortcut "$SMPROGRAMS\ClipForge\ClipForge.lnk" "$INSTDIR\ClipForge.exe"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ClipForge" "DisplayName" "ClipForge"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ClipForge" "DisplayVersion" "${VERSION}"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ClipForge" "InstallLocation" "$INSTDIR"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ClipForge" "UninstallString" '$\"$INSTDIR\Uninstall-ClipForge.exe$\"'
    WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ClipForge" "NoModify" 1
    WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ClipForge" "NoRepair" 1
  registrationsdone:
    Goto installfinished
  appbusy:
    MessageBox MB_ICONSTOP "Close ClipForge and retry. The selected folder must allow application updates." /SD IDOK
    SetErrorLevel 3
    Abort
  installfinished:
SectionEnd

Function un.CheckRoot
  ; An uninstall override must satisfy the same root constraints as installation.
  ; Check ancestors as well as registered files: an alias of the whole workspace
  ; otherwise hides its reparse point from checks of children.
  StrCpy $R4 "$INSTDIR" 4
  ${If} $R4 == "\\?\"
    StrCpy $INSTDIR "$INSTDIR" "" 4
  ${EndIf}
  StrCpy $R4 "$INSTDIR" 2 1
  StrCmp "$R4" ":\" 0 unrootunsafe
  System::Call 'kernel32::GetFullPathNameW(w "$INSTDIR", i ${NSIS_MAX_STRLEN}, w .r4, p 0) i.r0'
  IntCmp $0 0 unrootunsafe unrootunsafe
  IntCmp $0 ${NSIS_MAX_STRLEN} unrootunsafe 0 unrootunsafe
  StrCpy $INSTDIR "$4"
  ${GetRoot} "$INSTDIR" $R0
  StrCmp "$INSTDIR" "$R0" unrootunsafe
  StrCmp "$INSTDIR" "$R0\" unrootunsafe
  System::Call 'shlwapi::PathIsPrefixW(w "$APPDATA", w "$INSTDIR") i.r0'
  IntCmp $0 0 +2
    Goto unrootunsafe
  System::Call 'shlwapi::PathIsPrefixW(w "$LOCALAPPDATA", w "$INSTDIR") i.r0'
  IntCmp $0 0 +2
    Goto unrootunsafe
  System::Call 'shlwapi::PathIsPrefixW(w "$WINDIR", w "$INSTDIR") i.r0'
  IntCmp $0 0 +2
    Goto unrootunsafe
  System::Call 'shlwapi::PathIsPrefixW(w "$PROGRAMFILES", w "$INSTDIR") i.r0'
  IntCmp $0 0 +2
    Goto unrootunsafe
  System::Call 'shlwapi::PathIsPrefixW(w "$PROGRAMFILES64", w "$INSTDIR") i.r0'
  IntCmp $0 0 +2
    Goto unrootunsafe
  StrCpy $R3 "$INSTDIR"
  unrootancestors:
    System::Call 'kernel32::GetFileAttributesW(w "$R3") i.r0'
    IntCmp $0 -1 unrootparent
    IntOp $R2 $0 & 0x400
    IntCmp $R2 0 unrootparent
      Goto unrootunsafe
    unrootparent:
    ${GetParent} "$R3" $R2
    StrCmp "$R3" "$R2" unrootdone
    StrCmp "$R2" "" unrootdone
    StrCpy $R3 "$R2"
    Goto unrootancestors
  unrootunsafe:
    MessageBox MB_ICONSTOP "The application root is linked or outside a supported private workspace. Application files and data were preserved." /SD IDOK
    SetErrorLevel 4
    Abort
  unrootdone:
FunctionEnd

Function un.onInit
  StrCpy $FixtureMode 0
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/FIXTURE" $R1
  IfErrors +2
    StrCpy $FixtureMode 1
  Call un.CheckRoot
  IfFileExists "$INSTDIR\clipforge-portable.json" +3
    MessageBox MB_ICONSTOP "The ClipForge workspace marker is missing. Application files were preserved." /SD IDOK
    Abort
  !define CHECK_UNINSTALL
  !include "${CHECK_MANIFEST}"
  !undef CHECK_UNINSTALL
  IfFileExists "$INSTDIR\ClipForge.exe" 0 unready
    System::Call 'kernel32::CreateFileW(w "$INSTDIR\ClipForge.exe", i 0x40000000, i 7, p 0, i 3, i 0, p 0) p.r0'
    IntCmp $0 -1 unbusy
    System::Call 'kernel32::CloseHandle(p r0)'
    Goto unready
  unbusy:
    MessageBox MB_ICONSTOP "Close ClipForge before uninstalling. Your data will be preserved." /SD IDOK
    SetErrorLevel 3
    Abort
  unready:
FunctionEnd

Section "Uninstall"
  ; Explicit package file list only. Never recursively remove user data or tools.
  !include "${DELETE_MANIFEST}"
  Delete "$INSTDIR\Uninstall-ClipForge.exe"
  StrCmp $FixtureMode 1 unregistrationsdone
    SetShellVarContext current
    Delete "$SMPROGRAMS\ClipForge\ClipForge.lnk"
    RMDir "$SMPROGRAMS\ClipForge"
    DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ClipForge"
  unregistrationsdone:
  ; Keep ownership marker, allowing reinstallation alongside preserved user data.
SectionEnd
