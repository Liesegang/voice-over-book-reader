"""Select complete, tested CI packages for the exact release commit."""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import subprocess
from urllib.parse import urlencode


EXPECTED_ARTIFACTS = frozenset({
    "voice-over-book-reader-windows-x86_64",
    "voice-over-book-reader-windows-x64-installer",
    "voice-over-book-reader-macos-aarch64",
    "voice-over-book-reader-macos-x86_64",
    "voice-over-book-reader-linux-x86_64",
})


def same_repository(value, repository, repository_id):
    return (isinstance(value, dict)
            and value.get("id") == repository_id
            and str(value.get("full_name", "")).casefold() == repository.casefold())


def eligible_run(run, repository, repository_id, commit):
    return (type(run.get("id")) is int and run["id"] > 0
            and run.get("status") == "completed"
            and run.get("conclusion") == "success"
            and run.get("event") in {"push", "workflow_dispatch"}
            and run.get("head_sha") == commit
            and run.get("path", "").split("@", 1)[0] == ".github/workflows/ci.yml"
            and same_repository(run.get("repository"), repository, repository_id)
            and same_repository(run.get("head_repository"), repository, repository_id))


def package_ids(artifacts, run_id, repository_id, commit, now):
    if (len(artifacts) != len(EXPECTED_ARTIFACTS)
            or {item.get("name") for item in artifacts} != EXPECTED_ARTIFACTS):
        return None
    ids = []
    for item in sorted(artifacts, key=lambda value: value["name"]):
        source = item.get("workflow_run") or {}
        if not isinstance(source, dict):
            return None
        try:
            expires = datetime.fromisoformat(item["expires_at"].replace("Z", "+00:00"))
        except (KeyError, TypeError, ValueError, AttributeError):
            return None
        if (type(item.get("id")) is not int or item["id"] <= 0
                or item.get("expired") is not False
                or expires.tzinfo is None or expires <= now
                or type(item.get("size_in_bytes")) is not int or item["size_in_bytes"] <= 0
                or not re.fullmatch(r"sha256:[0-9a-f]{64}", str(item.get("digest", "")))
                or source.get("id") != run_id
                or source.get("repository_id") != repository_id
                or source.get("head_repository_id") != repository_id
                or source.get("head_sha") != commit):
            return None
        ids.append(item["id"])
    return ids if len(set(ids)) == len(ids) else None


def pages(api, endpoint, key, parameters=None):
    # GitHub limits filtered workflow searches to 1,000 results. Artifacts use
    # the same bounded pagination; a complete package run contains only five.
    for page in range(1, 11):
        query = urlencode({**(parameters or {}), "per_page": 100, "page": page})
        response = api(f"{endpoint}?{query}")
        items = response.get(key)
        if not isinstance(items, list):
            raise ValueError(f"Missing GitHub API list: {key}")
        yield from items
        if len(items) < 100:
            return
    # The filtered workflow endpoint exposes at most 1,000 results. If none of
    # those runs qualifies, the caller builds fresh packages. A truncated
    # artifact listing already exceeds the five-package limit and is rejected.


def find_packages(repository, repository_id, commit, api, now=None):
    now = now or datetime.now(timezone.utc)
    endpoint = f"repos/{repository}/actions/workflows/ci.yml/runs"
    runs = pages(api, endpoint, "workflow_runs", {"head_sha": commit, "status": "success"})
    for run in runs:
        if not eligible_run(run, repository, repository_id, commit):
            continue
        artifacts = list(pages(api, f"repos/{repository}/actions/runs/{run['id']}/artifacts", "artifacts"))
        ids = package_ids(artifacts, run["id"], repository_id, commit, now)
        if ids is not None:
            return {"run_id": run["id"], "artifact_ids": ids}
    return None


def github_api(endpoint):
    result = subprocess.run(
        ["gh", "api", "--method", "GET", "-H", "Accept: application/vnd.github+json", endpoint],
        capture_output=True, text=True, check=False,
    )
    if result.returncode:
        # Credentials remain inside gh; neither its stderr nor authenticated
        # request headers are copied into workflow output.
        raise RuntimeError(f"GitHub package lookup failed (exit {result.returncode})")
    return json.loads(result.stdout)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--commit", required=True)
    args = parser.parse_args()
    repository = os.environ["GITHUB_REPOSITORY"]
    repository_id = int(os.environ["GITHUB_REPOSITORY_ID"])
    if (not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository)
            or repository_id <= 0 or not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", args.commit)):
        parser.error("A repository and full validated commit are required")
    selected = find_packages(repository, repository_id, args.commit, github_api)
    run_id = str(selected["run_id"]) if selected else ""
    artifact_ids = ",".join(map(str, selected["artifact_ids"])) if selected else ""
    with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as output:
        output.write(f"run_id={run_id}\nartifact_ids={artifact_ids}\n")
    print(f"Selected tested package run {run_id}" if selected else "No complete tested package run; build all four targets")


if __name__ == "__main__":
    main()
