; Build with scripts/build-windows-installer.py. Do not version the AppId.
; Per-user registration offers a choice; Windows alone owns the user's default.
#ifndef PackageDir
  #error Supply /DPackageDir with the verified native Windows package directory.
#endif
#ifndef AppVersion
  #error Supply /DAppVersion with the package BUILD.json version.
#endif
#ifndef InstallerOutputDir
  #define InstallerOutputDir "..\..\target\installers"
#endif
#ifndef InstallerBaseName
  #define InstallerBaseName "voice-over-book-reader-" + AppVersion + "-windows-x64-setup"
#endif
#if !FileExists(PackageDir + "\voice-over-book-reader.exe")
  #error PackageDir must contain voice-over-book-reader.exe.
#endif
#if !FileExists(PackageDir + "\reader-dictionary-popup.exe")
  #error PackageDir must contain reader-dictionary-popup.exe.
#endif
#if !FileExists(PackageDir + "\reader-voiceroid-bridge.exe")
  #error PackageDir must contain the x86 reader-voiceroid-bridge.exe.
#endif
#if !FileExists(PackageDir + "\licenses\LICENSE-MIT")
  #error PackageDir must contain the package licenses directory.
#endif

#define AppName "Voice Over Book Reader"
#define AppExe "voice-over-book-reader.exe"
#define AppProgId "VoiceOverBookReader.Epub"
#define CapabilitiesKey "Software\VoiceOverBookReader\Capabilities"

