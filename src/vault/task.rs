use crate::config::{Config, VaultConfig};
use crate::vault::note::{Note, NoteError};
use regex::Regex;
use serde_yaml::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

static TICKET_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Z][A-Z0-9]+-\d+\b").unwrap());

/// Jira keys mentioned in `text`, in order, without duplicates.
pub fn ticket_keys(text: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for m in TICKET_KEY.find_iter(text) {
        if !keys.iter().any(|k| k == m.as_str()) {
            keys.push(m.as_str().to_string());
        }
    }
    keys
}

/// An agent session held in one of the task's worktrees, stored in the note as
/// `<session id> <worktree path> <date>`; old bare worktree names still parse.
/// Whitespace/empty fields use `v2:` plus a JSON string array. Codex session IDs
/// have a `codex:` prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRef {
    pub id: String,
    pub worktree: String,
    pub date: String,
}

impl SessionRef {
    fn parse(entry: &str) -> Option<SessionRef> {
        if let Some(encoded) = entry.strip_prefix("v2:") {
            let [id, worktree, date]: [String; 3] = serde_json::from_str(encoded).ok()?;
            return Some(SessionRef { id, worktree, date });
        }
        let mut parts = entry.split_whitespace();
        let id = parts.next()?.to_string();
        let rest = entry.trim().strip_prefix(&id)?.trim();
        if let Some((worktree, date)) = rest.rsplit_once(char::is_whitespace)
            && date.len() == 10
            && chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok()
        {
            return Some(SessionRef {
                id,
                worktree: worktree.trim().to_string(),
                date: date.into(),
            });
        }
        Some(SessionRef {
            id,
            worktree: parts.next().unwrap_or_default().to_string(),
            date: parts.next().unwrap_or_default().to_string(),
        })
    }

    pub fn tree_name(&self) -> &str {
        Path::new(&self.worktree)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&self.worktree)
    }

    pub fn matches_tree(&self, path: &Path) -> bool {
        if Path::new(&self.worktree).is_absolute() {
            Path::new(&self.worktree) == path
        } else {
            path.file_name()
                .is_some_and(|name| name == self.worktree.as_str())
        }
    }

    pub fn entry(&self) -> String {
        if [&self.id, &self.worktree, &self.date]
            .iter()
            .any(|s| s.is_empty() || s.chars().any(char::is_whitespace))
            || self.id.starts_with("v2:")
        {
            format!(
                "v2:{}",
                serde_json::to_string(&[&self.id, &self.worktree, &self.date])
                    .expect("session strings")
            )
        } else {
            format!("{} {} {}", self.id, self.worktree, self.date)
        }
    }
}

/// The `state:` a task note carries, derived from its tickets, MRs and
/// worktrees; a closed task has `done: true` instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Active,
    Review,
    Idea,
    Finished,
}

impl TaskState {
    pub fn parse(text: &str) -> Option<TaskState> {
        match text {
            "active" => Some(TaskState::Active),
            "review" => Some(TaskState::Review),
            "idea" => Some(TaskState::Idea),
            "finished" => Some(TaskState::Finished),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TaskState::Active => "active",
            TaskState::Review => "review",
            TaskState::Idea => "idea",
            TaskState::Finished => "finished",
        }
    }
}

/// A task note: shared context for one piece of work and the tickets it covers.
#[derive(Debug)]
pub struct Task {
    pub note: Note,
    /// Stable note identity: `<project>-<number>` for new tasks, filename for legacy notes.
    pub name: String,
    pub tickets: Vec<String>,
    pub tags: Vec<String>,
    /// The task this one is part of, by name: `parent: "[[name]]"`.
    pub parent: Option<String>,
    pub done: bool,
    pub state: Option<TaskState>,
    pub sessions: Vec<SessionRef>,
    /// When the note was created, RFC 3339; set by `load_tasks`.
    pub created: Option<String>,
}

