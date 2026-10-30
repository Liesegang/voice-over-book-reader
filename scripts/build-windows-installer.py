#!/usr/bin/env python3
"""Validate a native Windows package, compile its per-user installer, and write SHA-256."""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import struct
import subprocess
import tempfile
import zipfile

TARGET = "x86_64-pc-windows-msvc"
PROCESSES = {
    "voice-over-book-reader.exe": (0x8664, 0x20B),
    "reader-dictionary-popup.exe": (0x8664, 0x20B),
    "reader-voiceroid-bridge.exe": (0x14C, 0x10B),
}
REQUIRED_TEXT = (
    "README.md", "SPEECH.md", "DICTIONARIES.md", "LAYOUT.md", "EPUB-SUPPORT.md",
    "THIRD-PARTY-NOTICES.txt", "licenses/LICENSE-MIT", "licenses/Apache-2.0.txt",
    "licenses/COPYING-IPADIC.txt", "licenses/COPYING-THIRD-PARTY.txt",
    "licenses/Sonic-LICENSE.txt", "licenses/pyvcroid2-LICENSE.txt",
    "licenses/fonts/LICENSE-NotoSans.txt", "licenses/fonts/LICENSE-NotoSerif.txt",
    "licenses/fonts/LICENSE-STIXTwoMath.txt", "licenses/fonts/LICENSE-JetBrainsMono.txt",
    "LICENSE-AUDIT.md", "LICENSE-AUDIT-AIVIS-VOICEVOX.md", "LICENSE-AUDIT-KOKORO.md",
    "licenses/kokoro/audio.cpp-LICENSE.txt", "licenses/kokoro/Cutlet-LICENSE.txt",
    "licenses/kokoro/Kokoro-BASE-MODEL-CARD.md", "licenses/kokoro/Kokoro-ONNX-MODEL-CARD.md",
    "licenses/kokoro/Misaki-LICENSE.txt", "licenses/kokoro/NOTICE.txt",
    "licenses/kokoro/NVIDIA-CUDA-cuBLAS-LICENSE.txt", "licenses/kokoro/NVIDIA-cuDNN-LICENSE.txt",
    "licenses/kokoro/NVIDIA-cuDNN-ACKNOWLEDGEMENTS.html", "licenses/kokoro/SOURCES.json",
    "licenses/aivis-native/JPreprocess-LICENSE.txt", "licenses/aivis-native/OpenJTalk-NOTICE.txt",
    "licenses/aivis-native/NAIST-JDIC-NOTICE.txt", "licenses/aivis-native/BERT-MODEL-CARD.md",
    "licenses/aivis-native/CC-BY-SA-4.0.txt", "licenses/aivis-native/SOURCES.json",
)
OPTIONAL_PACKAGE_FILES = {"CI-WORKFLOW.yml", "RELEASE-WORKFLOW.yml", "README.ja.md"}
MAX_ARCHIVE_FILES = 10_000
MAX_ARCHIVE_BYTES = 2 * 1024**3
DEVICE_NAME = re.compile(r"(?:CON|PRN|AUX|NUL|COM[1-9¹²³]|LPT[1-9¹²³])(?:\.|$)", re.I)


def validate_relative_name(name: str) -> tuple[str, ...]:
    """Reject paths that change meaning when extracted on Windows."""
    if not name or "\\" in name or any(ord(char) < 32 for char in name):
        raise ValueError(f"Unsafe package path: {name!r}")
    parts = name.removesuffix("/").split("/")
    for part in parts:
        if (not part or part in {".", ".."} or part.endswith((".", " "))
                or any(char in '<>:"|?*' for char in part) or DEVICE_NAME.match(part)):
            raise ValueError(f"Unsafe package path: {name!r}")
    return tuple(parts)


