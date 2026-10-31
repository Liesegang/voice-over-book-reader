"""CEF distribution contract. No runtime downloader or sandbox bypass exists here."""
from __future__ import annotations

import hashlib
import json
import os
import posixpath
from pathlib import Path, PurePosixPath
import re
import shutil
import struct
import subprocess

PINS = json.loads(Path(__file__).with_name("cef-runtime-pins.json").read_text())
HELPER = "voice-over-book-reader-cef-helper"
FRAMEWORK = "Chromium Embedded Framework.framework"
MAC_HELPERS = ("Helper", "Helper (GPU)", "Helper (Renderer)", "Helper (Plugin)", "Helper (Alerts)")
COMMON = ("icudtl.dat", "resources.pak", "chrome_100_percent.pak", "chrome_200_percent.pak", "v8_context_snapshot.bin")
WINDOWS = ("libcef.dll", "chrome_elf.dll", "d3dcompiler_47.dll", "dxcompiler.dll", "dxil.dll", "vk_swiftshader.dll", "vk_swiftshader_icd.json", "vulkan-1.dll")
# CEF 154's CEF_BINARY_FILES / README.redistrib.txt: ANGLE is no longer
# distributed as separate libEGL/libGLESv2 libraries on Linux. Keep the Vulkan
# loader with SwiftShader so software rendering has all of its SDK components.
LINUX = ("libcef.so", "chrome-sandbox", "libvk_swiftshader.so", "libvulkan.so.1", "vk_swiftshader_icd.json")


def required(version: str) -> bool:
    # Preserve the ability to inspect old immutable packages; new releases fail
    # closed without their engine. This is not an optional application renderer.
    return tuple(map(int, version.split("."))) >= (0, 3, 0)


def sdk_path(target: str, configured: Path | None = None) -> Path:
    configured = configured or (Path(os.environ["CEF_PATH"]) if os.environ.get("CEF_PATH") else None)
    if configured is None:
        raise FileNotFoundError("CEF_PATH is required: run scripts/prepare-cef-sdk.py at build time")
    pin = PINS["targets"][target]
    for candidate in (configured, configured / pin["directory"], configured / "154.0.34" / pin["directory"]):
        archive = candidate / "archive.json"
        if archive.is_file():
            value = json.loads(archive.read_text())
            expected = f"cef_binary_{PINS['cef']}_{pin['platform']}_minimal.tar.bz2"
            if value.get("name") != expected or value.get("sha1") != pin["sha1"]:
                raise ValueError("CEF SDK differs from the pinned version/platform/archive")
            return candidate
    raise FileNotFoundError("Pinned CEF archive.json is missing")


def pe_exported_names(path: Path) -> set[str]:
    """Read the real DLL export directory without executing the DLL."""
    data = path.read_bytes()
    try:
        if data[:2] != b"MZ": raise ValueError("Not a PE image")
        start = struct.unpack_from("<I", data, 60)[0]
        machine, sections = struct.unpack_from("<HH", data, start + 4)
        opt_size, flags = struct.unpack_from("<HH", data, start + 20)
        if data[start:start+4] != b"PE\0\0" or machine != 0x8664 or not flags & 0x2000:
            raise ValueError("CEF client must be an x64 DLL")
        opt = start + 24
        if struct.unpack_from("<H", data, opt)[0] != 0x20B: raise ValueError("Expected PE32+ client")
        export_rva = struct.unpack_from("<I", data, opt + 112)[0]
        section_start = opt + opt_size
        def offset(rva):
            for index in range(sections):
                virtual_size, address, raw_size, raw = struct.unpack_from("<4I", data, section_start + index * 40 + 8)
                if address <= rva < address + max(virtual_size, raw_size):
                    result = raw + rva - address
                    if result >= len(data): break
                    return result
            raise ValueError("Invalid PE export RVA")
        directory = offset(export_rva)
        count = struct.unpack_from("<I", data, directory + 24)[0]
        if count > 32768: raise ValueError("Oversized export table")
        names = offset(struct.unpack_from("<I", data, directory + 32)[0])
        result = set()
        for index in range(count):
            name = offset(struct.unpack_from("<I", data, names + index * 4)[0])
            end = data.index(b"\0", name, min(len(data), name + 4096))
            result.add(data[name:end].decode("ascii"))
        return result
    except (IndexError, struct.error, UnicodeError) as error:
        raise ValueError("Malformed CEF client export directory") from error


