#ifndef RUFIN_STAGE_DIR
#define RUFIN_STAGE_DIR "..\..\dist\windows\Rufin"
#endif

#ifndef RUFIN_OUTPUT_DIR
#define RUFIN_OUTPUT_DIR "..\..\dist"
#endif

#ifndef RUFIN_VERSION
#define RUFIN_VERSION "0.0.0"
#endif

[Setup]
AppId={{3F541AF6-38BC-4F74-98AF-9EB3C3629BB1}
AppName=Rufin
AppVersion={#RUFIN_VERSION}
AppPublisher=screwy
AppPublisherURL=https://github.com/screwys/Rufin
AppSupportURL=https://github.com/screwys/Rufin/issues
AppUpdatesURL=https://github.com/screwys/Rufin/releases
DefaultDirName={localappdata}\Programs\Rufin
DefaultGroupName=Rufin
DisableProgramGroupPage=yes
LicenseFile={#RUFIN_STAGE_DIR}\LICENSE
OutputDir={#RUFIN_OUTPUT_DIR}
OutputBaseFilename=Rufin-{#RUFIN_VERSION}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
SetupIconFile=assets\rufin.ico
WizardSmallImageFile=assets\rufin-wizard-small.png
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest
UninstallDisplayIcon={app}\rufin.ico
UninstallDisplayName=Rufin

[Messages]
WelcomeLabel1=Welcome to Rufin, fully native music client, written in Rust!
WelcomeLabel2= This will install Rufin on your computer.

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#RUFIN_STAGE_DIR}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\Rufin"; Filename: "{app}\rufin.exe"; WorkingDir: "{app}"; IconFilename: "{app}\rufin.ico"
Name: "{autodesktop}\Rufin"; Filename: "{app}\rufin.exe"; WorkingDir: "{app}"; IconFilename: "{app}\rufin.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\rufin.exe"; Description: "{cm:LaunchProgram,Rufin}"; Flags: nowait postinstall skipifsilent