def extract_zip(source: Path, destination: Path) -> Path:
    """Validate the whole ZIP before writing, without extractall or link support."""
    with zipfile.ZipFile(source) as archive:
        entries = archive.infolist()
        if len(entries) > MAX_ARCHIVE_FILES or sum(item.file_size for item in entries) > MAX_ARCHIVE_BYTES:
            raise ValueError("Package ZIP exceeds the extraction limit")
        seen: dict[str, bool] = {}
        validated = []
        for entry in entries:
            parts = validate_relative_name(entry.orig_filename)
            mode = entry.external_attr >> 16
            kind = stat.S_IFMT(mode)
            if entry.flag_bits & 1 or kind not in (0, stat.S_IFREG, stat.S_IFDIR):
                raise ValueError(f"Encrypted or special ZIP entry: {entry.filename!r}")
            if (kind == stat.S_IFDIR) != entry.is_dir() and kind != 0:
                raise ValueError(f"Conflicting ZIP entry type: {entry.filename!r}")
            canonical = "/".join(parts).casefold()
            if canonical in seen:
                raise ValueError(f"Duplicate Windows ZIP path: {entry.filename!r}")
            seen[canonical] = entry.is_dir()
            validated.append((entry, parts))
        for entry, parts in validated:
            for count in range(1, len(parts)):
                if seen.get("/".join(parts[:count]).casefold()) is False:
                    raise ValueError(f"ZIP file used as a directory: {entry.filename!r}")
        for entry, parts in validated:
            path = destination.joinpath(*parts)
            if entry.is_dir():
                path.mkdir(parents=True, exist_ok=True)
            else:
                path.parent.mkdir(parents=True, exist_ok=True)
                with archive.open(entry) as incoming, path.open("xb") as outgoing:
                    shutil.copyfileobj(incoming, outgoing)
    if (destination / "BUILD.json").is_file():
        return destination
    children = list(destination.iterdir())
    if len(children) == 1 and children[0].is_dir() and (children[0] / "BUILD.json").is_file():
        return children[0]
    raise ValueError("ZIP must contain one native package root with BUILD.json")


def validate_tree(root: Path) -> set[str]:
    if root.is_symlink() or root.is_junction():
        raise ValueError(f"Package root must not be a link: {root}")
    seen: set[str] = set()
    files_present: set[str] = set()
    for directory, folders, files in os.walk(root, followlinks=False):
        for name in folders + files:
            path = Path(directory) / name
            relative = path.relative_to(root).as_posix()
            validate_relative_name(relative)
            if path.is_symlink() or path.is_junction() or not (path.is_file() or path.is_dir()):
                raise ValueError(f"Package contains a link or special file: {relative}")
            canonical = relative.casefold()
            if canonical in seen:
                raise ValueError(f"Duplicate Windows package path: {relative}")
            seen.add(canonical)
            if path.is_file():
                files_present.add(relative)
            elif path.parent == root and name != "licenses":
                raise ValueError(f"Unexpected package directory; use an untouched release ZIP: {relative}")
    return files_present


def validate_pe(path: Path, expected: tuple[int, int] | None = None) -> tuple[int, int]:
    with path.open("rb") as file:
        dos = file.read(64)
        if len(dos) != 64 or dos[:2] != b"MZ":
            raise ValueError(f"Not a PE executable: {path.name}")
        offset = struct.unpack_from("<I", dos, 60)[0]
        if not 64 <= offset <= min(path.stat().st_size - 26, 16 * 1024**2):
            raise ValueError(f"Invalid PE header offset: {path.name}")
        file.seek(offset)
        header = file.read(26)
        machine = struct.unpack_from("<H", header, 4)[0]
        optional_size, characteristics, magic = struct.unpack_from("<HHH", header, 20)
        if (header[:4] != b"PE\0\0" or optional_size < 2
                or offset + 24 + optional_size > path.stat().st_size
                or not characteristics & 0x2 or characteristics & 0x2000
                or (machine, magic) not in {(0x8664, 0x20B), (0x14C, 0x10B)}):
            raise ValueError(f"Unsupported PE executable: {path.name}")
        if expected is not None and (machine, magic) != expected:
            raise ValueError(f"Wrong PE architecture for {path.name}; expected {expected!r}")
    return machine, magic


def validate_package(root: Path) -> str:
    files = validate_tree(root)
    allowed = set(PROCESSES) | set(REQUIRED_TEXT) | OPTIONAL_PACKAGE_FILES | {"BUILD.json"}
    if unknown := files - allowed:
        raise ValueError("Unexpected package files; use an untouched release ZIP: " + ", ".join(sorted(unknown)))
    if (root / "BUILD.json").stat().st_size > 64 * 1024:
        raise ValueError("BUILD.json is unexpectedly large")
    metadata = json.loads((root / "BUILD.json").read_text(encoding="utf-8"))
    if metadata.get("application") != "voice-over-book-reader" or metadata.get("target") != TARGET:
        raise ValueError("BUILD.json must identify the native x86_64-pc-windows-msvc reader")
    version = metadata.get("version")
    if (not isinstance(version, str) or not re.fullmatch(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)", version)
            or any(int(part) > 65535 for part in version.split("."))):
        raise ValueError("BUILD.json version must be three numeric components from 0 to 65535")
    processes = metadata.get("native_processes")
    if (not isinstance(processes, list) or len(processes) != 3
            or not all(isinstance(item, str) for item in processes)
            or set(processes) != set(PROCESSES)):
        raise ValueError("BUILD.json must list exactly the three native Windows processes")
    for name, expected in PROCESSES.items():
        validate_pe(root / name, expected)
    for relative in REQUIRED_TEXT:
        if not (root / relative).read_text(encoding="utf-8").strip():
            raise ValueError(f"Missing or empty required documentation/license: {relative}")
    return version


