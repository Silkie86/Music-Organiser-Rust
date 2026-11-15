; The Windows installer (docs/installer.md). Built with Inno Setup 6 by installer/build.ps1, which passes the defines
; below from product.toml and Cargo.toml, so the name, logo and version are never typed here.
;
; What it does: puts the app (one .exe) and the Bitwig extension it installs into Bitwig in the user's own Programs
; folder (Solomon, 2026-11-15: no Windows Yes, for the install or any update), adds Start menu and optional Desktop
; shortcuts and the Apps list entry, and asks where the data folder goes (Solomon, 2026-11-15). It never writes to a
; data folder's files, a DAW's folders, or anything else the user made: the app does all of that itself.
;
; Plain ASCII on purpose, like the other scripts.

#ifndef AppName
  #error Build this with installer/build.ps1, which gives it the app's name, version and files
#endif

[Setup]
; Windows knows the installed app by this id, never by its name (product.toml installer_id).
AppId={{{#AppId}}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppWebsite}
AppSupportURL={#AppWebsite}/issues
AppUpdatesURL={#AppWebsite}/releases
VersionInfoVersion={#AppVersionNumbers}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} setup
; The user's own Programs folder, no administrator rights (Solomon, 2026-11-15).
PrivilegesRequired=lowest
DefaultDirName={userpf}\{#AppName}
DisableProgramGroupPage=yes
UsePreviousAppDir=yes
SetupIconFile={#LogoIco}
UninstallDisplayIcon={app}\{#ExeName}
UninstallDisplayName={#AppName}
WizardStyle=modern
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}
Compression=lzma2/max
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
; A running app or helper is closed (they keep everything safe on close: F12 drafts, settings) and not restarted by
; Windows: the last page, or an update, starts the app again itself.
CloseApplications=force
RestartApplications=no
SetupLogging=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#ExeFile}"; DestDir: "{app}"; DestName: "{#ExeName}"; Flags: ignoreversion
#ifdef PdbFile
; Beside the app, so a crash log can say where in the code it happened (rule 17).
Source: "{#PdbFile}"; DestDir: "{app}"; DestName: "music_organiser.pdb"; Flags: ignoreversion
#endif
#ifdef BitwigExtension
; Beside the app, where Settings > DAWs looks for it (mo_dawlink::scripts::bitwig_extension_source).
Source: "{#BitwigExtension}"; DestDir: "{app}\extensions"; Flags: ignoreversion
#endif

[Icons]
Name: "{userprograms}\{#AppName}"; Filename: "{app}\{#ExeName}"; Comment: "{#AppDescription}"
Name: "{userdesktop}\{#AppName}"; Filename: "{app}\{#ExeName}"; Comment: "{#AppDescription}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#ExeName}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
; An update the app started itself (it passes /UPDATED=1) opens the new version once it's in place.
Filename: "{app}\{#ExeName}"; Flags: nowait; Check: StartAfterUpdate

[UninstallDelete]
; Only what the installer put in: the extensions folder (if the app left nothing else in it) and the app's folder.
Type: dirifempty; Name: "{app}\extensions"
Type: dirifempty; Name: "{app}"

[Code]
const
  // mo_app::paths and mo_app::install_choice: keep these the same.
  DataFolderName = 'MusicOrganiser';
  PointerFileName = 'MusicOrganiser-folder.txt';
  StartedHereFileName = 'started-here.txt';
  MoveAskedFileName = 'MusicOrganiser-move-asked.txt';
  MovedInFileName = 'moved-in.json';
  // mo_app::helper_start: the helper's line in the user's startup list.
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  HelperEntry = 'MusicOrganiserHelper';

var
  DataPage: TInputDirWizardPage;
  CurrentData: String;
  HasData: Boolean;
  // The pointer names a folder that isn't there now (its drive unplugged, say): nothing about the data is changed.
  DataMissing: Boolean;

function FirstDataDir: String;
begin
  Result := ExpandConstant('{localappdata}\') + DataFolderName;
end;

function PointerFile: String;
begin
  Result := ExpandConstant('{localappdata}\') + PointerFileName;
end;

function MoveAskedFile: String;
begin
  Result := ExpandConstant('{localappdata}\') + MoveAskedFileName;
end;

// The folder the pointer names, or '' (no pointer, or nothing in it). The app writes it as UTF-8; a folder name
// outside plain ASCII may read wrongly here, which only changes what the page shows (the app judges an ask itself).
function PointerNames: String;
var
  Text: AnsiString;
begin
  Result := '';
  if FileExists(PointerFile) and LoadStringFromFile(PointerFile, Text) then
    Result := Trim(String(Text));
end;

// Whether the first folder holds anything of the app's: any file or folder but the launchers' own folders.
function FirstFolderUsed: Boolean;
var
  Found: TFindRec;
begin
  Result := False;
  if FindFirst(FirstDataDir + '\*', Found) then
  try
    repeat
      if (Found.Name <> '.') and (Found.Name <> '..') and (CompareText(Found.Name, 'run-latest-rust') <> 0) and
         (CompareText(Found.Name, 'run-latest') <> 0) then
      begin
        Result := True;
        Exit;
      end;
    until not FindNext(Found);
  finally
    FindClose(Found);
  end;
end;

// Where the data is now, as the app judges it (mo_app::paths::resolve_in): the pointer's folder when a move finished
// there or the installer started it there, else the first folder. Sets HasData and DataMissing.
procedure LookAtData;
var
  Named: String;
begin
  CurrentData := FirstDataDir;
  DataMissing := False;
  Named := PointerNames;
  if Named <> '' then
  begin
    if FileExists(AddBackslash(Named) + MovedInFileName) or FileExists(AddBackslash(Named) + StartedHereFileName) then
      CurrentData := Named
    else
    begin
      CurrentData := Named;
      DataMissing := True;
    end;
  end;
  HasData := (Named <> '') or FirstFolderUsed;
end;

// The folder the data goes into for a pick, as Settings > Data does it (mo_app::data_move::target_in): a folder of
// the app's data name inside it, unless it's one already (so picking I:\ means I:\MusicOrganiser).
function TargetIn(Picked: String): String;
begin
  Picked := RemoveBackslashUnlessRoot(Trim(Picked));
  if CompareText(ExtractFileName(Picked), DataFolderName) = 0 then
    Result := Picked
  else
    Result := AddBackslash(Picked) + DataFolderName;
end;

function SameFolder(A, B: String): Boolean;
begin
  Result := CompareText(RemoveBackslashUnlessRoot(A), RemoveBackslashUnlessRoot(B)) = 0;
end;

procedure InitializeWizard;
var
  Words: String;
begin
  LookAtData;
  if DataMissing then
    Words := 'Your data is in ' + CurrentData + ', which can''t be found now (is its drive plugged in?). The ' +
      'setup leaves it as it is: once the drive is back, {#AppName} finds it again, and Settings can move it.'
  else if HasData then
    Words := 'Your data (songs, sounds, settings, notes) is in ' + CurrentData + '. It stays there unless you ' +
      'pick another place: then {#AppName} copies it there, checks every file, and switches at its next start. ' +
      'Nothing is removed from the old folder.'
  else
    Words := '{#AppName} keeps its databases, settings, notes and logs in one folder. A fast drive with room to ' +
      'spare is best. You can move it later in Settings > General.';
  DataPage := CreateInputDirPage(wpSelectDir, 'Where should your data go?', Words,
    'Data folder:', False, '');
  DataPage.Add('');
  DataPage.Values[0] := GetPreviousData('DataFolder', CurrentData);
  if HasData then
    DataPage.Values[0] := CurrentData;
  // For tests and a silent install: /DATADIR=<folder>.
  if ExpandConstant('{param:DATADIR|}') <> '' then
    DataPage.Values[0] := ExpandConstant('{param:DATADIR|}');
end;

function NextButtonClick(CurPageID: Integer): Boolean;
var
  Picked: String;
begin
  Result := True;
  if (CurPageID = DataPage.ID) then
  begin
    Picked := Trim(DataPage.Values[0]);
    if (Length(Picked) < 3) or (Copy(Picked, 2, 2) <> ':\') and (Copy(Picked, 1, 2) <> '\\') then
    begin
      MsgBox('Please pick a whole folder path, like I:\' + DataFolderName + '.', mbError, MB_OK);
      Result := False;
    end;
  end;
end;

procedure RegisterPreviousData(PreviousDataKey: Integer);
begin
  SetPreviousData(PreviousDataKey, 'DataFolder', DataPage.Values[0]);
end;

// One line, as UTF-8 (what the app reads; it skips the byte-order mark this may start with).
function SaveLine(FileName, Line: String): Boolean;
var
  Lines: TArrayOfString;
begin
  SetArrayLength(Lines, 1);
  Lines[0] := Line;
  Result := SaveStringsToUTF8File(FileName, Lines, False);
  if not Result then
    Log('Could not write ' + FileName);
end;

// Says what didn't work, on a message the user sees (and in the setup's log).
procedure Tell(Words: String);
begin
  Log(Words);
  SuppressibleMsgBox(Words, mbError, MB_OK, IDOK);
end;

// After the files are in: leave the app the note it needs for the data folder (mo_app::install_choice).
procedure CurStepChanged(CurStep: TSetupStep);
var
  Target: String;
begin
  if CurStep <> ssPostInstall then
    Exit;
  Target := TargetIn(DataPage.Values[0]);
  if DataMissing then
    Exit;
  if HasData then
  begin
    // Data here already: the app moves it itself, checked, if another place was picked.
    if not SameFolder(Target, CurrentData) and not SameFolder(DataPage.Values[0], CurrentData) then
      if not SaveLine(MoveAskedFile, Target) then
        Tell('The setup couldn''t leave {#AppName} the ask to move your data to ' + Target + '. Your data stays in ' +
          CurrentData + '; Settings > General can move it.');
  end
  else if not SameFolder(Target, FirstDataDir) then
  begin
    // A new PC: start the data in the picked folder. The pointer is written last, so a half-done step leaves the
    // app starting in its usual folder.
    if not (ForceDirectories(Target) and
            SaveLine(Target + '\' + StartedHereFileName, 'The {#AppName} installer started the app''s data in this folder.') and
            SaveLine(PointerFile, Target)) then
      Tell('The setup couldn''t start the data folder in ' + Target + ' (can this account write there?). ' +
        '{#AppName} keeps its data in ' + FirstDataDir + ' for now; Settings > General can move it.');
  end;
end;

// The app passes /UPDATED when it runs this setup to update itself.
function StartAfterUpdate: Boolean;
begin
  Result := WizardSilent and (ExpandConstant('{param:UPDATED|no}') <> 'no');
end;

// Uninstall: the helper's startup line goes only when it starts this copy of the app. Nothing of the data folder is
// touched.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Command: String;
begin
  if CurUninstallStep <> usUninstall then
    Exit;
  if RegQueryStringValue(HKCU, RunKey, HelperEntry, Command) then
    if Pos(Lowercase(ExpandConstant('{app}\{#ExeName}')), Lowercase(Command)) > 0 then
      RegDeleteValue(HKCU, RunKey, HelperEntry);
end;