def copy_file(source: Path, target: Path) -> None:
    if not source.is_file(): raise FileNotFoundError(f"Required CEF artifact is missing: {source.name}")
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, target)


def pe_imported_names(path: Path) -> set[str]:
    """Read import names with bounded seeks; do not load/execute the image."""
    with path.open("rb") as source:
        def read(at, length):
            source.seek(at)
            data = source.read(length)
            if len(data) != length: raise ValueError("Truncated PE import directory")
            return data
        if read(0, 2) != b"MZ": raise ValueError("Not a PE image")
        pe = struct.unpack("<I", read(60, 4))[0]
        if read(pe, 4) != b"PE\0\0": raise ValueError("Not a PE image")
        coff = read(pe + 4, 20)
        sections = struct.unpack_from("<H", coff, 2)[0]
        opt_size = struct.unpack_from("<H", coff, 16)[0]
        opt = pe + 24
        magic = struct.unpack("<H", read(opt, 2))[0]
        if magic not in (0x10B, 0x20B) or sections > 96: raise ValueError("Unsupported PE image")
        import_rva = struct.unpack("<I", read(opt + (120 if magic == 0x20B else 104), 4))[0]
        table = [struct.unpack_from("<4I", read(opt + opt_size + i * 40, 40), 8) for i in range(sections)]
        def offset(rva):
            for virtual, address, size, raw in table:
                if address <= rva < address + max(virtual, size): return raw + rva - address
            raise ValueError("Invalid PE import RVA")
        names = set()
        if not import_rva: return names
        start = offset(import_rva)
        for i in range(4096):
            entry = read(start + i * 20, 20)
            if entry == b"\0" * 20: return names
            name_at = offset(struct.unpack_from("<I", entry, 12)[0])
            source.seek(name_at)
            name = source.read(4096).split(b"\0", 1)[0].decode("ascii").lower()
            names.add(name)
        raise ValueError("Oversized PE import directory")


def require_static_crt(images) -> None:
    for path in images:
        missing = [name for name in pe_imported_names(path) if name.startswith(("vcruntime", "msvcp", "concrt", "vcomp"))]
        if missing:
            raise ValueError(f"{path.name} requires a separately installed VC runtime; build shipping images with +crt-static: {missing}")


def validate_engine_info(info: dict, target: str | None = None) -> None:
    target = target or info.get("target")
    if (target not in PINS["targets"] or info.get("target") != target
            or info.get("type") != "chromium-cef" or info.get("bindings") != PINS["bindings"]
            or info.get("cef") != PINS["cef"] or info.get("sandbox") != "enabled"
            or info.get("runtime_download_required") is not False):
        raise ValueError("Unexpected or incomplete bundled Chromium runtime")
    pin = PINS["targets"][target]
    sdk = info.get("sdk_archive", {})
    if (sdk.get("name") != f"cef_binary_{PINS['cef']}_{pin['platform']}_minimal.tar.bz2"
            or sdk.get("sha1") != pin["sha1"]):
        raise ValueError("Bundled Chromium SDK identity differs")


