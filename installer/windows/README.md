# Windows installer source

`voice-over-book-reader.iss` packages a verified native Windows release. It requires Inno Setup 6.3 or newer.

With Python 3.12 or newer, run the wrapper from the repository root, for example:

```powershell
python scripts/build-windows-installer.py --package target/packages/voice-over-book-reader-0.2.4-x86_64-pc-windows-msvc.zip --iscc 'C:\Program Files (x86)\Inno Setup 6\ISCC.exe' --output-dir target/installers
python scripts/test-windows-installer.py
```

`--package` also accepts the extracted package directory. Select the compiler with `--iscc`, `ISCC` / `INNO_SETUP_ISCC`, or `PATH`. The wrapper also searches standard Inno Setup 6 install locations. The wrapper checks the Windows architecture and all three native executables. It also checks required documentation and licenses. ZIP extraction rejects path traversal, Windows path aliases, links and duplicate paths. Publishing the installer and checksum refuses existing output files. The input ZIP or extracted package is unchanged.

Prefer the original release ZIP. A folder used to run the app can contain WebView profiles or other user data. Validation rejects unknown files and extra top-level directories. This keeps user data out of the installer. Update the allowed package file list when adding release documents or notices.

The wrapper supplies these Inno Setup preprocessor definitions:

### `PackageDir`

Absolute directory containing the complete extracted native package.

### `AppVersion`

Package version from `BUILD.json`, such as `0.2.1`.

### `InstallerOutputDir`

Absolute output directory; defaults to `target/installers`.

### `InstallerBaseName`

Filename without `.exe`. The default is:

```text
voice-over-book-reader-VERSION-windows-x64-setup
```

The AppId `64EAA344-75DC-4DE6-9B49-2E6308BC4146` is permanent across releases. The installer uses the previous installation directory and task choices when upgrading. It does not close running apps automatically; close the installed reader before upgrading.

Installation is per-user, without elevation or machine-wide writes. Its default directory is:

```text
%LOCALAPPDATA%\Programs\Voice Over Book Reader
```

The x64 reader, x64 dictionary helper, x86 DLL host, and notices stay together. The original icon is added to the installation. The Start menu contains the reader and an uninstall shortcut.

The visible `registerepub` task registers the app as an EPUB candidate for the current user. It uses these entries:

- `OpenWithProgids`
- `Applications`
- `Capabilities`
- `RegisteredApplications`

 Its private ProgID is `VoiceOverBookReader.Epub`. Both open commands use:

```text
"INSTALL_DIR\voice-over-book-reader.exe" --reader "%1"
```

Extension defaults and Explorer's `UserChoice` stay unchanged. Other applications' registrations also stay unchanged. The optional finish-page action opens Windows Default apps so the user can choose. Supported Windows 11 versions open this app's settings page. Other versions open the Default apps list. Silent installs neither launch the reader nor open Settings.

Uninstall removes installed files and this app's own registration. Under `.epub\OpenWithProgids`, it removes only `VoiceOverBookReader.Epub`. It removes that shared key only if empty. Uninstalling preserves application data, model caches, Calibre files, and books.

The blue book/speaker icon and its SVG source are original MIT-licensed project assets. The following command regenerates nine PNG-backed ICO sizes, from 16 to 256 px. It needs no third-party dependencies.

```text
python installer/windows/assets/generate_icon.py
```

Primary references:

- [Inno Setup: PrivilegesRequired](https://jrsoftware.org/ishelp/topic_setup_privilegesrequired.htm)
- [Inno Setup: Registry entries and uninstall flags](https://jrsoftware.org/ishelp/topic_registrysection.htm)
- [Inno Setup: ChangesAssociations](https://jrsoftware.org/ishelp/topic_setup_changesassociations.htm)
- [Microsoft: Register file types and OpenWithProgids](https://learn.microsoft.com/en-us/windows/win32/shell/fa-file-types)
- [Microsoft: Application registration](https://learn.microsoft.com/en-us/windows/win32/shell/app-registration)
- [Microsoft: Launch the Default Apps settings page](https://learn.microsoft.com/en-us/windows/apps/develop/launch/launch-default-apps-settings)
