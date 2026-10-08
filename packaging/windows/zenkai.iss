; Built by build.ps1, which passes the version and the folders on the command line:
;   iscc /DAppVersion=0.1.0 /DExeDir=..\..\target\dist /DOutputDir=..\..\dist zenkai.iss

#ifndef AppVersion
  #error Pass the version with /DAppVersion=x.y.z
#endif
#ifndef ExeDir
  #define ExeDir "..\..\target\dist"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
; Never change AppId: Windows uses it to match upgrades and the uninstaller.
AppId={{20C0FE1B-59E0-4985-8377-20D8CBD20083}
AppName=Zenkai
AppVersion={#AppVersion}
AppVerName=Zenkai {#AppVersion}
AppPublisher=Zenkai
DefaultDirName={autopf}\Zenkai
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
ChangesAssociations=yes
SetupIconFile=..\..\crates\app\zenkai.ico
UninstallDisplayIcon={app}\zenkai.exe
UninstallDisplayName=Zenkai
OutputDir={#OutputDir}
OutputBaseFilename=zenkai-{#AppVersion}-windows-x64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "openwith"; Description: "Add Zenkai to ""Open with"" for .xlsx and .csv files"; GroupDescription: "File types:"; Flags: unchecked

[Files]
Source: "{#ExeDir}\zenkai.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Zenkai"; Filename: "{app}\zenkai.exe"
Name: "{autodesktop}\Zenkai"; Filename: "{app}\zenkai.exe"; Tasks: desktopicon

; Only adds Zenkai to the "Open with" list. The default value of .xlsx and .csv is never
; written, so the program that opens them by default does not change.
[Registry]
Root: HKA; Subkey: "Software\Classes\Zenkai.xlsx"; ValueType: string; ValueName: ""; ValueData: "Excel Workbook (Zenkai)"; Flags: uninsdeletekey; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Zenkai.xlsx\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\zenkai.exe"",0"; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Zenkai.xlsx\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\zenkai.exe"" ""%1"""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.xlsx\OpenWithProgids"; ValueType: string; ValueName: "Zenkai.xlsx"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Zenkai.csv"; ValueType: string; ValueName: ""; ValueData: "CSV File (Zenkai)"; Flags: uninsdeletekey; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Zenkai.csv\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\zenkai.exe"",0"; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Zenkai.csv\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\zenkai.exe"" ""%1"""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.csv\OpenWithProgids"; ValueType: string; ValueName: "Zenkai.csv"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith

[Run]
Filename: "{app}\zenkai.exe"; Description: "{cm:LaunchProgram,Zenkai}"; Flags: nowait postinstall skipifsilent
