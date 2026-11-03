; Compile only from an inspected package-release.py Windows package.
; makensis -DPACKAGE_DIR=/absolute/extracted/package -DOUTPUT_FILE=/absolute/Setup.exe tools/windows-installer.nsi
Unicode true
!include "x64.nsh"
!ifndef PACKAGE_DIR
  !error "PACKAGE_DIR must name the verified extracted Windows package"
!endif
!ifndef OUTPUT_FILE
  !error "OUTPUT_FILE must name the installer to create"
!endif

Name "Trigon Studio 0.1.0 Preview"
OutFile "${OUTPUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\Trigon"
RequestExecutionLevel user
ManifestDPIAware true
SetCompressor zlib
SetOverwrite on
AllowSkipFiles off
ShowInstDetails show
ShowUninstDetails show
VIProductVersion "0.1.0.0"
VIAddVersionKey /LANG=1033 "ProductName" "Trigon Studio"
VIAddVersionKey /LANG=1033 "FileDescription" "Trigon Studio preview installer (unsigned)"
VIAddVersionKey /LANG=1033 "FileVersion" "0.1.0"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Trigon contributors — MIT OR Apache-2.0"

Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_OK|MB_ICONSTOP "This preview requires 64-bit Windows."
    Abort
  ${EndIf}
FunctionEnd

Section "Trigon Studio"
  SetShellVarContext current
  SetOutPath "$INSTDIR"
  File "${PACKAGE_DIR}/trigon-studio.exe"
  File "${PACKAGE_DIR}/trigon.exe"
  File "${PACKAGE_DIR}/README.txt"
  File "${PACKAGE_DIR}/PROVENANCE.json"
  SetOutPath "$INSTDIR\LICENSES"
  File "${PACKAGE_DIR}/LICENSES/LICENSE-MIT"
  File "${PACKAGE_DIR}/LICENSES/LICENSE-APACHE"
  File "${PACKAGE_DIR}/LICENSES/ASSET-ATTRIBUTION.md"
  File "${PACKAGE_DIR}/LICENSES/Trigon-Artwork-NOTICE.md"
  File "${PACKAGE_DIR}/LICENSES/Lucide-Feather-LICENSE.txt"
  File "${PACKAGE_DIR}/LICENSES/Inter-OFL.txt"
  File "${PACKAGE_DIR}/LICENSES/IBM-Plex-Mono-OFL.txt"
  File "${PACKAGE_DIR}/LICENSES/Liberation-Sans-OFL.txt"
  File "${PACKAGE_DIR}/LICENSES/Liberation-OFL.txt"
  File "${PACKAGE_DIR}/LICENSES/THIRD-PARTY-NOTICES.txt"
  SetOutPath "$INSTDIR"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateDirectory "$SMPROGRAMS\Trigon"
  CreateShortCut "$SMPROGRAMS\Trigon\Trigon Studio.lnk" "$INSTDIR\trigon-studio.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Trigon" "DisplayName" "Trigon Studio 0.1.0 Preview"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Trigon" "DisplayVersion" "0.1.0"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Trigon" "Publisher" "Trigon contributors"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Trigon" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Trigon" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Trigon" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Trigon" "DisplayIcon" "$INSTDIR\trigon-studio.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Trigon" "URLInfoAbout" "https://github.com/WCKDai/trigon"
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Trigon" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Trigon" "NoRepair" 1
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  ; Delete only installed files. Never recursively remove a folder or user documents.
  ; Preserve the uninstall registration if Windows refuses a locked executable.
  ClearErrors
  Delete "$INSTDIR\trigon-studio.exe"
  IfErrors remove_failed
  Delete "$INSTDIR\trigon.exe"
  IfErrors remove_failed
  Delete "$INSTDIR\README.txt"
  Delete "$INSTDIR\PROVENANCE.json"
  Delete "$INSTDIR\LICENSES\LICENSE-MIT"
  Delete "$INSTDIR\LICENSES\LICENSE-APACHE"
  Delete "$INSTDIR\LICENSES\ASSET-ATTRIBUTION.md"
  Delete "$INSTDIR\LICENSES\Trigon-Artwork-NOTICE.md"
  Delete "$INSTDIR\LICENSES\Lucide-Feather-LICENSE.txt"
  Delete "$INSTDIR\LICENSES\Inter-OFL.txt"
  Delete "$INSTDIR\LICENSES\IBM-Plex-Mono-OFL.txt"
  Delete "$INSTDIR\LICENSES\Liberation-Sans-OFL.txt"
  Delete "$INSTDIR\LICENSES\Liberation-OFL.txt"
  Delete "$INSTDIR\LICENSES\THIRD-PARTY-NOTICES.txt"
  RMDir "$INSTDIR\LICENSES"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\Trigon\Trigon Studio.lnk"
  RMDir "$SMPROGRAMS\Trigon"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Trigon"
  Goto remove_done
remove_failed:
  MessageBox MB_OK|MB_ICONSTOP "Trigon is still in use or its executable cannot be removed. Close Studio and any Trigon commands, then run Uninstall again."
  SetErrorLevel 1
  Abort
remove_done:
SectionEnd
