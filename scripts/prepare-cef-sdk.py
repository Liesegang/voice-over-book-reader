#!/usr/bin/env python3
"""Build-time only: fetch the fixed official CEF SDK; never used by installed apps."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import tarfile
import tempfile
import urllib.parse
import urllib.request

PINS = json.loads(Path(__file__).with_name("cef-runtime-pins.json").read_text())


def prepare(target: str, directory: Path) -> Path:
    pin = PINS["targets"][target]
    archive_name = f"cef_binary_{PINS['cef']}_{pin['platform']}_minimal.tar.bz2"
    directory.mkdir(parents=True, exist_ok=True)
    sdk = directory / pin["directory"]
    expected = {"type": "minimal", "name": archive_name, "sha1": pin["sha1"]}
    if sdk.is_dir() and (sdk / "archive.json").is_file():
        if json.loads((sdk / "archive.json").read_text()) == expected:
            return sdk.resolve()
        raise ValueError("Existing CEF SDK differs from the pinned archive")
    archive = directory / archive_name
    if not archive.exists():
        with tempfile.NamedTemporaryFile(dir=directory, suffix=".partial", delete=False) as output:
            temporary = Path(output.name)
            try:
                with urllib.request.urlopen(PINS["cdn"] + "/" + urllib.parse.quote(archive_name), timeout=90) as response:
                    shutil.copyfileobj(response, output, 1024 * 1024)
            except BaseException:
                output.close()
                temporary.unlink(missing_ok=True)
                raise
        temporary.rename(archive)
    with archive.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha1").hexdigest()
    if archive.stat().st_size != pin["bytes"] or digest != pin["sha1"]:
        raise ValueError("CEF SDK size or official pinned SHA-1 differs")
    # Match download-cef 3.0.0's layout, retaining original legal notices too.
    with tempfile.TemporaryDirectory(prefix="cef-extract-", dir=directory) as temporary:
        extracted = Path(temporary)
        with tarfile.open(archive) as source:
            source.extractall(extracted, filter="data")
        original = extracted / archive_name.removesuffix(".tar.bz2")
        stage = extracted / pin["directory"]
        (original / "Release").rename(stage)
        if "apple" not in target:
            for path in (original / "Resources").iterdir():
                path.rename(stage / path.name)
        for name in ("CMakeLists.txt", "cmake", "include", "libcef_dll", "CREDITS.html"):
            (original / name).rename(stage / name)
        for path in original.iterdir():
            if path.is_file() and path.name.upper().startswith(("LICENSE", "NOTICE")):
                shutil.copy2(path, stage / path.name)
        (stage / "archive.json").write_text(json.dumps(expected, indent=2) + "\n")
        stage.rename(sdk)
    return sdk.resolve()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, choices=PINS["targets"])
    parser.add_argument("--directory", required=True, type=Path)
    parser.add_argument("--github-env", type=Path)
    args = parser.parse_args()
    sdk = prepare(args.target, args.directory)
    if args.github_env:
        with args.github_env.open("a", encoding="utf8") as output:
            output.write(f"CEF_PATH={sdk}\n")
    print(f"Prepared pinned CEF {PINS['cef']} for {args.target}")
