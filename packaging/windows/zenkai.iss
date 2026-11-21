; Built by package.ps1 from the folder it lays out, the same one it zips:
;   iscc /DAppVersion=0.1.0 /DLayoutDir=..\..\dist\zenkai-0.1.0-windows-x64 /DOutputDir=..\..\dist zenkai.iss

#ifndef AppVersion
  #error Pass the version with /DAppVersion=x.y.z
#endif
#ifndef LayoutDir
  #error Pass the laid out install folder with /DLayoutDir=path
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif
#ifndef OutputBaseName
  #define OutputBaseName "zenkai-" + AppVersion + "-windows-x64-setup"
#endif

[Setup]
; Never change AppId: Windows uses it to match upgrades and the uninstaller, and the app finds
; its uninstall entry by it to keep DisplayVersion current after an update.
AppId={{20C0FE1B-59E0-4985-8377-20D8CBD20083}
AppName=Zenkai
AppVersion={#AppVersion}
AppVerName=Zenkai {#AppVersion}
AppPublisher=Zenkai
; Per user only, in a folder the user owns: the updater swaps app\ without ever asking for
; elevation, which a Program Files install would need.
DefaultDirName={localappdata}\Programs\Zenkai
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
ChangesAssociations=yes
SetupIconFile=..\..\crates\app\zenkai.ico
UninstallDisplayIcon={app}\zenkai.exe
UninstallDisplayName=Zenkai
OutputDir={#OutputDir}
OutputBaseFilename={#OutputBaseName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "openwith"; Description: "Add Zenkai to ""Open with"" for .xlsx, .csv, Markdown and PDF files"; GroupDescription: "File types:"; Flags: unchecked

; A reinstall starts from a clean slate: what an update left behind goes before the files land.
[InstallDelete]
Type: filesandordirs; Name: "{app}\app"
Type: filesandordirs; Name: "{app}\app.old"
Type: filesandordirs; Name: "{app}\.update"

[Files]
Source: "{#LayoutDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

; Everything points at the launcher at the root, which an update never moves.
[Icons]
Name: "{autoprograms}\Zenkai"; Filename: "{app}\zenkai.exe"; AppUserModelID: "EmilioGiordano.Zenkai"
Name: "{autodesktop}\Zenkai"; Filename: "{app}\zenkai.exe"; AppUserModelID: "EmilioGiordano.Zenkai"; Tasks: desktopicon

; Only adds Zenkai to the "Open with" list. The default value of .xlsx, .csv, .md and .markdown is never
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
Root: HKA; Subkey: "Software\Classes\Zenkai.md"; ValueType: string; ValueName: ""; ValueData: "Markdown File (Zenkai)"; Flags: uninsdeletekey; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Zenkai.md\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\zenkai.exe"",0"; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Zenkai.md\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\zenkai.exe"" ""%1"""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.md\OpenWithProgids"; ValueType: string; ValueName: "Zenkai.md"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.markdown\OpenWithProgids"; ValueType: string; ValueName: "Zenkai.md"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Zenkai.pdf"; ValueType: string; ValueName: ""; ValueData: "PDF Document (Zenkai)"; Flags: uninsdeletekey; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Zenkai.pdf\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\zenkai.exe"",0"; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Zenkai.pdf\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\zenkai.exe"" ""%1"""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: string; ValueName: "Zenkai.pdf"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith

[Run]
Filename: "{app}\zenkai.exe"; Description: "{cm:LaunchProgram,Zenkai}"; Flags: nowait postinstall skipifsilent

; An update replaces app\ with files the installer never wrote, and leaves app.old\ and .update\
; (its marker and lock included). %LOCALAPPDATA%\Zenkai, with the recovery files, is kept.
[UninstallDelete]
Type: filesandordirs; Name: "{app}\app"
Type: filesandordirs; Name: "{app}\app.old"
Type: filesandordirs; Name: "{app}\.update"
