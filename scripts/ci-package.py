#!/usr/bin/env python3
"""Package native Voice Over Book Reader binaries and dependency notices; never downloads models."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import posixpath
import re
import shutil
import subprocess
import struct
import tempfile
import zipfile
from urllib.parse import urlsplit
import stat
import cef_bundle

SUPPORTED_TARGETS = (
    "x86_64-pc-windows-msvc", "aarch64-apple-darwin",
    "x86_64-apple-darwin", "x86_64-unknown-linux-gnu",
)


def rewrite_document_links(text: str, source: str, paths: dict[str, str]) -> str:
    """Resolve local Markdown links relative to their source and packaged locations."""
    def target(value: str) -> str:
        parsed = urlsplit(value)
        if parsed.scheme or parsed.netloc or not parsed.path or parsed.path.startswith("/"):
            return value
        normalized = posixpath.normpath(posixpath.join(posixpath.dirname(source), parsed.path))
        if normalized not in paths:
            return value
        destination = paths[normalized]
        rewritten = (destination if urlsplit(destination).scheme else
                     posixpath.relpath(destination, posixpath.dirname(paths.get(source, source)) or "."))
        if parsed.query:
            rewritten += "?" + parsed.query
        if parsed.fragment:
            rewritten += "#" + parsed.fragment
        return rewritten

    text = re.sub(r"(!?\[[^\]]*\]\()([^\s)]+)([^)]*\))",
                  lambda match: match[1] + target(match[2]) + match[3], text)
    return re.sub(r"(?m)^(\s*\[[^\]]+\]:\s*)([^\s]+)(.*)$",
                  lambda match: match[1] + target(match[2]) + match[3], text)


def cargo_metadata(root: Path, target: str) -> dict:
    result = subprocess.run(
        ["cargo", "+1.97.1", "metadata", "--locked", "--format-version", "1", "--filter-platform", target],
        cwd=root, check=True, stdout=subprocess.PIPE, text=True, encoding="utf-8",
    )
    return json.loads(result.stdout)


def license_candidates(package: dict) -> list[Path]:
    directory = Path(package["manifest_path"]).parent
    candidates = []
    if package.get("license_file"):
        explicit = Path(package["license_file"])
        candidates.append(explicit if explicit.is_absolute() else directory / explicit)
    for path in directory.iterdir():
        name = path.name.lower()
        if path.is_file() and (
            name.startswith(("license", "licence", "copying", "notice"))
            or name in {"copyright", "copyright.txt", "ofl.txt"}
        ):
            candidates.append(path)
    return sorted(set(path for path in candidates if path.is_file()))


def third_party_notices(metadata: dict) -> str:
    workspace = set(metadata["workspace_members"])
    resolved = {node["id"] for node in metadata.get("resolve", {}).get("nodes", [])}
    lines = [
        "Voice Over Book Reader — third-party notices", "",
        "This file records the dependency versions and license expressions resolved for this build.",
        "Available license and notice files from the published crate packages follow each entry.",
        "The Rust workspace is licensed MIT; bundled font notices are in licenses/fonts.",
        "Sonic's Apache-2.0 notice is included separately in licenses/Sonic-LICENSE.txt.", "",
        "Covered source, including MPL-2.0 dependencies: each entry below provides the exact",
        "published crate version's source archive. Download that archive to obtain its covered",
        "source and original notices. MPL-2.0 terms: https://www.mozilla.org/MPL/2.0/", "",
        "Irodori, Kokoro, ONNX Runtime, Qwen3, audio.cpp, and PDFium runtime/model assets are not included in this ZIP.",
        "The app obtains selected native assets separately. See SPEECH.md for pins and license sources.", "",
    ]
    for package in sorted(metadata["packages"], key=lambda item: (item["name"], item["version"])):
        if package["id"] in workspace or package["id"] not in resolved:
            continue
        lines.extend([
            "=" * 78,
            f"{package['name']} {package['version']}",
            f"License: {package.get('license') or 'See included license file'}",
            f"Source: {package.get('repository') or 'https://crates.io/crates/' + package['name']}", "",
            f"Source archive: https://crates.io/api/v1/crates/{package['name']}/{package['version']}/download", "",
        ])
        for path in license_candidates(package):
            if path.stat().st_size > 512 * 1024:
                raise RuntimeError(f"Unexpectedly large license file: {path}")
            lines.extend([f"--- {path.name} ---", path.read_text(encoding="utf-8", errors="replace"), ""])
    return "\n".join(lines)


def package(root: Path, target: str, output_dir: Path, cef_path: Path | None = None) -> Path:
    if target not in SUPPORTED_TARGETS:
        raise ValueError(f"Unsupported package target: {target}")
    metadata = cargo_metadata(root, target)
    application = next(item for item in metadata["packages"] if item["name"] == "voice-over-book-reader")
    version = application["version"]
    uses_cef = cef_bundle.required(version)
    sdk = cef_bundle.sdk_path(target, cef_path) if uses_cef else None
    target_dir = Path(metadata["target_directory"]) / target / "release"
    extension = ".exe" if "windows" in target else ""
    binaries = [target_dir / f"{name}{extension}" for name in ("voice-over-book-reader", "reader-dictionary-popup")]
    if "windows" in target:
        bridge = Path(metadata["target_directory"]) / "i686-pc-windows-msvc" / "release" / "reader-voiceroid-bridge.exe"
        if not bridge.is_file():
            raise FileNotFoundError(f"Build the VOICEROID2 bridge with --target i686-pc-windows-msvc first: {bridge}")
        with bridge.open("rb") as file:
            dos = file.read(64)
            if len(dos) != 64 or dos[:2] != b"MZ":
                raise ValueError(f"Invalid VOICEROID2 bridge PE header: {bridge}")
            file.seek(struct.unpack_from("<I", dos, 60)[0])
            header = file.read(6)
            if header != b"PE\0\0\x4c\x01":
                raise ValueError(f"VOICEROID2 bridge must be PE32 x86, not the 64-bit workspace stub: {bridge}")
        binaries.append(bridge)
    for binary in binaries:
        if uses_cef and "windows" in target and binary.name == "voice-over-book-reader.exe":
            continue  # The CEF sandbox bootstrap is the shipped executable.
        if not binary.is_file():
            raise FileNotFoundError(f"Build the release workspace with --target {target} first: {binary}")
    output_dir.mkdir(parents=True, exist_ok=True)
    archive_path = output_dir / f"voice-over-book-reader-{version}-{target}.zip"
    if archive_path.exists():
        raise FileExistsError(f"Refusing to overwrite an existing package: {archive_path}")
    with tempfile.TemporaryDirectory(prefix="voice-over-book-reader-package-") as temporary:
        staging = Path(temporary) / f"voice-over-book-reader-{version}-{target}"
        staging.mkdir()
        executable_dir = staging
        if "apple-darwin" in target:
            app = staging / "Voice Over Book Reader.app"
            executable_dir = app / "Contents" / "MacOS"
            executable_dir.mkdir(parents=True)
            plist = {
                "CFBundleName": "Voice Over Book Reader", "CFBundleDisplayName": "Voice Over Book Reader", "CFBundleExecutable": "voice-over-book-reader",
                "CFBundleIdentifier": "jp.liesegang.voice-over-book-reader", "CFBundlePackageType": "APPL",
                "CFBundleShortVersionString": version, "CFBundleVersion": version,
                "LSMinimumSystemVersion": "15.0", "NSHighResolutionCapable": True,
            }
            with (app / "Contents" / "Info.plist").open("wb") as file:
                plistlib.dump(plist, file)
        for binary in binaries:
            if uses_cef and "windows" in target and binary.name == "voice-over-book-reader.exe":
                continue
            shutil.copy2(binary, executable_dir / binary.name)
        engine = cef_bundle.stage(root, target, target_dir, staging, sdk, version) if uses_cef else None
        licenses = staging / "licenses"
        font_licenses = licenses / "fonts"
        font_licenses.mkdir(parents=True)
        for path in sorted((root / "assets" / "fonts").glob("LICENSE*")):
            shutil.copy2(path, font_licenses / path.name)
        shutil.copy2(root / "crates" / "reader-speech" / "vendor" / "sonic" / "LICENSE", licenses / "Sonic-LICENSE.txt")
        shutil.copy2(root / "crates" / "reader-voiceroid-bridge" / "licenses" / "pyvcroid2-LICENSE", licenses / "pyvcroid2-LICENSE.txt")
        shutil.copy2(root / "licenses" / "Apache-2.0.txt", licenses / "Apache-2.0.txt")
        kokoro_source = root / "crates" / "reader-speech" / "licenses" / "kokoro"
        shutil.copytree(kokoro_source, licenses / "kokoro")
        aivis_source = root / "crates" / "reader-speech" / "licenses" / "aivis-native"
        shutil.copytree(aivis_source, licenses / "aivis-native")
        documents = {
            "docs/speech.md": "SPEECH.md", "docs/dictionaries.md": "DICTIONARIES.md",
            "docs/layout.md": "LAYOUT.md", "docs/epub-support.md": "EPUB-SUPPORT.md",
            "docs/license-audit.md": "LICENSE-AUDIT.md",
            "docs/license-audit-aivis-voicevox.md": "LICENSE-AUDIT-AIVIS-VOICEVOX.md",
            "docs/license-audit-kokoro.md": "LICENSE-AUDIT-KOKORO.md",
        }
        if uses_cef:
            documents["docs/chromium-runtime.md"] = "CHROMIUM.md"
        readme_links = {**documents, "README.md": "README.md", "README.ja.md": "README.ja.md",
                        "licenses/Apache-2.0.txt": "licenses/Apache-2.0.txt",
                        "crates/reader-speech/vendor/sonic/LICENSE": "licenses/Sonic-LICENSE.txt",
                        "crates/reader-speech/licenses/kokoro": "licenses/kokoro",
                        "crates/reader-speech/licenses/aivis-native": "licenses/aivis-native"}
        for notice_source in (kokoro_source, aivis_source):
            for path in notice_source.rglob("*"):
                if path.is_file():
                    readme_links[path.relative_to(root).as_posix()] = "licenses/" + notice_source.name + "/" + path.relative_to(notice_source).as_posix()
        for source in ("native.rs", "kokoro_ort_assets.rs"):
            relative = "crates/reader-speech/src/" + source
            readme_links[relative] = f"https://github.com/Liesegang/voice-over-book-reader/blob/v{version}/{relative}"
        for pattern in ("LICENSE*", "COPYING*"):
            for path in root.glob(pattern):
                if path.is_file():
                    shutil.copy2(path, licenses / path.name)
                    readme_links[path.name] = f"licenses/{path.name}"
        (staging / "THIRD-PARTY-NOTICES.txt").write_text(third_party_notices(metadata), encoding="utf-8")
        workflow = root / ".github" / "workflows" / "ci.yml"
        if workflow.is_file():
            shutil.copy2(workflow, staging / "CI-WORKFLOW.yml")
            readme_links[".github/workflows/ci.yml"] = "CI-WORKFLOW.yml"
        release_workflow = root / ".github/workflows/release.yml"
        if release_workflow.is_file():
            shutil.copy2(release_workflow, staging / "RELEASE-WORKFLOW.yml")
            readme_links[".github/workflows/release.yml"] = "RELEASE-WORKFLOW.yml"
        for source, destination in documents.items():
            contents = (root / source).read_text(encoding="utf-8")
            (staging / destination).write_text(rewrite_document_links(contents, source, readme_links), encoding="utf-8")
        readmes = [root / name for name in ("README.md", "README.ja.md") if (root / name).is_file()]
        for path in readmes:
            readme = path.read_text(encoding="utf-8")
            (staging / path.name).write_text(rewrite_document_links(readme, path.name, readme_links), encoding="utf-8")
        if not readmes:
            (staging / "README.txt").write_text(
                "Voice Over Book Reader native EPUB reader\nKeep every packaged executable together.\n"
                "Models and native inference libraries are obtained on first use, not bundled here.\n",
                encoding="utf-8",
            )
        build_info = {
            "application": "voice-over-book-reader", "display_name": "Voice Over Book Reader",
            "version": version, "target": target,
            "rust": "1.97.1" if uses_cef else "1.97.0", "commit": os.environ.get("GITHUB_SHA", "local"),
            "native_smoke_tests_run": False,
            "native_processes": [binary.name for binary in binaries],
        }
        if engine:
            build_info["book_engine"] = {key: value for key, value in engine.items() if key not in {"files_before", "files"}}
        (staging / "BUILD.json").write_text(json.dumps(build_info, indent=2), encoding="utf-8")
        if "apple-darwin" in target:
            # Ad-hoc signing needs no credentials. This is not Apple notarization.
            # Sign nested code inside out (Apple TN2206). Signing the main
            # CFBundleExecutable also inspects its enclosing .app, so unsigned
            # Intel helpers must already be signed before that bundle is sealed.
            if uses_cef:
                cef_bundle.sign_macos(app)
            else:
                for binary in binaries:
                    if binary.name != "voice-over-book-reader":
                        subprocess.run(["codesign", "--force", "--sign", "-", str(executable_dir / binary.name)], check=True)
                subprocess.run(["codesign", "--force", "--sign", "-", str(app)], check=True)
                subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
        if engine:
            cef_bundle.write_manifest(staging, engine)
        with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
            for path in sorted(staging.rglob("*")):
                if path.is_symlink():
                    entry = zipfile.ZipInfo(path.relative_to(staging.parent).as_posix())
                    entry.create_system = 3
                    entry.external_attr = (stat.S_IFLNK | 0o777) << 16
                    archive.writestr(entry, os.readlink(path).encode("utf8"))
                elif path.is_file():
                    archive.write(path, path.relative_to(staging.parent))
    with archive_path.open("rb") as file:
        digest = hashlib.file_digest(file, "sha256").hexdigest()
    archive_path.with_suffix(".zip.sha256").write_text(f"{digest}  {archive_path.name}\n", encoding="ascii")
    return archive_path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, choices=SUPPORTED_TARGETS)
    parser.add_argument("--output-dir", type=Path, default=Path("target/packages"))
    parser.add_argument("--cef-path", type=Path, help="Matching SDK prepared at build time (defaults to CEF_PATH)")
    arguments = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    output_dir = arguments.output_dir if arguments.output_dir.is_absolute() else root / arguments.output_dir
    print(package(root, arguments.target, output_dir, arguments.cef_path))


if __name__ == "__main__":
    main()
