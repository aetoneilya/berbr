#![cfg(unix)]
mod common;

use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    config: PathBuf,
    capture: PathBuf,
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

impl Fixture {
    fn codex_prefix(&self) -> String {
        let tasks = self.root.join("vault/tasks").canonicalize().unwrap();
        let cache = if cfg!(target_os = "macos") {
            self.root.join("home/Library/Caches/berbr")
        } else {
            self.root.join("cache/berbr")
        }
        .canonicalize()
        .unwrap();
        format!(
            "command codex --no-daemon '--add-dir' '{}' '--add-dir' '{}'",
            tasks.display(),
            cache.display()
        )
    }

    fn new(workflow: &str) -> Self {
        let root = common::temp_dir("berbr-agent-cli");
        fs::create_dir_all(root.join("vault/tasks")).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        let root = root.canonicalize().unwrap();
        let repo = root.join("repo with space");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(
            &repo,
            &[
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
            ],
        );
        let config = root.join("config.toml");
        let quote = |p: &Path| toml::Value::String(p.to_string_lossy().into_owned());
        fs::write(&config, format!("[vault]\npath = {}\n[workflow]\n{workflow}\n[[project]]\nname = \"test\"\ntask_dirs = [\"tasks\"]\n[[project.repo]]\npath = {}\n", quote(&root.join("vault")), quote(&repo))).unwrap();
        let capture = root.join("commands.jsonl");
        let fake = r#"#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
if args[:2] == ['pane', 'label']:
    sys.exit(2) # The classic backend does not support pane labels.
with open(os.environ['BERBR_TEST_CAPTURE'], 'a') as f:
    f.write(json.dumps(args) + '\n')
result = {'agents': [], 'workspaces': []}
if args[:2] == ['agent', 'list'] and os.path.exists(os.environ['BERBR_TEST_AGENTS']):
    with open(os.environ['BERBR_TEST_AGENTS']) as f:
        result['agents'] = json.load(f)
if args[:2] == ['workspace', 'create']:
    result = {'tab': {'tab_id': 'test-tab'}, 'root_pane': {'pane_id': 'test-pane'}}
elif args[:2] == ['tab', 'create']:
    result = {'tab': {'tab_id': 'test-tab'}, 'root_pane': {'pane_id': 'test-pane'}}
elif args[:2] == ['pane', 'split']:
    result = {'pane': {'pane_id': 'test-pane'}}
print(json.dumps({'result': result}))
"#;
        for (name, text) in [("herdr", fake), ("sh", "#!/bin/sh\nexit 0\n")] {
            let path = root.join("bin").join(name);
            fs::write(&path, text).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self {
            root,
            repo,
            config,
            capture,
        }
    }

    fn command(&self, program: &str) -> Command {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut command = Command::new(program);
        command
            .current_dir(&self.repo)
            .env("PATH", path)
            .env("HOME", self.root.join("home"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("BERBR_CONFIG", &self.config)
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude"))
            .env("CODEX_HOME", self.root.join("codex"))
            .env("HERDR_ENV", "1")
            .env("HERDR_WORKSPACE_ID", "test-space")
            .env("BERBR_TEST_CAPTURE", &self.capture)
            .env("BERBR_TEST_AGENTS", self.root.join("agents.json"));
        command
    }

    fn run(&self, args: &[&str]) -> Vec<Vec<String>> {
        let _ = fs::remove_file(&self.capture);
        let output = self
            .command(env!("CARGO_BIN_EXE_berbr"))
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "args={args:?}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        self.commands()
    }

    fn commands(&self) -> Vec<Vec<String>> {
        fs::read_to_string(&self.capture)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// Exercise the real TUI in a terminal, with only herdr replaced by the fixture.
    fn tui(&self, navigation: &str, answer: &str) -> Vec<Vec<String>> {
        self.tui_with_tree(navigation, answer, "")
    }

    fn tui_with_tree(&self, navigation: &str, answer: &str, tree_answer: &str) -> Vec<Vec<String>> {
        let _ = fs::remove_file(&self.capture);
        let driver = r#"
import fcntl, os, pty, select, struct, subprocess, sys, termios, time
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 180, 0, 0))
process = subprocess.Popen([sys.argv[1]], stdin=slave, stdout=slave, stderr=slave)
os.close(slave)
screen = b''
def wait_for(text):
    global screen
    deadline = time.monotonic() + 10
    while text not in screen:
        if time.monotonic() >= deadline or process.poll() is not None:
            raise AssertionError('waiting for %r: %r' % (text, screen[-6000:]))
        if select.select([master], [], [], 0.1)[0]:
            screen += os.read(master, 65536)
try:
    wait_for(b'new task')
    os.write(master, sys.argv[2].encode() + b'n')
    wait_for(b'New agent')
    screen = b''
    # Crossterm decodes Esc+Esc as Escape without waiting for another byte.
    # A bare Esc followed by the cleanup q could otherwise become Alt-q.
    os.write(master, b'\x1b\x1b' if sys.argv[3] == '\x1b' else sys.argv[3].encode())
    if sys.argv[4]:
        wait_for(b'Worktree')
        screen = b''
        os.write(master, sys.argv[4].encode())
    if sys.argv[3] != '\x1b':
        wait_for(b'tab task')
    # Drain the PTY while closing; refresh output can fill its buffer.
    # Cancellation may finish between frames, replacing its transient footer.
    deadline, next_quit = time.monotonic() + 10, 0
    while process.poll() is None:
        assert time.monotonic() < deadline, screen[-6000:]
        if time.monotonic() >= next_quit:
            os.write(master, b'q')
            next_quit = time.monotonic() + 0.25
        if select.select([master], [], [], 0.1)[0]:
            try:
                screen += os.read(master, 65536)
            except OSError:
                process.wait(timeout=1)
    assert process.returncode == 0, screen
finally:
    if process.poll() is None:
        process.kill()
        process.wait()
    os.close(master)
"#;
        let output = self
            .command("python3")
            .env("TERM", "xterm-256color")
            .env_remove("WT_POPUP")
            .env_remove("BERBR_POPUP")
            .args([
                "-c",
                driver,
                env!("CARGO_BIN_EXE_berbr"),
                navigation,
                answer,
                tree_answer,
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        self.commands()
    }

    fn tree(&self) -> PathBuf {
        let tree = self.repo.join(".claude/worktrees/task");
        git(
            &self.repo,
            &[
                "worktree",
                "add",
                "-qb",
                "DEMO-1-task",
                tree.to_str().unwrap(),
            ],
        );
        tree
    }

    fn live_agent(&self, kind: &str, tree: &Path) {
        fs::write(
            self.root.join("agents.json"),
            json!([{
                "name":"already-open", "agent_status":"idle", "cwd":tree,
                "pane_id":"old-pane", "tab_id":"old-tab", "workspace_id":"old-space",
                "agent":kind, "agent_session":{"value":"shared"}
            }])
            .to_string(),
        )
        .unwrap();
    }

    fn sessions(&self, tree: &Path, both: bool) {
        let note = if both {
            "---\ntickets: [DEMO-1]\nsessions: [\"shared task 2026-10-29\", \"codex:shared task 2026-10-29\"]\n---\nContext\n"
        } else {
            "---\ntickets: [DEMO-1]\nsessions: [\"codex:shared task 2026-10-29\"]\n---\nContext\n"
        };
        fs::write(self.root.join("vault/tasks/task.md"), note).unwrap();
        let slug: String = tree
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
        let claude = self.root.join("claude/projects").join(slug);
        fs::create_dir_all(&claude).unwrap();
        if both {
            fs::write(
                claude.join("shared.jsonl"),
                json!({"cwd":tree,"timestamp":"2026-10-29T10:00:00Z"}).to_string() + "\n",
            )
            .unwrap();
        }
        let codex = self.root.join("codex/sessions/2026/10/29");
        fs::create_dir_all(&codex).unwrap();
        fs::write(codex.join("rollout-fixture.jsonl"), json!({"type":"session_meta","timestamp":"2026-10-29T10:00:00Z","payload":{"id":"shared","cwd":tree,"source":"cli"}}).to_string() + "\n").unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn pane_command(commands: &[Vec<String>]) -> &str {
    &commands
        .iter()
        .find(|c| {
            c.first().map(String::as_str) == Some("pane")
                && c.get(1).map(String::as_str) == Some("run")
        })
        .expect("agent launch command")[3]
}

#[test]
fn starts_each_cli_with_the_same_task_brief() {
    for kind in ["claude", "codex"] {
        let fixture = Fixture::new("");
        let commands = fixture.run(&["start", "task", "--no-worktree", "--bg", "--agent", kind]);
        let command = pane_command(&commands);
        let prefix = if kind == "codex" {
            fixture.codex_prefix()
        } else {
            "command claude '--name' 'task'".into()
        };
        assert!(command.starts_with(&format!("{prefix} ")), "{command}");
        assert!(
            command.contains("Read it and wait for my command."),
            "{command}"
        );
        assert!(command.contains("task note"));
    }
}

#[test]
fn prepared_worktree_command_keeps_the_requested_cli() {
    for kind in ["claude", "codex"] {
        let fixture = Fixture::new("");
        let commands = fixture.run(&["start", "task", "-n", "task", "--bg", "--agent", kind]);
        let command = pane_command(&commands);
        let prefix = if kind == "codex" {
            fixture.codex_prefix()
        } else {
            "command claude --name 'task'".into()
        };
        assert!(command.contains(&format!("&& {prefix} ")), "{command}");
        assert!(command.contains(" prepare "));
    }
}

#[test]
fn resumes_the_owning_cli_even_when_native_ids_collide() {
    for kind in ["claude", "codex"] {
        let fixture = Fixture::new("");
        let tree = fixture.tree();
        fixture.sessions(&tree, true);
        let commands = fixture.run(&["select", "task", "--bg", "--agent", kind]);
        let expected = if kind == "codex" {
            format!("{} 'resume' 'shared'", fixture.codex_prefix())
        } else {
            "command claude --name 'task' '--resume' 'shared'".into()
        };
        assert_eq!(pane_command(&commands), expected);
    }
}

#[test]
fn select_automatically_resumes_codex_despite_claude_default() {
    let fixture = Fixture::new("");
    let tree = fixture.tree();
    fixture.sessions(&tree, false);
    let commands = fixture.run(&["select", "task", "--bg"]);
    assert_eq!(
        pane_command(&commands),
        format!("{} 'resume' 'shared'", fixture.codex_prefix())
    );
}

fn git_snapshot(repo: &Path) -> Vec<Vec<u8>> {
    [
        vec!["status", "--porcelain"],
        vec!["show-ref"],
        vec!["worktree", "list", "--porcelain"],
    ]
    .into_iter()
    .map(|args| {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success());
        out.stdout
    })
    .collect()
}

#[test]
fn explicit_tree_resumes_its_latest_session_without_changing_git() {
    let fixture = Fixture::new("");
    let tree = fixture.tree();
    fixture.sessions(&tree, false);
    fs::write(fixture.root.join("vault/tasks/task.md"),
        "---\ntickets: [DEMO-1]\nsessions: [\"codex:shared task 2026-10-29\", \"codex:recent task 2026-10-30\"]\n---\nContext\n").unwrap();
    fs::write(fixture.root.join("codex/sessions/2026/10/29/rollout-recent.jsonl"),
        json!({"type":"session_meta","timestamp":"2026-10-30T10:00:00Z","payload":{"id":"recent","cwd":tree,"source":"cli"}}).to_string() + "\n").unwrap();
    let before = git_snapshot(&fixture.repo);
    let commands = fixture.run(&["select", "task", "--tree", "task", "--bg"]);
    assert_eq!(
        pane_command(&commands),
        format!("{} 'resume' 'recent'", fixture.codex_prefix())
    );
    assert_eq!(git_snapshot(&fixture.repo), before);
    assert!(
        commands
            .iter()
            .any(|c| c.contains(&tree.to_string_lossy().into_owned()))
    );
}

#[test]
fn explicit_tree_without_sessions_starts_fresh_without_changing_git() {
    let fixture = Fixture::new("");
    let tree = fixture.tree();
    fs::write(
        fixture.root.join("vault/tasks/task.md"),
        "---\ntickets: [DEMO-1]\n---\nContext\n",
    )
    .unwrap();
    let before = git_snapshot(&fixture.repo);
    let commands = fixture.run(&[
        "select", "task", "--tree", "task", "--bg", "--agent", "codex",
    ]);
    assert!(pane_command(&commands).starts_with(&format!("{} 'Context:", fixture.codex_prefix())));
    assert_eq!(git_snapshot(&fixture.repo), before);
    assert!(
        commands
            .iter()
            .any(|c| c.contains(&tree.to_string_lossy().into_owned()))
    );
}

#[test]
fn explicit_tree_names_the_first_checkout_of_a_task_without_trees_or_sessions() {
    let fixture = Fixture::new("");
    fs::write(
        fixture.root.join("vault/tasks/task.md"),
        "---\ntitle: Demo idea\n---\nContext\n",
    )
    .unwrap();
    let commands = fixture.run(&["select", "task", "--tree", "chosen-tree", "--bg"]);
    let command = pane_command(&commands);
    assert!(command.contains(" prepare "), "{command}");
    assert!(
        command.contains(".herdr/worktrees/chosen-tree"),
        "{command}"
    );
    let note = fs::read_to_string(fixture.root.join("vault/tasks/task.md")).unwrap();
    assert!(note.contains("chosen-tree"));
    let cache = if cfg!(target_os = "macos") {
        fixture.root.join("home/Library/Caches/berbr")
    } else {
        fixture.root.join("cache/berbr")
    };
    let spec_path = fs::read_dir(cache)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("prepare-")
        })
        .unwrap();
    let spec: serde_json::Value = serde_json::from_slice(&fs::read(spec_path).unwrap()).unwrap();
    assert_eq!(spec["name"], "chosen-tree");
}

#[test]
fn explicit_missing_tree_is_refused_when_the_task_has_worktrees_or_sessions() {
    for (other_tree, saved) in [(true, false), (false, true), (true, true)] {
        let fixture = Fixture::new("");
        let tree = fixture.tree();
        if saved {
            fixture.sessions(&tree, false);
        } else {
            fs::write(
                fixture.root.join("vault/tasks/task.md"),
                "---\ntickets: [DEMO-1]\n---\nContext\n",
            )
            .unwrap();
        }
        git(
            &fixture.repo,
            &["worktree", "remove", tree.to_str().unwrap()],
        );
        if other_tree {
            let other = fixture.repo.join(".claude/worktrees/other");
            git(
                &fixture.repo,
                &[
                    "worktree",
                    "add",
                    "-qb",
                    "DEMO-1-other",
                    other.to_str().unwrap(),
                ],
            );
        }
        let before = git_snapshot(&fixture.repo);
        let out = fixture
            .command(env!("CARGO_BIN_EXE_berbr"))
            .args(["select", "task", "--tree", "task", "--bg"])
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("has no worktree task"));
        assert!(!tree.exists());
        assert_eq!(git_snapshot(&fixture.repo), before);
        assert!(
            !fixture
                .commands()
                .iter()
                .any(|c| c.starts_with(&["pane".into(), "run".into()]))
        );
    }
}

#[test]
fn review_uses_the_chosen_cli_and_its_own_model_setting() {
    let fixture = Fixture::new(
        "agent = \"codex\"\nreview_agent = \"claude\"\nreview_model = \"claude-model\"\ncodex_review_model = \"codex-model\"",
    );
    fixture.tree();
    let configured = fixture.run(&["review", "task"]);
    assert!(
        pane_command(&configured)
            .starts_with("command claude --name 'task-review' '--model' 'claude-model'")
    );
    let overridden = fixture.run(&["review", "task", "--agent", "codex"]);
    assert!(
        pane_command(&overridden).starts_with("command codex --no-daemon '--model' 'codex-model'")
    );
}

#[test]
fn selecting_codex_does_not_focus_claude_with_the_same_session_id() {
    let fixture = Fixture::new("");
    let tree = fixture.tree();
    fixture.sessions(&tree, true);
    fs::write(
        fixture.root.join("agents.json"),
        json!([{
            "name":"claude-live", "agent_status":"idle", "cwd":tree,
            "pane_id":"old-pane", "tab_id":"old-tab", "workspace_id":"old-space",
            "agent":"claude", "agent_session":{"value":"shared"}
        }])
        .to_string(),
    )
    .unwrap();
    let commands = fixture.run(&["select", "task", "--bg", "--agent", "codex"]);
    assert_eq!(
        pane_command(&commands),
        format!("{} 'resume' 'shared'", fixture.codex_prefix())
    );
}

#[test]
fn saved_codex_ids_can_resume_without_a_plain_rollout_file() {
    let fixture = Fixture::new("");
    let tree = fixture.tree();
    fixture.sessions(&tree, false);
    fs::remove_file(
        fixture
            .root
            .join("codex/sessions/2026/10/29/rollout-fixture.jsonl"),
    )
    .unwrap();
    let commands = fixture.run(&["select", "task", "--bg"]);
    assert_eq!(
        pane_command(&commands),
        format!("{} 'resume' 'shared'", fixture.codex_prefix())
    );
}

#[test]
fn new_session_bypasses_live_agents_and_saved_conversations() {
    for kind in ["codex", "claude"] {
        let fixture = Fixture::new("");
        let tree = fixture.tree();
        fixture.sessions(&tree, true);
        fixture.live_agent(kind, &tree);
        let brief = fixture.repo.join(".git/worktrees/task/task.md");
        fs::write(&brief, "Task brief").unwrap();
        let commands = fixture.run(&["select", "task", "--new", "--agent", kind]);
        let command = pane_command(&commands);
        let prefix = if kind == "codex" {
            fixture.codex_prefix()
        } else {
            "command claude --name 'task'".into()
        };
        assert!(
            command.starts_with(&format!("{prefix} 'Context:")),
            "{command}"
        );
        assert!(command.contains(fixture.root.join("vault/tasks/task.md").to_str().unwrap()));
        assert!(command.contains(brief.to_str().unwrap()));
        assert!(
            !commands
                .iter()
                .any(|c| c.starts_with(&["agent".into(), "focus".into()]))
        );
        assert!(
            commands
                .iter()
                .any(|c| c.starts_with(&["tab".into(), "focus".into()]))
        );
    }
}

#[test]
fn new_session_and_session_picker_are_mutually_exclusive() {
    let fixture = Fixture::new("");
    let output = fixture
        .command(env!("CARGO_BIN_EXE_berbr"))
        .args(["select", "task", "--new", "--session"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
    assert!(fixture.commands().is_empty());
}

#[test]
fn tui_n_can_start_either_cli_despite_the_project_default_and_live_agents() {
    for (default, kind, answer) in [("claude", "codex", "\r"), ("codex", "claude", "\x1b[B\r")] {
        let fixture = Fixture::new(&format!("agent = \"{default}\""));
        let tree = fixture.tree();
        fixture.sessions(&tree, true);
        fixture.live_agent(kind, &tree);
        let commands = fixture.tui("", answer);
        let command = pane_command(&commands);
        let prefix = if kind == "codex" {
            fixture.codex_prefix()
        } else {
            "command claude --name 'task'".into()
        };
        assert!(
            command.starts_with(&format!("{prefix} 'Context:")),
            "{command}"
        );
        assert!(command.contains("Read it and wait for my command."));
    }
}

#[test]
fn tui_n_on_a_worktree_uses_that_worktree_without_asking() {
    let fixture = Fixture::new("");
    let tree = fixture.tree();
    fixture.sessions(&tree, true);
    let other = fixture.repo.join(".claude/worktrees/other");
    git(
        &fixture.repo,
        &[
            "worktree",
            "add",
            "-qb",
            "DEMO-1-other",
            other.to_str().unwrap(),
        ],
    );
    // Expand the initially collapsed task, then go task → ticket → other → task.
    // Choose the second worktree, not the first.
    let commands = fixture.tui(" jjj", "\r");
    let create = commands
        .iter()
        .find(|c| c.starts_with(&["workspace".into(), "create".into()]))
        .unwrap();
    assert!(
        create.contains(&tree.to_string_lossy().into_owned()),
        "{create:?}"
    );
    assert!(pane_command(&commands).starts_with(&format!("{} 'Context:", fixture.codex_prefix())));
}

#[test]
fn tui_new_agent_picker_can_be_cancelled_without_opening_a_tab() {
    let fixture = Fixture::new("");
    let tree = fixture.tree();
    fixture.sessions(&tree, true);
    let commands = fixture.tui("", "\x1b");
    assert!(!commands.iter().any(|c| matches!(
        c.first().map(String::as_str),
        Some("pane" | "workspace" | "tab")
    ) && c.get(1).is_some_and(|s| s == "create" || s == "run")));
}

#[test]
fn tui_n_on_a_task_asks_which_worktree_to_use() {
    let fixture = Fixture::new("");
    let tree = fixture.tree();
    fixture.sessions(&tree, true);
    let other = fixture.repo.join(".claude/worktrees/other");
    git(
        &fixture.repo,
        &[
            "worktree",
            "add",
            "-qb",
            "DEMO-1-other",
            other.to_str().unwrap(),
        ],
    );
    let commands = fixture.tui_with_tree("", "\r", "task\r");
    let create = commands
        .iter()
        .find(|c| c.starts_with(&["workspace".into(), "create".into()]))
        .unwrap();
    assert!(
        create.contains(&tree.to_string_lossy().into_owned()),
        "{create:?}"
    );
    assert!(pane_command(&commands).starts_with(&format!("{} 'Context:", fixture.codex_prefix())));
}

#[test]
fn fresh_codex_session_restores_a_removed_worktree_from_claude_history() {
    let fixture = Fixture::new("");
    let tree = fixture.tree();
    fixture.sessions(&tree, true);
    fs::write(
        fixture.root.join("vault/tasks/task.md"),
        "---\ntickets: [DEMO-1]\nsessions: [\"shared task 2026-10-29\"]\n---\nContext\n",
    )
    .unwrap();
    fs::remove_file(
        fixture
            .root
            .join("codex/sessions/2026/10/29/rollout-fixture.jsonl"),
    )
    .unwrap();
    git(
        &fixture.repo,
        &["worktree", "remove", tree.to_str().unwrap()],
    );
    let commands = fixture.run(&["select", "task", "--new", "--agent", "codex", "--bg"]);
    assert!(tree.exists());
    assert!(pane_command(&commands).starts_with(&format!("{} 'Context:", fixture.codex_prefix())));
}

fn fake_naming_cli(fixture: &Fixture, kind: &str) {
    let script = r#"#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
prompt = sys.stdin.read()
root = Path(os.environ['BERBR_TEST_CAPTURE']).parent
(root / 'naming.json').write_text(json.dumps({'kind': Path(sys.argv[0]).name, 'args': args, 'input': prompt, 'cwd': os.getcwd()}))
if os.environ.get('BERBR_TEST_NAMING_FAIL'):
    print('generation failed', file=sys.stderr)
    sys.exit(1)
result = {'title': 'Generated task', 'worktree': 'generated-task'}
if Path(sys.argv[0]).name == 'claude':
    print(json.dumps({'structured_output': result}))
else:
    schema = json.loads(Path(args[args.index('--output-schema') + 1]).read_text())
    assert schema['required'] == ['title', 'worktree']
    Path(args[args.index('--output-last-message') + 1]).write_text(json.dumps(result))
"#;
    let path = fixture.root.join("bin").join(kind);
    fs::write(&path, script).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn naming_uses_its_configured_cli_and_model_independently_of_the_work_agent() {
    for (kind, work) in [("claude", "codex"), ("codex", "claude")] {
        let fixture = Fixture::new(&format!(
            "agent = \"{work}\"\ngeneration_agent = \"{kind}\"\ngeneration_model = \"test model; literal\""
        ));
        fake_naming_cli(&fixture, kind);
        let description = "Implement a task\nPreserve multiline context and $(literal).";
        let commands = fixture.run(&[
            "start",
            "--description",
            description,
            "--no-worktree",
            "--bg",
            "--agent",
            work,
        ]);
        assert!(pane_command(&commands).starts_with(&format!("command {work}")));
        assert!(pane_command(&commands).contains("then work on the task description"));
        let call: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.root.join("naming.json")).unwrap()).unwrap();
        assert_eq!(call["kind"], kind);
        let args = call["args"].as_array().unwrap();
        let at = args.iter().position(|arg| arg == "--model").unwrap();
        assert_eq!(args[at + 1], "test model; literal");
        assert!(
            call["input"]
                .as_str()
                .unwrap()
                .contains("Preserve multiline context")
        );
        assert_ne!(
            call["cwd"].as_str().unwrap(),
            fixture.repo.to_str().unwrap()
        );
        if kind == "codex" {
            assert_eq!(args[0], "--no-daemon");
            assert!(args.iter().any(|arg| arg == "--ephemeral"));
        }
        let note = fs::read_dir(fixture.root.join("vault/tasks"))
            .unwrap()
            .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
            .collect::<String>();
        assert!(note.contains(description));
        assert!(note.contains("Generated task"));
    }
}

#[test]
fn generation_failure_creates_no_notes_or_worktrees_and_an_explicit_title_skips_generation() {
    let fixture = Fixture::new("generation_agent = \"codex\"");
    fake_naming_cli(&fixture, "codex");
    let output = fixture
        .command(env!("CARGO_BIN_EXE_berbr"))
        .env("BERBR_TEST_NAMING_FAIL", "1")
        .args(["start", "--description", "Work on this", "--bg"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("generation failed"));
    assert_eq!(
        fs::read_dir(fixture.root.join("vault/tasks"))
            .unwrap()
            .count(),
        0
    );
    assert!(fixture.commands().is_empty());
    fs::remove_file(fixture.root.join("naming.json")).unwrap();
    fixture.run(&[
        "start",
        "Explicit title",
        "--description",
        "Work on this",
        "--no-worktree",
        "--bg",
    ]);
    assert!(!fixture.root.join("naming.json").exists());
}

#[test]
fn saved_settings_are_visible_to_subsequent_commands_and_drive_new_work() {
    let fixture = Fixture::new("");
    let settings = |args: &[&str]| {
        let output = fixture
            .command(env!("CARGO_BIN_EXE_berbr"))
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let mut text = fs::read_to_string(&fixture.config).unwrap();
    text.push_str("\n[[project]]\nname = \"other\"\ntask_dirs = [\"other-tasks\"]\nagent = \"claude\"\ngeneration_agent = \"codex\"\n");
    fs::write(&fixture.config, text).unwrap();
    let saved = settings(&[
        "settings",
        "--default-agent",
        "codex",
        "--generation-agent",
        "claude",
        "--generation-model",
        "haiku",
    ]);
    assert_eq!(saved["agent"], "codex");
    assert_eq!(saved["generation_agent"], "claude");
    assert_eq!(settings(&["settings"]), saved);
    assert_eq!(settings(&["settings", "-p", "other"]), saved);
    assert_eq!(settings(&["settings", "-p", "unrelated"]), saved);
    assert_eq!(saved["scope"], "global");
    let before = fs::read(&fixture.config).unwrap();
    let refused = fixture
        .command(env!("CARGO_BIN_EXE_berbr"))
        .args(["settings", "--task-naming-model", "invented-model"])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert_eq!(fs::read(&fixture.config).unwrap(), before);
    let commands = fixture.run(&["start", "Explicit title", "--no-worktree", "--bg"]);
    assert!(pane_command(&commands).starts_with("command codex --no-daemon"));
    let cleared = settings(&["settings", "--generation-model", ""]);
    assert_eq!(cleared["generation_model"], "");
    assert_eq!(cleared["agent"], "codex");
}

#[test]
fn opening_a_saved_agent_resumes_that_exact_session_instead_of_the_latest() {
    let fixture = Fixture::new("");
    let tree = fixture.tree();
    fixture.sessions(&tree, false);
    fs::write(fixture.root.join("codex/sessions/2026/10/29/rollout-recent.jsonl"),
        json!({"type":"session_meta","timestamp":"2026-10-30T10:00:00Z","payload":{"id":"recent","cwd":tree,"source":"cli"}}).to_string()+"\n").unwrap();
    let before = git_snapshot(&fixture.repo);
    let commands = fixture.run(&["open", tree.to_str().unwrap(), "--resume", "codex:shared"]);
    assert_eq!(
        pane_command(&commands),
        format!("{} 'resume' 'shared'", fixture.codex_prefix())
    );
    assert_eq!(git_snapshot(&fixture.repo), before);
    let output = fixture
        .command(env!("CARGO_BIN_EXE_berbr"))
        .args([
            "open",
            tree.to_str().unwrap(),
            "--resume",
            "codex:unrelated",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not belong"));
}
