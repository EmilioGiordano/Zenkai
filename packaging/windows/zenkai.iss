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
; elevation, which a Program Files install would need. The folder is fixed, so neither a shared
; or protected folder nor one holding other files can be picked; per-user installs need no choice.
DefaultDirName={localappdata}\Programs\Zenkai
DisableDirPage=yes
DisableProgramGroupPage=yes
LicenseFile={#LayoutDir}\LICENSE
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

; A reinstall starts from a clean slate: what an update left behind goes before the files land,
; and only from a folder that already holds a Zenkai install. The check runs per entry and looks
; for app\zenkai.exe, so app\ goes last or the entries after it would find no install.
[InstallDelete]
Type: filesandordirs; Name: "{app}\app.old"; Check: IsZenkaiInstall
Type: filesandordirs; Name: "{app}\.update"; Check: IsZenkaiInstall
Type: filesandordirs; Name: "{app}\app"; Check: IsZenkaiInstall

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

[Code]
var
  RemoveUpdateFolders: Boolean;

// The launcher at the root and the app under app\ mark a folder as a Zenkai install; nothing is
// ever deleted wholesale from a folder without both.
function IsZenkaiInstall(): Boolean;
begin
  Result := FileExists(ExpandConstant('{app}\zenkai.exe')) and
    FileExists(ExpandConstant('{app}\app\zenkai.exe'));
end;

// Windows refuses to open a running program for writing. Any failure counts as running, so
// nothing is replaced or removed while Zenkai, its launcher or a helper may still run from here.
function IsRunning(const Path: String): Boolean;
var
  Stream: TFileStream;
begin
  Result := False;
  if not FileExists(Path) then
    Exit;
  try
    Stream := TFileStream.Create(Path, fmOpenReadWrite);
    Stream.Free;
  except
    Result := True;
  end;
end;

function ZenkaiRunning(): Boolean;
begin
  Result := IsRunning(ExpandConstant('{app}\zenkai.exe')) or
    IsRunning(ExpandConstant('{app}\app\zenkai.exe')) or
    IsRunning(ExpandConstant('{app}\app\zenkai-pdf-host.exe')) or
    IsRunning(ExpandConstant('{app}\app\zenkai-values-host.exe')) or
    IsRunning(ExpandConstant('{app}\app.old\zenkai.exe'));
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  if ZenkaiRunning() then
    Result := 'Zenkai is running. Close it, then run Setup again.';
end;

function InitializeUninstall(): Boolean;
begin
  Result := not ZenkaiRunning();
  if not Result then
    SuppressibleMsgBox('Zenkai is running. Close it, then uninstall again.', mbError, MB_OK, IDOK);
end;

// An update replaces app\ with files the installer never wrote, and leaves app.old\ and .update\
// (its marker and lock included). They go after the uninstaller removed its own files, but only
// if the folder was a Zenkai install before it started. %LOCALAPPDATA%\Zenkai is kept.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    RemoveUpdateFolders := IsZenkaiInstall();
  if (CurUninstallStep = usPostUninstall) and RemoveUpdateFolders then
  begin
    DelTree(ExpandConstant('{app}\app'), True, True, True);
    DelTree(ExpandConstant('{app}\app.old'), True, True, True);
    DelTree(ExpandConstant('{app}\.update'), True, True, True);
    RemoveDir(ExpandConstant('{app}'));
  end;
end;
