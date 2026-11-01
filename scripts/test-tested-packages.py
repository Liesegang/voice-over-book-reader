"""Release-package selection tests; all API responses are local fixtures."""
import copy
from datetime import datetime, timezone
import importlib.util
from pathlib import Path
import unittest
from urllib.parse import parse_qs, urlsplit

spec = importlib.util.spec_from_file_location("tested_packages", Path(__file__).with_name("find-tested-packages.py"))
selector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(selector)

REPOSITORY = "example/reader"
REPOSITORY_ID = 321
COMMIT = "a" * 40
NOW = datetime(2026, 11, 1, tzinfo=timezone.utc)


def run_fixture(run_id=20):
    repository = {"id": REPOSITORY_ID, "full_name": REPOSITORY}
    return {"id": run_id, "status": "completed", "conclusion": "success", "event": "push",
            "head_sha": COMMIT, "path": ".github/workflows/ci.yml",
            "repository": repository, "head_repository": repository.copy()}


def artifacts_fixture(run_id=20):
    return [{"id": index + 1, "name": name, "size_in_bytes": 1000, "expired": False,
             "expires_at": "2027-01-01T00:00:00Z", "digest": "sha256:" + str(index) * 64,
             "workflow_run": {"id": run_id, "repository_id": REPOSITORY_ID,
                              "head_repository_id": REPOSITORY_ID, "head_sha": COMMIT}}
            for index, name in enumerate(sorted(selector.EXPECTED_ARTIFACTS))]


class SelectionTests(unittest.TestCase):
    def select(self, runs, artifacts=None):
        artifacts = artifacts_fixture() if artifacts is None else artifacts
        calls = []

        def api(endpoint):
            calls.append(endpoint)
            return {"workflow_runs": runs} if "/workflows/" in endpoint else {"artifacts": artifacts}

        return selector.find_packages(REPOSITORY, REPOSITORY_ID, COMMIT, api, NOW), calls

    def test_complete_same_commit_run_returns_exact_immutable_ids(self):
        selected, _ = self.select([run_fixture()])
        self.assertEqual(selected, {"run_id": 20, "artifact_ids": [1, 2, 3, 4, 5]})
        run = run_fixture()
        run["event"] = "workflow_dispatch"
        self.assertIsNotNone(self.select([run])[0])

    def test_wrong_commit_repository_fork_event_or_status_never_reads_artifacts(self):
        variants = [
            {"head_sha": "b" * 40}, {"status": "in_progress"}, {"conclusion": "failure"},
            {"event": "pull_request"}, {"event": "pull_request_target"},
            {"path": ".github/workflows/other.yml"},
            {"repository": {"id": 999, "full_name": REPOSITORY}},
            {"repository": {"id": REPOSITORY_ID, "full_name": "someone/reader"}},
            {"head_repository": {"id": 999, "full_name": "fork/reader"}},
        ]
        for changed in variants:
            with self.subTest(changed=changed):
                run = run_fixture()
                run.update(changed)
                selected, calls = self.select([run])
                self.assertIsNone(selected)
                self.assertEqual(len(calls), 1)

    def test_expired_missing_extra_and_duplicate_names_force_a_rebuild(self):
        expired = artifacts_fixture()
        expired[0]["expired"] = True
        stale = artifacts_fixture()
        stale[0]["expires_at"] = "2026-10-31T23:59:59Z"
        duplicate = artifacts_fixture()
        duplicate[-1]["name"] = duplicate[0]["name"]
        for artifacts in [expired, stale, artifacts_fixture()[:-1], artifacts_fixture() + [artifacts_fixture()[0]], duplicate]:
            with self.subTest(artifacts=artifacts):
                self.assertIsNone(self.select([run_fixture()], artifacts)[0])

    def test_artifact_provenance_digest_size_expiry_and_ids_are_checked(self):
        changes = [{"digest": "sha256:bad"}, {"digest": None}, {"size_in_bytes": 0},
                   {"size_in_bytes": True}, {"id": -1}, {"expired": None},
                   {"expires_at": "invalid"}, {"expires_at": "2027-01-01T00:00:00"}]
        for key, value in [("id", 99), ("repository_id", 999), ("head_repository_id", 999), ("head_sha", "b" * 40)]:
            source = artifacts_fixture()[0]["workflow_run"].copy()
            source[key] = value
            changes.append({"workflow_run": source})
        for changed in changes:
            with self.subTest(changed=changed):
                artifacts = artifacts_fixture()
                artifacts[0].update(changed)
                self.assertIsNone(self.select([run_fixture()], artifacts)[0])
        artifacts = artifacts_fixture()
        artifacts[-1]["id"] = artifacts[0]["id"]
        self.assertIsNone(self.select([run_fixture()], artifacts)[0])

    def test_search_continues_to_an_older_complete_run(self):
        def api(endpoint):
            if "/workflows/" in endpoint:
                return {"workflow_runs": [run_fixture(21), run_fixture(20)]}
            return {"artifacts": [] if "/21/" in endpoint else artifacts_fixture(20)}

        self.assertEqual(selector.find_packages(REPOSITORY, REPOSITORY_ID, COMMIT, api, NOW)["run_id"], 20)

    def test_workflow_search_paginates_and_preserves_the_sha_filter(self):
        calls = []

        def api(endpoint):
            calls.append(endpoint)
            query = parse_qs(urlsplit(endpoint).query)
            if "/workflows/" in endpoint:
                self.assertEqual(query["head_sha"], [COMMIT])
                self.assertEqual(query["status"], ["success"])
                return {"workflow_runs": [{**run_fixture(100 + i), "event": "pull_request"} for i in range(100)]
                        if query["page"] == ["1"] else [run_fixture()]}
            return {"artifacts": artifacts_fixture()}

        self.assertIsNotNone(selector.find_packages(REPOSITORY, REPOSITORY_ID, COMMIT, api, NOW))
        self.assertEqual(len(calls), 3)

    def test_artifact_pagination_does_not_hide_extra_artifacts(self):
        calls = []

        def api(endpoint):
            calls.append(endpoint)
            if "/workflows/" in endpoint:
                return {"workflow_runs": [run_fixture()]}
            page = parse_qs(urlsplit(endpoint).query)["page"]
            return {"artifacts": [copy.deepcopy(artifacts_fixture()[0]) for _ in range(100)]
                    if page == ["1"] else artifacts_fixture()}

        self.assertIsNone(selector.find_packages(REPOSITORY, REPOSITORY_ID, COMMIT, api, NOW))
        self.assertEqual(len(calls), 3)

    def test_no_runs_and_malformed_api_responses_fail_closed(self):
        self.assertIsNone(self.select([])[0])
        with self.assertRaisesRegex(ValueError, "Missing GitHub API list"):
            selector.find_packages(REPOSITORY, REPOSITORY_ID, COMMIT, lambda _: {}, NOW)
        self.assertEqual(len(list(selector.pages(lambda _: {"items": [0] * 100}, "endpoint", "items"))), 1000)


if __name__ == "__main__":
    unittest.main()