/// The note `[[name]]`, `[[name|alias]]` or a bare `name` points at.
fn link_target(link: &str) -> Option<String> {
    let inner = link.trim().trim_start_matches("[[").trim_end_matches("]]");
    let name = inner.split(['|', '#']).next().unwrap_or("").trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Labels are compared the same way in the CLI, snapshots and space lookup.
pub fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

pub fn task_space_matches(task: &Task, label: &str) -> bool {
    same_name(&task.name, label) || task.title().is_some_and(|title| same_name(&title, label))
}

/// Immutable identity wins; a legacy title is usable only when unambiguous.
pub fn task_space<'a>(
    task: &Task,
    spaces: &'a [crate::client::herdr::Workspace],
) -> Result<Option<&'a crate::client::herdr::Workspace>, String> {
    let saved = task.note.string("space");
    let ids: Vec<_> = spaces
        .iter()
        .filter(|s| saved.as_deref() == Some(s.workspace_id.as_str()))
        .collect();
    let ids = if ids.is_empty() {
        spaces
            .iter()
            .filter(|s| same_name(&task.name, &s.label))
            .collect()
    } else {
        ids
    };
    let matches = if ids.is_empty() {
        spaces
            .iter()
            .filter(|s| {
                task.title()
                    .is_some_and(|title| same_name(&title, &s.label))
            })
            .collect()
    } else {
        ids
    };
    match matches.as_slice() {
        [] => Ok(None),
        [space] => Ok(Some(*space)),
        _ => Err(format!(
            "{}: ambiguous Herdr space; give the intended space the task ID",
            task.name
        )),
    }
}

/// Resolve legacy labels using all task identities, not only workspace names.
pub fn task_space_in<'a>(
    task: &Task,
    spaces: &'a [crate::client::herdr::Workspace],
    tasks: &[Task],
) -> Result<Option<&'a crate::client::herdr::Workspace>, String> {
    let space = task_space(task, spaces)?;
    if let Some(space) = space
        && !same_name(&task.name, &space.label)
        && task.note.string("space").as_deref() != Some(space.workspace_id.as_str())
        && tasks
            .iter()
            .filter(|t| task_space_matches(t, &space.label))
            .count()
            > 1
    {
        return Err(format!(
            "{}: ambiguous Herdr title {}; use the task ID as the space name",
            task.name, space.label
        ));
    }
    Ok(space)
}

impl Task {
    pub fn space_name(&self) -> String {
        self.title().unwrap_or_else(|| self.name.clone())
    }

    /// Stable identity is the note's file name; titles never move the note.
    pub fn title(&self) -> Option<String> {
        self.note
            .string("title")
            .filter(|s| !s.trim().is_empty())
            .or_else(|| self.note.string("id").is_none().then(|| self.name.clone()))
    }

    pub fn display_name(&self) -> String {
        match self.title() {
            Some(title) if title != self.name => format!("{}  {title}", self.name),
            _ => self.name.clone(),
        }
    }

    pub fn from_note(note: Note) -> Task {
        Task {
            name: note.name(),
            tickets: note
                .strings("tickets")
                .iter()
                .flat_map(|t| ticket_keys(t))
                .collect(),
            tags: note.strings("tags"),
            parent: note.string("parent").and_then(|p| link_target(&p)),
            done: note.flag("done"),
            state: note.string("state").as_deref().and_then(TaskState::parse),
            sessions: note
                .strings("sessions")
                .iter()
                .filter_map(|s| SessionRef::parse(s))
                .collect(),
            created: None,
            note,
        }
    }

