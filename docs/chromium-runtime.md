# Bundled Chromium book engine

Voice Over Book Reader 0.3 uses Chromium Embedded Framework for the book document.
The library, menus, settings, speech controls, and window remain native Rust/egui.
CEF renders author XHTML/CSS/SVG and fonts in the fixed reading surface.
It handles Japanese vertical text and ruby. Native controls overlay that surface.

Every portable package and Windows installer includes CEF 154.0.34.
Its required libraries, locales and resource packs are included too.
The installed application does not download Chromium.
Speech models and optional speech runtimes have separate terms and installation.

The Windows executable is the matching official CEF sandbox bootstrap. It loads
the same-named client DLL and passes the original sandbox context to RunWinMain.
Keep the complete directory together. The dictionary and x86 VOICEROID2 helper
are separate native processes.

The macOS application contains the CEF framework and five standard CEF Helper
applications. Nested libraries, the framework, helpers and outer application are
signed inside out. CI verifies the complete bundle with codesign --deep --strict.
Ad-hoc signing is not Apple notarization.

Linux includes libcef, its helper executable and chrome-sandbox.
The application keeps Chromium sandboxing enabled.
Kernel/user-namespace policy still applies.
Some systems require a root-owned setuid sandbox helper.
That needs administrator setup according to the CEF distribution instructions.
No --no-sandbox launcher or automatic privilege change is supplied.

CEF-MANIFEST.json identifies the pinned SDK archive and engine version.
It records sandbox mode, relative runtime paths and SHA-256 hashes.
licenses/cef contains the original CEF BSD license and complete Chromium credits.
It also includes the MPL-2.0 welding license.
The manifest links the exact public commit for modified MPL-covered source.

Use scripts/prepare-cef-sdk.py for build-time SDK preparation.
scripts/cef-runtime-pins.json fixes official CDN archive names, sizes and hashes.
The preparer sets CEF_PATH when passed --github-env.
Rust 1.97.1, CMake, Ninja and a platform C/C++ toolchain are required.
Packaging excludes browser profiles and SDK source directories.
It also excludes debug symbols, books and speech weights.

Ubuntu 24.04 needs GTK 3 and WebKitGTK 4.1 (for dictionaries).
It also needs ALSA, udev, xkbcommon, Wayland/X11, Vulkan, NSS, NSPR and GBM.
The apt development/runtime package list is in `.github/workflows/ci.yml`.
On Windows, use the MSVC target and set `LIBCLANG_PATH` to the LLVM `bin`
directory.
Shipping images use `RUSTFLAGS=-C target-feature=+crt-static`.
Always specify `--target` for that build.
Validation rejects own clients/helpers requiring separate VCRUNTIME/MSVCP libraries.
CEF generates long C++ object names. On Windows, set `CARGO_TARGET_DIR` to a
short absolute path before building. CI uses `RUNNER_TEMP/vobr-target`.
The packager reads the build directory from Cargo metadata.
It uses that directory for the app and the i686 VOICEROID2 helper.
Final ZIP/installer output paths are independent of the Cargo build directory.

For example, prepare a Windows SDK with:

```sh
python scripts/prepare-cef-sdk.py --target x86_64-pc-windows-msvc --directory .cef-sdk
```

Set `CEF_PATH` to the absolute path of `.cef-sdk/cef_windows_x86_64`.
Then follow the build and package commands in the root README.
Other target SDK directories are listed in `scripts/cef-runtime-pins.json`.
Binding generation needs SDK headers. A used application bundle lacks them.

CI runs builds and unit/fixture checks for Windows x64, Linux x64 and both Macs.
Passing builds do not certify untested GPUs, GUIs, audio devices or TTS backends.
Windows x64 was checked in the actual native application with accelerated CEF.
See the [GUI validation scope](epub-support.md#windowsで確認した範囲).
The initial integration also exercised SAPI speech highlights and page following.
These checks do not certify full EPUB conformance or other speech backends.
macOS and Linux GUI/audio operation remains untested on real devices.
