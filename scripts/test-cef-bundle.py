#!/usr/bin/env python3
"""CEF packaging fixtures: no Chromium process, network, GPU or models."""
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import struct
import stat
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import cef_bundle as cef

spec = importlib.util.spec_from_file_location("package_fixtures", Path(__file__).with_name("ci-test-package.py"))
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)


def client_dll(export=b"RunWinMain"):
    data = bytearray(1024)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 60, 64)
    data[64:68] = b"PE\0\0"
    struct.pack_into("<HH", data, 68, 0x8664, 1)
    struct.pack_into("<HH", data, 84, 240, 0x2002)
    struct.pack_into("<H", data, 88, 0x20B)
    struct.pack_into("<II", data, 200, 0x1000, 128)
    struct.pack_into("<4I", data, 336, 512, 0x1000, 512, 512)
    struct.pack_into("<I", data, 536, 1)
    struct.pack_into("<I", data, 544, 0x1080)
    struct.pack_into("<I", data, 640, 0x1090)
    data[656:656 + len(export) + 1] = export + b"\0"
    return bytes(data)


def executable(machine=0x8664, magic=0x20B):
    data = bytearray(client_dll())
    struct.pack_into("<H", data, 68, machine)
    struct.pack_into("<H", data, 86, 0x2)
    struct.pack_into("<H", data, 88, magic)
    return bytes(data)


