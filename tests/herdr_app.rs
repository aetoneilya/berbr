#![cfg(unix)]
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
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "berbr-app-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
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
  out={'spaces':[{'id':'sp-local','name':'test-0','machine':None,'tabs':[{'id':'tb-local','name':'local','terminals':[{'terminal':'t1','pane':'p1','app':'codex','status':'idle'}]}]}, {'id':'prod/sp-remote','name':'remote','machine':'prod','tabs':[{'id':'prod/tb-remote','terminals':[{'terminal':'prod/t2','app':'codex','status':'working'}]}]}]}
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
  out={'address':a[1],'cwd':str(r/'repo'),'foreground':None if starting else {'name':'codex' if busy else 'zsh','cwd':str(r/'repo')},'labels':{},'status':'working' if busy else None}
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
    assert_eq!(data["version"], 2);
    assert!(data.to_string().contains("задача"));
    assert_eq!(fs::read(path).unwrap(), before);
    assert!(f.calls().iter().any(|c| c[0] == "hc" && c[1][0] == "get"));
    assert!(!f.calls().iter().any(|c| c.to_string().contains("prod/")));
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
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
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
    git(&["worktree", "add", "-qb", "DEMO-1-test", wt.to_str().unwrap()]);
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
        vec!["done", "DEMO-1-test", "--dry-run", "--json"],
        vec!["done", "--merged", "--dry-run", "--json"],
        vec!["done", "DEMO-1-test", "--force", "--dry-run", "--json"],
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
        assert_eq!(plan["note_done"], args[1] == "test-0" || args[1] == "DEMO-1");
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
            .args(["done", "DEMO-1-test", "--dry-run", "--json"])
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
        ["done", "DEMO-1-test", "--dry-run", "--json"],
        ["done", "--merged", "--dry-run", "--json"],
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
