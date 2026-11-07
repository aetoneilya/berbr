#![cfg(unix)]
mod common;
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output},
};

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn git(&self, path: &std::path::Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(path)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }
    fn new() -> Self {
        let root = common::temp_dir("berbr-app");
        for path in ["bin", "vault/tasks", "repo", "home"] {
            fs::create_dir_all(root.join(path)).unwrap();
        }
        let root = root.canonicalize().unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(root.join("repo"))
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        git(&["init", "-q"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "fixture",
        ]);
        fs::write(root.join("config.toml"),format!("[vault]\npath = {:?}\n[[project]]\nname = \"test\"\ntask_dirs = [\"tasks\"]\n[[project.repo]]\npath = {:?}\n",root.join("vault"),root.join("repo"))).unwrap();
        fs::write(
            root.join("vault/tasks/test-0.md"),
            "---\nid: test-0\ntitle: 'Task \"quotes\"'\n---\nDescription with Unicode: задача\n",
        )
        .unwrap();
        let script = r#"#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
r=Path(os.environ['BERBR_TEST_ROOT']); kind=Path(sys.argv[0]).name; a=sys.argv[1:]
with (r/'calls.jsonl').open('a') as f: f.write(json.dumps([kind,a])+'\n')
if kind=='herdr-app':
 if a==['status']: out={'space':'sp-caller','tab':'tb-caller','focused_pane':'p-caller'}
 elif a==['ls']:
  if os.environ.get('BERBR_TEST_DOWN'): sys.exit(1)
  out={'spaces':[{'id':'sp-local','name':os.environ.get('BERBR_TEST_SPACE_NAME','test-0'),'machine':None,'tabs':[{'id':'tb-local','name':'local','terminals':[{'terminal':'t1','pane':'p1','app':'codex','status':'idle'}]}]}, {'id':'prod/sp-remote','name':'remote','machine':'prod','tabs':[{'id':'prod/tb-remote','terminals':[{'terminal':'prod/t2','app':'codex','status':'working'}]}]}]}
 elif a[:2]==['do','space.new']: out={'space':'sp-created','tab':'tb-created','terminal':'t7'}
 elif a[:2]==['do','tab.new']: out={'tab':'tb-created','terminal':'t7'}
 elif a[:2]==['do','pane.split']: out={'terminal':'t7'}
 else: out={'ok':True}
else:
 if a[:1]==['get']:
  busy=os.environ.get('BERBR_TEST_BUSY') and a[1]=='t7'
  starting=False
  if os.environ.get('BERBR_TEST_STARTING') and a[1]=='t7':
   count=r/'ready-count'; n=int(count.read_text()) if count.exists() else 0; count.write_text(str(n+1)); starting=n==0
  out={'address':a[1],'cwd':os.environ.get('BERBR_TEST_AGENT_CWD',str(r/'repo')),'foreground':None if starting else {'name':'codex' if busy else 'zsh','cwd':os.environ.get('BERBR_TEST_AGENT_CWD',str(r/'repo'))},'labels':{},'status':'working' if busy else None}
 else: out={}
print(json.dumps(out))
"#;
        for bin in ["herdr-app", "hc"] {
            let path = root.join("bin").join(bin);
            fs::write(&path, script).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self { root }
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_berbr"));
        cmd.current_dir(self.root.join("repo"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.root.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("HOME", self.root.join("home"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("BERBR_CONFIG", self.root.join("config.toml"))
            .env("BERBR_BACKEND", "herdr-app")
            .env("BERBR_TEST_ROOT", &self.root)
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude"))
            .env("CODEX_HOME", self.root.join("codex"))
            .env_remove("HERDR_PLUGIN_CONTEXT_JSON");
        cmd
    }
    fn calls(&self) -> Vec<Value> {
        fs::read_to_string(self.root.join("calls.jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn success(out: Output) -> Value {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn native_snapshot_preserves_notes_and_never_queries_remote_machines() {
    let f = Fixture::new();
    let path = f.root.join("vault/tasks/test-0.md");
    let before = fs::read(&path).unwrap();
    let data = success(
        f.command()
            .args(["app-snapshot", "--cached"])
            .output()
            .unwrap(),
    );
    assert_eq!(data["version"], 3);
    assert!(data.to_string().contains("задача"));
    assert_eq!(fs::read(path).unwrap(), before);
    assert!(f.calls().iter().any(|c| c[0] == "hc" && c[1][0] == "get"));
    assert!(!f.calls().iter().any(|c| c.to_string().contains("prod/")));
}

fn find_tree(value: &Value) -> &Value {
    if value.get("kind").is_some_and(|kind| kind == "tree") {
        return value;
    }
    value
        .as_array()
        .or_else(|| value["children"].as_array())
        .into_iter()
        .flatten()
        .find_map(|child| {
            let tree = find_tree(child);
            (!tree.is_null()).then_some(tree)
        })
        .unwrap_or(&Value::Null)
}

#[test]
fn snapshot_diff_uses_merge_base_and_includes_committed_staged_and_unstaged_changes() {
    let f = Fixture::new();
    let repo = f.root.join("repo");
    f.git(&repo, &["branch", "-M", "main"]);
    fs::write(repo.join("file"), "one\ntwo\n").unwrap();
    f.git(&repo, &["add", "file"]);
    f.git(&repo, &["commit", "-qm", "base"]);
    let tree = repo.join(".claude/worktrees/work");
    f.git(
        &repo,
        &[
            "worktree",
            "add",
            "-qb",
            "TEST-1-work",
            tree.to_str().unwrap(),
        ],
    );
    fs::write(
        f.root.join("vault/tasks/test-0.md"),
        "---\nid: test-0\ntickets: [TEST-1]\n---\n",
    )
    .unwrap();
    let snapshot = || {
        success(
            f.command()
                .args(["app-snapshot", "--cached"])
                .output()
                .unwrap(),
        )
    };
    let data = snapshot();
    assert_eq!(find_tree(&data["rows"][0])["diff"], Value::Null);
    // The configured local fallback works without origin/HEAD.
    let config = f.root.join("config.toml");
    fs::write(
        &config,
        format!(
            "{}\nmain_branch = \"main\"\n",
            fs::read_to_string(&config).unwrap()
        ),
    )
    .unwrap();
    assert_eq!(
        find_tree(&snapshot()["rows"][0])["diff"],
        serde_json::json!({"adds":0,"dels":0,"files":0})
    );
    fs::write(tree.join("file"), "one\ntwo\nthree\n").unwrap();
    fs::write(tree.join("committed"), "commit\n").unwrap();
    f.git(&tree, &["add", "file", "committed"]);
    f.git(&tree, &["commit", "-qm", "branch work"]);
    fs::write(tree.join("staged"), "stage\n").unwrap();
    f.git(&tree, &["add", "staged"]);
    fs::write(tree.join("file"), "two\nthree\n").unwrap();
    fs::write(repo.join("upstream"), "upstream\n").unwrap();
    f.git(&repo, &["add", "upstream"]);
    f.git(&repo, &["commit", "-qm", "default advanced"]);
    f.git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    f.git(
        &repo,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    // origin/HEAD takes precedence over an invalid configured fallback.
    fs::write(
        &config,
        fs::read_to_string(&config)
            .unwrap()
            .replace("main_branch = \"main\"", "main_branch = \"missing\""),
    )
    .unwrap();
    assert_eq!(
        find_tree(&snapshot()["rows"][0])["diff"],
        serde_json::json!({"adds":3,"dels":1,"files":3})
    );
    f.git(&tree, &["checkout", "--detach"]);
    let data = snapshot();
    let detached = find_tree(&data["rows"]);
    assert_eq!(detached["kind"], "tree");
    assert_eq!(detached["diff"], Value::Null);
}

#[test]
fn add_existing_key_creates_named_worktree_without_prompts_or_focus() {
    for kind in ["codex", "claude"] {
        let f = Fixture::new();
        let repo = f.root.join("repo");
        let caller = repo.join(".claude/worktrees/caller");
        f.git(
            &repo,
            &[
                "worktree",
                "add",
                "-qb",
                "TEST-1-caller",
                caller.to_str().unwrap(),
            ],
        );
        let note = f.root.join("vault/tasks/test-0.md");
        fs::write(
            &note,
            "---\nid: test-0\ntickets: [TEST-1]\n---\nKeep context\n",
        )
        .unwrap();
        let before = fs::read(&note).unwrap();
        let out = f
            .command()
            .current_dir(&caller)
            .env_remove("HERDR_WORKSPACE_ID")
            .args([
                "add",
                "TEST-1",
                "--name",
                "followup",
                "--description",
                "Do the followup",
                "--agent",
                kind,
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let tree = repo.join(".herdr/worktrees/followup");
        assert!(tree.is_dir());
        assert_eq!(
            f.git(&tree, &["branch", "--show-current"]),
            "TEST-1-followup"
        );
        let output = String::from_utf8(out.stdout).unwrap();
        assert_eq!(output.lines().count(), 1);
        assert!(output.contains(tree.to_str().unwrap()));
        assert!(output.contains("tab: tb-created; terminal: t7"));
        assert_eq!(fs::read(&note).unwrap(), before);
        let calls = f.calls();
        assert!(
            calls
                .iter()
                .any(|c| c[1][1] == "tab.new" && c[1][2].as_str().unwrap().contains("sp-local"))
        );
        assert!(
            !calls.iter().any(
                |c| c[1][1] == "tab.select" && c[1][2].as_str().unwrap().contains("tb-created")
            )
        );
        let send = calls
            .iter()
            .find(|c| c[0] == "hc" && c[1][0] == "send")
            .unwrap();
        assert!(send[1][2].as_str().unwrap().contains(kind));
        assert!(
            send[1][2]
                .as_str()
                .unwrap()
                .contains("work on the task description")
        );
        let error = f
            .command()
            .args(["add", "test-0", "TEST-1"])
            .output()
            .unwrap();
        assert!(!error.status.success());
        assert!(error.stdout.is_empty());
        let error = String::from_utf8(error.stderr).unwrap();
        assert_eq!(error.lines().count(), 1);
        assert!(error.contains("--name") && error.contains("--in"));
        let reuse = f
            .command()
            .args(["add", "test-0", "TEST-1", "--in", "followup"])
            .output()
            .unwrap();
        assert!(
            reuse.status.success(),
            "{}",
            String::from_utf8_lossy(&reuse.stderr)
        );
        for args in [
            vec!["add", "test-0", "--in", "followup"],
            vec!["add", "--in", "followup"],
        ] {
            let calls_before = f.calls().len();
            let reuse = f
                .command()
                .current_dir(&tree)
                .env_remove("HERDR_WORKSPACE_ID")
                .args(args)
                .args(["--agent", kind])
                .output()
                .unwrap();
            assert!(
                reuse.status.success(),
                "{}",
                String::from_utf8_lossy(&reuse.stderr)
            );
            assert_eq!(fs::read(&note).unwrap(), before);
            let git_dir = f.git(&tree, &["rev-parse", "--absolute-git-dir"]);
            let brief = fs::read_to_string(PathBuf::from(git_dir).join("task.md")).unwrap();
            assert!(brief.contains(note.to_str().unwrap()));
            assert!(brief.contains("Other tickets of the task: TEST-1."));
            assert!(brief.contains("See the task note."));
            let calls = f.calls();
            let added = &calls[calls_before..];
            assert!(added.iter().any(|c| c[1][1] == "tab.new"));
            assert!(!added.iter().any(
                |c| c[1][1] == "tab.select" && c[1][2].as_str().unwrap().contains("tb-created")
            ));
            let send = added
                .iter()
                .find(|c| c[0] == "hc" && c[1][0] == "send")
                .unwrap();
            let command = send[1][2].as_str().unwrap();
            assert!(command.contains(kind));
            assert!(!command.contains("--resume") && !command.contains("--continue"));
        }
        for args in [vec!["add"], vec!["add", "test-0"]] {
            let error = f.command().args(args).output().unwrap();
            assert!(!error.status.success());
            assert!(String::from_utf8_lossy(&error.stderr).contains("berbr add [<task>] <KEY>"));
        }
        // Missing current context must return an error instead of opening a picker.
        let error = f
            .command()
            .env_remove("HERDR_WORKSPACE_ID")
            .args(["add", "TEST-1", "--name", "another"])
            .output()
            .unwrap();
        assert!(!error.status.success());
        assert!(String::from_utf8_lossy(&error.stderr).contains("pass its ID explicitly"));
    }
}

#[test]
fn add_reports_checkout_and_launch_failures_to_the_caller() {
    let f = Fixture::new();
    let note = f.root.join("vault/tasks/test-0.md");
    fs::write(&note, "---\nid: test-0\ntickets: [TEST-1]\n---\n").unwrap();
    f.git(&f.root.join("repo"), &["branch", "TEST-1-collision"]);
    // This branch is already checked out elsewhere, so checkout must fail here.
    let other = f.root.join("elsewhere");
    f.git(
        &f.root.join("repo"),
        &[
            "worktree",
            "add",
            other.to_str().unwrap(),
            "TEST-1-collision",
        ],
    );
    let out = f
        .command()
        .args(["add", "test-0", "TEST-1", "--name", "collision"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert_eq!(String::from_utf8_lossy(&out.stderr).lines().count(), 1);
    let out = f
        .command()
        .env("BERBR_TEST_BUSY", "1")
        .args(["add", "test-0", "TEST-1", "--name", "busy"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not an available shell"));
}

#[test]
fn app_launch_restores_caller_and_keeps_codex_arguments() {
    let f = Fixture::new();
    let out = f
        .command()
        .args(["start", "new task", "--agent", "codex", "--bg"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let calls = f.calls();
    assert!(
        calls
            .iter()
            .any(|c| c[0] == "herdr-app" && c[1][1] == "space.new")
    );
    assert!(
        calls
            .iter()
            .any(|c| c[1][1] == "pane.focus" && c[1][2].as_str().unwrap().contains("p-caller"))
    );
    let send = calls
        .iter()
        .find(|c| c[0] == "hc" && c[1][0] == "send")
        .unwrap();
    assert_eq!(send[1][1], "t7");
    assert!(send[1][2].as_str().unwrap().contains("codex --no-daemon"));
    assert!(
        send[1][2]
            .as_str()
            .unwrap()
            .contains("BERBR_BACKEND=herdr-app")
    );
}

#[test]
fn app_launch_never_types_into_a_busy_terminal() {
    let f = Fixture::new();
    let out = f
        .command()
        .env("BERBR_TEST_BUSY", "1")
        .args(["start", "new task", "--bg"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not an available shell"));
    assert!(!f.calls().iter().any(|c| c[0] == "hc" && c[1][0] == "send"));
}

#[test]
fn unavailable_app_does_not_look_like_an_empty_agent_list() {
    let f = Fixture::new();
    let out = f
        .command()
        .env("BERBR_TEST_DOWN", "1")
        .args(["app-snapshot", "--cached"])
        .output()
        .unwrap();
    let data = success(out);
    assert!(
        data["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("Herdr"))
    );
}

#[test]
fn app_launch_waits_for_the_new_shell_foreground() {
    let f = Fixture::new();
    let out = f
        .command()
        .env("BERBR_TEST_STARTING", "1")
        .args(["start", "new task", "--bg"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let gets = f
        .calls()
        .into_iter()
        .filter(|c| c[0] == "hc" && c[1][0] == "get" && c[1][1] == "t7")
        .count();
    assert!(gets >= 2);
    assert!(f.calls().iter().any(|c| c[0] == "hc" && c[1][0] == "send"));
}

#[test]
fn done_dry_run_preserves_notes_worktrees_branches_and_caches() {
    let f = Fixture::new();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(f.root.join("repo"))
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    };
    git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    let wt = f.root.join("repo/.claude/worktrees/test-0");
    git(&[
        "worktree",
        "add",
        "-qb",
        "DEMO-1-test",
        wt.to_str().unwrap(),
    ]);
    let note = f.root.join("vault/tasks/test-0.md");
    fs::write(
        &note,
        "---\nid: test-0\ntickets: [DEMO-1]\n---\nkeep this body\n",
    )
    .unwrap();
    // A transcript makes an accidental sessions-cache save observable too.
    let slug: String = wt
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let transcripts = f.root.join("claude/projects").join(slug);
    fs::create_dir_all(&transcripts).unwrap();
    fs::write(
        transcripts.join("session.jsonl"),
        format!(
            "{{\"cwd\":{:?},\"timestamp\":\"2026-11-04T00:00:00Z\"}}\n",
            wt
        ),
    )
    .unwrap();
    fn cache_files(
        path: &std::path::Path,
    ) -> std::collections::BTreeMap<PathBuf, (Vec<u8>, std::time::SystemTime)> {
        let mut files = std::collections::BTreeMap::new();
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(cache_files(&path));
            } else {
                files.insert(
                    path.clone(),
                    (
                        fs::read(&path).unwrap(),
                        fs::metadata(&path).unwrap().modified().unwrap(),
                    ),
                );
            }
        }
        files
    }
    let caches = [f.root.join("cache"), f.root.join("home/Library/Caches")];
    for cache in &caches {
        let dir = cache.join("berbr");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("remote-test.json"), "{\"tickets\":{},\"mrs\":[]}").unwrap();
        fs::write(dir.join("sessions-v3.json"), "{}").unwrap();
    }
    let cached_before: Vec<_> = caches.iter().map(|p| cache_files(p)).collect();
    let before = fs::read(&note).unwrap();
    let index_path = PathBuf::from(
        String::from_utf8(git(&[
            "-C",
            wt.to_str().unwrap(),
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "index",
        ]))
        .unwrap()
        .trim(),
    );
    let index_before = fs::read(&index_path).unwrap();
    for args in [
        vec!["done", "test-0", "--dry-run", "--json"],
        vec!["done", "DEMO-1", "--dry-run", "--json"],
        vec!["done", "--tree", "DEMO-1-test", "--dry-run", "--json"],
        vec!["done", "--merged", "--dry-run", "--json"],
        vec![
            "done",
            "--tree",
            "DEMO-1-test",
            "--force",
            "--dry-run",
            "--json",
        ],
    ] {
        let plan = success(
            f.command()
                .env_remove("JIRA_TOKEN")
                .env_remove("GITLAB_TOKEN")
                .args(&args)
                .output()
                .unwrap(),
        );
        assert!(plan["blockers"].as_array().unwrap().is_empty());
        assert_eq!(
            plan["note_done"],
            args[1] == "test-0" || args[1] == "DEMO-1"
        );
        assert_eq!(fs::read(&note).unwrap(), before);
        assert!(wt.is_dir());
        git(&["show-ref", "--verify", "refs/heads/DEMO-1-test"]);
        assert_eq!(fs::read(&index_path).unwrap(), index_before);
        for (cache, before) in caches.iter().zip(&cached_before) {
            assert_eq!(
                &cache_files(cache),
                before,
                "dry-run wrote {}",
                cache.display()
            );
        }
    }
    let human = f
        .command()
        .args(["done", "test-0", "--dry-run"])
        .output()
        .unwrap();
    assert!(human.status.success());
    assert!(String::from_utf8_lossy(&human.stdout).contains("mark the note done"));
    assert_eq!(fs::read(&note).unwrap(), before);
    let invalid = f
        .command()
        .args(["done", "test-0", "--json"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    fs::write(wt.join("dirty"), "uncommitted").unwrap();
    let plan = success(
        f.command()
            .args(["done", "--tree", "DEMO-1-test", "--dry-run", "--json"])
            .output()
            .unwrap(),
    );
    assert!(!plan["blockers"].as_array().unwrap().is_empty());
    assert_eq!(plan["keep"][0]["reason"], "uncommitted changes");
    assert!(wt.join("dirty").exists());
    // No token means no network request. Partial task data is a plan blocker,
    // and dry-run still returns one successful JSON response.
    let config = f.root.join("config.toml");
    let configured = fs::read_to_string(&config).unwrap().replace(
        "task_dirs = [\"tasks\"]",
        "task_dirs = [\"tasks\"]\ngitlab = { host = \"gitlab.invalid\", project = \"ai/test\" }",
    );
    fs::write(&config, configured).unwrap();
    let partial = success(
        f.command()
            .env_remove("GITLAB_TOKEN")
            .args(["done", "test-0", "--dry-run", "--json"])
            .output()
            .unwrap(),
    );
    assert!(!partial["blockers"].as_array().unwrap().is_empty());
    assert_eq!(fs::read(&note).unwrap(), before);
    for (cache, before) in caches.iter().zip(&cached_before) {
        assert_eq!(&cache_files(cache), before);
    }
    for cache in &caches {
        fs::remove_file(cache.join("berbr/remote-test.json")).unwrap();
    }
    for args in [
        vec!["done", "--tree", "DEMO-1-test", "--dry-run", "--json"],
        vec!["done", "--merged", "--dry-run", "--json"],
    ] {
        let missing = success(
            f.command()
                .env_remove("GITLAB_TOKEN")
                .args(args)
                .output()
                .unwrap(),
        );
        assert!(
            missing["blockers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|b| b.as_str().unwrap().contains("partial data"))
        );
        assert!(wt.exists());
    }
    assert!(
        f.calls()
            .iter()
            .all(|c| c[1][0] == "ls" || c[1][0] == "get" || c[1][0] == "status")
    );
}

#[test]
fn start_uses_title_slug_and_claude_display_name_in_prepare_and_main_checkout() {
    for no_worktree in [false, true] {
        let f = Fixture::new();
        let mut cmd = f.command();
        cmd.args([
            "start",
            "Parser: исправить repeated calls with cache",
            "--bg",
        ]);
        if no_worktree {
            cmd.arg("--no-worktree");
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let calls = f.calls();
        let create = calls.iter().find(|c| c[1][1] == "space.new").unwrap();
        let args: Value = serde_json::from_str(create[1][2].as_str().unwrap()).unwrap();
        assert_eq!(args["name"], "Parser: исправить repeated calls with cache");
        let tab = calls.iter().find(|c| c[1][1] == "tab.rename").unwrap();
        let args: Value = serde_json::from_str(tab[1][2].as_str().unwrap()).unwrap();
        assert_eq!(args["name"], "parser-repeated-calls-with");
        let send = calls
            .iter()
            .find(|c| c[0] == "hc" && c[1][0] == "send")
            .unwrap();
        let command = send[1][2].as_str().unwrap();
        let label = if no_worktree {
            "Parser: исправить repeated calls with cache"
        } else {
            "parser-repeated-calls-with"
        };
        let flag = if no_worktree { "'--name'" } else { "--name" };
        assert!(command.contains(&format!("{flag} '{label}'")), "{command}");
        if !no_worktree {
            let spec_path = f.root.join("home/Library/Caches/berbr/prepare-t7.json");
            // Cache root differs on Linux and macOS.
            let spec_path = if spec_path.exists() {
                spec_path
            } else {
                f.root.join("cache/berbr/prepare-t7.json")
            };
            let spec: Value = serde_json::from_slice(&fs::read(spec_path).unwrap()).unwrap();
            assert_eq!(spec["name"], "parser-repeated-calls-with");
            assert_eq!(spec["branch"], "parser-repeated-calls-with");
        }
    }
}

#[test]
fn rename_preserves_identity_body_and_branches_and_renames_existing_space_and_caller_tab() {
    for legacy in [false, true] {
        let f = Fixture::new();
        let path = f.root.join("vault/tasks/test-0.md");
        let header = if legacy { "" } else { "id: test-0\n" };
        fs::write(&path, format!("---\n{header}title: Old title\ntags: [keep]\n---\nKeep body\n\n## Agent notes\nKeep finding\n")).unwrap();
        let branches = f.git(&f.root.join("repo"), &["branch", "--list"]);
        let out = f
            .command()
            .env("HERDR_TAB_ID", "tb-caller")
            .env(
                "BERBR_TEST_SPACE_NAME",
                if legacy { "OLD TITLE" } else { "test-0" },
            )
            .args(["rename", "test-0", "New title", "--tab", "new-tab"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout).lines().count(), 3);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("title: New title"));
        assert!(text.contains("tags: [keep]"));
        assert!(text.ends_with("Keep body\n\n## Agent notes\nKeep finding\n"));
        assert_eq!(text.contains("id:"), !legacy);
        assert!(!path.with_file_name("New title.md").exists());
        assert_eq!(f.git(&f.root.join("repo"), &["branch", "--list"]), branches);
        let calls = f.calls();
        for (command, id, name) in [
            ("space.rename", "sp-local", "New title"),
            ("tab.rename", "tb-caller", "new-tab"),
        ] {
            assert!(calls.iter().any(|c| c[1][1] == command
                && serde_json::from_str::<Value>(c[1][2].as_str().unwrap()).unwrap()
                    == serde_json::json!({"id":id,"name":name})));
        }
    }
}

#[test]
fn rename_resolves_current_worktree_and_caller_terminal_and_can_skip_unknown_tab() {
    let f = Fixture::new();
    let tree = f.root.join("repo/.claude/worktrees/friendly-name");
    f.git(
        &f.root.join("repo"),
        &[
            "worktree",
            "add",
            "-qb",
            "friendly-name",
            tree.to_str().unwrap(),
        ],
    );
    let path = f.root.join("vault/tasks/test-0.md");
    fs::write(
        &path,
        "---\nid: test-0\ntitle: Old title\nworktrees: [friendly-name]\n---\nBody\n",
    )
    .unwrap();
    let out = f
        .command()
        .current_dir(&tree)
        .env_remove("HERDR_TAB_ID")
        .env_remove("HERDR_PANE_ID")
        .env("HC_TERMINAL_ID", "t1")
        .args(["rename", "New title", "--tab", "caller-label"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("tab: tb-local — caller-label"));
    let out = f
        .command()
        .current_dir(&tree)
        .env_remove("HERDR_TAB_ID")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HC_TERMINAL_ID")
        .args(["rename", "Another title", "--tab", "unknown"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("tab: skipped"));
    assert_eq!(f.git(&tree, &["branch", "--show-current"]), "friendly-name");
}

#[test]
fn select_names_first_tree_from_title_and_reuses_title_or_id_space() {
    for space_name in ["test-0", "TASK \"QUOTES\""] {
        let f = Fixture::new();
        let out = f
            .command()
            .env("BERBR_TEST_SPACE_NAME", space_name)
            .args(["select", "test-0", "--bg"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let calls = f.calls();
        assert!(!calls.iter().any(|c| c[1][1] == "space.new"));
        assert!(calls.iter().any(|c| c[1][1] == "tab.new" && serde_json::from_str::<Value>(c[1][2].as_str().unwrap()).unwrap() == serde_json::json!({"space":"sp-local", "name":"task-quotes", "folder":f.root.join("repo")})));
    }
}

#[test]
fn rename_validates_title_and_shares_note_update_with_task_update() {
    let f = Fixture::new();
    let path = f.root.join("vault/tasks/test-0.md");
    let before = fs::read(&path).unwrap();
    for title in [" ", "two\nlines"] {
        let out = f
            .command()
            .args(["rename", "test-0", title])
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("non-empty single line"));
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    let out = f
        .command()
        .args(["task", "update", "test-0", "--title", "Shared title"])
        .output()
        .unwrap();
    assert_eq!(success(out)["title"], "Shared title");
    let out = f
        .command()
        .env("BERBR_TEST_SPACE_NAME", "unrelated")
        .args(["rename", "test-0", "Final title"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).lines().count(), 1);
    assert!(path.exists());
    assert!(!f.calls().iter().any(|c| c[1][1] == "space.rename"));
}

#[test]
fn remove_ticket_named_tree_must_not_resolve_whole_task() {
    let f = Fixture::new();
    fs::write(
        f.root.join("vault/tasks/test-0.md"),
        "---\nid: test-0\ntitle: Actual task\ntickets: [DEMO-12]\n---\nBody\n",
    )
    .unwrap();
    f.git(
        &f.root.join("repo"),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "DEMO-12-work",
            f.root
                .join("repo/.claude/worktrees/DEMO-12")
                .to_str()
                .unwrap(),
        ],
    );
    f.git(
        &f.root.join("repo"),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "DEMO-12-other",
            f.root
                .join("repo/.claude/worktrees/other-work")
                .to_str()
                .unwrap(),
        ],
    );
    f.git(
        &f.root.join("repo"),
        &["update-ref", "refs/remotes/origin/HEAD", "HEAD"],
    );
    let plan = success(
        f.command()
            .args(["done", "--tree", "DEMO-12", "--dry-run", "--json"])
            .output()
            .unwrap(),
    );
    assert!(plan["blockers"].as_array().unwrap().is_empty());
    assert_eq!(plan["remove"].as_array().unwrap().len(), 1);
    assert_eq!(
        plan["kind"], "trees",
        "Remove-worktree argv resolves whole task: {plan}"
    );
}

#[test]
fn add_in_must_reject_another_tasks_worktree() {
    let f = Fixture::new();
    let tree = f.root.join("repo/.claude/worktrees/foreign");
    f.git(
        &f.root.join("repo"),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "foreign-branch",
            tree.to_str().unwrap(),
        ],
    );
    fs::write(
        f.root.join("vault/tasks/test-1.md"),
        "---\nid: test-1\ntitle: Other task\nworktrees: [foreign]\n---\nOther context\n",
    )
    .unwrap();
    let out = f
        .command()
        .args(["add", "test-0", "DEMO-12", "--in", "foreign", "--bg"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "add claims other task's worktree, overwrites its brief and launches new agent: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn native_snapshot_must_report_hc_failure() {
    let f = Fixture::new();
    fs::write(f.root.join("bin/hc"), "#!/bin/sh\nexit 1\n").unwrap();
    let out = f
        .command()
        .args(["app-snapshot", "--cached"])
        .output()
        .unwrap();
    let v = success(out);
    assert!(
        v["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("hc"))
    );
}

#[test]
fn snapshot_must_warn_about_skipped_malformed_notes() {
    let f = Fixture::new();
    fs::write(
        f.root.join("vault/tasks/broken.md"),
        "---\nstate: [\n---\nBody\n",
    )
    .unwrap();
    let out = f
        .command()
        .args(["app-snapshot", "--cached"])
        .output()
        .unwrap();
    assert!(out.status.success());

    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        v["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("broken.md"))
    );
}

#[test]
fn status_must_preserve_quoted_frontmatter() {
    let f = Fixture::new();
    let note = f.root.join("vault/tasks/test-0.md");
    fs::write(
        &note,
        "---\nid: test-0\ntitle: Demo\n\"state\": active\ndone: false\n---\nBody\n",
    )
    .unwrap();
    let out = f.command().arg("status").output().unwrap();
    assert!(out.status.success());
    let saved = fs::read_to_string(&note).unwrap();
    let yaml = saved
        .strip_prefix("---\n")
        .unwrap()
        .split("\n---")
        .next()
        .unwrap();
    assert!(
        serde_yaml::from_str::<serde_yaml::Value>(yaml).is_ok(),
        "status exits successfully after corrupting valid note: {saved}"
    );
}

#[test]
fn task_close_warns_before_discarding_dirty_files_but_tree_remove_refuses() {
    let f = Fixture::new();
    let repo = f.root.join("repo");
    let tree = repo.join(".claude/worktrees/work");
    f.git(
        &repo,
        &["worktree", "add", "-qb", "work", tree.to_str().unwrap()],
    );
    f.git(&repo, &["update-ref", "refs/remotes/origin/HEAD", "HEAD"]);
    fs::write(tree.join("local.txt"), "synthetic uncommitted content").unwrap();
    let note = f.root.join("vault/tasks/test-0.md");
    fs::write(
        &note,
        "---\nid: test-0\nworktrees: [work]\ndone: false\n---\nBody\n",
    )
    .unwrap();
    let plan = success(
        f.command()
            .args(["done", "test-0", "--dry-run", "--json"])
            .output()
            .unwrap(),
    );
    assert!(plan["blockers"].as_array().unwrap().is_empty());
    assert_eq!(plan["dirty"][0]["tree"], "work");
    assert_eq!(plan["dirty"][0]["uncommitted"], 1);
    assert_eq!(plan["remove"][0]["uncommitted"], 1);
    let refused = f
        .command()
        .args(["done", "--tree", "work", "--yes"])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(tree.join("local.txt").exists());
    let closed = f
        .command()
        .args(["done", "test-0", "--yes"])
        .output()
        .unwrap();
    assert!(
        closed.status.success(),
        "{}",
        String::from_utf8_lossy(&closed.stderr)
    );
    assert!(String::from_utf8_lossy(&closed.stdout).contains("work, losing 1 uncommitted"));
    assert!(!tree.exists());
    assert!(fs::read_to_string(note).unwrap().contains("done: true"));
}

#[test]
fn typed_removal_refuses_duplicate_names_and_accepts_repository_or_full_path() {
    let f = Fixture::new();
    let beta = f.root.join("beta");
    fs::create_dir(&beta).unwrap();
    f.git(&beta, &["init", "-q"]);
    f.git(&beta, &["commit", "--allow-empty", "-qm", "base"]);
    fs::OpenOptions::new()
        .append(true)
        .open(f.root.join("config.toml"))
        .map(|mut file| {
            use std::io::Write;
            writeln!(file, "[[project.repo]]\npath = {beta:?}").unwrap();
        })
        .unwrap();
    for repo in [f.root.join("repo"), beta.clone()] {
        let tree = repo.join(".claude/worktrees/same");
        f.git(
            &repo,
            &["worktree", "add", "-qb", "same", tree.to_str().unwrap()],
        );
        f.git(&repo, &["update-ref", "refs/remotes/origin/HEAD", "HEAD"]);
    }
    let untyped = f
        .command()
        .args(["done", "same", "--dry-run", "--json"])
        .output()
        .unwrap();
    assert!(!untyped.status.success());
    assert!(String::from_utf8_lossy(&untyped.stderr).contains("done --tree"));
    let ambiguous = f
        .command()
        .args(["done", "--tree", "same", "--dry-run", "--json"])
        .output()
        .unwrap();
    assert!(!ambiguous.status.success());
    assert!(String::from_utf8_lossy(&ambiguous.stderr).contains("ambiguous"));
    let scoped = success(
        f.command()
            .args([
                "done",
                "--tree",
                "same",
                "--repo",
                beta.to_str().unwrap(),
                "--dry-run",
                "--json",
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(scoped["remove"].as_array().unwrap().len(), 1);
    assert_eq!(
        scoped["remove"][0]["path"],
        beta.join(".claude/worktrees/same").to_str().unwrap()
    );
    let path = beta.join(".claude/worktrees/same");
    let scoped = success(
        f.command()
            .args([
                "done",
                "--tree",
                path.to_str().unwrap(),
                "--dry-run",
                "--json",
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(scoped["remove"][0]["path"], path.to_str().unwrap());
}

#[test]
fn creation_rejects_path_names_before_creating_a_note_or_space() {
    let f = Fixture::new();
    let before = fs::read_dir(f.root.join("vault/tasks")).unwrap().count();
    for args in [
        vec![
            "start",
            "Synthetic task",
            "--name",
            "../outside",
            "--no-agent",
        ],
        vec!["select", "test-0", "--tree", "../outside"],
        vec!["add", "test-0", "DEMO-12", "--name", "../outside"],
    ] {
        let out = f.command().args(args).output().unwrap();
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("single non-empty directory name"));
    }
    assert_eq!(
        fs::read_dir(f.root.join("vault/tasks")).unwrap().count(),
        before
    );
    assert!(!f.calls().iter().any(|c| c[1][0] == "do"));
}

#[test]
fn colliding_task_slugs_get_distinct_checkout_names_and_keep_the_old_brief() {
    let f = Fixture::new();
    let repo = f.root.join("repo");
    let tree = repo.join(".claude/worktrees/refactor-cache-key-handling");
    f.git(
        &repo,
        &[
            "worktree",
            "add",
            "-qb",
            "old-owner",
            tree.to_str().unwrap(),
        ],
    );
    let git_dir = f.git(&tree, &["rev-parse", "--absolute-git-dir"]);
    let brief = PathBuf::from(git_dir).join("task.md");
    fs::write(&brief, "original task brief").unwrap();
    for title in [
        "Refactor cache key handling alpha",
        "Refactor cache key handling beta",
    ] {
        let out = f
            .command()
            .args(["start", title, "--no-agent", "--bg"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert_eq!(fs::read_to_string(brief).unwrap(), "original task brief");
    let names: Vec<String> = f
        .calls()
        .iter()
        .filter(|c| c[1][1] == "tab.rename")
        .map(|c| {
            serde_json::from_str::<Value>(c[1][2].as_str().unwrap()).unwrap()["name"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        names,
        [
            "refactor-cache-key-handling-test-1",
            "refactor-cache-key-handling-test-2"
        ]
    );
}

#[test]
fn native_read_and_focus_reuse_one_inventory_and_focus_the_known_terminal() {
    for select in [false, true] {
        let f = Fixture::new();
        let repo = f.root.join("repo");
        let tree = repo.join(".claude/worktrees/work");
        f.git(
            &repo,
            &["worktree", "add", "-qb", "work", tree.to_str().unwrap()],
        );
        fs::write(
            f.root.join("vault/tasks/test-0.md"),
            "---\nid: test-0\nworktrees: [work]\n---\nBody\n",
        )
        .unwrap();
        let mut command = f.command();
        command.env("BERBR_TEST_AGENT_CWD", &tree);
        if select {
            command.args(["select", "test-0", "--tree", "work"]);
        } else {
            command.args(["app-snapshot", "--cached"]);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let calls = f.calls();
        assert_eq!(
            calls
                .iter()
                .filter(|c| c[0] == "herdr-app" && c[1][0] == "ls")
                .count(),
            1
        );
        assert_eq!(
            calls
                .iter()
                .filter(|c| c[0] == "hc" && c[1][0] == "get")
                .count(),
            1
        );
        if select {
            assert!(
                calls
                    .iter()
                    .any(|c| c[1][1] == "agent.focus" && c[1][2].as_str().unwrap().contains("t1"))
            );
        }
    }
}

#[test]
fn only_snapshots_compute_diffs_and_repository_ref_facts_are_batched() {
    let f = Fixture::new();
    let repo = f.root.join("repo");
    let tree = repo.join(".claude/worktrees/work");
    f.git(
        &repo,
        &["worktree", "add", "-qb", "work", tree.to_str().unwrap()],
    );
    f.git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    f.git(
        &repo,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    fs::write(
        f.root.join("vault/tasks/test-0.md"),
        "---\nid: test-0\nworktrees: [work]\n---\nBody\n",
    )
    .unwrap();
    let wrapper = f.root.join("bin/git");
    fs::write(&wrapper,"#!/usr/bin/env python3\nimport os,sys,json\nwith open(os.environ['BERBR_TEST_GIT_LOG'],'a') as f: f.write(json.dumps(sys.argv[1:])+'\\n')\nos.execv('/usr/bin/git',['git',*sys.argv[1:]])\n").unwrap();
    fs::set_permissions(wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    for (args, snapshot, online) in [
        (vec!["status", "--no-write"], false, true),
        (vec!["done", "test-0", "--dry-run", "--json"], false, true),
        (vec!["select", "test-0", "--tree", "work"], false, false),
        (vec!["app-snapshot", "--cached"], true, false),
        (vec!["app-snapshot"], true, true),
    ] {
        let log = f.root.join("git-calls.jsonl");
        fs::write(&log, "").unwrap();
        let out = f
            .command()
            .env("BERBR_TEST_GIT_LOG", &log)
            .env("BERBR_TEST_AGENT_CWD", &tree)
            .args(&args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let calls: Vec<Vec<String>> = fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let count = |word: &str| {
            calls
                .iter()
                .filter(|args| args.iter().any(|s| s == word))
                .count()
        };
        assert_eq!(
            count("merge-base"),
            usize::from(snapshot),
            "{args:?}: {calls:?}"
        );
        assert_eq!(count("diff"), usize::from(snapshot), "{args:?}");
        assert_eq!(
            count("for-each-ref"),
            usize::from(snapshot || online),
            "{args:?}"
        );
        assert_eq!(count("list"), 1, "worktree discovery repeated: {calls:?}");
        assert!(
            !calls
                .iter()
                .any(|args| args.windows(2).any(|pair| pair == ["log", "-1"]))
        );
    }
}

#[test]
fn configured_checkout_root_keeps_legacy_trees_and_excludes_new_root_once() {
    for directory in [".herdr/worktrees", "dev/trees"] {
        let f = Fixture::new();
        let repo = f.root.join("repo");
        let legacy = repo.join(".claude/worktrees/legacy");
        f.git(
            &repo,
            &["worktree", "add", "-qb", "legacy", legacy.to_str().unwrap()],
        );
        fs::write(
            f.root.join("vault/tasks/test-0.md"),
            "---\nid: test-0\nworktrees: [legacy]\n---\nBody\n",
        )
        .unwrap();
        let config = f.root.join("config.toml");
        fs::write(
            &config,
            format!(
                "[workflow]\nworktree_dir = {directory:?}\n{}",
                fs::read_to_string(&config).unwrap()
            ),
        )
        .unwrap();
        for name in ["new-one", "new-two"] {
            let out = f
                .command()
                .args(["add", "test-0", "DEMO-42", "--name", name, "--bg"])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(repo.join(directory).join(name).is_dir());
            assert!(!repo.join(".claude/worktrees").join(name).exists());
        }
        let exclude = fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert_eq!(
            exclude
                .lines()
                .filter(|line| *line == format!("/{directory}/"))
                .count(),
            1
        );
        assert!(!repo.join(".gitignore").exists());
        let snapshot = success(
            f.command()
                .args(["app-snapshot", "--cached"])
                .output()
                .unwrap(),
        );
        assert!(snapshot.to_string().contains(legacy.to_str().unwrap()));
        assert!(
            snapshot
                .to_string()
                .contains(repo.join(directory).join("new-one").to_str().unwrap())
        );
        let refused = f
            .command()
            .args(["add", "test-0", "DEMO-43", "--name", "legacy", "--bg"])
            .output()
            .unwrap();
        assert!(!refused.status.success());
        assert!(legacy.is_dir());
        assert!(!repo.join(directory).join("legacy").exists());
        let reused = f
            .command()
            .args(["add", "test-0", "DEMO-42", "--in", "legacy", "--bg"])
            .output()
            .unwrap();
        assert!(
            reused.status.success(),
            "{}",
            String::from_utf8_lossy(&reused.stderr)
        );
        assert!(String::from_utf8_lossy(&reused.stdout).contains(legacy.to_str().unwrap()));
    }
}

#[test]
fn native_terminal_details_use_at_most_eight_concurrent_reads() {
    let f = Fixture::new();
    fs::write(f.root.join("bin/herdr-app"), r#"#!/usr/bin/env python3
import json
print(json.dumps({'spaces':[{'id':'space','name':'test-0','machine':None,'tabs':[{'id':'tab','name':'demo','terminals':[{'terminal':f't{i}','app':'codex','status':'idle'} for i in range(20)]}]}]}))
"#).unwrap();
    fs::write(f.root.join("bin/hc"),r#"#!/usr/bin/env python3
import os,sys,json,time,fcntl
from pathlib import Path
root=Path(os.environ['BERBR_TEST_ROOT']); path=root/'detail-pool.json'
def update(delta):
 with path.open('a+') as f:
  fcntl.flock(f,fcntl.LOCK_EX); f.seek(0)
  value=json.loads(f.read() or '{"active":0,"peak":0,"reads":0}')
  value['active']+=delta; value['peak']=max(value['peak'],value['active']); value['reads']+=int(delta>0)
  f.seek(0); f.truncate(); f.write(json.dumps(value)); f.flush()
update(1); time.sleep(.035); update(-1)
print(json.dumps({'address':sys.argv[2],'cwd':str(root/'repo'),'foreground':{'name':'codex','cwd':str(root/'repo')},'labels':{}}))
"#).unwrap();
    let snapshot = success(
        f.command()
            .args(["app-snapshot", "--cached"])
            .output()
            .unwrap(),
    );
    assert!(snapshot["warnings"].as_array().unwrap().is_empty());
    let pool: Value =
        serde_json::from_slice(&fs::read(f.root.join("detail-pool.json")).unwrap()).unwrap();
    assert_eq!(pool["reads"], 20);
    assert_eq!(pool["active"], 0);
    assert!(pool["peak"].as_u64().unwrap() <= 8);
}

#[test]
fn changes_are_logged_with_their_failure_and_reads_only_in_debug() {
    let fixture = Fixture::new();
    let logs = fixture.root.join("logs");
    let run = |args: &[&str], debug: bool| {
        let mut cmd = fixture.command();
        cmd.env("BERBR_LOG_DIR", &logs).args(args);
        if debug {
            cmd.env("BERBR_DEBUG", "1");
        } else {
            cmd.env_remove("BERBR_DEBUG");
        }
        cmd.output().unwrap()
    };
    let log = || fs::read_to_string(logs.join("berbr.log")).unwrap_or_default();
    run(&["app-snapshot", "--cached"], false);
    assert_eq!(log(), "", "a read is not logged without debug");
    assert!(
        !run(&["rename", "missing-task", "New title"], false)
            .status
            .success()
    );
    let text = log();
    assert!(
        text.contains(r#"start ["rename", "missing-task", "New title"]"#),
        "{text}"
    );
    assert!(
        text.contains(r#"failed "#) && text.contains("missing-task"),
        "{text}"
    );
    run(&["app-snapshot", "--cached"], true);
    let text = log();
    assert!(
        text.contains(r#"ok "#) && text.contains(r#"["app-snapshot", "--cached"]"#),
        "{text}"
    );
    assert!(
        text.contains("git [") || text.contains("herdr-app ["),
        "{text}"
    );
    let out = run(&["logs", "--path"], false);
    assert!(
        String::from_utf8_lossy(&out.stdout)
            .starts_with(&logs.join("berbr.log").display().to_string())
    );
}

#[test]
fn migrate_copies_tasks_out_of_the_vault_and_berbr_then_reads_its_own_folder() {
    let fixture = Fixture::new();
    let vault = fixture.root.join("vault");
    fs::write(
        vault.join("tasks/test-1.md"),
        "---\nid: test-1\ntitle: Closed one\ndone: true\n---\nOld work\n",
    )
    .unwrap();
    fs::create_dir_all(vault.join(".worktable")).unwrap();
    fs::write(vault.join(".worktable/test.next"), "5").unwrap();
    let own = fixture.root.join("home/.berbr");
    let tasks = own.join("projects/test/tasks");

    let out = fixture
        .command()
        .args(["migrate", "--dry-run"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("Would copy 2 task(s), 1 closed"));
    assert!(!own.exists());

    let out = fixture.command().arg("migrate").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let open = fs::read_to_string(tasks.join("test-0.md")).unwrap();
    assert!(open.contains("id: test-0") && open.contains("created:") && open.contains("задача"));
    assert!(
        fs::read_to_string(tasks.join("done/test-1.md"))
            .unwrap()
            .contains("Old work")
    );
    assert_eq!(
        fs::read_to_string(own.join("projects/test/.worktable/test.next")).unwrap(),
        "5"
    );
    let config = fs::read_to_string(own.join("config.toml")).unwrap();
    assert!(
        !config.contains("[vault]")
            && !config.contains("task_dirs")
            && config.contains("[[project.repo]]")
    );
    // the vault keeps everything
    assert!(vault.join("tasks/test-0.md").is_file() && vault.join("tasks/test-1.md").is_file());
    assert!(
        fs::read_to_string(fixture.root.join("config.toml"))
            .unwrap()
            .contains("[vault]")
    );

    // without an explicit config, berbr takes ~/.berbr/config.toml and its folders
    let out = fixture
        .command()
        .env_remove("BERBR_CONFIG")
        .args(["app-snapshot", "--cached"])
        .output()
        .unwrap();
    let snapshot = success(out);
    let notes: Vec<String> = snapshot["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["note"].as_str().map(String::from))
        .collect();
    assert!(
        notes
            .iter()
            .any(|n| n.ends_with(".berbr/projects/test/tasks/test-0.md")),
        "{notes:?}"
    );
    // a second run copies nothing over what is there
    let out = fixture.command().arg("migrate").output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("Copied 0 task(s)"));
}
