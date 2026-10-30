#!/usr/bin/env python3
"""Installer boundary tests; no real installer, registry writes, network, or GUI."""
import hashlib
import importlib.util
import json
from pathlib import Path
import stat
import struct
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import warnings
import zipfile

spec = importlib.util.spec_from_file_location("windows_installer", Path(__file__).with_name("build-windows-installer.py"))
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


def pe(machine=0x8664, magic=0x20B):
    data = bytearray(256)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 60, 64)
    data[64:68] = b"PE\0\0"
    struct.pack_into("<H", data, 68, machine)
    struct.pack_into("<HHH", data, 84, 2, 0x2, magic)
    return bytes(data)


class InstallerTests(unittest.TestCase):
    def fixture(self, root):
        root.mkdir(parents=True)
        metadata = {"application": "voice-over-book-reader", "version": "0.2.2",
                    "target": installer.TARGET, "native_processes": list(installer.PROCESSES)}
        (root / "BUILD.json").write_text(json.dumps(metadata), encoding="utf-8")
        for name, (machine, magic) in installer.PROCESSES.items():
            (root / name).write_bytes(pe(machine, magic))
        for relative in installer.REQUIRED_TEXT:
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("Japanese / 日本語 fixture license or documentation\n", encoding="utf-8")
        return root

    def archive(self, root, destination):
        with zipfile.ZipFile(destination, "w") as archive:
            for path in root.rglob("*"):
                if path.is_file():
                    archive.write(path, f"{root.name}/{path.relative_to(root).as_posix()}")
        return destination

    def test_real_package_contract_and_wrong_architectures(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = self.fixture(Path(temporary) / "package")
            self.assertEqual(installer.validate_package(root), "0.2.2")
            for name in installer.PROCESSES:
                with self.subTest(name=name):
                    old = (root / name).read_bytes()
                    (root / name).write_bytes(pe(0x14C, 0x10B) if "bridge" not in name else pe())
                    with self.assertRaisesRegex(ValueError, "architecture"):
                        installer.validate_package(root)
                    (root / name).write_bytes(old)

    def test_required_documents_and_metadata_are_not_optional(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = self.fixture(Path(temporary) / "package")
            (root / "licenses/fonts/LICENSE-NotoSerif.txt").unlink()
            with self.assertRaises(FileNotFoundError):
                installer.validate_package(root)
            (root / "licenses/fonts/LICENSE-NotoSerif.txt").write_text("license")
            for key, value in (("target", "aarch64-apple-darwin"), ("version", "../escape"),
                               ("native_processes", ["voice-over-book-reader.exe"]),
                               ("native_processes", ["voice-over-book-reader.exe"] * 3)):
                metadata_path = root / "BUILD.json"
                original = metadata_path.read_text()
                metadata = json.loads(original)
                metadata[key] = value
                metadata_path.write_text(json.dumps(metadata))
                with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                    installer.validate_package(root)
                metadata_path.write_text(original)

    def test_speech_original_notices_and_provenance_are_required(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = self.fixture(Path(temporary) / "package")
            for relative in (name for name in installer.REQUIRED_TEXT if name.startswith(("licenses/kokoro/", "licenses/aivis-native/"))):
                path = root / relative
                original = path.read_bytes()
                path.unlink()
                with self.subTest(relative=relative), self.assertRaises(FileNotFoundError):
                    installer.validate_package(root)
                path.write_bytes(original)

    def test_runtime_profiles_and_user_books_are_not_bundled_from_a_used_folder(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            for index, relative in enumerate(("reader-dictionary-popup.exe.WebView2/EBWebView/profile",
                                               "my-book.epub", "reader.sqlite3")):
                root = self.fixture(base / str(index))
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b"User data must never enter a distributable")
                with self.subTest(relative=relative), self.assertRaisesRegex(ValueError, "untouched release ZIP"):
                    installer.validate_package(root)

    def test_zip_nested_package_round_trip_keeps_notices(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            root = self.fixture(base / "日本語 package")
            archive = self.archive(root, base / "release.zip")
            before = hashlib.sha256(archive.read_bytes()).hexdigest()
            with installer.package_directory(archive) as extracted:
                self.assertEqual(installer.validate_package(extracted), "0.2.2")
                self.assertEqual((extracted / "licenses/Sonic-LICENSE.txt").read_bytes(),
                                 (root / "licenses/Sonic-LICENSE.txt").read_bytes())
                self.assertEqual((extracted / "licenses/kokoro/NVIDIA-CUDA-cuBLAS-LICENSE.txt").read_bytes(),
                                 (root / "licenses/kokoro/NVIDIA-CUDA-cuBLAS-LICENSE.txt").read_bytes())
                extracted_path = extracted
            self.assertFalse(extracted_path.exists())
            self.assertEqual(hashlib.sha256(archive.read_bytes()).hexdigest(), before)

    def test_zip_rejects_windows_path_escapes_before_extracting_any_file(self):
        bad_names = ("../escape", "/absolute", "C:/drive", "dir\\..\\escape", "dir/../escape",
                     "dir//file", "dir/./file", "book:stream", "file.", "folder /file", "CON.txt")
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            for index, name in enumerate(bad_names):
                archive_path = base / f"{index}.zip"
                destination = base / f"extract-{index}"
                destination.mkdir()
                with zipfile.ZipFile(archive_path, "w") as archive:
                    archive.writestr("safe.txt", "must not be extracted before validation finishes")
                    archive.writestr(name, "unsafe")
                with self.subTest(name=name), self.assertRaises(ValueError):
                    installer.extract_zip(archive_path, destination)
                self.assertEqual(list(destination.iterdir()), [])

    def test_zip_rejects_symlinks_duplicates_and_file_directory_conflicts(self):
        cases = (("book.txt", "BOOK.TXT"), ("same", "same"), ("folder", "folder/book"))
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            for index, names in enumerate(cases):
                archive_path = base / f"{index}.zip"
                with warnings.catch_warnings():
                    warnings.simplefilter("ignore", UserWarning)
                    with zipfile.ZipFile(archive_path, "w") as archive:
                        for name in names:
                            archive.writestr(name, "fixture")
                with self.subTest(names=names), self.assertRaises(ValueError):
                    installer.extract_zip(archive_path, base / f"extract-{index}")
            archive_path = base / "link.zip"
            with zipfile.ZipFile(archive_path, "w") as archive:
                entry = zipfile.ZipInfo("link")
                entry.create_system = 3
                entry.external_attr = (stat.S_IFLNK | 0o777) << 16
                archive.writestr(entry, "../outside")
            with self.assertRaisesRegex(ValueError, "special"):
                installer.extract_zip(archive_path, base / "extract-link")

    def test_installer_compile_publishes_sha_and_quotes_paths_without_a_shell(self):
        with tempfile.TemporaryDirectory(prefix="installer test 日本語 ") as temporary:
            base = Path(temporary)
            root = self.fixture(base / "native package")
            output_dir = base / "new installers"
            compiler = base / "Inno Setup 6/ISCC.exe"
            calls = []

            def compile_fixture(command, **kwargs):
                calls.append((command, kwargs))
                definitions = dict(item[2:].split("=", 1) for item in command if item.startswith("/D"))
                self.assertEqual(definitions["PackageDir"], str(root.resolve()))
                self.assertEqual(definitions["AppVersion"], "0.2.2")
                path = Path(definitions["InstallerOutputDir"]) / (definitions["InstallerBaseName"] + ".exe")
                path.write_bytes(pe(0x14C, 0x10B))

            with patch.object(installer.subprocess, "run", side_effect=compile_fixture):
                result = installer.build_installer(root, compiler, output_dir, base)
            self.assertEqual(calls[0][0][0], str(compiler))
            self.assertNotIn("shell", calls[0][1])
            self.assertTrue(calls[0][1]["check"])
            self.assertEqual(result.with_suffix(".exe.sha256").read_text().split()[0],
                             hashlib.sha256(result.read_bytes()).hexdigest())
            self.assertTrue((root / "BUILD.json").exists())
            with patch.object(installer.subprocess, "run") as compile_again:
                with self.assertRaises(FileExistsError):
                    installer.build_installer(root, compiler, output_dir, base)
                compile_again.assert_not_called()

    def test_failed_compile_or_existing_checksum_never_publishes_an_exe(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            root = self.fixture(base / "package")
            output_dir = base / "output"
            with patch.object(installer.subprocess, "run", side_effect=subprocess.CalledProcessError(2, ["ISCC"])):
                with self.assertRaises(subprocess.CalledProcessError):
                    installer.build_installer(root, base / "ISCC.exe", output_dir, base)
            self.assertEqual(list(output_dir.iterdir()), [])
            checksum = output_dir / "voice-over-book-reader-0.2.2-windows-x64-setup.exe.sha256"
            checksum.write_text("preserve me")
            with patch.object(installer.subprocess, "run") as compile_again:
                with self.assertRaises(FileExistsError):
                    installer.build_installer(root, base / "ISCC.exe", output_dir, base)
                compile_again.assert_not_called()
            self.assertEqual(checksum.read_text(), "preserve me")
            with self.assertRaises(ValueError):
                installer.build_installer(root, base / "ISCC.exe", root / "output", base)

    def test_compiler_explicit_environment_and_missing_path(self):
        with tempfile.TemporaryDirectory() as temporary:
            compiler = Path(temporary) / "ISCC.exe"
            compiler.write_bytes(b"fixture")
            with patch.dict(installer.os.environ, {"ISCC": str(compiler)}, clear=True):
                self.assertEqual(installer.find_iscc(), compiler.resolve())
                self.assertEqual(installer.find_iscc(compiler), compiler.resolve())
                with self.assertRaises(FileNotFoundError):
                    installer.find_iscc(Path(temporary) / "missing.exe")


if __name__ == "__main__":
    unittest.main()
