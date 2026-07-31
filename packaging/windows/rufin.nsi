!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "Win\COM.nsh"
!include "Win\Propkey.nsh"

!ifndef RUFIN_STAGE_DIR
!define RUFIN_STAGE_DIR "..\..\dist\windows\Rufin"
!endif

!ifndef RUFIN_STAGE_FILES
!define RUFIN_STAGE_FILES "${RUFIN_STAGE_DIR}\*"
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
!define MUI_WELCOMEPAGE_TITLE "Welcome to Rufin"
!define MUI_WELCOMEPAGE_TEXT " This will install Rufin on your computer."
!define MUI_FINISHPAGE_RUN "$INSTDIR\bin\rufin.exe"
!define RUFIN_APP_ID "io.github.screwys.Rufin"

Var LegacyInstallDir
Var LegacyInstallOwned

!macro CreateRufinShortcut SHORTCUT_PATH TARGET_PATH ICON_PATH
    !insertmacro ComHlpr_CreateInProcInstance ${CLSID_ShellLink} ${IID_IShellLink} r0 ""
    ${If} $0 P<> 0
        ${IShellLink::SetPath} $0 '("${TARGET_PATH}").r1'
        ${IShellLink::SetWorkingDirectory} $0 '("$INSTDIR").r2'
        ${IShellLink::SetIconLocation} $0 '("${ICON_PATH}", 0).r3'
        ${If} $1 = 0
        ${AndIf} $2 = 0
        ${AndIf} $3 = 0
            ${IUnknown::QueryInterface} $0 '("${IID_IPropertyStore}",.r1)'
            ${If} $1 P<> 0
                System::Call "oleaut32::SysAllocString(w '${RUFIN_APP_ID}') p .r4"
                System::Call '*${SYSSTRUCT_PROPERTYKEY}(${PKEY_AppUserModel_ID})p.r2'
                System::Call '*${SYSSTRUCT_PROPVARIANT}(${VT_BSTR},, p r4)p.r3'
                ${IPropertyStore::SetValue} $1 '($2, $3)'
                ${IPropertyStore::Commit} $1 ""
                System::Call "oleaut32::SysFreeString(p r4)"
                System::Free $2
                System::Free $3
                ${IUnknown::Release} $1 ""
            ${EndIf}
            ${IUnknown::QueryInterface} $0 '("${IID_IPersistFile}",.r1)'
            ${If} $1 P<> 0
                ${IPersistFile::Save} $1 '("${SHORTCUT_PATH}", 1)'
                ${IUnknown::Release} $1 ""
            ${EndIf}
        ${EndIf}
        ${IUnknown::Release} $0 ""
    ${EndIf}
!macroend

!macro RequireRufinClosed EXECUTABLE LABEL
    IfFileExists "${EXECUTABLE}" 0 ${LABEL}_not_running
    Delete "${EXECUTABLE}.rufin-install"
    ClearErrors
    Rename "${EXECUTABLE}" "${EXECUTABLE}.rufin-install"
    IfErrors runtime_is_running
    Rename "${EXECUTABLE}.rufin-install" "${EXECUTABLE}"
    IfErrors runtime_is_running

${LABEL}_not_running:
!macroend

Function .onInit
    StrCpy $INSTDIR "$LOCALAPPDATA\Programs\Rufin"
    StrCpy $LegacyInstallOwned 0
    ReadRegStr $LegacyInstallDir HKCU "Software\Rufin" "InstallDir"
    StrCmp $LegacyInstallDir "" legacy_install_done
    GetFullPathName $LegacyInstallDir "$LegacyInstallDir"
    GetFullPathName $INSTDIR "$INSTDIR"
    StrCmp $LegacyInstallDir $INSTDIR legacy_install_done
    IfFileExists "$LegacyInstallDir\Uninstall.exe" 0 legacy_install_done
    IfFileExists "$LegacyInstallDir\rufin.ico" 0 legacy_install_done
    IfFileExists "$LegacyInstallDir\bin\rufin.exe" legacy_install_owned
    IfFileExists "$LegacyInstallDir\rufin.exe" 0 legacy_install_done

legacy_install_owned:
    StrCpy $LegacyInstallOwned 1

