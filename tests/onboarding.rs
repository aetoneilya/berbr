mod common;
use serde_json::Value;
use std::{
    fs,
    process::{Command, Output},
};

struct Setup {
    root: std::path::PathBuf,
}
impl Setup {
    fn new() -> Self {
        Self {
            root: common::temp_dir("berbr-onboard"),
        }
    }
    fn repo(&self, name: &str) -> std::path::PathBuf {
        let p = self.root.join(name);
        fs::create_dir_all(&p).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(&p)
                .status()
                .unwrap()
                .success()
        );
        p
    }
    fn run(&self, args: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_berbr"));
        c.current_dir(&self.root)
            .args(args)
            .env("BERBR_CONFIG", self.root.join("config.toml"))
            .env("BERBR_HOME", self.root.join("home"))
            .env("HOME", self.root.join("home"))
            .env("XDG_CACHE_HOME", self.root.join("cache"));
        for key in [
            "HERDR_PLUGIN_CONTEXT_JSON",
            "HERDR_ENV",
            "HERDR_WORKSPACE_ID",
            "HERDR_TAB_ID",
            "HERDR_PANE_ID",
            "HC_TERMINAL_ID",
        ] {
            c.env_remove(key);
        }
        c.output().unwrap()
    }
    fn success(&self, args: &[&str]) -> Value {
        let o = self.run(args);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice(&o.stdout).unwrap()
    }
}
impl Drop for Setup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn first_snapshot_is_setup_and_connect_creates_a_usable_config_without_a_task() {
    let s = Setup::new();
    let repo = s.repo("my repo");
    assert_eq!(
        s.success(&["app-snapshot", "--cached"])["setup_required"],
        true
    );
    assert!(!s.root.join("config.toml").exists());
    let result = s.success(&[
        "init",
        "--repo",
        repo.to_str().unwrap(),
        "--name",
        "demo",
        "--agent",
        "codex",
    ]);
    assert_eq!(result["project"], "demo");
    let config = fs::read_to_string(s.root.join("config.toml")).unwrap();
    let doc: toml::Value = toml::from_str(&config).unwrap();
    assert_eq!(doc["workflow"]["agent"].as_str(), Some("codex"));
    assert_eq!(doc["workflow"]["generation_agent"].as_str(), Some("codex"));
    let task = s.run(&["start", "First task", "--no-worktree", "--no-agent"]);
    assert!(
        task.status.success(),
        "{}",
        String::from_utf8_lossy(&task.stderr)
    );
    assert!(s.root.join("home/projects/demo/tasks/demo-0.md").exists());
}
#[test]
fn adding_a_project_preserves_existing_settings_and_refuses_duplicates_or_bad_input() {
    let s = Setup::new();
    let a = s.repo("first");
    let b = s.repo("second");
    s.success(&["init", "--repo", a.to_str().unwrap(), "--agent", "claude"]);
    let config = s.root.join("config.toml");
    let original = format!(
        "# keep this comment\n{}",
        fs::read_to_string(&config).unwrap()
    );
    fs::write(&config, &original).unwrap();
    s.success(&["init", "--repo", b.to_str().unwrap(), "--name", "second"]);
    let text = fs::read_to_string(&config).unwrap();
    assert!(text.starts_with(&original));
    for args in [
        vec!["init", "--repo", a.to_str().unwrap()],
        vec!["init", "--repo", b.to_str().unwrap(), "--name", "../bad"],
        vec!["init", "--repo", "/does/not/exist"],
    ] {
        assert!(!s.run(&args).status.success());
        assert_eq!(fs::read_to_string(&config).unwrap(), text);
    }
}
#[test]
fn invalid_config_is_reported_without_overwriting_it_or_entering_setup() {
    let s = Setup::new();
    let repo = s.repo("repo");
    let config = s.root.join("config.toml");
    fs::write(&config, "broken = [").unwrap();
    assert!(!s.run(&["app-snapshot", "--cached"]).status.success());
    assert!(
        !s.run(&["init", "--repo", repo.to_str().unwrap()])
            .status
            .success()
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), "broken = [");
    let doctor = s.success(&["doctor"]);
    assert!(doctor["config_error"].as_str().is_some());
}
#[test]
fn initialization_preserves_a_symlinked_config() {
    use std::os::unix::fs::symlink;
    let s = Setup::new();
    let a = s.repo("first");
    let b = s.repo("second");
    s.success(&["init", "--repo", a.to_str().unwrap()]);
    let path = s.root.join("config.toml");
    let actual = s.root.join("actual.toml");
    fs::rename(&path, &actual).unwrap();
    symlink(&actual, &path).unwrap();
    s.success(&["init", "--repo", b.to_str().unwrap()]);
    assert!(
        fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(fs::read_to_string(actual).unwrap().contains("second"));
}

#[test]
fn github_setup_uses_origin_and_refuses_other_hosts_without_writing_config() {
    let s = Setup::new();
    let repo = s.repo("github");
    assert!(
        Command::new("git")
            .current_dir(&repo)
            .args(["remote", "add", "origin", "git@github.com:Example/Demo.git"])
            .status()
            .unwrap()
            .success()
    );
    s.success(&["init", "--repo", repo.to_str().unwrap(), "--github"]);
    let config = fs::read_to_string(s.root.join("config.toml")).unwrap();
    let doc: toml::Value = toml::from_str(&config).unwrap();
    assert_eq!(
        doc["project"][0]["repo"][0]["github_repo"].as_str(),
        Some("example/demo")
    );
    let t = Setup::new();
    let other = t.repo("other");
    assert!(
        Command::new("git")
            .current_dir(&other)
            .args([
                "remote",
                "add",
                "origin",
                "git@git.example.com:demo/app.git"
            ])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        !t.run(&["init", "--repo", other.to_str().unwrap(), "--github"])
            .status
            .success()
    );
    assert!(!t.root.join("config.toml").exists());
}