    /// Writes a derived `state`, tickets found on the task's work and sessions
    /// the note does not list yet into its frontmatter; the note's own entries
    /// stay as written, found ones go after them. True when the note changed on
    /// disk: the task then holds them too, taken out of `tickets` and `sessions`.
    pub fn record(
        &mut self,
        state: Option<TaskState>,
        tickets: &mut Vec<String>,
        sessions: &mut Vec<SessionRef>,
    ) -> Result<bool, NoteError> {
        let state_changed = state.is_some_and(|state| self.state != Some(state));
        if !state_changed && tickets.is_empty() && sessions.is_empty() {
            return Ok(false);
        }
        // write_back holds the vault lock. A preview must be re-read and its
        // metadata checked before any managed fields are changed in memory.
        self.note.reload_preview_for_update()?;
        if let Some(state) = state
            && state_changed
        {
            self.note
                .set("state", Value::String(state.as_str().to_string()));
        }
        if !tickets.is_empty() {
            let entries = self
                .note
                .strings("tickets")
                .into_iter()
                .chain(tickets.iter().cloned())
                .map(Value::String)
                .collect();
            self.note.set("tickets", Value::Sequence(entries));
        }
        if !sessions.is_empty() {
            let entries = self
                .sessions
                .iter()
                .chain(sessions.iter())
                .map(|s| Value::String(s.entry()))
                .collect();
            self.note.set("sessions", Value::Sequence(entries));
        }
        if !self.note.save()? {
            return Ok(false);
        }
        if state.is_some() {
            self.state = state;
        }
        self.sessions.append(sessions);
        self.tickets.append(tickets);
        Ok(true)
    }
}

/// The tasks of the configured vault, and the notes skipped because they
/// could not be read.
pub fn load(config: &Config) -> Result<(Vec<Task>, Vec<NoteError>), String> {
    Ok(load_tasks(&config.vault))
}

/// Task notes under the configured folders; notes whose frontmatter does not
/// parse are reported and left alone.
pub fn load_tasks(vault: &VaultConfig) -> (Vec<Task>, Vec<NoteError>) {
    load_with(vault, Note::read)
}

/// App snapshots need metadata and bounded human previews, never editable notes.
pub fn load_previews(config: &Config) -> (Vec<Task>, Vec<NoteError>) {
    load_with(&config.vault, Note::read_preview)
}

fn load_with(
    vault: &VaultConfig,
    read: fn(&Path) -> Result<Note, NoteError>,
) -> (Vec<Task>, Vec<NoteError>) {
    let mut tasks = Vec::new();
    let mut errors = Vec::new();
    let added = git_added(vault);
    for dir in &vault.task_dirs {
        for path in markdown_files(&vault.path.join(dir)) {
            match read(&path) {
                Ok(note) => {
                    let mut task = Task::from_note(note);
                    // written at creation since berbr has its own folder;
                    // older vault notes have only their first commit
                    task.created = task
                        .note
                        .string("created")
                        .or_else(|| {
                            path.strip_prefix(&vault.path)
                                .ok()
                                .and_then(|rel| added.get(rel).cloned())
                        })
                        .or_else(|| birth_time(&path));
                    tasks.push(task);
                }
                Err(err) => errors.push(err),
            }
        }
    }
    tasks.sort_by(|a, b| a.name.cmp(&b.name));
    (tasks, errors)
}

/// When each note first appeared in the vault's git history (the vault is
/// auto-committed). The file's own birth time is useless: saving a note goes
/// through a temp file and a rename.
fn git_added(vault: &VaultConfig) -> HashMap<PathBuf, String> {
    if vault.own {
        return HashMap::new();
    }
    let mut args = vec![
        "-c".to_string(),
        "core.quotePath=false".to_string(),
        "log".to_string(),
        "--diff-filter=A".to_string(),
        "--name-only".to_string(),
        "--format=@%aI".to_string(),
        "--".to_string(),
    ];
    args.extend(
        vault
            .task_dirs
            .iter()
            .map(|d| d.to_string_lossy().to_string()),
    );
    let Ok(output) = crate::log::command(
        "git",
        Command::new("git").arg("-C").arg(&vault.path).args(&args),
    ) else {
        return HashMap::new();
    };
    let mut added = HashMap::new();
    let mut date = String::new();
    // newest commits first: the last date seen for a path is its first add
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(d) = line.strip_prefix('@') {
            date = d.to_string();
        } else if !line.is_empty() {
            added.insert(PathBuf::from(line), date.clone());
        }
    }
    added
}

fn birth_time(path: &Path) -> Option<String> {
    let created = std::fs::metadata(path).ok()?.created().ok()?;
    Some(chrono::DateTime::<chrono::Local>::from(created).to_rfc3339())
}