def stage(root: Path, target: str, release: Path, package: Path, sdk: Path, version: str) -> dict:
    """Copy only clean build artifacts and SDK resources, never a used browser profile."""
    files_before = {p.relative_to(package).as_posix() for p in package.rglob("*") if p.is_file()}
    if "windows" in target:
        client = release / "voice_over_book_reader.dll"
        if "RunWinMain" not in pe_exported_names(client):
            raise ValueError("CEF client DLL must export RunWinMain")
        copy_file(client, package / "voice-over-book-reader.dll")
        copy_file(sdk / "bootstrap.exe", package / "voice-over-book-reader.exe")
        copy_file(root / "installer/windows/cef-bootstrap.exe.manifest", package / "voice-over-book-reader.exe.manifest")
        for name in COMMON + WINDOWS: copy_file(sdk / name, package / name)
        for name in ("libEGL.dll", "libGLESv2.dll"):
            if (sdk / name).is_file(): copy_file(sdk / name, package / name)
        locales = package / "locales"
        shutil.copytree(sdk / "locales", locales)
        require_static_crt(package / name for name in ("voice-over-book-reader.dll", "reader-dictionary-popup.exe", "reader-voiceroid-bridge.exe"))
    elif "apple" in target:
        app = package / "Voice Over Book Reader.app"
        frameworks = app / "Contents/Frameworks"
        frameworks.mkdir(parents=True, exist_ok=True)
        shutil.copytree(sdk / FRAMEWORK, frameworks / FRAMEWORK, symlinks=True)
        for suffix in MAC_HELPERS:
            name = f"voice-over-book-reader {suffix}"
            contents = frameworks / (name + ".app") / "Contents"
            copy_file(release / HELPER, contents / "MacOS" / name)
            import plistlib
            with (contents / "Info.plist").open("wb") as output:
                plistlib.dump({"CFBundleName": name, "CFBundleExecutable": name,
                    "CFBundleIdentifier": "jp.liesegang.voice-over-book-reader." + re.sub(r"[^a-z]", "", suffix.lower()),
                    "CFBundleVersion": version, "CFBundleShortVersionString": version,
                    "CFBundlePackageType": "APPL", "LSUIElement": True,
                    "LSMinimumSystemVersion": "15.0", "LSEnvironment": {"MallocNanoZone": "0"}}, output)
    else:
        for name in COMMON + LINUX: copy_file(sdk / name, package / name)
        for path in sdk.iterdir():
            if re.fullmatch(r"lib[\w+-]+\.so(?:\.\d+)*", path.name):
                copy_file(path, package / path.name)
        shutil.copytree(sdk / "locales", package / "locales")
        copy_file(release / HELPER, package / HELPER)
        # User namespaces are preferred. Setuid setup is an explicit administrator
        # action on systems whose policy requires it; never disable the sandbox.
        (package / "chrome-sandbox").chmod(0o755)
    notices = package / "licenses/cef"
    copy_file(sdk / "CREDITS.html", notices / "CHROMIUM-CREDITS.html")
    if (sdk / "LICENSE.txt").is_file():
        copy_file(sdk / "LICENSE.txt", notices / "CEF-SDK-LICENSE.txt")
    for name in ("CEF-LICENSE.txt", "NOTICE.txt", "welding-MPL-2.0.txt", "SOURCES.json"):
        copy_file(root / "licenses/cef" / name, notices / name)
    return {"type": "chromium-cef", "target": target, "bindings": PINS["bindings"], "cef": PINS["cef"],
        "chromium": PINS["chromium"], "sandbox": "enabled", "runtime_download_required": False,
        "sdk_archive": json.loads((sdk / "archive.json").read_text()),
        "covered_source": f"https://github.com/Liesegang/voice-over-book-reader/tree/{os.environ.get('GITHUB_SHA', 'v'+version)}/crates/reader-renderer-cef/vendor/welding",
        "files_before": files_before}


def sign_macos(app: Path) -> None:
    framework = app / "Contents/Frameworks" / FRAMEWORK
    def sign(path): subprocess.run(["codesign", "--force", "--sign", "-", str(path)], check=True)
    for path in sorted(framework.rglob("*.dylib"), key=lambda p: len(p.parts), reverse=True):
        if not path.is_symlink(): sign(path)
    sign(framework)
    for helper in sorted((app / "Contents/Frameworks").glob("*.app")): sign(helper)
    for helper in (app / "Contents/MacOS").iterdir():
        if helper.name != "voice-over-book-reader" and helper.is_file(): sign(helper)
    sign(app)
    subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)


