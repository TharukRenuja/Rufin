!include "MUI2.nsh"

!ifndef RUFIN_STAGE_DIR
!define RUFIN_STAGE_DIR "..\..\dist\windows\Rufin"
!endif

!ifndef RUFIN_OUTPUT_DIR
!define RUFIN_OUTPUT_DIR "..\..\dist"
!endif

!ifndef RUFIN_ASSET_DIR
!define RUFIN_ASSET_DIR "assets"
!endif

!ifndef RUFIN_VERSION
!define RUFIN_VERSION "0.0.0"
!endif

!ifndef RUFIN_VERSION_QUAD
!define RUFIN_VERSION_QUAD "0.0.0.0"
!endif

Unicode true
Name "Rufin"
OutFile "${RUFIN_OUTPUT_DIR}/Rufin-${RUFIN_VERSION}-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\Rufin"
InstallDirRegKey HKCU "Software\Rufin" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
Icon "${RUFIN_ASSET_DIR}/rufin.ico"
UninstallIcon "${RUFIN_ASSET_DIR}/rufin.ico"

VIProductVersion "${RUFIN_VERSION_QUAD}"
VIAddVersionKey /LANG=1033 "ProductName" "Rufin"
VIAddVersionKey /LANG=1033 "CompanyName" "screwy"
VIAddVersionKey /LANG=1033 "FileDescription" "Rufin installer"
VIAddVersionKey /LANG=1033 "FileVersion" "${RUFIN_VERSION}"
VIAddVersionKey /LANG=1033 "ProductVersion" "${RUFIN_VERSION}"
VIAddVersionKey /LANG=1033 "LegalCopyright" "GPL-3.0-or-later"

!define MUI_ABORTWARNING
!define MUI_ICON "${RUFIN_ASSET_DIR}/rufin.ico"
!define MUI_UNICON "${RUFIN_ASSET_DIR}/rufin.ico"
!define MUI_WELCOMEPAGE_TITLE "Welcome to Rufin, GTK4/libadwaita music client for Jellyfin, Navidrome/OpenSubsonic and local libraries."
!define MUI_WELCOMEPAGE_TEXT " This will install Rufin on your computer."
!define MUI_FINISHPAGE_RUN "$INSTDIR\rufin.exe"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${RUFIN_STAGE_DIR}/LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "Rufin" RufinSection
    SectionIn RO
    SetOutPath "$INSTDIR"
    File /r "${RUFIN_STAGE_DIR}/*"
    WriteUninstaller "$INSTDIR\Uninstall.exe"

    WriteRegStr HKCU "Software\Rufin" "InstallDir" "$INSTDIR"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Rufin" \
        "DisplayName" "Rufin"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Rufin" \
        "DisplayVersion" "${RUFIN_VERSION}"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Rufin" \
        "DisplayIcon" "$INSTDIR\rufin.ico"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Rufin" \
        "Publisher" "screwy"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Rufin" \
        "URLInfoAbout" "https://github.com/screwys/Rufin"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Rufin" \
        "UninstallString" '$\"$INSTDIR\Uninstall.exe$\"'
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Rufin" \
        "QuietUninstallString" '$\"$INSTDIR\Uninstall.exe$\" /S'
    WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Rufin" \
        "NoModify" 1
    WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Rufin" \
        "NoRepair" 1

    CreateDirectory "$SMPROGRAMS\Rufin"
    CreateShortcut "$SMPROGRAMS\Rufin\Rufin.lnk" "$INSTDIR\rufin.exe" \
        "" "$INSTDIR\rufin.ico"
    CreateShortcut "$SMPROGRAMS\Rufin\Uninstall Rufin.lnk" "$INSTDIR\Uninstall.exe"
SectionEnd

Section /o "Desktop shortcut" DesktopSection
    CreateShortcut "$DESKTOP\Rufin.lnk" "$INSTDIR\rufin.exe" "" "$INSTDIR\rufin.ico"
SectionEnd

Section "Uninstall"
    Delete "$DESKTOP\Rufin.lnk"
    RMDir /r "$SMPROGRAMS\Rufin"
    RMDir /r "$INSTDIR"
    DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Rufin"
    DeleteRegKey HKCU "Software\Rufin"
SectionEnd
