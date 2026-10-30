#!/usr/bin/env python3
"""Packaging fixture tests; no real release, models, network, or GUI required."""
import hashlib
import importlib.util
import json
import plistlib
import struct
import subprocess
from pathlib import Path, PurePosixPath
import tempfile
import unittest
from unittest.mock import patch
import zipfile

spec = importlib.util.spec_from_file_location("ci_package", Path(__file__).with_name("ci-package.py"))
packager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packager)

class AdHocSigningContract:
    """Model TN2206 nested-code sealing; this does not implement Mach-O signing."""

    marker = b"fixture-ad-hoc-signature\n"

    def __init__(self, corrupt_before_verify=False):
        self.corrupt_before_verify = corrupt_before_verify
        self.verified = False
        self.seals = {}

    def sign_file(self, path):
        path.write_bytes(self.marker + path.read_bytes().removeprefix(self.marker))

    def run(self, command, check):
        if command[0] != "codesign" or not check:
            raise AssertionError("This fixture only handles checked codesign operations")
        target = Path(command[-1])
        app = target if target.suffix == ".app" else next(
            parent for parent in target.parents if parent.suffix == ".app"
        )
        with (app / "Contents/Info.plist").open("rb") as file:
            main_name = plistlib.load(file)["CFBundleExecutable"]
        executable_dir = app / "Contents/MacOS"
        main = executable_dir / main_name
        helpers = [path for path in executable_dir.iterdir() if path.is_file() and path != main]

        if "--verify" in command:
            if "--deep" not in command or "--strict" not in command:
                raise AssertionError("Nested-code verification must be recursive and strict")
            if self.corrupt_before_verify:
                helpers[0].write_bytes(helpers[0].read_bytes() + b"modified after sealing")
            actual = {path.name: hashlib.sha256(path.read_bytes()).hexdigest()
                      for path in executable_dir.iterdir() if path.is_file()}
            if self.seals.get(app) != actual:
                raise subprocess.CalledProcessError(1, command, stderr="invalid nested-code seal")
            self.verified = True
        elif target == app or target == main:
            # Signing CFBundleExecutable inside its .app has the outer bundle's
            # nesting contract too. This is where the original Intel CI failed.
            unsigned = [path for path in helpers if not path.read_bytes().startswith(self.marker)]
            if unsigned and "--deep" not in command:
                raise subprocess.CalledProcessError(1, command, stderr="unsigned nested helper")
            for helper in unsigned:
                self.sign_file(helper)
            self.sign_file(main)
            self.seals[app] = {
                path.name: hashlib.sha256(path.read_bytes()).hexdigest()
                for path in executable_dir.iterdir() if path.is_file()
            }
            signature_dir = app / "Contents/_CodeSignature"
            signature_dir.mkdir(exist_ok=True)
            (signature_dir / "CodeResources").write_text(json.dumps(self.seals[app]), encoding="utf-8")
        else:
            self.sign_file(target)
        return subprocess.CompletedProcess(command, 0)