legacy_install_done:
FunctionEnd

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${RUFIN_STAGE_DIR}/LICENSE"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "Rufin" RufinSection
    SectionIn RO
    !insertmacro RequireRufinClosed "$INSTDIR\bin\rufin.exe" current_bin
    !insertmacro RequireRufinClosed "$INSTDIR\rufin.exe" current_root
    StrCmp $LegacyInstallOwned 1 0 runtime_not_running
    !insertmacro RequireRufinClosed "$LegacyInstallDir\bin\rufin.exe" legacy_bin
    !insertmacro RequireRufinClosed "$LegacyInstallDir\rufin.exe" legacy_root
    Goto runtime_not_running

runtime_is_running:
    IfSilent runtime_silent_abort runtime_show_running

runtime_show_running:
    MessageBox MB_OK|MB_ICONEXCLAMATION "Close Rufin and try the installation again."

runtime_silent_abort:
    SetErrorLevel 2
    Abort

runtime_not_running:
    RMDir /r "$INSTDIR\bin"
    RMDir /r "$INSTDIR\etc"
    RMDir /r "$INSTDIR\lib"
    RMDir /r "$INSTDIR\libexec"
    RMDir /r "$INSTDIR\share"
    Delete /REBOOTOK "$INSTDIR\rufin.exe"
    Delete /REBOOTOK "$INSTDIR\*.dll"
    Delete /REBOOTOK "$INSTDIR\gspawn-win64-helper.exe"
    Delete /REBOOTOK "$INSTDIR\gspawn-win64-helper-console.exe"
    SetOutPath "$INSTDIR"
    File /r "${RUFIN_STAGE_FILES}"
    WriteUninstaller "$INSTDIR\Uninstall.exe"

    StrCmp $LegacyInstallOwned 1 0 legacy_install_removed
    RMDir /r "$LegacyInstallDir\bin"
    RMDir /r "$LegacyInstallDir\etc"
    RMDir /r "$LegacyInstallDir\lib"
    RMDir /r "$LegacyInstallDir\libexec"
    RMDir /r "$LegacyInstallDir\share"
    Delete "$LegacyInstallDir\rufin.exe"
    Delete "$LegacyInstallDir\*.dll"
    Delete "$LegacyInstallDir\gspawn-win64-helper.exe"
    Delete "$LegacyInstallDir\gspawn-win64-helper-console.exe"
    Delete "$LegacyInstallDir\LICENSE"
    Delete "$LegacyInstallDir\rufin.ico"
    Delete "$LegacyInstallDir\Uninstall.exe"
    RMDir "$LegacyInstallDir"

legacy_install_removed:
    DeleteRegKey HKCU "Software\Rufin"

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
    !insertmacro CreateRufinShortcut \
        "$SMPROGRAMS\Rufin\Rufin.lnk" \
        "$INSTDIR\bin\rufin.exe" \
        "$INSTDIR\rufin.ico"
    CreateShortcut "$SMPROGRAMS\Rufin\Uninstall Rufin.lnk" "$INSTDIR\Uninstall.exe"
    IfFileExists "$DESKTOP\Rufin.lnk" create_existing_desktop no_existing_desktop

create_existing_desktop:
    !insertmacro CreateRufinShortcut \
        "$DESKTOP\Rufin.lnk" \
        "$INSTDIR\bin\rufin.exe" \
        "$INSTDIR\rufin.ico"

no_existing_desktop:
SectionEnd

Section /o "Desktop shortcut" DesktopSection
    !insertmacro CreateRufinShortcut \
        "$DESKTOP\Rufin.lnk" \
        "$INSTDIR\bin\rufin.exe" \
        "$INSTDIR\rufin.ico"
SectionEnd

Section "Uninstall"
    Delete "$DESKTOP\Rufin.lnk"
    RMDir /r "$SMPROGRAMS\Rufin"
    RMDir /r "$INSTDIR\bin"
    RMDir /r "$INSTDIR\etc"
    RMDir /r "$INSTDIR\lib"
    RMDir /r "$INSTDIR\libexec"
    RMDir /r "$INSTDIR\share"
    Delete "$INSTDIR\LICENSE"
    Delete "$INSTDIR\rufin.ico"
    Delete "$INSTDIR\Uninstall.exe"
    RMDir "$INSTDIR"
    DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Rufin"
    DeleteRegKey HKCU "Software\Rufin"
SectionEnd
