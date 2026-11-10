#!/usr/bin/env python3
"""Guard an exact Berbr cleanup; remote landing must be verified by the agent."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys


def config_context():
    # Paths/backend selection only; never persist tokens or the full environment.
    keys = ("BERBR_CONFIG", "WORKTABLE_CONFIG", "HERDR_PLUGIN_CONFIG_DIR",
            "BERBR_HOME", "XDG_CONFIG_HOME", "BERBR_BACKEND", "HERDR_BIN_PATH",
            "HERDR_ENV", "HC_TERMINAL_SOCKET")
    return {"user_home": str(Path.home()),
            "env": {key: os.environ.get(key) for key in keys}}


def run(argv, cwd):
    return subprocess.check_output(argv, cwd=cwd, text=True, stderr=subprocess.PIPE)


def write_json(path, value):
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n")
    temporary.replace(path)


def receipt(state, status, **details):
    write_json(state / "receipt.json", {"status": status, **details})


def inside(path, root):
    return path == root or root in path.parents


def plan_for(binary, project, task, cwd, merged_mr_heads):
    plan = json.loads(run([binary, "-p", project, "done", task,
                           "--dry-run", "--json"], cwd))
    if plan.get("kind") != "task" or plan.get("name") != task:
        raise ValueError("dry-run does not identify the exact task")
    for key in ("blockers", "dirty", "keep"):
        if plan.get(key) != []:
            raise ValueError(f"unsafe or missing {key}: {plan.get(key)}")
    if plan.get("note_done") is not True:
        raise ValueError("plan does not close the task note")
    if type(plan.get("close_space")) is not bool:
        raise ValueError("unknown space cleanup")
    for key in ("remove", "close_tickets", "leave_tickets"):
        if not isinstance(plan.get(key), list):
            raise ValueError(f"missing {key}")
    paths = set()
    for tree in plan["remove"]:
        if type(tree.get("uncommitted")) is not int or tree["uncommitted"] != 0:
            raise ValueError(f"dirty or unknown inspection: {tree}")
        if type(tree.get("unpushed")) is not int or tree["unpushed"] < 0:
            raise ValueError(f"unknown unpushed inspection: {tree}")
        if type(tree.get("merged")) is not bool:
            raise ValueError("unknown branch inspection")
        path = Path(tree["path"])
        if not path.is_absolute() or str(path.resolve()) in paths:
            raise ValueError("non-absolute or duplicate worktree path")
        if tree["unpushed"] and (not tree["merged"] or str(path.resolve()) not in merged_mr_heads):
            raise ValueError("positive unpushed count requires a provider-confirmed merged MR HEAD")
        paths.add(str(path.resolve()))
    # Steps are display text; every operational field must remain identical.
    return {key: value for key, value in plan.items() if key != "steps"}


def inspect(plan, stable_paths, merged_mr_heads):
    heads = {}
    for tree in plan["remove"]:
        path = Path(tree["path"]).resolve(strict=True)
        if any(inside(p.resolve(), path) for p in stable_paths):
            raise ValueError(f"runner, cwd or state would be deleted: {path}")
        if Path(run(["git", "rev-parse", "--show-toplevel"], path).strip()).resolve() != path:
            raise ValueError(f"not a worktree root: {path}")
        if run(["git", "status", "--porcelain", "--untracked-files=all"], path):
            raise ValueError(f"worktree has local changes: {path}")
        heads[str(path)] = run(["git", "rev-parse", "HEAD"], path).strip()
    for path, head in merged_mr_heads.items():
        if heads.get(path) != head:
            raise ValueError(f"provider-confirmed MR HEAD does not match the worktree: {path}")
    return heads


def prepare(args):
    state = Path(args.state_dir).resolve()
    cwd = Path(args.cwd).resolve(strict=True)
    binary = shutil.which(args.berbr)
    if not binary:
        raise ValueError("berbr executable not found")
    binary = str(Path(binary).resolve(strict=True))
    merged_mr_heads = {}
    for item in args.merged_mr_head:
        path, head = item.rsplit("=", 1)
        path = str(Path(path).resolve(strict=True))
        if path in merged_mr_heads or not head:
            raise ValueError("duplicate or missing provider-confirmed MR HEAD")
        merged_mr_heads[path] = head
    plan = plan_for(binary, args.project, args.task, cwd, merged_mr_heads)
    runtime = Path(sys.executable).resolve(strict=True)
    heads = inspect(plan, [state, cwd, Path(binary), runtime], merged_mr_heads)
    state.mkdir(mode=0o700, parents=True, exist_ok=False)
    try:
        shutil.copyfile(Path(__file__).resolve(), state / "close_task.py")
        context = config_context()
        write_json(state / "manifest.json", {
            "project": args.project, "task": args.task, "cwd": str(cwd),
            "berbr": binary, "plan": plan, "heads": heads,
            "merged_mr_heads": merged_mr_heads,
            "config_context": context,
        })
        # hc programs inherit the core's environment, not the calling harness's.
        # Carry only the explicitly reviewed config/backend context, never tokens.
        environment = ["env"]
        for key, value in context["env"].items():
            if value is None:
                environment.extend(["-u", key])
        environment.extend(f"{key}={value}" for key, value in context["env"].items()
                           if value is not None)
        suffix = hashlib.sha256(str(state).encode()).hexdigest()[:12]
        write_json(state / "launch.json", [
            "hc", "run", "--bg", "--no-parent", "--outlive-parent",
            "--label", "where=cleanup", "--name", f"done-{suffix}", "--cwd", str(cwd),
            *environment, str(runtime), str(state / "close_task.py"), "close",
            "--state-dir", str(state),
        ])
        receipt(state, "prepared")
    except Exception:
        receipt(state, "error", reason="preparation incomplete; do not execute")
        raise
    print(state)


def close(args):
    state = Path(args.state_dir).resolve(strict=True)
    # Never replace or remove this lock automatically, including after failure.
    lock = os.open(state / "execution.lock", os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    os.close(lock)
    started = False
    try:
        if json.loads((state / "receipt.json").read_text())["status"] != "prepared":
            raise ValueError("state is not prepared")
        manifest = json.loads((state / "manifest.json").read_text())
        if config_context() != manifest["config_context"]:
            raise ValueError("Berbr config environment changed; use the prepared context")
        binary, cwd = manifest["berbr"], manifest["cwd"]
        project, task = manifest["project"], manifest["task"]
        merged_mr_heads = manifest["merged_mr_heads"]
        plan = plan_for(binary, project, task, cwd, merged_mr_heads)
        if plan != manifest["plan"]:
            raise ValueError("cleanup plan changed; inspect the new scope")
        heads = inspect(plan, [state, Path(cwd), Path(binary), Path(__file__)], merged_mr_heads)
        if heads != manifest["heads"]:
            raise ValueError("worktree HEAD changed")
        receipt(state, "closing", project=project, task=task)
        started = True
        with (state / "done.log").open("w") as log:
            result = subprocess.run([binary, "-p", project, "done", task, "--yes"],
                                    cwd=cwd, stdout=log, stderr=subprocess.STDOUT)
        remaining = [t["path"] for t in plan["remove"] if Path(t["path"]).exists()]
        done = None
        observation_error = None
        try:
            observed = json.loads(run([binary, "-p", project, "task", "show", task], cwd))
            if observed.get("id") != task:
                raise ValueError("task show returned a different task")
            done = observed.get("done")
        except Exception as error:
            observation_error = str(error)
        status = "closed" if result.returncode == 0 and done is True and not remaining else "partial"
        receipt(state, status, project=project, task=task, exit_code=result.returncode,
                note_done=done, remaining=remaining, observation_error=observation_error)
        print(status, state / "receipt.json")
        return 0 if status == "closed" else 1
    except Exception as error:
        receipt(state, "partial" if started else "blocked", reason=str(error))
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="mode", required=True)
    prep = sub.add_parser("prepare")
    prep.add_argument("--project", required=True)
    prep.add_argument("--task", required=True)
    prep.add_argument("--cwd", required=True)
    prep.add_argument("--state-dir", required=True)
    prep.add_argument("--berbr", default="berbr")
    prep.add_argument("--merged-mr-head", action="append", default=[], metavar="PATH=SHA",
                      help="provider-confirmed merged MR source HEAD; permits known positive unpushed count")
    closing = sub.add_parser("close")
    closing.add_argument("--state-dir", required=True)
    args = parser.parse_args()
    try:
        return prepare(args) if args.mode == "prepare" else close(args)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print(f"cleanup stopped: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
