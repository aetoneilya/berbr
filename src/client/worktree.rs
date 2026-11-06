use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("failed to execute git command: {0}")]
    Io(#[from] std::io::Error),

    #[error("git command exited with status {0}")]
    NonZeroExit(String),

    #[error("repo path not utf8")]
    RepoPathNotUtf8,

    #[error("failed to obtain current branch")]
    CurrentBranchNotFound,

    #[error("worktree name must be a single non-empty directory name")]
    InvalidName,

    #[error(
        "worktree_dir must be a non-empty repository-relative directory without parent traversal"
    )]
    InvalidDirectory,
}

pub(crate) type Result<T> = std::result::Result<T, GitError>;

#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
}

impl Worktree {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    }
}

fn git_output(path: &Path, args: &[&str]) -> Result<String> {
    git_stdout(path, args).map(|out| out.trim_start().to_string())
}

/// git's stdout without its trailing newline: leading spaces mean something in
/// `git status --short`.
fn git_stdout(path: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(GitError::NonZeroExit(format!(
            "(git {}) {}",
            args.first().unwrap_or(&""),
            stderr.lines().next().unwrap_or("").trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string())
}

/// Root of the main checkout, also when `path` is inside a linked worktree.
pub fn main_checkout(path: &Path) -> Result<PathBuf> {
    let common_dir = git_output(
        path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let common_dir = PathBuf::from(common_dir);
    Ok(common_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or(common_dir))
}

/// Root of the worktree containing `path`.
pub fn worktree_root(path: &Path) -> Result<PathBuf> {
    git_output(path, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

/// Linked worktrees of the repository, without the main checkout.
pub fn linked_worktrees(repo: &Path) -> Result<Vec<Worktree>> {
    let output = git_output(repo, &["worktree", "list", "--porcelain"])?;

    let worktrees = output
        .split("\n\n")
        .filter_map(|block| {
            let mut path = None;
            let mut branch = None;
            for line in block.lines() {
                if let Some(p) = line.strip_prefix("worktree ") {
                    path = Some(PathBuf::from(p));
                } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
                    branch = Some(b.to_string());
                }
            }
            path.map(|path| Worktree { path, branch })
        })
        .skip(1)
        .collect();

    Ok(worktrees)
}

pub fn current_branch(path: &Path) -> Result<String> {
    let branch = git_output(path, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch.is_empty() {
        return Err(GitError::CurrentBranchNotFound);
    }
    Ok(branch)
}

pub fn changed_files(path: &Path) -> Result<usize> {
    let output = git_output(path, &["--no-optional-locks", "status", "--porcelain"])?;
    Ok(output.lines().filter(|l| !l.is_empty()).count())
}

#[derive(Debug, Default, serde::Serialize)]
pub struct Diff {
    pub adds: usize,
    pub dels: usize,
    pub files: usize,
}

/// Current tracked changes against the merge-base, requested only by snapshots.
pub fn diff(path: &Path, default: &str) -> Result<Diff> {
    let base = git_output(path, &["merge-base", "HEAD", default])?;
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["diff", "--shortstat", "--no-ext-diff", &base, "--"])
        .env("LC_ALL", "C")
        .output()?;
    if !output.status.success() {
        return Err(GitError::NonZeroExit(
            String::from_utf8_lossy(&output.stderr).trim().into(),
        ));
    }
    let mut diff = Diff::default();
    for part in String::from_utf8_lossy(&output.stdout).trim().split(',') {
        let mut words = part.split_whitespace();
        let Some(count) = words.next().and_then(|s| s.parse().ok()) else {
            continue;
        };
        match words.next() {
            Some("file" | "files") => diff.files = count,
            Some("insertion(+)" | "insertions(+)") => diff.adds = count,
            Some("deletion(-)" | "deletions(-)") => diff.dels = count,
            _ => {}
        }
    }
    Ok(diff)
}

/// `git status --short`: one line per changed file.
pub fn short_status(path: &Path) -> Result<String> {
    git_stdout(path, &["status", "--short"])
}

/// Whether origin has `branch`. Branches are created --no-track: origin/<branch>
/// is what unpushed commits are counted against.
pub fn on_origin(path: &Path, branch: &str) -> bool {
    let remote_branch = format!("origin/{branch}");
    git_output(path, &["rev-parse", "--verify", "--quiet", &remote_branch]).is_ok()
}

/// `git log --oneline` of `range`, newest first, at most `max` commits.
pub fn log_oneline(path: &Path, range: &str, max: usize) -> Result<String> {
    git_output(path, &["log", "--oneline", &format!("-{max}"), range])
}

pub fn remove_worktree(repo: &Path, worktree: &Path, force: bool) -> Result<()> {
    let path = worktree.to_str().ok_or(GitError::RepoPathNotUtf8)?;
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(path);
    git_output(repo, &args).map(|_| ())
}

pub fn delete_branch(repo: &Path, branch: &str, force: bool) -> Result<()> {
    let flag = if force { "-D" } else { "-d" };
    git_output(repo, &["branch", flag, branch]).map(|_| ())
}

pub const DEFAULT_DIRECTORY: &str = ".herdr/worktrees";
pub const LEGACY_DIRECTORY: &str = ".claude/worktrees";

pub fn validate_directory(directory: &Path) -> Result<()> {
    if directory.as_os_str().is_empty()
        || directory.to_string_lossy().chars().any(char::is_control)
        || directory
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(GitError::InvalidDirectory);
    }
    Ok(())
}

pub fn worktree_roots(main: &Path, directory: &Path) -> Vec<PathBuf> {
    let mut roots = vec![main.join(directory), main.join(LEGACY_DIRECTORY)];
    roots.dedup();
    roots
}

pub fn worktree_dir(main: &Path, directory: &Path, name: &str) -> PathBuf {
    main.join(directory).join(name)
}

pub fn existing_worktree(main: &Path, directory: &Path, name: &str) -> Result<Option<PathBuf>> {
    let found: Vec<_> = worktree_roots(main, directory)
        .into_iter()
        .map(|root| root.join(name))
        .filter(|p| p.symlink_metadata().is_ok())
        .collect();
    match found.as_slice() {
        [] => Ok(None),
        [path] => Ok(Some(path.clone())),
        _ => Err(GitError::NonZeroExit(format!(
            "worktree {name} is ambiguous across worktree directories; use its full path"
        ))),
    }
}

pub fn ensure_excluded(main: &Path, directory: &Path) -> Result<()> {
    validate_directory(directory)?;
    let path = main.join(".git/info/exclude");
    let mut options = std::fs::OpenOptions::new();
    let mut file = options.create(true).append(true).read(true).open(path)?;
    // A per-file advisory lock makes concurrent first starts append only once.
    file.lock()?;
    use std::io::{Read, Write};
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let mut escaped = String::new();
    for ch in directory.to_string_lossy().chars() {
        if ['\\', '*', '?', '[', ']'].contains(&ch) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    let entry = format!("/{escaped}/");
    if !text.lines().any(|line| line.trim() == entry) {
        if !text.is_empty() && !text.ends_with('\n') {
            writeln!(file)?;
        }
        writeln!(file, "{entry}")?;
    }
    Ok(())
}

pub fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty()
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_control())
    {
        return Err(GitError::InvalidName);
    }
    Ok(())
}

/// A create-new marker reserves names across simultaneous/asynchronous starts.
/// Markers retain the task identity even when checkout fails, so retrying a new
/// task can never take over another task's pending checkout.
pub fn reserve_name(
    main: &Path,
    directory: &Path,
    base: &str,
    task: &str,
    branch_prefix: &str,
    key: Option<&str>,
) -> Result<String> {
    validate_name(base)?;
    validate_directory(directory)?;
    let reservations = main.join(directory).join(".berbr-reservations");
    ensure_excluded(main, directory)?;
    std::fs::create_dir_all(&reservations)?;
    let refs = Refs::read(main)?;
    for counter in 0.. {
        let name = match counter {
            0 => base.to_string(),
            1 => format!("{base}-{task}"),
            n => format!("{base}-{task}-{n}"),
        };
        let suffix = key.map_or_else(|| name.clone(), |k| format!("{k}-{name}"));
        if worktree_roots(main, directory).iter().any(|root| {
            root.join(&name).symlink_metadata().is_ok()
                || root
                    .join(".berbr-reservations")
                    .join(&name)
                    .symlink_metadata()
                    .is_ok()
        }) || refs.contains_branch(&format!("{branch_prefix}{suffix}"))
        {
            continue;
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(reservations.join(&name))
        {
            Ok(mut file) => {
                use std::io::Write;
                file.write_all(task.as_bytes())?;
                return Ok(name);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    unreachable!()
}

/// How to create a checkout under the configured directory on `branch`: an
/// existing local or remote branch is checked out, otherwise the branch is
/// created from a freshly fetched origin/HEAD.
pub struct WorktreePlan {
    /// The origin branch to fetch first.
    pub fetch: Option<String>,
    /// Arguments of `git worktree add`.
    pub add: Vec<String>,
}

pub fn plan_worktree(
    main: &Path,
    directory: &Path,
    name: &str,
    branch: &str,
) -> Result<WorktreePlan> {
    validate_name(name)?;
    validate_directory(directory)?;
    if existing_worktree(main, directory, name)?.is_some() {
        return Err(GitError::NonZeroExit(format!(
            "worktree {name} already exists in a configured or legacy directory"
        )));
    }
    let dir = worktree_dir(main, directory, name);
    let path = dir.to_str().ok_or(GitError::RepoPathNotUtf8)?.to_string();
    let refs = Refs::read(main)?;
    if refs.contains_branch(branch) {
        return Ok(WorktreePlan {
            add: vec!["worktree".into(), "add".into(), path, branch.into()],
            fetch: None,
        });
    }
    let base = refs
        .default_branch(None)
        .unwrap_or_else(|| "HEAD".to_string());
    Ok(WorktreePlan {
        fetch: base.strip_prefix("origin/").map(str::to_string),
        add: ["worktree", "add", "--no-track", "-b", branch, &path, &base]
            .map(String::from)
            .to_vec(),
    })
}

/// Two fetches at once in one repo (two tasks started together, an agent's
/// own fetch) race for the remote ref: the loser fails with "cannot lock ref"
/// and goes again.
pub fn fetch(main: &Path, remote_branch: &str) -> Result<()> {
    let mut tries = 0;
    loop {
        match git_output(main, &["fetch", "-q", "origin", remote_branch]) {
            Err(GitError::NonZeroExit(e)) if tries < 3 && e.contains("cannot lock ref") => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(500 * tries));
            }
            result => return result.map(|_| ()),
        }
    }
}

/// The plan's `git worktree add`; `progress` leaves git's own checkout progress
/// on the terminal.
pub fn add_worktree(main: &Path, plan: &WorktreePlan, progress: bool) -> Result<()> {
    if !progress {
        let args: Vec<&str> = plan.add.iter().map(String::as_str).collect();
        return git_output(main, &args).map(|_| ());
    }
    let status = Command::new("git")
        .arg("-C")
        .arg(main)
        .args(&plan.add)
        .stdout(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        return Err(GitError::NonZeroExit(
            "(git worktree add) failed".to_string(),
        ));
    }
    Ok(())
}

/// The worktree `plan_worktree` describes; an existing directory is reused as is.
pub fn create_worktree(main: &Path, directory: &Path, name: &str, branch: &str) -> Result<PathBuf> {
    validate_name(name)?;
    validate_directory(directory)?;
    let dir = worktree_dir(main, directory, name);
    if dir.exists() {
        if worktree_root(&dir)? == dir
            && main_checkout(&dir)? == main_checkout(main)?
            && current_branch(&dir)? == branch
        {
            return Ok(dir);
        }
        return Err(GitError::NonZeroExit(format!(
            "{} already exists with a different checkout",
            dir.display()
        )));
    }
    if existing_worktree(main, directory, name)?.is_some() {
        return Err(GitError::NonZeroExit(format!(
            "worktree {name} already exists in another worktree directory"
        )));
    }
    ensure_excluded(main, directory)?;
    // Restore only an already selected branch; Git refuses if it disappeared.
    let path = dir.to_str().ok_or(GitError::RepoPathNotUtf8)?;
    let plan = WorktreePlan {
        fetch: None,
        add: ["worktree", "add", path, branch]
            .map(str::to_string)
            .to_vec(),
    };
    add_worktree(main, &plan, false)?;
    copy_worktree_includes(main, &dir);
    Ok(dir)
}

/// Unlike resume, add must fail if another caller already created this path.
pub fn create_new_worktree(
    main: &Path,
    directory: &Path,
    name: &str,
    branch: &str,
) -> Result<PathBuf> {
    validate_directory(directory)?;
    validate_name(name)?;
    if existing_worktree(main, directory, name)?.is_some() {
        return Err(GitError::NonZeroExit(format!(
            "worktree {name} already exists in a configured or legacy directory"
        )));
    }
    ensure_excluded(main, directory)?;
    let dir = worktree_dir(main, directory, name);
    let plan = plan_worktree(main, directory, name, branch)?;
    // a failed fetch leaves the worktree on the remote branch as last fetched
    if let Some(remote_branch) = &plan.fetch {
        let _ = fetch(main, remote_branch);
    }
    add_worktree(main, &plan, false)?;
    copy_worktree_includes(main, &dir);
    Ok(dir)
}

/// Untracked local files listed in `.worktreeinclude` (e.g. `.claude/settings.local.json`
/// with the memory mapping), copied the way Claude Code does for its own worktrees.
pub fn copy_worktree_includes(main: &Path, dir: &Path) {
    let Ok(list) = std::fs::read_to_string(main.join(".worktreeinclude")) else {
        return;
    };
    for entry in list.lines().map(str::trim) {
        if entry.is_empty() || entry.starts_with('#') {
            continue;
        }
        let (from, to) = (main.join(entry), dir.join(entry));
        if from.exists() && !to.exists() {
            let _ = copy_recursive(&from, &to);
        }
    }
}

fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// The worktree's git dir, `.git/worktrees/<name>`: never committed, removed
/// with the worktree.
pub fn git_dir(path: &Path) -> Result<PathBuf> {
    git_output(path, &["rev-parse", "--absolute-git-dir"]).map(PathBuf::from)
}

/// Immutable ref facts read once per repository and reused for every checkout.
#[derive(Default)]
pub struct Refs {
    items: std::collections::HashMap<String, (String, Option<i64>, String)>,
}

impl Refs {
    pub fn read(main: &Path) -> Result<Self> {
        let text = git_output(
            main,
            &[
                "for-each-ref",
                "--format=%(refname)%00%(objectname)%00%(committerdate:unix)%00%(symref)",
                "refs/heads",
                "refs/remotes/origin",
            ],
        )?;
        let mut items = std::collections::HashMap::new();
        for line in text.lines() {
            let fields: Vec<_> = line.split('\0').collect();
            if let [name, sha, time, target] = fields.as_slice() {
                items.insert(
                    name.to_string(),
                    (sha.to_string(), time.parse().ok(), target.to_string()),
                );
            }
        }
        Ok(Self { items })
    }

    pub fn default_branch(&self, fallback: Option<&str>) -> Option<String> {
        self.items
            .get("refs/remotes/origin/HEAD")
            .and_then(|(_, _, target)| target.strip_prefix("refs/remotes/").map(str::to_string))
            .or_else(|| {
                fallback
                    .filter(|b| self.items.contains_key(&format!("refs/heads/{b}")))
                    .map(str::to_string)
            })
    }

    pub fn branches(&self) -> Vec<String> {
        let mut branches: Vec<_> = self
            .items
            .keys()
            .filter_map(|r| {
                r.strip_prefix("refs/heads/")
                    .or_else(|| r.strip_prefix("refs/remotes/origin/"))
            })
            .filter(|b| *b != "HEAD")
            .map(str::to_string)
            .collect();
        branches.sort();
        branches.dedup();
        branches
    }

    pub fn contains_branch(&self, branch: &str) -> bool {
        ["refs/heads/", "refs/remotes/origin/"]
            .iter()
            .any(|prefix| self.items.contains_key(&format!("{prefix}{branch}")))
    }

    pub fn head(&self, branch: &str) -> Option<String> {
        self.items
            .get(&format!("refs/heads/{branch}"))
            .map(|r| r.0.clone())
    }
    pub fn time(&self, branch: &str) -> Option<i64> {
        self.items
            .get(&format!("refs/heads/{branch}"))
            .and_then(|r| r.1)
    }

    pub fn unpushed(&self, path: &Path, branch: &str) -> Result<usize> {
        let remote = format!("refs/remotes/origin/{branch}");
        let base = if self.items.contains_key(&remote) {
            remote
        } else {
            "refs/remotes/origin/HEAD".into()
        };
        let count = git_output(
            path,
            &[
                "rev-list",
                "--count",
                &format!("{base}..refs/heads/{branch}"),
            ],
        )?;
        count
            .parse()
            .map_err(|e| GitError::NonZeroExit(format!("invalid ahead count: {e}")))
    }
}

#[cfg(test)]
mod reservation_tests {
    use super::*;

    #[test]
    fn validates_every_creation_path_before_using_it() {
        for name in [
            "",
            " ",
            ".",
            "..",
            "../outside",
            "/home/dev/demo",
            "a/b",
            "a\\b",
            "a\nb",
        ] {
            assert!(validate_name(name).is_err(), "{name:?}");
            assert!(
                plan_worktree(
                    Path::new("/absent"),
                    Path::new(DEFAULT_DIRECTORY),
                    name,
                    "demo"
                )
                .is_err()
            );
            assert!(
                create_worktree(
                    Path::new("/absent"),
                    Path::new(DEFAULT_DIRECTORY),
                    name,
                    "demo"
                )
                .is_err()
            );
            assert!(
                create_new_worktree(
                    Path::new("/absent"),
                    Path::new(DEFAULT_DIRECTORY),
                    name,
                    "demo"
                )
                .is_err()
            );
        }
        assert!(validate_name("two words").is_ok());
    }

    #[test]
    fn simultaneous_task_names_are_reserved_uniquely() {
        let root = crate::table::fixtures::temp_dir();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        let names = std::thread::scope(|scope| {
            (0..8)
                .map(|n| {
                    let root = &root;
                    scope.spawn(move || {
                        reserve_name(
                            root,
                            Path::new(DEFAULT_DIRECTORY),
                            "shared-slug",
                            &format!("demo-{n}"),
                            "demo/",
                            None,
                        )
                        .unwrap()
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|job| job.join().unwrap())
                .collect::<std::collections::HashSet<_>>()
        });
        assert_eq!(names.len(), 8);
        assert!(names.contains("shared-slug"));
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn configured_roots_are_relative_and_cannot_escape_the_repository() {
        for directory in [
            "",
            ".",
            "../trees",
            "dev/../outside",
            "/home/dev/trees",
            "dev/\ntrees",
        ] {
            assert!(
                validate_directory(Path::new(directory)).is_err(),
                "{directory}"
            );
        }
        for directory in [DEFAULT_DIRECTORY, "dev/trees", "task trees"] {
            assert!(validate_directory(Path::new(directory)).is_ok());
        }
    }

    #[test]
    fn legacy_paths_and_pending_reservations_prevent_new_root_collisions() {
        let root = crate::table::fixtures::temp_dir();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        let legacy = root.join(LEGACY_DIRECTORY);
        std::fs::create_dir_all(legacy.join(".berbr-reservations")).unwrap();
        std::fs::write(legacy.join(".berbr-reservations/pending"), "demo-old").unwrap();
        std::fs::create_dir(legacy.join("existing")).unwrap();
        for name in ["pending", "existing"] {
            assert_eq!(
                reserve_name(
                    &root,
                    Path::new(DEFAULT_DIRECTORY),
                    name,
                    "demo-1",
                    "demo/",
                    None
                )
                .unwrap(),
                format!("{name}-demo-1")
            );
        }
        let exclude = std::fs::read_to_string(root.join(".git/info/exclude")).unwrap();
        ensure_excluded(&root, Path::new("dev/[trees]")).unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["check-ignore", "-q", "dev/[trees]/future/file"])
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            exclude
                .lines()
                .filter(|l| *l == "/.herdr/worktrees/")
                .count(),
            1
        );
    }
}