class CefPackageTests(unittest.TestCase):
    def fixture(self, root, target):
        metadata = base.PackageTests.fixture(self, root)
        metadata["packages"][0]["version"] = "0.3.0"
        for name in ("CEF-LICENSE.txt", "NOTICE.txt", "welding-MPL-2.0.txt", "SOURCES.json"):
            path = root / "licenses/cef" / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("Original fixture notice")
        (root / "docs/chromium-runtime.md").write_text("Bundled Chromium fixture")
        (root / "installer/windows").mkdir(parents=True)
        (root / "installer/windows/cef-bootstrap.exe.manifest").write_text("<assembly/>")
        release = root / "target" / target / "release"
        release.mkdir(parents=True)
        extension = ".exe" if "windows" in target else ""
        for name in ("voice-over-book-reader" + extension, "reader-dictionary-popup" + extension, cef.HELPER):
            path = release / name
            path.write_bytes(b"native fixture only")
            path.chmod(0o755)
        (release / "voice_over_book_reader.dll").write_bytes(client_dll())
        if "windows" in target:
            (release / "reader-dictionary-popup.exe").write_bytes(executable())
            bridge = base.PackageTests.bridge(self, root)
            bridge.write_bytes(executable(0x14C, 0x10B))
        sdk = root / "sdk"
        sdk.mkdir()
        pin = cef.PINS["targets"][target]
        (sdk / "archive.json").write_text(json.dumps({"type": "minimal", "name": f"cef_binary_{cef.PINS['cef']}_{pin['platform']}_minimal.tar.bz2", "sha1": pin["sha1"]}))
        (sdk / "CREDITS.html").write_text("Original Chromium credits fixture")
        if "apple" in target:
            framework = sdk / cef.FRAMEWORK
            (framework / "Libraries").mkdir(parents=True)
            (framework / "Libraries/libEGL.dylib").write_bytes(b"dylib fixture")
            (framework / "Chromium Embedded Framework").write_bytes(b"framework fixture")
        else:
            for name in cef.COMMON + (cef.WINDOWS + ("bootstrap.exe",) if "windows" in target else cef.LINUX):
                data = client_dll(b"CEFExport") if name.endswith(".dll") else executable() if name == "bootstrap.exe" else b"sdk fixture: " + name.encode()
                (sdk / name).write_bytes(data)
            (sdk / "locales").mkdir()
            for locale in ("en-US.pak", "ja.pak"): (sdk / "locales" / locale).write_bytes(b"locale fixture")
        (sdk / "private-profile.json").write_text("Must not be shipped")
        (sdk / "developer.pdb").write_bytes(b"Must not be shipped")
        return metadata, release, sdk

    def test_windows_uses_bootstrap_and_matching_exported_client(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            metadata, release, sdk = self.fixture(root, "x86_64-pc-windows-msvc")
            # A normal Rust executable is neither needed nor shipped as the entry.
            (release / "voice-over-book-reader.exe").unlink()
            with patch.object(base.packager, "cargo_metadata", return_value=metadata):
                package = base.packager.package(root, "x86_64-pc-windows-msvc", root / "packages", sdk)
            with zipfile.ZipFile(package) as archive:
                prefix = package.stem + "/"
                cef.validate_archive(archive, prefix, "x86_64-pc-windows-msvc")
                self.assertEqual(archive.read(prefix + "voice-over-book-reader.exe"), (sdk / "bootstrap.exe").read_bytes())
                self.assertEqual(archive.read(prefix + "voice-over-book-reader.dll"), client_dll())
                self.assertIn(prefix + "reader-voiceroid-bridge.exe", archive.namelist())
                self.assertIn(prefix + "locales/ja.pak", archive.namelist())
                self.assertFalse(any("private-profile" in n or n.endswith(".pdb") for n in archive.namelist()))
                engine = json.loads(archive.read(prefix + "CEF-MANIFEST.json"))
                self.assertEqual(engine["sandbox"], "enabled")
                self.assertIs(engine["runtime_download_required"], False)

    def test_short_external_cargo_target_keeps_client_and_x86_bridge(self):
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            root = work / "checkout-with-a-long-name"
            root.mkdir()
            metadata, _, sdk = self.fixture(root, "x86_64-pc-windows-msvc")
            short = work / "t"
            # Both source and destination are isolated children of this fixture.
            self.assertEqual(short.resolve().parent, work.resolve())
            (root / "target").rename(short)
            metadata["target_directory"] = str(short)
            self.assertFalse((root / "target/x86_64-pc-windows-msvc/release").exists())
            with patch.object(base.packager, "cargo_metadata", return_value=metadata):
                package = base.packager.package(root, "x86_64-pc-windows-msvc", root / "packages", sdk)
            with zipfile.ZipFile(package) as archive:
                prefix = package.stem + "/"
                cef.validate_archive(archive, prefix, "x86_64-pc-windows-msvc")
                self.assertEqual(archive.read(prefix + "voice-over-book-reader.dll"), client_dll())
                bridge = short / "i686-pc-windows-msvc/release/reader-voiceroid-bridge.exe"
                self.assertEqual(archive.read(prefix + bridge.name), bridge.read_bytes())
                self.assertEqual(package.parent, root / "packages")

    def test_missing_or_wrong_sdk_and_missing_export_fail_closed(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            metadata, release, sdk = self.fixture(root, "x86_64-pc-windows-msvc")
            with patch.object(base.packager, "cargo_metadata", return_value=metadata):
                with self.assertRaises(FileNotFoundError):
                    base.packager.package(root, "x86_64-pc-windows-msvc", root / "missing", root / "unknown-sdk")
                (release / "voice_over_book_reader.dll").write_bytes(client_dll(b"WrongEntry"))
                with self.assertRaisesRegex(ValueError, "RunWinMain"):
                    base.packager.package(root, "x86_64-pc-windows-msvc", root / "no-entry", sdk)
                (sdk / "archive.json").write_text('{}')
                with self.assertRaisesRegex(ValueError, "pinned"):
                    base.packager.package(root, "x86_64-pc-windows-msvc", root / "wrong-sdk", sdk)

    def test_linux_contains_helper_sandbox_and_hashes_every_runtime(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            metadata, release, sdk = self.fixture(root, "x86_64-unknown-linux-gnu")
            with patch.object(base.packager, "cargo_metadata", return_value=metadata):
                package = base.packager.package(root, "x86_64-unknown-linux-gnu", root / "packages", sdk)
            with zipfile.ZipFile(package) as archive:
                cef.validate_archive(archive, package.stem + "/", "x86_64-unknown-linux-gnu")
                archive.extractall(root / "verify")
            installed = root / "verify" / package.stem
            names = cef.validate_manifest(installed)
            self.assertIn("libcef.so", names)
            self.assertIn(cef.HELPER, names)
            self.assertIn("chrome-sandbox", names)
            self.assertFalse((installed / "chrome-sandbox").stat().st_mode & 0o6000)
            if os.name != "nt":
                with zipfile.ZipFile(package) as archive:
                    self.assertEqual(archive.getinfo(package.stem + "/chrome-sandbox").external_attr >> 16 & 0o7777, 0o755)
            (installed / "libcef.so").write_bytes(b"corrupt runtime")
            with self.assertRaisesRegex(ValueError, "hash differs"):
                cef.validate_manifest(installed)

    def test_mac_has_all_helpers_and_signs_framework_before_outer_bundle(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            metadata, release, sdk = self.fixture(root, "aarch64-apple-darwin")
            calls = []
            with patch.object(base.packager, "cargo_metadata", return_value=metadata), patch.object(cef.subprocess, "run", side_effect=lambda command, check: calls.append(command)):
                package = base.packager.package(root, "aarch64-apple-darwin", root / "packages", sdk)
            with zipfile.ZipFile(package) as archive:
                cef.validate_archive(archive, package.stem + "/", "aarch64-apple-darwin")
                for suffix in cef.MAC_HELPERS:
                    name = "voice-over-book-reader " + suffix
                    prefix = package.stem + "/Voice Over Book Reader.app/Contents/Frameworks/" + name + ".app/Contents/"
                    self.assertIn(prefix + "MacOS/" + name, archive.namelist())
                    self.assertIs(plistlib.loads(archive.read(prefix + "Info.plist"))["LSUIElement"], True)
            targets = [Path(call[-1]) for call in calls]
            framework_index = next(i for i, path in enumerate(targets) if path.name == cef.FRAMEWORK)
            outer_index = next(i for i, path in enumerate(targets) if path.name == "Voice Over Book Reader.app")
            self.assertLess(framework_index, outer_index)
            self.assertEqual(len([p for p in targets[:outer_index] if p.suffix == ".app"]), 5)
            self.assertEqual(calls[-1][:4], ["codesign", "--verify", "--deep", "--strict"])

    def test_cef_windows_package_is_accepted_by_installer_and_tampering_is_rejected(self):
        spec = importlib.util.spec_from_file_location("cef_installer_test", Path(__file__).with_name("build-windows-installer.py"))
        installer = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(installer)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            metadata, release, sdk = self.fixture(root, "x86_64-pc-windows-msvc")
            # The generic fixture omits some legacy documentation; copy the exact
            # already-tested required text contract without copying settings/data.
            for relative in installer.REQUIRED_TEXT:
                path = root / relative
                if relative.startswith("licenses/"):
                    source = relative.removeprefix("licenses/")
                    if source.startswith(("kokoro/", "aivis-native/")):
                        path = root / "crates/reader-speech/licenses" / source
                    elif source.startswith("fonts/"):
                        path = root / "assets/fonts" / Path(source).name
                    elif source in {"Sonic-LICENSE.txt", "pyvcroid2-LICENSE.txt", "Apache-2.0.txt"}:
                        continue
                elif relative != "README.md":
                    continue
                if not path.exists():
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text("Required original fixture notice")
            # Root LICENSE/COPYING patterns are copied by the production packager.
            for name in ("LICENSE-MIT", "COPYING-IPADIC.txt", "COPYING-THIRD-PARTY.txt"):
                (root / name).write_text("Original root fixture notice")
            with patch.object(base.packager, "cargo_metadata", return_value=metadata):
                package = base.packager.package(root, "x86_64-pc-windows-msvc", root / "packages", sdk)
            with installer.package_directory(package) as extracted:
                self.assertEqual(installer.validate_package(extracted), "0.3.0")
                (extracted / "libcef.dll").write_bytes(b"changed")
                with self.assertRaisesRegex(ValueError, "hash differs"):
                    installer.validate_package(extracted)

    def test_mac_framework_internal_symlink_is_preserved_in_zip(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            metadata, release, sdk = self.fixture(root, "x86_64-apple-darwin")
            versions = sdk / cef.FRAMEWORK / "Versions"
            (versions / "A/Resources").mkdir(parents=True)
            (versions / "A/Resources/fixture.pak").write_bytes(b"resource")
            try: (versions / "Current").symlink_to("A", target_is_directory=True)
            except OSError: self.skipTest("Host does not permit creating a symlink; macOS/Linux CI exercise this contract")
            with patch.object(base.packager, "cargo_metadata", return_value=metadata), patch.object(cef.subprocess, "run"):
                package = base.packager.package(root, "x86_64-apple-darwin", root / "packages", sdk)
            with zipfile.ZipFile(package) as archive:
                name = package.stem + "/Voice Over Book Reader.app/Contents/Frameworks/" + cef.FRAMEWORK + "/Versions/Current"
                self.assertTrue(stat.S_ISLNK(archive.getinfo(name).external_attr >> 16))
                self.assertEqual(archive.read(name), b"A")
                cef.validate_archive(archive, package.stem + "/", "x86_64-apple-darwin")

    def test_shipping_gate_rejects_a_separately_installed_vc_runtime_dependency(self):
        data = bytearray(client_dll())
        struct.pack_into("<I", data, 208, 0x1100)
        struct.pack_into("<I", data, 780, 0x1140)
        name = b"VCRUNTIME140.dll\0"
        data[832:832+len(name)] = name
        with tempfile.TemporaryDirectory() as temporary:
            image = Path(temporary) / "client.dll"
            image.write_bytes(data)
            self.assertIn("vcruntime140.dll", cef.pe_imported_names(image))
            with self.assertRaisesRegex(ValueError, "crt-static"):
                cef.require_static_crt([image])


if __name__ == "__main__": unittest.main()
