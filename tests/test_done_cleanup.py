"""End-to-end cleanup guard tests; only temporary Git repositories and fake Berbr."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "skills/done/scripts/close_task.py"


class CleanupTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="done-test-")
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name).resolve()
        self.cwd = self.root / "main"
        self.cwd.mkdir()
        self.tree = self.root / "task"
        self.tree.mkdir()
        self.git("init", "-q")
        (self.tree / ".gitignore").write_text(".runner/\n")
        self.git("add", ".gitignore")
        self.git("-c", "user.name=Demo", "-c", "user.email=demo@example.com",
                 "commit", "-qm", "initial")
        self.state = self.root / "state"
        self.fixture = self.root / "fixture.json"
        self.data = {"plan": {
            "kind": "task", "name": "demo-7", "blockers": [], "dirty": [],
            "keep": [], "remove": [{"tree": "task", "path": str(self.tree),
                                     "uncommitted": 0, "unpushed": 0, "merged": True}],
            "close_tickets": ["DEMO-7"], "leave_tickets": ["DEMO-8"],
            "note_done": True, "close_space": True, "steps": ["close task"],
        }, "done": False, "calls": []}
        self.save()
        self.binary = self.root / "berbr"
        self.binary.write_text(f"#!{sys.executable}\n" + '''
import json, os, pathlib, shutil, sys
fixture = pathlib.Path(__file__).with_name("fixture.json")
data = json.loads(fixture.read_text())
args = sys.argv[1:]
assert args[:2] == ["-p", "demo"], args
if args[2:] == ["done", "demo-7", "--dry-run", "--json"]:
    print(json.dumps(data["plan"]))
elif args[2:] == ["done", "demo-7", "--yes"]:
    data["calls"].append(args)
    for tree in data["plan"]["remove"]:
        shutil.rmtree(tree["path"])
    data["done"] = not data.get("jira_error", False)
    if "task_pane_open" in data:
        data["task_pane_open"] = not bool(os.environ.get("HC_TERMINAL_SOCKET"))
    fixture.write_text(json.dumps(data))
    print("Jira error" if data.get("jira_error") else "closed")
    sys.exit(1 if data.get("jira_error") or data.get("ui_error") else 0)
elif args[2:] == ["task", "show", "demo-7"]:
    if data.get("show_error"):
        sys.exit(1)
    print(json.dumps({"id": "demo-7", "done": data["done"]}))
else:
    raise AssertionError(args)
''')
        self.binary.chmod(0o700)

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.tree, stderr=subprocess.PIPE)

    def save(self):
        self.fixture.write_text(json.dumps(self.data))

    def invoke(self, *args, script=SCRIPT, env=None):
        return subprocess.run([sys.executable, str(script), *map(str, args)],
                              cwd=self.cwd, env=env, text=True, capture_output=True)

    def prepare(self, *extra, **kwargs):
        return self.invoke("prepare", "--project", "demo", "--task", "demo-7",
                           "--cwd", self.cwd, "--berbr", self.binary,
                           "--state-dir", self.state, *extra, **kwargs)

    def close(self, **kwargs):
        return self.invoke("close", "--state-dir", self.state,
                           script=self.state / "close_task.py", **kwargs)

    def status(self):
        return json.loads((self.state / "receipt.json").read_text())["status"]

    def assert_no_close(self):
        self.assertEqual(json.loads(self.fixture.read_text())["calls"], [])
        self.assertTrue(self.tree.exists())

    def test_stable_runner_survives_source_removal_and_records_success(self):
        source = self.tree / ".runner/close_task.py"
        source.parent.mkdir()
        shutil.copyfile(SCRIPT, source)
        result = self.prepare(script=source)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.status(), "prepared")
        self.assert_no_close()
        result = self.close()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.status(), "closed")
        self.assertFalse(source.exists())
        self.assertEqual(json.loads(self.fixture.read_text())["calls"],
                         [["-p", "demo", "done", "demo-7", "--yes"]])
        self.assertIn("closed", (self.state / "done.log").read_text())
        self.assertNotEqual(self.close().returncode, 0)
        self.assertEqual(self.status(), "closed")

    def test_unsafe_plans_never_reach_cleanup(self):
        base = json.loads(json.dumps(self.data))
        variants = [
            ("dirty", [{"tree": "task", "uncommitted": 1}]),
            ("blockers", ["CI pending"]), ("keep", [{"tree": "task"}]),
            ("name", "demo-8"), ("note_done", False),
            ("remove", [{**base["plan"]["remove"][0], "unpushed": None}]),
            ("remove", [{**base["plan"]["remove"][0], "unpushed": 2}]),
            ("remove", [{**base["plan"]["remove"][0], "uncommitted": None}]),
            ("remove", [{**base["plan"]["remove"][0], "unpushed": -1}]),
        ]
        for key, value in variants:
            with self.subTest(key=key, value=value):
                self.data = json.loads(json.dumps(base))
                self.data["plan"][key] = value
                self.save()
                self.assertNotEqual(self.prepare().returncode, 0)
                self.assertFalse(self.state.exists())
                self.assert_no_close()

    def test_changed_ticket_scope_blocks_execution(self):
        self.assertEqual(self.prepare().returncode, 0)
        self.data["plan"]["close_tickets"].append("DEMO-9")
        self.save()
        self.assertNotEqual(self.close().returncode, 0)
        self.assertEqual(self.status(), "blocked")
        self.assert_no_close()

    def test_changed_worktree_inventory_blocks_execution(self):
        self.assertEqual(self.prepare().returncode, 0)
        self.data["plan"]["remove"] = []
        self.save()
        self.assertNotEqual(self.close().returncode, 0)
        self.assert_no_close()

    def test_changed_head_blocks_execution(self):
        self.assertEqual(self.prepare().returncode, 0)
        self.git("-c", "user.name=Demo", "-c", "user.email=demo@example.com",
                 "commit", "--allow-empty", "-qm", "concurrent work")
        self.assertNotEqual(self.close().returncode, 0)
        self.assert_no_close()

    def test_local_untracked_work_blocks_execution(self):
        self.assertEqual(self.prepare().returncode, 0)
        (self.tree / "untracked.txt").write_text("preserve me")
        self.assertNotEqual(self.close().returncode, 0)
        self.assert_no_close()

    def test_nested_state_or_cwd_is_rejected(self):
        self.state = self.tree / ".runner/state"
        self.assertNotEqual(self.prepare().returncode, 0)
        self.assertFalse(self.state.exists())
        self.state = self.root / "state"
        self.cwd = self.tree
        self.assertNotEqual(self.prepare().returncode, 0)
        self.assert_no_close()

    def test_config_environment_drift_blocks_execution(self):
        self.assertEqual(self.prepare().returncode, 0)
        env = dict(os.environ, BERBR_CONFIG=str(self.root / "different-config"))
        self.assertNotEqual(self.close(env=env).returncode, 0)
        self.assert_no_close()

    def test_lost_socket_blocks_no_space_pane_cleanup(self):
        self.data["plan"]["remove"] = []
        self.data["plan"]["close_space"] = False
        self.data["task_pane_open"] = True
        self.save()
        env = dict(os.environ, HC_TERMINAL_SOCKET=str(self.root / "herdr.sock"))
        env.pop("BERBR_BACKEND", None)
        env.pop("HERDR_ENV", None)
        self.assertEqual(self.prepare(env=env).returncode, 0)
        del env["HC_TERMINAL_SOCKET"]
        self.assertNotEqual(self.close(env=env).returncode, 0)
        self.assertEqual(self.status(), "blocked")
        self.assert_no_close()
        self.assertTrue(json.loads(self.fixture.read_text())["task_pane_open"])

    def test_launch_argv_preserves_context_in_different_core_environment(self):
        env = dict(os.environ, HC_TERMINAL_SOCKET=str(self.root / "herdr.sock"),
                   BERBR_BACKEND="herdr-app", BERBR_CONFIG=str(self.root / "config.toml"))
        env.pop("HERDR_ENV", None)
        self.assertEqual(self.prepare(env=env).returncode, 0)
        argv = json.loads((self.state / "launch.json").read_text())
        self.assertIn("--no-parent", argv)
        self.assertIn("--outlive-parent", argv)
        self.assertEqual(argv[argv.index("--cwd") + 1], str(self.cwd))
        core_env = dict(env, HERDR_ENV="1", BERBR_BACKEND="legacy")
        core_env.pop("BERBR_CONFIG")
        core_env.pop("HC_TERMINAL_SOCKET")
        # Simulate only the program started by hc; never contact a live core.
        result = subprocess.run(argv[argv.index("env"):], cwd=self.cwd, env=core_env,
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.status(), "closed")

    def test_squash_merge_deleted_source_can_close_with_confirmed_head(self):
        self.git("checkout", "-qb", "feature")
        (self.tree / "feature.txt").write_text("finished feature")
        self.git("add", "feature.txt")
        self.git("-c", "user.name=Demo", "-c", "user.email=demo@example.com",
                 "commit", "-qm", "feature")
        head = self.git("rev-parse", "HEAD").decode().strip()
        self.git("checkout", "-qb", "integration", "HEAD~1")
        self.git("merge", "--squash", "feature")
        self.git("-c", "user.name=Demo", "-c", "user.email=demo@example.com",
                 "commit", "-qm", "squashed feature")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")
        self.git("symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main")
        self.git("checkout", "-q", "feature")
        self.assertEqual(self.git("diff", "integration", "feature"), b"")
        self.assertEqual(self.git("rev-list", "--count", "origin/HEAD..HEAD").strip(), b"1")
        self.data["plan"]["remove"][0]["unpushed"] = 1
        self.save()
        result = self.prepare("--merged-mr-head", f"{self.tree}={head}")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.close().returncode, 0)
        self.assertEqual(self.status(), "closed")

    def test_wrong_provider_head_never_reaches_cleanup(self):
        self.data["plan"]["remove"][0]["unpushed"] = 1
        self.save()
        self.assertNotEqual(self.prepare("--merged-mr-head", f"{self.tree}={'0' * 40}").returncode, 0)
        self.assert_no_close()

    def test_positive_count_on_unmerged_branch_rejected_even_with_head_assertion(self):
        head = self.git("rev-parse", "HEAD").decode().strip()
        self.data["plan"]["remove"][0].update(unpushed=1, merged=False)
        self.save()
        self.assertNotEqual(self.prepare("--merged-mr-head", f"{self.tree}={head}").returncode, 0)
        self.assert_no_close()

    def test_jira_failure_records_partial_and_preserves_log(self):
        self.assertEqual(self.prepare().returncode, 0)
        self.data["jira_error"] = True
        self.save()
        self.assertNotEqual(self.close().returncode, 0)
        self.assertEqual(self.status(), "partial")
        self.assertFalse(self.tree.exists())
        self.assertFalse(json.loads(self.fixture.read_text())["done"])
        self.assertIn("Jira error", (self.state / "done.log").read_text())

    def test_ui_failure_is_partial_even_if_note_closed(self):
        self.assertEqual(self.prepare().returncode, 0)
        self.data["ui_error"] = True
        self.save()
        self.assertNotEqual(self.close().returncode, 0)
        self.assertEqual(self.status(), "partial")
        self.assertTrue(json.loads(self.fixture.read_text())["done"])

    def test_unobservable_completion_is_not_success(self):
        self.assertEqual(self.prepare().returncode, 0)
        self.data["show_error"] = True
        self.save()
        self.assertNotEqual(self.close().returncode, 0)
        self.assertEqual(self.status(), "partial")


if __name__ == "__main__":
    unittest.main()