def write_manifest(package: Path, info: dict) -> dict:
    before = info.pop("files_before")
    records = []
    for path in sorted(package.rglob("*")):
        name = path.relative_to(package).as_posix()
        if name in before and name != "voice-over-book-reader.exe": continue
        if path.is_symlink():
            link = os.readlink(path)
            resolved = (path.parent / link).resolve()
            if not resolved.is_relative_to(package.resolve()): raise ValueError("Framework symlink escapes package")
            data = link.encode("utf8")
        elif path.is_file():
            with path.open("rb") as source:
                digest = hashlib.file_digest(source, "sha256").hexdigest()
            records.append({"path": name, "bytes": path.stat().st_size, "sha256": digest})
            continue
        else: continue
        record = {"path": name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
        if path.is_symlink(): record["symlink"] = link
        records.append(record)
    info["files"] = records
    (package / "CEF-MANIFEST.json").write_text(json.dumps(info, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    return info


def validate_manifest(root: Path) -> set[str]:
    info = json.loads((root / "CEF-MANIFEST.json").read_text(encoding="utf8"))
    validate_engine_info(info)
    names = set()
    for record in info["files"]:
        relative = PurePosixPath(record["path"])
        if relative.is_absolute() or ".." in relative.parts or "\\" in record["path"] or ":" in record["path"] or not relative.parts:
            raise ValueError("Unsafe CEF manifest path")
        if record["path"] in names: raise ValueError("Duplicate CEF manifest path")
        names.add(record["path"])
        path = root.joinpath(*relative.parts)
        if path.is_symlink():
            data = os.readlink(path).encode("utf8")
            size, digest = len(data), hashlib.sha256(data).hexdigest()
        else:
            size = path.stat().st_size
            with path.open("rb") as source: digest = hashlib.file_digest(source, "sha256").hexdigest()
        if size != record["bytes"] or digest != record["sha256"]:
            raise ValueError(f"CEF runtime hash differs: {record['path']}")
    if not names: raise ValueError("Empty CEF runtime")
    return names | {"CEF-MANIFEST.json"}


def validate_archive(archive, prefix: str, target: str) -> set[str]:
    """Validate the distributed engine before publication, without extraction."""
    info = json.loads(archive.read(prefix + "CEF-MANIFEST.json"))
    validate_engine_info(info, target)
    names = set()
    for record in info["files"]:
        name = record["path"]
        relative = PurePosixPath(name)
        if relative.is_absolute() or ".." in relative.parts or "\\" in name or ":" in name or not relative.parts or name in names:
            raise ValueError("Unsafe or duplicate CEF runtime path")
        names.add(name)
        if "symlink" in record:
            link = record["symlink"]
            resolved = posixpath.normpath(posixpath.join(posixpath.dirname(name), link))
            if link.startswith("/") or resolved == ".." or resolved.startswith("../"):
                raise ValueError("CEF framework symlink escapes the package")
            if archive.read(prefix + name).decode("utf8") != link:
                raise ValueError("CEF symlink differs")
        with archive.open(prefix + name) as source:
            digest = hashlib.file_digest(source, "sha256").hexdigest()
        if archive.getinfo(prefix + name).file_size != record["bytes"] or digest != record["sha256"]:
            raise ValueError("Bundled CEF runtime bytes differ: " + name)
    mandatory = {"licenses/cef/CEF-LICENSE.txt", "licenses/cef/CHROMIUM-CREDITS.html", "licenses/cef/welding-MPL-2.0.txt"}
    if "windows" in target:
        mandatory |= set(COMMON + WINDOWS) | {"voice-over-book-reader.exe", "voice-over-book-reader.dll", "locales/en-US.pak", "locales/ja.pak"}
        allowed_dlls = set(WINDOWS) | {"voice-over-book-reader.dll", "libEGL.dll", "libGLESv2.dll"}
        if {name for name in names if name.lower().endswith(".dll")} - allowed_dlls:
            raise ValueError("Unexpected runtime DLL")
    elif "apple" in target:
        base = "Voice Over Book Reader.app/Contents/Frameworks/"
        mandatory.add(base + FRAMEWORK + "/Chromium Embedded Framework")
        for suffix in MAC_HELPERS:
            helper = "voice-over-book-reader " + suffix
            mandatory.add(base + helper + ".app/Contents/MacOS/" + helper)
    else:
        mandatory |= set(COMMON + LINUX) | {HELPER, "locales/en-US.pak", "locales/ja.pak"}
    if mandatory - names:
        raise ValueError("Incomplete Chromium runtime: " + ", ".join(sorted(mandatory - names)))
    return names
