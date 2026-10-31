#!/usr/bin/env python3
"""Pinned SDK extraction contracts, without network or real SDK downloads."""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("prepare_cef", Path(__file__).with_name("prepare-cef-sdk.py"))
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)
TARGET = "x86_64-pc-windows-msvc"


class SdkTests(unittest.TestCase):
    def archive(self, directory, escape=False):
        pin = dict(prepare.PINS["targets"][TARGET])
        stem = f"cef_binary_{prepare.PINS['cef']}_{pin['platform']}_minimal"
        archive = directory / (stem + ".tar.bz2")
        with tarfile.open(archive, "w:bz2") as output:
            for name in ("Release", "Resources", "cmake", "include", "libcef_dll"):
                info = tarfile.TarInfo(stem + "/" + name)
                info.type = tarfile.DIRTYPE
                output.addfile(info)
            for name in ("Release/libcef.dll", "Resources/resources.pak", "CMakeLists.txt", "CREDITS.html", "LICENSE.txt"):
                data = ("Unmodified fixture: " + name).encode()
                info = tarfile.TarInfo(stem + "/" + name)
                info.size = len(data)
                output.addfile(info, io.BytesIO(data))
            if escape:
                info = tarfile.TarInfo("../outside")
                info.size = 1
                output.addfile(info, io.BytesIO(b"x"))
        pin.update({"bytes": archive.stat().st_size, "sha1": hashlib.sha1(archive.read_bytes()).hexdigest()})
        return pin

    def test_fixed_archive_is_verified_flattened_and_keeps_original_legal_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            pin = self.archive(directory)
            with patch.dict(prepare.PINS["targets"], {TARGET: pin}), patch.object(prepare.urllib.request, "urlopen", side_effect=AssertionError("No network")):
                sdk = prepare.prepare(TARGET, directory)
                self.assertTrue((sdk / "resources.pak").is_file())
                self.assertTrue((sdk / "include").is_dir())
                self.assertEqual((sdk / "LICENSE.txt").read_text(), "Unmodified fixture: LICENSE.txt")
                self.assertEqual(json.loads((sdk / "archive.json").read_text())["sha1"], pin["sha1"])
                self.assertEqual(prepare.prepare(TARGET, directory), sdk)

    def test_wrong_archive_hash_and_traversal_are_rejected_before_sdk_publication(self):
        for escape in (False, True):
            with self.subTest(escape=escape), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                pin = self.archive(directory, escape)
                if not escape: pin["sha1"] = "0" * 40
                with patch.dict(prepare.PINS["targets"], {TARGET: pin}):
                    with self.assertRaises((ValueError, tarfile.FilterError)):
                        prepare.prepare(TARGET, directory)
                self.assertFalse((directory / pin["directory"]).exists())
                self.assertFalse((directory.parent / "outside").exists())


if __name__ == "__main__": unittest.main()