[Setup]
AppId={{64EAA344-75DC-4DE6-9B49-2E6308BC4146}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Voice Over Book Reader contributors
VersionInfoVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
UsePreviousAppDir=yes
UsePreviousGroup=yes
UsePreviousTasks=yes
ChangesAssociations=yes
SetupIconFile=assets\voice-over-book-reader.ico
UninstallDisplayIcon={app}\voice-over-book-reader.ico
OutputDir={#InstallerOutputDir}
OutputBaseFilename={#InstallerBaseName}
Compression=lzma2/normal
SolidCompression=yes
WizardStyle=modern
CloseApplications=no
RestartApplications=no
Uninstallable=yes
UninstallLogging=yes

[Languages]
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
japanese.AssociationGroup=既定のアプリは Windows の設定で選べます:
japanese.RegisterEpub=EPUB を開くアプリの候補に追加する
japanese.OpenDefaultApps=Windows の既定アプリ設定を開く（EPUB の設定）
japanese.LaunchApp=Voice Over Book Reader を起動する
japanese.UninstallApp=Voice Over Book Reader をアンインストール
japanese.EpubDescription=EPUB 書籍
japanese.AppDescription=日本語の縦書きと読み上げに対応した EPUB リーダー
english.AssociationGroup=Choose the default app later in Windows Settings:
english.RegisterEpub=Add Voice Over Book Reader to the apps that can open EPUB files
english.OpenDefaultApps=Open Windows Default apps settings to choose an EPUB reader
english.LaunchApp=Launch Voice Over Book Reader
english.UninstallApp=Uninstall Voice Over Book Reader
english.EpubDescription=EPUB book
english.AppDescription=EPUB reader with Japanese vertical text and text-to-speech

[Tasks]
Name: "registerepub"; Description: "{cm:RegisterEpub}"; GroupDescription: "{cm:AssociationGroup}"

[Files]
; Copy the complete audited package, including all three processes and notices.
; No app data, models, Calibre library, or imported books live under {app}.
Source: "{#PackageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "assets\voice-over-book-reader.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"; WorkingDir: "{app}"; IconFilename: "{app}\voice-over-book-reader.ico"; AppUserModelID: "jp.liesegang.voice-over-book-reader"
Name: "{group}\{cm:UninstallApp}"; Filename: "{uninstallexe}"; IconFilename: "{app}\voice-over-book-reader.ico"

[Registry]
; Every key is per-user. Never write .epub's default value or Explorer UserChoice.
; Shared key: remove only our named OpenWithProgids value on uninstall.
Root: HKCU; Subkey: "Software\Classes\.epub\OpenWithProgids"; ValueType: string; ValueName: "{#AppProgId}"; ValueData: ""; Flags: uninsdeletevalue uninsdeletekeyifempty; Tasks: registerepub

; Private ProgID: deleting this tree cannot affect another reader's registration.
Root: HKCU; Subkey: "Software\Classes\{#AppProgId}"; ValueType: string; ValueName: ""; ValueData: "{cm:EpubDescription}"; Flags: uninsdeletekey; Tasks: registerepub
Root: HKCU; Subkey: "Software\Classes\{#AppProgId}"; ValueType: string; ValueName: "FriendlyTypeName"; ValueData: "{cm:EpubDescription}"; Tasks: registerepub
Root: HKCU; Subkey: "Software\Classes\{#AppProgId}\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\voice-over-book-reader.ico"",0"; Tasks: registerepub
Root: HKCU; Subkey: "Software\Classes\{#AppProgId}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --reader ""%1"""; Tasks: registerepub

; Open with also consults the Applications entry and its SupportedTypes list.
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "{#AppName}"; Flags: uninsdeletekey; Tasks: registerepub
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "{cm:AppDescription}"; Tasks: registerepub
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".epub"; ValueData: ""; Tasks: registerepub
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\voice-over-book-reader.ico"",0"; Tasks: registerepub
Root: HKCU; Subkey: "Software\Classes\Applications\{#AppExe}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --reader ""%1"""; Tasks: registerepub

; The current-user RegisteredApplications value points Windows Settings here.
Root: HKCU; Subkey: "{#CapabilitiesKey}"; ValueType: string; ValueName: "ApplicationName"; ValueData: "{#AppName}"; Flags: uninsdeletekey; Tasks: registerepub
Root: HKCU; Subkey: "{#CapabilitiesKey}"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "{cm:AppDescription}"; Tasks: registerepub
Root: HKCU; Subkey: "{#CapabilitiesKey}"; ValueType: string; ValueName: "ApplicationIcon"; ValueData: """{app}\voice-over-book-reader.ico"",0"; Tasks: registerepub
Root: HKCU; Subkey: "{#CapabilitiesKey}\FileAssociations"; ValueType: string; ValueName: ".epub"; ValueData: "{#AppProgId}"; Tasks: registerepub
Root: HKCU; Subkey: "{#CapabilitiesKey}\MimeAssociations"; ValueType: string; ValueName: "application/epub+zip"; ValueData: "{#AppProgId}"; Tasks: registerepub
Root: HKCU; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "{#AppName}"; ValueData: "{#CapabilitiesKey}"; Flags: uninsdeletevalue; Tasks: registerepub

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchApp}"; WorkingDir: "{app}"; Flags: nowait postinstall skipifsilent
Filename: "{code:DefaultAppsUri}"; Description: "{cm:OpenDefaultApps}"; Flags: shellexec nowait postinstall skipifsilent unchecked; Check: IsEpubRegistrationSelected

[Code]
function IsEpubRegistrationSelected: Boolean;
begin
  Result := WizardIsTaskSelected('registerepub');
end;

function DefaultAppsUri(Param: String): String;
var
  Version: TWindowsVersion;
  UpdateBuild: Cardinal;
  SupportsAppPage: Boolean;
begin
  Result := 'ms-settings:defaultapps';
  GetWindowsVersionEx(Version);
  SupportsAppPage := Version.Build >= 22631;
  if not SupportsAppPage and
    RegQueryDWordValue(HKLM,
      'SOFTWARE\Microsoft\Windows NT\CurrentVersion', 'UBR', UpdateBuild) then
  begin
    SupportsAppPage :=
      ((Version.Build = 22000) and (UpdateBuild >= 1817)) or
      ((Version.Build = 22621) and (UpdateBuild >= 1555));
  end;
  if SupportsAppPage then
    Result := Result + '?registeredAppUser=Voice%20Over%20Book%20Reader';
end;

// Deliberately no UninstallDelete section: uninstall removes installed package
// files and our registry entries, while user books, settings and caches survive.