fn markdown_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            files.extend(markdown_files(&path));
        } else if path.extension().is_some_and(|e| e == "md") {
            files.push(path);
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_preview_record_does_not_read_the_full_file() {
        let dir = crate::table::fixtures::temp_dir();
        let path = dir.join("demo-1.md");
        std::fs::write(
            &path,
            "---\nid: demo-1\nstate: idea\n---\nHuman requirements.\n",
        )
        .unwrap();
        let mut task = Task::from_note(Note::read_preview(&path).unwrap());
        std::fs::remove_file(&path).unwrap();
        let _lock = crate::vault::lock().unwrap();
        assert!(
            !task
                .record(Some(TaskState::Idea), &mut vec![], &mut vec![])
                .unwrap()
        );
        assert!(matches!(task.note.save(), Err(NoteError::Preview(_))));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn preview_write_back_rejects_changed_frontmatter_before_mutating_anything() {
        let dir = crate::table::fixtures::temp_dir();
        let path = dir.join("demo-1.md");
        let source = "---\nid: demo-1\nproject: demo\nstate: idea\ntickets: [DEMO-1]\nsessions: []\ndone: false\n---\nHuman requirements.\n\n## Agent notes\nHistory.\n";
        for (old, new) in [
            ("id: demo-1", "id: demo-2"),
            ("project: demo", "project: other-demo"),
            ("state: idea", "state: review"),
            ("tickets: [DEMO-1]", "tickets: [DEMO-3]"),
            (
                "sessions: []",
                "sessions: ['codex:other demo-tree 2026-01-01']",
            ),
            ("done: false", "done: true"),
        ] {
            std::fs::write(&path, source).unwrap();
            let preview = Task::from_note(Note::read_preview(&path).unwrap());
            let mut table = crate::table::fixtures::table(vec![preview], vec![], vec![], vec![]);
            table.tasks[0].state = Some(TaskState::Active);
            table.tasks[0].found_tickets.push("DEMO-2".into());
            table.tasks[0].new_sessions.push(SessionRef {
                id: "codex:demo".into(),
                worktree: "demo-tree".into(),
                date: "2026-01-02".into(),
            });
            let current = source.replace(old, new);
            std::fs::write(&path, &current).unwrap();
            assert!(crate::table::write_back(&mut table).is_empty());
            assert_eq!(
                table.warnings,
                [NoteError::Changed(path.clone()).to_string()]
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), current);
            assert_eq!(table.tasks[0].task.state, Some(TaskState::Idea));
            assert_eq!(table.tasks[0].found_tickets, ["DEMO-2"]);
            assert_eq!(table.tasks[0].new_sessions.len(), 1);
            assert!(matches!(
                table.tasks[0].task.note.save(),
                Err(NoteError::Preview(_))
            ));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn extracts_keys_once() {
        assert_eq!(
            ticket_keys("demo/DEMO-42-a2i, DEMO-42: x, see DEMO-7"),
            vec!["DEMO-42", "DEMO-7"]
        );
        assert!(ticket_keys("utf-8 and x86-64").is_empty());
    }

    #[test]
    fn session_entry_roundtrip() {
        let s = SessionRef::parse("a9c0 a2i-config 2026-10-08").unwrap();
        assert_eq!(s.worktree, "a2i-config");
        assert_eq!(s.entry(), "a9c0 a2i-config 2026-10-08");
        let codex = SessionRef::parse("codex:a9c0 a2i-config 2026-10-29").unwrap();
        assert_eq!(codex.entry(), "codex:a9c0 a2i-config 2026-10-29");
        assert_ne!(codex.id, s.id);
    }
}

#[cfg(test)]
mod regressions {
    use super::*;
    #[test]
    fn session_refs_must_roundtrip_worktree_with_spaces() {
        let s = SessionRef {
            id: "codex:demo".into(),
            worktree: "two words".into(),
            date: "2026-11-01".into(),
        };
        assert_eq!(SessionRef::parse(&s.entry()).unwrap(), s);
    }
}
