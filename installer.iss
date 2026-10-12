#define Name "Hisoka"

#define CargoToml SourcePath + "Cargo.toml"
#define Version ""
#define FH FileOpen(CargoToml)
#define Line ""

#sub ScanLine
  #expr Line = Trim(FileRead(FH))
  #if (Version == "") && (Copy(Line, 1, 7) == "version")
    #expr Version = Copy(Line, Pos('"', Line) + 1, RPos('"', Line) - Pos('"', Line) - 1)
  #endif
#endsub

#for { 0; !FileEof(FH) && Version == ""; 0 } ScanLine
#expr FileClose(FH)

#if Version == ""
  #error "Could not find version in Cargo.toml"
#endif

#define Publisher "NeyrowZ"
#define ServiceExe "hisoka-service.exe"
#define CliExe "hisoka.exe"

[Setup]
AppId={{8C426104-7029-4D98-9E4D-F5DA53B85143}
AppName={#Name}
AppVersion={#Version}
AppPublisher={#Publisher}
DefaultGroupName={#Name}
DisableProgramGroupPage=yes
DefaultDirName={autopf}\{#Name}
UninstallDisplayIcon={app}\bin\{#CliExe}
OutputDir=.\target\installer
OutputBaseFilename=hisoka-installer
SolidCompression=yes
WizardStyle=modern dynamic
PrivilegesRequired=admin
ChangesEnvironment=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
Source: ".\target\release\{#ServiceExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: ".\target\release\{#CliExe}"; DestDir: "{app}\bin"; Flags: ignoreversion

[Tasks]
Name: "autostart"; \
    Description: "Start the Hisoka service automatically with Windows"; \
    GroupDescription: "Service configuration:"

[UninstallDelete]
Type: filesandordirs; Name: "{app}"

[Code]

const
  EnvKey = 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';
  ERROR_SERVICE_DOES_NOT_EXIST = 1060;

{ ---------- sc.exe helpers ---------- }

{ Returns sc.exe's exit code, or -1 if it could not be launched. }
function ScExec(const Params: string): Integer;
var
  ResultCode: Integer;
begin
  if Exec(ExpandConstant('{sys}\sc.exe'), Params, '', SW_HIDE,
          ewWaitUntilTerminated, ResultCode) then
    Result := ResultCode
  else
    Result := -1;
end;

function IsServiceStopped(): Boolean;
var
  ResultCode: Integer;
begin
  Result := False;
  if ScExec('query "{#Name}"') = ERROR_SERVICE_DOES_NOT_EXIST then
  begin
    Result := True;
    Exit;
  end;
  if Exec(ExpandConstant('{sys}\cmd.exe'),
          '/c sc query "{#Name}" | find "STOPPED" >nul',
          '', SW_HIDE, ewWaitUntilTerminated, ResultCode) then
    Result := (ResultCode = 0);
end;

procedure StopServiceAndWait();
var
  I: Integer;
begin
  if ScExec('query "{#Name}"') = ERROR_SERVICE_DOES_NOT_EXIST then
    Exit;
  ScExec('stop "{#Name}"');
  for I := 1 to 30 do   { up to ~15 s }
  begin
    if IsServiceStopped() then Break;
    Sleep(500);
  end;
end;

{ ---------- PATH helpers ---------- }

function NeedsAddPath(const Dir: string): Boolean;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(HKEY_LOCAL_MACHINE, EnvKey, 'Path', OrigPath) then
  begin
    Result := True;
    Exit;
  end;
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(OrigPath) + ';') = 0;
end;

procedure AddToPath(const Dir: string);
var
  OrigPath: string;
begin
  if not NeedsAddPath(Dir) then Exit;
  if not RegQueryStringValue(HKEY_LOCAL_MACHINE, EnvKey, 'Path', OrigPath) then
    OrigPath := '';
  if (OrigPath <> '') and (OrigPath[Length(OrigPath)] <> ';') then
    OrigPath := OrigPath + ';';
  RegWriteExpandStringValue(HKEY_LOCAL_MACHINE, EnvKey, 'Path', OrigPath + Dir);
end;

procedure RemoveFromPath(const Dir: string);
var
  OrigPath: string;
  P: Integer;
begin
  if not RegQueryStringValue(HKEY_LOCAL_MACHINE, EnvKey, 'Path', OrigPath) then
    Exit;
  P := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(OrigPath) + ';');
  if P = 0 then Exit;

  if P = 1 then
    Delete(OrigPath, 1, Length(Dir) + 1)      { entry + trailing ';' }
  else
    Delete(OrigPath, P - 1, Length(Dir) + 1); { leading ';' + entry }

  RegWriteExpandStringValue(HKEY_LOCAL_MACHINE, EnvKey, 'Path', OrigPath);
end;

{ ---------- Install ---------- }

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  { Release the exe before it gets replaced (no-op on fresh install). }
  StopServiceAndWait();
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ServicePath, StartMode: string;
begin
  if CurStep <> ssPostInstall then Exit;

  AddToPath(ExpandConstant('{app}\bin'));

  ServicePath := ExpandConstant('{app}\{#ServiceExe}');

  if WizardIsTaskSelected('autostart') then
    StartMode := 'auto'
  else
    StartMode := 'demand';

  { Update the service if it exists, otherwise create it. }
  if ScExec('config "{#Name}" binPath= "' + ServicePath +
            '" start= ' + StartMode) <> 0 then
  begin
    if ScExec('create "{#Name}" binPath= "' + ServicePath +
              '" start= ' + StartMode) <> 0 then
    begin
      MsgBox('Could not register the Hisoka service.'#13#10 +
             'Please check the service executable and try again.',
             mbError, MB_OK);
      Exit;
    end;
  end;

  { Start it now, regardless of the autostart choice. }
  if ScExec('start "{#Name}"') <> 0 then
    MsgBox('The service was installed but could not be started.'#13#10 +
           'Use the Hisoka CLI to inspect the service.',
           mbInformation, MB_OK);
end;

{ ---------- Uninstall ---------- }

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  case CurUninstallStep of
    usUninstall:
      begin
        StopServiceAndWait();
        ScExec('delete "{#Name}"');
      end;
    usPostUninstall:
      RemoveFromPath(ExpandConstant('{app}\bin'));
  end;
end;