class PackageTests(unittest.TestCase):
    def fixture(self, root: Path) -> dict:
        for relative, content in {
            "assets/fonts/LICENSE-NotoSans.txt": "Font license fixture",
            "crates/reader-speech/vendor/sonic/LICENSE": "Sonic license fixture",
            "crates/reader-voiceroid-bridge/licenses/pyvcroid2-LICENSE": "pyvcroid2 MIT fixture",
            "licenses/Apache-2.0.txt": "Third-party Apache fixture",
            "crates/reader-speech/licenses/kokoro/NOTICE.txt": "Kokoro notice fixture\n",
            "crates/reader-speech/licenses/kokoro/NVIDIA-CUDA-cuBLAS-LICENSE.txt": "Original NVIDIA terms fixture\n",
            "crates/reader-speech/licenses/kokoro/NVIDIA-cuDNN-ACKNOWLEDGEMENTS.html": "<p>Original acknowledgement fixture</p>\n",
            "crates/reader-speech/licenses/kokoro/SOURCES.json": "[]\n",
            "crates/reader-speech/licenses/aivis-native/JPreprocess-LICENSE.txt": "Original BSD fixture\n",
            "crates/reader-speech/licenses/aivis-native/CC-BY-SA-4.0.txt": "Original BERT license fixture\n",
            "crates/reader-speech/licenses/aivis-native/SOURCES.json": "{}\n",
            "docs/speech.md": "Speech documentation fixture",
            "docs/dictionaries.md": "Dictionary documentation fixture",
            "docs/layout.md": "Layout documentation fixture",
            "docs/epub-support.md": "EPUB support documentation fixture",
            "docs/license-audit.md": "[Aivis](license-audit-aivis-voicevox.md#engines)\n[Speech](speech.md)\n[MIT](../LICENSE-MIT)\n[Upstream](https://example.invalid/license-audit.md)\n[1]: license-audit-kokoro.md#kokoro\n",
            "docs/license-audit-aivis-voicevox.md": "[Audit](license-audit.md)\n",
            "docs/license-audit-kokoro.md": "[Snapshots](../crates/reader-speech/licenses/kokoro/)\n[Notice](../crates/reader-speech/licenses/kokoro/NOTICE.txt)\n[Source](../crates/reader-speech/src/native.rs)\n",
            "dependency/Cargo.toml": "[package]\nname='fixture'\nversion='1.0.0'\n",
            "dependency/LICENSE": "Dependency full license fixture",
        }.items():
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")
        return {
            "workspace_members": ["voice-over-book-reader-id"], "target_directory": str(root / "target"),
            "packages": [
                {"id": "voice-over-book-reader-id", "name": "voice-over-book-reader", "version": "0.2.1", "manifest_path": str(root / "Cargo.toml")},
                {"id": "dependency-id", "name": "fixture", "version": "1.0.0", "license": "MIT",
                 "repository": "https://example.invalid/fixture", "manifest_path": str(root / "dependency/Cargo.toml")},
            ],
            "resolve": {"nodes": [{"id": "voice-over-book-reader-id"}, {"id": "dependency-id"}]},
        }

    def bridge(self, root: Path, machine=0x14c):
        path = root / "target/i686-pc-windows-msvc/release/reader-voiceroid-bridge.exe"
        path.parent.mkdir(parents=True, exist_ok=True)
        data = bytearray(70)
        data[:2] = b"MZ"
        struct.pack_into("<I", data, 60, 64)
        data[64:68] = b"PE\0\0"
        struct.pack_into("<H", data, 68, machine)
        path.write_bytes(data)
        return path

    def test_windows_zip_has_three_processes_notices_and_correct_sha256(self):
        with tempfile.TemporaryDirectory(prefix="voice-over-book-reader-package-test-") as temporary:
            root = Path(temporary)
            metadata = self.fixture(root)
            packager.cargo_metadata = lambda *_: metadata
            release = root / "target/x86_64-pc-windows-msvc/release"
            release.mkdir(parents=True)
            for name in ("voice-over-book-reader.exe", "reader-dictionary-popup.exe"):
                (release / name).write_bytes(b"packaging fixture only")
            self.bridge(root)
            path = packager.package(root, "x86_64-pc-windows-msvc", root / "packages")
            with zipfile.ZipFile(path) as archive:
                names = archive.namelist()
                self.assertTrue(any(name.endswith("/voice-over-book-reader.exe") for name in names))
                self.assertTrue(any(name.endswith("/reader-dictionary-popup.exe") for name in names))
                self.assertTrue(any(name.endswith("/reader-voiceroid-bridge.exe") for name in names))
                self.assertTrue(any(name.endswith("/licenses/pyvcroid2-LICENSE.txt") for name in names))
                prefix = names[0].split("/", 1)[0] + "/"
                for provider in ("kokoro", "aivis-native"):
                    source_notices = root / "crates/reader-speech/licenses" / provider
                    for original in source_notices.rglob("*"):
                        if original.is_file():
                            self.assertEqual(archive.read(prefix + "licenses/" + provider + "/" + original.relative_to(source_notices).as_posix()), original.read_bytes())
                notice = next(name for name in names if name.endswith("THIRD-PARTY-NOTICES.txt"))
                self.assertIn(b"Dependency full license fixture", archive.read(notice))
                self.assertIn(b"https://crates.io/api/v1/crates/fixture/1.0.0/download", archive.read(notice))
                self.assertIn(b"Covered source, including MPL-2.0", archive.read(notice))
                build = next(name for name in names if name.endswith("BUILD.json"))
                self.assertFalse(json.loads(archive.read(build))["native_smoke_tests_run"])
            self.assertEqual(path.with_suffix(".zip.sha256").read_text().split()[0], hashlib.sha256(path.read_bytes()).hexdigest())

    def test_missing_helper_fails_instead_of_shipping_partial_reader(self):
        with tempfile.TemporaryDirectory(prefix="voice-over-book-reader-package-test-") as temporary:
            root = Path(temporary)
            metadata = self.fixture(root)
            packager.cargo_metadata = lambda *_: metadata
            release = root / "target/x86_64-unknown-linux-gnu/release"
            release.mkdir(parents=True)
            (release / "voice-over-book-reader").write_bytes(b"fixture only")
            with self.assertRaises(FileNotFoundError):
                packager.package(root, "x86_64-unknown-linux-gnu", root / "packages")

    def test_mac_bundles_seal_unsigned_intel_and_already_signed_arm_helpers(self):
        for target, already_signed in [
            ("x86_64-apple-darwin", False),
            ("aarch64-apple-darwin", True),
        ]:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                metadata = self.fixture(root)
                packager.cargo_metadata = lambda *_: metadata
                release = root / "target" / target / "release"
                release.mkdir(parents=True)
                originals = {}
                for name in ("voice-over-book-reader", "reader-dictionary-popup"):
                    data = b"original compiled fixture " + name.encode()
                    if already_signed:
                        data = AdHocSigningContract.marker + data
                    (release / name).write_bytes(data)
                    originals[name] = data
                contract = AdHocSigningContract()
                with patch.object(packager.subprocess, "run", side_effect=contract.run):
                    archive_path = packager.package(root, target, root / "packages")
                self.assertTrue(contract.verified)
                with zipfile.ZipFile(archive_path) as archive:
                    prefix = archive.namelist()[0].split("/", 1)[0] + "/Voice Over Book Reader.app/Contents/"
                    seal = json.loads(archive.read(prefix + "_CodeSignature/CodeResources"))
                    self.assertEqual(set(seal), set(originals))
                    for name, original in originals.items():
                        signed = archive.read(prefix + "MacOS/" + name)
                        self.assertTrue(signed.startswith(contract.marker))
                        self.assertEqual(hashlib.sha256(signed).hexdigest(), seal[name])
                        self.assertEqual((release / name).read_bytes(), original)
                self.assertEqual(archive_path.with_suffix(".zip.sha256").read_text().split()[0],
                                 hashlib.sha256(archive_path.read_bytes()).hexdigest())

    def test_mac_signature_verification_failure_never_publishes_a_zip(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            metadata = self.fixture(root)
            packager.cargo_metadata = lambda *_: metadata
            target = "aarch64-apple-darwin"
            release = root / "target" / target / "release"
            release.mkdir(parents=True)
            for name in ("voice-over-book-reader", "reader-dictionary-popup"):
                (release / name).write_bytes(AdHocSigningContract.marker + b"fixture")
            contract = AdHocSigningContract(corrupt_before_verify=True)
            with patch.object(packager.subprocess, "run", side_effect=contract.run):
                with self.assertRaises(subprocess.CalledProcessError):
                    packager.package(root, target, root / "packages")
            self.assertEqual(list((root / "packages").iterdir()), [])

    def test_packaged_readme_links_resolve_without_changing_source_readme(self):
        with tempfile.TemporaryDirectory(prefix="voice-over-book-reader-package-test-") as temporary:
            root = Path(temporary)
            metadata = self.fixture(root)
            packager.cargo_metadata = lambda *_: metadata
            for relative, content in {
                "LICENSE-MIT": "MIT fixture",
                "COPYING-THIRD-PARTY.txt": "Third-party fixture",
                "COPYING-IPADIC.txt": "IPADIC fixture",
                ".github/workflows/ci.yml": "name: CI fixture\n",
                ".github/workflows/release.yml": "name: Release fixture\n",
                "README.ja.md": "[English](README.md)\n[音声](docs/speech.md)\n[ライセンス](LICENSE-MIT)\n[Release](.github/workflows/release.yml)\n",
                "README.md": (
                    "[Speech](docs/speech.md)\n[Engines](docs/speech.md#engines)\n"
                    "[Dictionaries](docs/dictionaries.md)\n"
                    "[Layout](docs/layout.md)\n"
                    "[Support](docs/epub-support.md)\n"
                    "[MIT](LICENSE-MIT)\n[Apache](licenses/Apache-2.0.txt)\n"
                    "[Notices](COPYING-THIRD-PARTY.txt)\n[IPADIC](COPYING-IPADIC.txt)\n"
                    "[CI](.github/workflows/ci.yml)\n[External](https://example.invalid/docs/speech.md)\n"
                ),
            }.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content, encoding="utf-8")
            original = (root / "README.md").read_bytes()
            release = root / "target/x86_64-pc-windows-msvc/release"
            release.mkdir(parents=True)
            for name in ("voice-over-book-reader.exe", "reader-dictionary-popup.exe"):
                (release / name).write_bytes(b"packaging fixture only")
            self.bridge(root)
            package = packager.package(root, "x86_64-pc-windows-msvc", root / "packages")
            with zipfile.ZipFile(package) as archive:
                names = archive.namelist()
                readme_name = next(name for name in names if name.endswith("/README.md"))
                readme = archive.read(readme_name).decode("utf-8")
                for target in (
                    "SPEECH.md", "SPEECH.md#engines", "DICTIONARIES.md", "LAYOUT.md", "EPUB-SUPPORT.md", "licenses/LICENSE-MIT", "licenses/Apache-2.0.txt",
                    "licenses/COPYING-THIRD-PARTY.txt", "licenses/COPYING-IPADIC.txt", "CI-WORKFLOW.yml",
                ):
                    self.assertIn(f"]({target})", readme)
                    self.assertIn(str(PurePosixPath(readme_name).parent / target.split("#", 1)[0]), names)
                self.assertIn("](https://example.invalid/docs/speech.md)", readme)
                japanese = archive.read(str(PurePosixPath(readme_name).parent / "README.ja.md")).decode("utf-8")
                self.assertIn("](SPEECH.md)", japanese)
                self.assertIn("](licenses/LICENSE-MIT)", japanese)
                self.assertIn("](RELEASE-WORKFLOW.yml)", japanese)
                self.assertIn(str(PurePosixPath(readme_name).parent / "RELEASE-WORKFLOW.yml"), names)
                audit_name = str(PurePosixPath(readme_name).parent / "LICENSE-AUDIT.md")
                audit = archive.read(audit_name).decode("utf-8")
                self.assertIn("](LICENSE-AUDIT-AIVIS-VOICEVOX.md#engines)", audit)
                self.assertIn("](SPEECH.md)", audit)
                self.assertIn("](licenses/LICENSE-MIT)", audit)
                self.assertIn("[1]: LICENSE-AUDIT-KOKORO.md#kokoro", audit)
                self.assertIn("](https://example.invalid/license-audit.md)", audit)
                kokoro = archive.read(str(PurePosixPath(readme_name).parent / "LICENSE-AUDIT-KOKORO.md")).decode("utf-8")
                self.assertIn("](licenses/kokoro)", kokoro)
                self.assertIn("](licenses/kokoro/NOTICE.txt)", kokoro)
                self.assertIn("](https://github.com/Liesegang/voice-over-book-reader/blob/v0.2.1/crates/reader-speech/src/native.rs)", kokoro)
            self.assertEqual((root / "README.md").read_bytes(), original)

    def test_target_cannot_escape_package_directory(self):
        with self.assertRaises(ValueError):
            packager.package(Path("."), "../../invalid", Path("packages"))

    def test_windows_rejects_missing_or_wrong_bitness_voiceroid_host(self):
        with tempfile.TemporaryDirectory(prefix="voiceroid-host-package-test-") as temporary:
            root = Path(temporary)
            metadata = self.fixture(root)
            packager.cargo_metadata = lambda *_: metadata
            with self.assertRaises(FileNotFoundError):
                packager.package(root, "x86_64-pc-windows-msvc", root / "packages")
            self.bridge(root, machine=0x8664)
            with self.assertRaises(ValueError):
                packager.package(root, "x86_64-pc-windows-msvc", root / "packages")


if __name__ == "__main__":
    unittest.main()
