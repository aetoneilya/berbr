use crate::client::agent::AgentKind;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone, Deserialize)]
pub struct JiraConfig {
    pub host: String,
    /// Where `berbr start --new-ticket` creates tickets, e.g. "DEMO".
    pub project: Option<String>,
    /// Their type, by name as the project shows it.
    #[serde(default = "default_issue_type")]
    pub issue_type: String,
}

fn default_issue_type() -> String {
    "Task".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitLabConfig {
    pub host: String,
    pub project: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubConfig {
    pub repo: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WorkflowConfig {
    /// Prepended to new task branches, e.g. "demo/".
    pub branch_prefix: String,
    /// Repository-relative root for new checkouts; legacy checkouts stay in place.
    pub worktree_dir: PathBuf,
    /// CLI used for new conversations; existing conversations keep their owner.
    pub agent: AgentKind,
    /// CLI used only to generate a task title and worktree name from description.
    pub generation_agent: AgentKind,
    /// The generation CLI's default model when empty.
    pub generation_model: Option<String>,
    /// Optional separate CLI for reviews.
    pub review_agent: Option<AgentKind>,
    /// An explicit --agent filter for this invocation only.
    #[serde(skip)]
    pub agent_override: Option<AgentKind>,
    /// Model for Claude reviews; Claude's default when empty.
    pub review_model: Option<String>,
    /// Model for Codex reviews; Codex's configured default when empty.
    pub codex_review_model: Option<String>,
    /// Claude model that names a task (title and worktree) from its
    /// description: a small one, the answer is a few words.
    pub naming_model: String,
    /// macOS app the TUI's `i` opens a worktree in.
    pub ide: String,
}

impl Default for WorkflowConfig {
    fn default() -> Self {
        WorkflowConfig {
            branch_prefix: String::new(),
            worktree_dir: PathBuf::from(crate::client::worktree::DEFAULT_DIRECTORY),
            agent: AgentKind::Claude,
            generation_agent: AgentKind::Claude,
            generation_model: None,
            review_agent: None,
            agent_override: None,
            review_model: None,
            codex_review_model: None,
            naming_model: "haiku".to_string(),
            ide: "IntelliJ IDEA".to_string(),
        }
    }
}

/// Where a project's task notes live: berbr's own folder,
/// `~/.berbr/projects/<project>`, or an Obsidian vault named by `[vault]`.
#[derive(Debug, Clone)]
pub struct VaultConfig {
    pub path: PathBuf,
    /// Folders scanned for task notes, relative to `path`.
    pub task_dirs: Vec<PathBuf>,
    /// Folder for notes created by `berbr start <name>`, relative to `path`.
    pub new_task_dir: PathBuf,
    /// berbr's own folder, not a vault: closed tasks move to `done/`, and
    /// notes open in an editor rather than Obsidian.
    pub own: bool,
}

/// berbr's home, like `~/.claude`: the config, logs and each project's tasks.
/// `BERBR_HOME` overrides.
pub fn berbr_home() -> PathBuf {
    if let Some(home) = std::env::var_os("BERBR_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(home);
    }
    dirs::home_dir().unwrap_or_default().join(".berbr")
}

/// A project's own task folder under berbr's home.
pub fn project_home(name: &str) -> PathBuf {
    berbr_home().join("projects").join(name)
}

/// A repository whose worktrees and merge requests belong to tasks.
#[derive(Debug, Clone, Deserialize)]
pub struct RepoConfig {
    pub path: PathBuf,
    /// Local default branch for diffs when origin/HEAD is unavailable.
    pub main_branch: Option<String>,
    /// Where its merge requests are; none without GitLab.
    pub gitlab_project: Option<String>,
    pub github_repo: Option<String>,
}

/// One project, as every command sees it: its notes, repos, and the Jira and
/// GitLab it has, if any.
#[derive(Debug, Clone)]
pub struct Config {
    /// Its tab in the TUI, `wt -p <name>`.
    pub name: String,
    pub jira: Option<JiraConfig>,
    pub gitlab: Option<GitLabConfig>,
    pub github: Option<GitHubConfig>,
    pub workflow: WorkflowConfig,
    pub vault: VaultConfig,
    pub repos: Vec<RepoConfig>,
}

/// The config file: the vault, defaults for every project, the projects.
#[derive(Debug, Deserialize)]
struct FileConfig {
    /// Log every command, git call and request to berbr's log, not only changes.
    #[serde(default)]
    debug: bool,
    /// An Obsidian vault for task notes; without it they live in berbr's home.
    vault: Option<VaultFile>,
    #[serde(default)]
    workflow: WorkflowConfig,
    #[serde(rename = "project")]
    projects: Vec<ProjectFile>,
}

#[derive(Debug, Deserialize)]
struct VaultFile {
    path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct ProjectFile {
    name: String,
    jira: Option<JiraConfig>,
    gitlab: Option<GitLabConfig>,
    github: Option<GitHubConfig>,
    /// Relative to the vault, or to the project's own folder; default `tasks`
    /// there, required with a vault.
    #[serde(default)]
    task_dirs: Vec<PathBuf>,
    /// Default: the first of `task_dirs`.
    new_task_dir: Option<PathBuf>,
    /// Default: `[workflow] branch_prefix`.
    branch_prefix: Option<String>,
    worktree_dir: Option<PathBuf>,
    #[serde(default, rename = "repo")]
    repos: Vec<RepoConfig>,
}

impl FileConfig {
    /// Each project with the shared parts filled in and `~` expanded.
    fn projects(self) -> Result<Vec<Config>, String> {
        let vault = self.vault.as_ref().map(|v| expand_home(&v.path));
        self.projects
            .into_iter()
            .map(|mut p| {
                if vault.is_none() {
                    if p.name.is_empty() || p.name.starts_with('.') || p.name.contains(['/', '\\'])
                    {
                        return Err(format!(
                            "project {:?}: its name is a folder in {}; no slashes or leading dots",
                            p.name,
                            berbr_home().display()
                        ));
                    }
                    if p.task_dirs.is_empty() {
                        p.task_dirs.push(PathBuf::from("tasks"));
                    }
                }
                let new_task_dir = p
                    .new_task_dir
                    .or_else(|| p.task_dirs.first().cloned())
                    .ok_or_else(|| format!("project {}: no task_dirs", p.name))?;
                let home = project_home(&p.name);
                let mut workflow = self.workflow.clone();
                if let Some(prefix) = p.branch_prefix {
                    workflow.branch_prefix = prefix;
                }
                if let Some(dir) = p.worktree_dir {
                    workflow.worktree_dir = dir;
                }
                crate::client::worktree::validate_directory(&workflow.worktree_dir)
                    .map_err(|e| e.to_string())?;
                if workflow.generation_model.is_none()
                    && workflow.generation_agent == AgentKind::Claude
                {
                    workflow.generation_model = Some(workflow.naming_model.clone());
                }
                if let Some(g) = &mut p.github {
                    g.repo = crate::client::issue::github_repo(&g.repo)?;
                }
                for repo in &mut p.repos {
                    if let Some(g) = &mut repo.github_repo {
                        *g = crate::client::issue::github_repo(g)?;
                    }
                    if repo.github_repo.is_some() && repo.gitlab_project.is_some() {
                        return Err(
                            "a repository cannot configure both github_repo and gitlab_project"
                                .into(),
                        );
                    }
                }
                Ok(Config {
                    name: p.name,
                    jira: p.jira,
                    gitlab: p.gitlab,
                    github: p.github,
                    workflow,
                    vault: VaultConfig {
                        path: vault.clone().unwrap_or(home),
                        task_dirs: p.task_dirs,
                        new_task_dir,
                        own: vault.is_none(),
                    },
                    repos: p
                        .repos
                        .into_iter()
                        .map(|r| RepoConfig {
                            path: expand_home(&r.path),
                            ..r
                        })
                        .collect(),
                })
            })
            .collect()
    }
}

impl Config {
    pub fn github_repos(&self) -> Vec<String> {
        let mut repos: Vec<_> = self
            .repos
            .iter()
            .filter_map(|r| r.github_repo.clone())
            .collect();
        if let Some(g) = &self.github {
            repos.push(g.repo.clone());
        }
        repos.sort();
        repos.dedup();
        repos
    }

    pub fn has_github(&self) -> bool {
        !self.github_repos().is_empty()
    }

    pub fn issue_key(&self, text: &str) -> Result<String, String> {
        if let Some(key) = crate::client::issue::github_key(text) {
            let (repo, _) =
                crate::client::issue::github_parts(&key).ok_or("invalid GitHub issue")?;
            if !self.github_repos().iter().any(|r| r == repo) {
                return Err(format!(
                    "GitHub repository {repo} is not configured in project {}",
                    self.name
                ));
            }
            return Ok(key);
        }
        if let Some(n) = text.strip_prefix('#') {
            if !n.chars().all(|c| c.is_ascii_digit()) || !n.parse::<u64>().is_ok_and(|n| n > 0) {
                return Err("invalid GitHub issue number".into());
            }
            let repos = self.github_repos();
            if repos.len() != 1 {
                return Err("ambiguous GitHub issue: use owner/repo#number".into());
            }
            return self.issue_key(&format!("{}#{n}", repos[0]));
        }
        if text.starts_with("https://github.com/")
            || text.starts_with("github.com/")
            || text.starts_with('#')
            || text
                .rsplit_once('#')
                .is_some_and(|(repo, _)| crate::client::issue::github_repo(repo).is_ok())
        {
            return Err("invalid GitHub issue: use owner/repo#number or https://github.com/owner/repo/issues/number".into());
        }
        Ok(text.to_string())
    }

    /// A Jira client for the project; an error when it has no Jira.
    pub fn jira_client(&self) -> Result<crate::client::jira::JiraClient, String> {
        let jira = self
            .jira
            .as_ref()
            .ok_or_else(|| format!("project {} has no Jira", self.name))?;
        crate::client::jira::JiraClient::from_env(&jira.host)
    }
}

pub fn expand_home(path: &Path) -> PathBuf {
    match (path.strip_prefix("~"), dirs::home_dir()) {
        (Ok(rest), Some(home)) => home.join(rest),
        _ => path.to_path_buf(),
    }
}

/// Explicit config, then an existing plugin config, then `~/.berbr/config.toml`,
/// then the standalone config.
pub(crate) fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .or_else(dirs::config_dir)
        .unwrap_or_else(|| PathBuf::from("."));

    std::env::var_os("BERBR_CONFIG")
        .or_else(|| std::env::var_os("WORKTABLE_CONFIG"))
        .map(PathBuf::from)
        .or_else(|| {
            let path =
                PathBuf::from(std::env::var_os("HERDR_PLUGIN_CONFIG_DIR")?).join("config.toml");
            path.is_file().then_some(path)
        })
        .or_else(|| {
            let path = berbr_home().join("config.toml");
            path.is_file().then_some(path)
        })
        .unwrap_or_else(|| default_config_path(&base))
}

fn default_config_path(base: &Path) -> PathBuf {
    let path = base.join("berbr").join("config.toml");
    let legacy = base.join("worktable").join("config.toml");
    if !path.exists() && legacy.is_file() {
        legacy
    } else {
        path
    }
}

/// Every configured project, in the file's order.
pub fn load_projects() -> Result<Vec<Config>, ConfigError> {
    let config_path = config_path();
    let contents =
        fs::read_to_string(&config_path).map_err(|e| ConfigError::Read(config_path.clone(), e))?;
    let file: FileConfig =
        toml::from_str(&contents).map_err(|e| ConfigError::Parse(config_path.clone(), e))?;
    crate::log::set_debug(file.debug);
    let projects = file
        .projects()
        .map_err(|e| ConfigError::Invalid(config_path.clone(), e))?;
    if projects.is_empty() {
        return Err(ConfigError::Invalid(config_path, "no [[project]]".into()));
    }
    Ok(projects)
}

/// The config for berbr's own task folders: `[vault]` and the projects'
/// `task_dirs` / `new_task_dir` dropped, every other line kept as it is. Fails
/// when the result does not parse, or still names a vault — a multi-line value
/// needs a hand edit then.
pub fn without_vault(text: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut in_vault = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            in_vault = trimmed.trim_end() == "[vault]";
        }
        let key = trimmed.split('=').next().unwrap_or("").trim();
        if in_vault || (trimmed.contains('=') && matches!(key, "task_dirs" | "new_task_dir")) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    let file: FileConfig = toml::from_str(&out).map_err(|e| e.to_string())?;
    if file.vault.is_some() {
        return Err("[vault] is still there".into());
    }
    file.projects()?;
    Ok(out)
}

/// A `[[project]]` with its notes in `task_dir` and, if given, one repo: the
/// config file gets it at its end, the rest of the file left as it is.
pub fn add_project(name: &str, task_dir: &str, repo: Option<&str>) -> Result<Config, String> {
    let path = config_path();
    let mut contents =
        fs::read_to_string(&path).map_err(|e| ConfigError::Read(path.clone(), e).to_string())?;
    if !contents.is_empty() && !contents.ends_with('\n') {
        contents.push('\n');
    }
    contents.push_str(&project_block(name, task_dir, repo));
    // checked before it is written: a file that no longer parses breaks every command
    let file: FileConfig =
        toml::from_str(&contents).map_err(|e| ConfigError::Parse(path.clone(), e).to_string())?;
    let config = file
        .projects()?
        .pop()
        .ok_or_else(|| "the project did not make it into the config".to_string())?;
    fs::write(&path, contents).map_err(|e| format!("can't write {}: {e}", path.display()))?;
    Ok(config)
}

/// First-run configuration, or append a new project without overwriting existing data.
pub fn initialize(
    repo: &Path,
    name: Option<&str>,
    agent: AgentKind,
    github: bool,
) -> Result<Config, String> {
    use std::io::Write;
    let local = crate::client::worktree::worktree_root(&expand_home(repo))
        .map_err(|e| format!("Choose a local Git working directory: {e}"))?;
    let repo = crate::client::worktree::main_checkout(&local)
        .map_err(|e| format!("Choose a local Git repository: {e}"))?;
    let repo = fs::canonicalize(&repo).map_err(|e| e.to_string())?;
    let name = name
        .filter(|s| !s.trim().is_empty())
        .map(str::trim)
        .or_else(|| repo.file_name().and_then(|s| s.to_str()))
        .ok_or("Enter a project name")?;
    let path = if config_path().exists()
        || std::env::var_os("BERBR_CONFIG").is_some()
        || std::env::var_os("WORKTABLE_CONFIG").is_some()
    {
        config_path()
    } else {
        berbr_home().join("config.toml")
    };
    // Preserve symlinked configurations instead of replacing the link itself.
    let path = if path.exists() {
        fs::canonicalize(&path).map_err(|e| e.to_string())?
    } else {
        path
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let original = match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("Cannot read {}: {e}", path.display())),
    };
    let mut text = original.clone().unwrap_or_default();
    if let Some(original) = &original {
        let file: FileConfig = toml::from_str(original).map_err(|e| e.to_string())?;
        if file.vault.is_some() {
            return Err(
                "This config uses a vault. Add the project in the TUI or config file.".into(),
            );
        }
        for project in file.projects()? {
            if project.name.eq_ignore_ascii_case(name)
                || project
                    .repos
                    .iter()
                    .any(|r| fs::canonicalize(&r.path).ok().as_ref() == Some(&repo))
            {
                return Err(format!(
                    "Project or repository already connected as {}",
                    project.name
                ));
            }
        }
    } else {
        text.push_str(&format!(
            "[workflow]\nagent = \"{0}\"\ngeneration_agent = \"{0}\"\ngeneration_model = \"\"\n\n",
            agent.binary()
        ));
    }
    text.push_str(&project_block(name, "tasks", Some(&repo.to_string_lossy())));
    if github {
        let output = std::process::Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(&repo)
            .output()
            .map_err(|e| e.to_string())?;
        let remote = String::from_utf8_lossy(&output.stdout);
        let slug = remote
            .trim()
            .strip_prefix("https://github.com/")
            .or_else(|| remote.trim().strip_prefix("git@github.com:"))
            .or_else(|| remote.trim().strip_prefix("ssh://git@github.com/"))
            .ok_or("GitHub setup needs a github.com origin remote")?;
        let slug =
            crate::client::issue::github_repo(slug.trim_end_matches('/').trim_end_matches(".git"))?;
        text.push_str(&format!("github_repo = {}\n", toml::Value::String(slug)));
    }
    let parsed: FileConfig = toml::from_str(&text).map_err(|e| e.to_string())?;
    let project = parsed.projects()?.pop().ok_or("No project was added")?;
    let mut pending =
        tempfile::NamedTempFile::new_in(path.parent().unwrap()).map_err(|e| e.to_string())?;
    if original.is_some() {
        pending
            .as_file()
            .set_permissions(
                fs::metadata(&path)
                    .map_err(|e| e.to_string())?
                    .permissions(),
            )
            .map_err(|e| e.to_string())?;
    }
    pending
        .write_all(text.as_bytes())
        .map_err(|e| e.to_string())?;
    pending.as_file().sync_all().map_err(|e| e.to_string())?;
    if let Some(original) = original {
        if fs::read_to_string(&path).map_err(|e| e.to_string())? != original {
            return Err("Config changed; retry connecting the project".into());
        }
        pending.persist(&path).map_err(|e| e.to_string())?;
    } else {
        pending
            .persist_noclobber(&path)
            .map_err(|e| e.to_string())?;
    }
    Ok(project)
}

/// Save universal defaults in [workflow]; retain project data and comments.
pub fn update_settings(
    agent: Option<AgentKind>,
    generation_agent: Option<AgentKind>,
    generation_model: Option<&str>,
) -> Result<WorkflowConfig, String> {
    update_settings_at(&config_path(), agent, generation_agent, generation_model)
}

fn update_settings_at(
    path: &Path,
    agent: Option<AgentKind>,
    generation_agent: Option<AgentKind>,
    generation_model: Option<&str>,
) -> Result<WorkflowConfig, String> {
    use std::io::Write;
    let path = fs::canonicalize(path).map_err(|e| e.to_string())?;
    let original = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut document: toml_edit::DocumentMut = original.parse().map_err(|e| format!("{e}"))?;
    if !document.contains_key("workflow") {
        document["workflow"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let entry = document["workflow"]
        .as_table_mut()
        .ok_or_else(|| "[workflow] must be a table".to_string())?;
    if let Some(agent) = agent {
        set_setting(entry, "agent", agent.binary());
    }
    if let Some(agent) = generation_agent {
        set_setting(entry, "generation_agent", agent.binary());
    }
    if let Some(model) = generation_model {
        // Empty selects the naming CLI's configured default.
        set_setting(entry, "generation_model", model.trim());
    }
    let updated = document.to_string();
    let parsed: FileConfig = toml::from_str(&updated).map_err(|e| e.to_string())?;
    let config = parsed
        .projects()?
        .into_iter()
        .next()
        .ok_or_else(|| "no [[project]]".to_string())?
        .workflow;
    if generation_agent.is_some() || generation_model.is_some() {
        crate::client::agent::validate_naming_model(
            config.generation_agent,
            config.generation_model.as_deref().unwrap_or("").trim(),
        )?;
    }
    let mut pending =
        tempfile::NamedTempFile::new_in(path.parent().unwrap()).map_err(|e| e.to_string())?;
    pending
        .as_file()
        .set_permissions(
            fs::metadata(&path)
                .map_err(|e| e.to_string())?
                .permissions(),
        )
        .map_err(|e| e.to_string())?;
    pending
        .write_all(updated.as_bytes())
        .map_err(|e| e.to_string())?;
    pending.as_file().sync_all().map_err(|e| e.to_string())?;
    if fs::read_to_string(&path).map_err(|e| e.to_string())? != original {
        return Err("config changed on disk; reload settings and retry".into());
    }
    pending.persist(&path).map_err(|e| e.to_string())?;
    Ok(config)
}

fn set_setting(table: &mut toml_edit::Table, key: &str, text: &str) {
    let mut value = toml_edit::Value::from(text);
    if let Some(previous) = table.get(key).and_then(toml_edit::Item::as_value) {
        *value.decor_mut() = previous.decor().clone();
    }
    table[key] = toml_edit::Item::Value(value);
}

fn project_block(name: &str, task_dir: &str, repo: Option<&str>) -> String {
    let quote = |s: &str| toml::Value::String(s.to_string()).to_string();
    let mut block = format!(
        "\n[[project]]\nname = {}\ntask_dirs = [{}]\n",
        quote(name),
        quote(task_dir)
    );
    if let Some(repo) = repo {
        block.push_str(&format!("\n[[project.repo]]\npath = {}\n", quote(repo)));
    }
    block
}

/// The project named `name`; without a name, the one whose repo `dir` is in,
/// else the first.
pub fn choose_project(
    projects: &[Config],
    name: Option<&str>,
    dir: Option<&Path>,
) -> Result<usize, String> {
    if let Some(name) = name {
        return projects
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
                format!("no project \"{name}\": {}", names.join(", "))
            });
    }
    Ok(dir.and_then(|dir| project_of(projects, dir)).unwrap_or(0))
}

/// The project a repo of which, or a worktree under it, holds `dir`.
pub fn project_of(projects: &[Config], dir: &Path) -> Option<usize> {
    projects
        .iter()
        .position(|p| p.repos.iter().any(|r| dir.starts_with(&r.path)))
}

/// berbr's own cache: last Jira and GitLab answers, the session index, the
/// notes lock, `berbr prepare` specs.
pub fn cache_dir() -> PathBuf {
    // Unit tests must not use the person's cache or share its note-write lock.
    #[cfg(test)]
    let base = {
        static CACHE: std::sync::LazyLock<PathBuf> =
            std::sync::LazyLock::new(crate::table::fixtures::temp_dir);
        CACHE.clone()
    };
    #[cfg(not(test))]
    let base = dirs::cache_dir().unwrap_or_else(std::env::temp_dir);
    cache_path(&base)
}

fn cache_path(base: &Path) -> PathBuf {
    // Keep existing state and the note-write lock shared with older binaries.
    let legacy = base.join("worktable");
    if legacy.is_dir() {
        legacy
    } else {
        base.join("berbr")
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("can't read config {}: {}", .0.display(), .1)]
    Read(PathBuf, #[source] std::io::Error),
    #[error("can't parse config {}: {}", .0.display(), .1)]
    Parse(PathBuf, #[source] toml::de::Error),
    #[error("config {}: {}", .0.display(), .1)]
    Invalid(PathBuf, String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vault_config_becomes_one_for_berbr_s_own_folders() {
        let text = "debug = true\n\n[vault]\npath = \"~/notes\"\n\n[workflow]\nbranch_prefix = \"dev/\"\n\n[[project]]\nname = \"demo\"\ntask_dirs = [\"30/tasks\"]\nnew_task_dir = \"30/tasks\"\njira = { host = \"https://jira.example.invalid\", project = \"DEMO\" }\n\n[[project.repo]]\npath = \"~/code/demo\"\n";
        let own = without_vault(text).unwrap();
        assert!(!own.contains("[vault]") && !own.contains("task_dirs") && !own.contains("~/notes"));
        assert!(own.contains("branch_prefix = \"dev/\"") && own.contains("[[project.repo]]"));
        let file: FileConfig = toml::from_str(&own).unwrap();
        let demo = file.projects().unwrap().remove(0);
        assert!(demo.vault.own);
        assert_eq!(demo.vault.path, project_home("demo"));
        assert_eq!(demo.vault.task_dirs, vec![PathBuf::from("tasks")]);
        assert!(without_vault("[vault]\npath = \"/v\"\n[[project]]\nname = \"a/b\"\n").is_err());
    }

    #[test]
    fn renamed_config_prefers_berbr_and_falls_back_to_worktable() {
        let base = crate::table::fixtures::temp_dir();
        let new = base.join("berbr/config.toml");
        let old = base.join("worktable/config.toml");
        assert_eq!(default_config_path(&base), new);
        fs::create_dir_all(old.parent().unwrap()).unwrap();
        fs::write(&old, "legacy").unwrap();
        assert_eq!(default_config_path(&base), old);
        fs::create_dir_all(new.parent().unwrap()).unwrap();
        fs::write(&new, "new").unwrap();
        assert_eq!(default_config_path(&base), new);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn existing_cache_stays_shared_with_legacy_binaries() {
        let base = crate::table::fixtures::temp_dir();
        let new = base.join("berbr");
        let old = base.join("worktable");
        assert_eq!(cache_path(&base), new);
        fs::create_dir(&new).unwrap();
        fs::create_dir(&old).unwrap();
        assert_eq!(cache_path(&base), old);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn projects_share_the_vault_and_defaults() {
        let file: FileConfig = toml::from_str(
            r#"
            [vault]
            path = "/v"
            [workflow]
            branch_prefix = "me/"
            worktree_dir = ".herdr/shared"

            [[project]]
            name = "demo"
            task_dirs = ["30-work/tasks"]
            jira = { host = "https://jira" }
            [[project.repo]]
            path = "/code/demo"
            gitlab_project = "demo/service"

            [[project]]
            name = "vpn"
            worktree_dir = "dev/trees"
            task_dirs = ["10/projects/vpn"]
            branch_prefix = ""
            [[project.repo]]
            path = "/code/vpn-bot"
            "#,
        )
        .unwrap();
        let projects = file.projects().unwrap();
        let (demo, vpn) = (&projects[0], &projects[1]);
        assert_eq!(demo.vault.new_task_dir, PathBuf::from("30-work/tasks"));
        assert_eq!(demo.workflow.branch_prefix, "me/");
        assert_eq!(demo.workflow.worktree_dir, PathBuf::from(".herdr/shared"));
        assert_eq!(vpn.workflow.worktree_dir, PathBuf::from("dev/trees"));
        assert!(demo.jira.is_some() && demo.gitlab.is_none());
        assert_eq!(vpn.vault.path, PathBuf::from("/v"));
        assert_eq!(vpn.workflow.branch_prefix, "");
        assert!(vpn.jira.is_none() && vpn.repos[0].gitlab_project.is_none());

        let dir = Path::new("/code/vpn-bot/.claude/worktrees/fix");
        assert_eq!(choose_project(&projects, None, Some(dir)), Ok(1));
        assert_eq!(choose_project(&projects, None, None), Ok(0));
        assert_eq!(choose_project(&projects, Some("VPN"), None), Ok(1));
        assert!(choose_project(&projects, Some("kitten"), None).is_err());
    }

    #[test]
    fn agent_defaults_are_universal_and_legacy_project_overrides_are_ignored() {
        let text = r#"
            [vault]
            path = "/v"
            [[project]]
            name = "legacy"
            task_dirs = ["tasks"]
        "#;
        let file: FileConfig = toml::from_str(text).unwrap();
        assert_eq!(
            file.projects().unwrap()[0].workflow.agent,
            AgentKind::Claude
        );
        let text = r#"
            [vault]
            path = "/v"
            [workflow]
            agent = "codex"
            review_agent = "claude"
            review_model = "claude-review-model"
            codex_review_model = "codex-review-model"
            [[project]]
            name = "default"
            task_dirs = ["tasks"]
            [[project]]
            name = "override"
            task_dirs = ["other"]
            agent = "claude"
        "#;
        let file: FileConfig = toml::from_str(text).unwrap();
        let projects = file.projects().unwrap();
        assert_eq!(projects[0].workflow.agent, AgentKind::Codex);
        assert_eq!(projects[1].workflow.agent, AgentKind::Codex);
        assert_eq!(projects[0].workflow.review_agent, Some(AgentKind::Claude));
        assert_eq!(
            projects[0].workflow.codex_review_model.as_deref(),
            Some("codex-review-model")
        );
        assert!(
            toml::from_str::<FileConfig>(&text.replace("agent = \"codex\"", "agent = \"typo\""))
                .is_err()
        );
    }

    #[test]
    fn naming_settings_are_universal_even_with_legacy_project_values() {
        let file: FileConfig = toml::from_str(
            r#"
            [workflow]
            agent = "codex"
            generation_agent = "claude"
            generation_model = "haiku"
            [[project]]
            name = "one"
            [[project]]
            name = "two"
            agent = "claude"
            generation_agent = "codex"
            generation_model = "old-model"
        "#,
        )
        .unwrap();
        for project in file.projects().unwrap() {
            assert_eq!(project.workflow.agent, AgentKind::Codex);
            assert_eq!(project.workflow.generation_agent, AgentKind::Claude);
            assert_eq!(project.workflow.generation_model.as_deref(), Some("haiku"));
        }
    }

    #[test]
    fn changing_work_agent_does_not_require_a_valid_naming_model() {
        let dir = crate::table::fixtures::temp_dir();
        let path = dir.join("config.toml");
        fs::write(
            &path,
            "[workflow]\ngeneration_model = \"removed-model\"\n[[project]]\nname = \"demo\"\n",
        )
        .unwrap();
        let updated = update_settings_at(&path, Some(AgentKind::Codex), None, None).unwrap();
        assert_eq!(updated.agent, AgentKind::Codex);
        assert_eq!(updated.generation_model.as_deref(), Some("removed-model"));
        let before = fs::read(&path).unwrap();
        assert!(
            update_settings_at(&path, None, Some(AgentKind::Claude), Some("invalid-model"))
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn global_settings_preserve_projects_comments_permissions_and_symlinks() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = crate::table::fixtures::temp_dir();
        let path = dir.join("config.toml");
        let link = dir.join("linked.toml");
        let source = "# Keep this\n[workflow]\nagent = \"claude\" # default CLI\n[[project]]\nname = \"one\" # chosen\n[[project.repo]]\npath = \"/repo\"\n[[project]]\nname = \"two\"\nagent = \"claude\" # legacy value retained\n";
        fs::write(&path, source).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&path, &link).unwrap();
        let config = update_settings_at(
            &link,
            Some(AgentKind::Codex),
            Some(AgentKind::Claude),
            Some(" haiku "),
        )
        .unwrap();
        assert_eq!(config.generation_model.as_deref(), Some("haiku"));
        let updated = fs::read_to_string(&path).unwrap();
        assert!(updated.starts_with("# Keep this\n"));
        assert!(updated.contains("agent = \"codex\" # default CLI"));
        assert!(updated.ends_with(
            "[[project]]\nname = \"two\"\nagent = \"claude\" # legacy value retained\n"
        ));
        for project in toml::from_str::<FileConfig>(&updated)
            .unwrap()
            .projects()
            .unwrap()
        {
            assert_eq!(project.workflow.agent, AgentKind::Codex);
            assert_eq!(project.workflow.generation_model.as_deref(), Some("haiku"));
        }
        assert!(link.is_symlink());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let cleared = update_settings_at(&link, None, None, Some("")).unwrap();
        assert_eq!(cleared.agent, AgentKind::Codex);
        assert_eq!(cleared.generation_model.as_deref(), Some(""));
        let before = fs::read(&path).unwrap();
        assert!(update_settings_at(&path, None, None, Some("invented-model")).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn settings_create_a_workflow_table_without_changing_project_tables() {
        let dir = crate::table::fixtures::temp_dir();
        let path = dir.join("config.toml");
        let source = "[[project]]\nname = \"one\"\n[[project]]\nname = \"two\"\n";
        fs::write(&path, source).unwrap();
        update_settings_at(&path, Some(AgentKind::Codex), None, Some("")).unwrap();
        let updated = fs::read_to_string(&path).unwrap();
        assert!(updated.starts_with(source));
        assert!(updated.contains("[workflow]"));
        for project in toml::from_str::<FileConfig>(&updated)
            .unwrap()
            .projects()
            .unwrap()
        {
            assert_eq!(project.workflow.agent, AgentKind::Codex);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_added_project_goes_last_with_its_repo() {
        let mut text =
            "[vault]\npath = \"/v\"\n\n[[project]]\nname = \"demo\"\ntask_dirs = [\"t\"]\n"
                .to_string();
        text.push_str(&project_block(
            "kcal \"bot\"",
            "10/projects/kcal",
            Some("~/code/kcal"),
        ));
        text.push_str(&project_block("notes", "10/notes", None));
        let file: FileConfig = toml::from_str(&text).unwrap();
        let projects = file.projects().unwrap();
        let names: Vec<_> = projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["demo", "kcal \"bot\"", "notes"]);
        assert_eq!(
            projects[1].vault.new_task_dir,
            PathBuf::from("10/projects/kcal")
        );
        assert!(projects[1].repos[0].path.ends_with("code/kcal"));
        assert!(projects[2].repos.is_empty());
    }
}

#[cfg(test)]
mod github_tests {
    use super::*;

    #[test]
    fn github_only_and_mixed_config_normalize_and_disambiguate_issue_arguments() {
        let configs = toml::from_str::<FileConfig>(
            r#"
[[project]]
name = "demo"
github = {repo = "Owner/Repo"}
[[project.repo]]
path = "/synthetic/demo"
[[project]]
name = "mixed"
jira = {host = "https://jira.example.invalid"}
gitlab = {host = "gitlab.example.invalid", project = "demo/service"}
github = {repo = "owner/repo"}
[[project.repo]]
path = "/synthetic/fork"
github_repo = "Fork/Repo"
"#,
        )
        .unwrap()
        .projects()
        .unwrap();
        let demo = &configs[0];
        assert!(demo.jira.is_none() && demo.gitlab.is_none());
        assert_eq!(demo.issue_key("#1").unwrap(), "github.com/owner/repo#1");
        assert_eq!(
            demo.issue_key("https://github.com/Owner/Repo/issues/1")
                .unwrap(),
            "github.com/owner/repo#1"
        );
        assert!(demo.issue_key("other/repo#1").is_err());
        for text in ["#0", "#invalid", "demo/repo#0", "demo/repo#invalid"] {
            assert!(demo.issue_key(text).is_err(), "{text}");
        }
        assert!(
            demo.issue_key("https://github.com/owner/repo/pull/1")
                .is_err()
        );
        let mixed = &configs[1];
        assert!(mixed.issue_key("#1").is_err());
        assert_eq!(
            mixed.issue_key("fork/repo#1").unwrap(),
            "github.com/fork/repo#1"
        );
        assert_eq!(mixed.issue_key("DEMO-1").unwrap(), "DEMO-1");
    }
}