@contextmanager
def package_directory(source: Path):
    if source.is_symlink() or source.is_junction():
        raise ValueError(f"Package source must not be a link: {source}")
    if source.is_dir():
        yield source
    elif source.is_file() and source.suffix.casefold() == ".zip":
        with tempfile.TemporaryDirectory(prefix="vobr-installer-package-") as temporary:
            yield extract_zip(source, Path(temporary))
    else:
        raise FileNotFoundError(f"Expected an extracted package directory or ZIP: {source}")


def find_iscc(explicit: Path | None = None) -> Path:
    specified = explicit or os.environ.get("ISCC") or os.environ.get("INNO_SETUP_ISCC")
    if specified:
        path = Path(specified).expanduser().resolve()
        if not path.is_file():
            raise FileNotFoundError(f"Inno compiler not found: {path}")
        return path
    candidate = shutil.which("ISCC.exe") or shutil.which("iscc")
    if candidate:
        return Path(candidate).resolve()
    for variable in ("ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"):
        if base := os.environ.get(variable):
            path = Path(base) / ("Programs/Inno Setup 6" if variable == "LOCALAPPDATA" else "Inno Setup 6") / "ISCC.exe"
            if path.is_file():
                return path.resolve()
    raise FileNotFoundError("Install Inno Setup 6.3+ or supply --iscc / ISCC with the compiler path")


def build_installer(source: Path, iscc: Path, output_dir: Path, root: Path) -> Path:
    output_dir = output_dir.resolve()
    with package_directory(source) as package:
        package = package.resolve()
        version = validate_package(package)
        if output_dir == package or package in output_dir.parents:
            raise ValueError("Installer output must be outside the source package")
        basename = f"voice-over-book-reader-{version}-windows-x64-setup"
        output = output_dir / f"{basename}.exe"
        checksum = output.with_suffix(".exe.sha256")
        if output.exists() or checksum.exists():
            raise FileExistsError(f"Refusing to overwrite installer or checksum: {output}")
        output_dir.mkdir(parents=True, exist_ok=True)
        # Compile separately, then publish with exclusive creates. Inno itself
        # normally overwrites outputs; it never receives the final output path.
        with tempfile.TemporaryDirectory(prefix=".vobr-installer-", dir=output_dir) as temporary:
            staging = Path(temporary)
            subprocess.run([
                str(iscc), "/Qp", f"/DPackageDir={package}", f"/DAppVersion={version}",
                f"/DInstallerOutputDir={staging}", f"/DInstallerBaseName={basename}",
                str(root / "installer/windows/voice-over-book-reader.iss"),
            ], cwd=root, check=True)
            compiled = staging / output.name
            validate_pe(compiled)
            with compiled.open("rb") as file:
                digest = hashlib.file_digest(file, "sha256").hexdigest()
            created_output = False
            created_checksum = False
            try:
                with output.open("xb") as outgoing, compiled.open("rb") as incoming:
                    created_output = True
                    shutil.copyfileobj(incoming, outgoing)
                with checksum.open("x", encoding="ascii", newline="\n") as file:
                    created_checksum = True
                    file.write(f"{digest}  {output.name}\n")
            except BaseException:
                if created_output:
                    output.unlink()
                if created_checksum:
                    checksum.unlink()
                raise
    return output


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", type=Path, required=True, help="Native Windows package directory or ZIP")
    parser.add_argument("--iscc", type=Path, help="Inno Setup compiler; otherwise ISCC/INNO_SETUP_ISCC, PATH, standard install paths")
    parser.add_argument("--output-dir", type=Path, default=Path("target/installers"))
    arguments = parser.parse_args()
    if os.name != "nt":
        parser.error("Compiling the Windows installer requires Windows and Inno Setup 6.3+")
    root = Path(__file__).resolve().parents[1]
    try:
        output = build_installer(arguments.package, find_iscc(arguments.iscc), arguments.output_dir, root)
    except (OSError, ValueError, TypeError, zipfile.BadZipFile, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Installer build failed: {error}\n")
    print(output)
    print(output.with_suffix(".exe.sha256"))


if __name__ == "__main__":
    main()
