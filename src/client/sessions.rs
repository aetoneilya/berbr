use super::agent::AgentKind;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// A Claude or Codex conversation. Codex ids use a `codex:` prefix; historical
/// bare ids remain Claude ids.
/// The launch directory is not where the work happened: an agent started in the
/// main checkout often moves into a worktree, so the transcript's `cwd` fields
/// are what ties a session to a worktree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    /// Day of the first message, YYYY-MM-DD (UTC).
    pub date: String,
    /// Working directories of the transcript's records, most frequent first.
    pub cwds: Vec<(String, usize)>,
    /// Where claude was launched: `--resume` finds the session only from there.
    pub first_cwd: Option<String>,
    pub last_cwd: Option<String>,
    /// Unix time the transcript last changed: the session's last activity. Taken
    /// from the file, not cached with the parsed transcript.
    #[serde(skip)]
    pub modified: u64,
}

impl Session {
    /// Names of the worktrees under `root` the session worked in, most used first.
    pub fn worktrees_under(&self, root: &Path) -> Vec<String> {
        let root = format!("{}/", root.to_string_lossy());
        let mut names: Vec<String> = Vec::new();
        for (cwd, _) in &self.cwds {
            let Some(rest) = cwd.strip_prefix(&root) else {
                continue;
            };
            let name = rest.split('/').next().unwrap_or(rest).to_string();
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }
}

fn projects_dir() -> Option<PathBuf> {
    let base = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".claude")))?;
    Some(base.join("projects"))
}

/// Claude names a project directory after the cwd with every character outside
/// [A-Za-z0-9-] replaced by '-'.
fn slug(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

#[derive(Serialize, Deserialize)]
struct Entry {
    len: u64,
    mtime: u64,
    identity: (u64, u64),
    offset: u64,
    fingerprint: u64,
    aggregate: Aggregate,
}

/// Parsed transcripts keyed by file, reused while a file's size and mtime stay
/// the same: transcripts are large (hundreds of MB in a busy repo).
#[derive(Default)]
pub struct Cache {
    entries: HashMap<String, Entry>,
    dirty: bool,
    codex: Option<Vec<Session>>,
    #[cfg(test)]
    bytes_read: usize,
}

impl Cache {
    fn path() -> PathBuf {
        crate::config::cache_dir().join("sessions-v4.json")
    }

    pub fn load() -> Cache {
        let entries = std::fs::read(Cache::path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Cache {
            entries,
            dirty: false,
            codex: None,
            #[cfg(test)]
            bytes_read: 0,
        }
    }

    pub fn save(&mut self) {
        self.prune();
        if !self.dirty {
            return;
        }
        let path = Cache::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec(&self.entries) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(tmp, path);
            }
        }
    }

    /// Forgets transcripts that are gone: Claude Code deletes old ones.
    fn prune(&mut self) {
        let before = self.entries.len();
        self.entries.retain(|file, _| Path::new(file).exists());
        self.dirty |= self.entries.len() != before;
    }

    fn session(&mut self, file: &Path, kind: AgentKind) -> Option<Session> {
        let mut input = std::fs::File::open(file).ok()?;
        let meta = input.metadata().ok()?;
        let len = meta.len();
        let mtime = meta
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos() as u64;
        let file_id = identity(&meta);
        let key = file.to_string_lossy().into_owned();
        if let Some(entry) = self.entries.get(&key)
            && entry.identity == file_id
            && entry.len == len
            && entry.mtime == mtime
        {
            #[cfg(test)]
            {
                self.bytes_read = 0;
            }
            return entry.aggregate.session(kind, mtime / 1_000_000_000);
        }
        let previous = self.entries.remove(&key).filter(|entry| {
            entry.identity == file_id
                && len > entry.len
                && entry.offset <= len
                && fingerprint(&mut input, entry.offset).ok() == Some(entry.fingerprint)
        });
        let (offset, mut aggregate) = previous.map_or_else(
            || (0, Aggregate::default()),
            |entry| (entry.offset, entry.aggregate),
        );
        if matches!(kind, AgentKind::Claude) {
            aggregate.id = Some(file.file_stem()?.to_string_lossy().into_owned());
        }
        let (offset, read_bytes) = read_records(&mut input, offset, &mut aggregate, kind).ok()?;
        #[cfg(test)]
        {
            self.bytes_read = read_bytes;
        }
        #[cfg(not(test))]
        let _ = read_bytes;
        let hash = fingerprint(&mut input, offset).ok()?;
        let session = aggregate.session(kind, mtime / 1_000_000_000);
        self.entries.insert(
            key,
            Entry {
                len,
                mtime,
                identity: file_id,
                offset,
                fingerprint: hash,
                aggregate,
            },
        );
        self.dirty = true;
        Some(session?).filter(|_| {
            input
                .metadata()
                .is_ok_and(|end| identity(&end) == file_id && end.len() >= len)
        })
    }
}

/// Claude sessions in its launch directory, plus Codex sessions that worked in
/// this directory or below it. Codex's recursive store is scanned once per build.
pub fn sessions_in(cwd: &Path, cache: &mut Cache) -> Vec<Session> {
    let mut found = Vec::new();
    if let Some(dir) = projects_dir().map(|p| p.join(slug(cwd)))
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        found.extend(
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
                .filter_map(|p| cache.session(&p, AgentKind::Claude)),
        );
    }
    if cache.codex.is_none() {
        let base = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|p| p.join(".codex")));
        let mut codex = Vec::new();
        if let Some(base) = base {
            for file in rollout_files(&base.join("sessions")) {
                if let Some(session) = cache.session(&file, AgentKind::Codex) {
                    codex.push(session);
                }
            }
        }
        cache.codex = Some(codex);
    }
    found.extend(
        cache
            .codex
            .as_ref()
            .into_iter()
            .flatten()
            .filter(|session| {
                session
                    .cwds
                    .iter()
                    .any(|(path, _)| Path::new(path).starts_with(cwd))
            })
            .cloned(),
    );
    found
}

fn rollout_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Do not follow symlink directories into unrelated stores.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            found.extend(rollout_files(&path));
        } else if kind.is_file()
            && path.extension().is_some_and(|e| e == "jsonl")
            && entry.file_name().to_string_lossy().starts_with("rollout-")
        {
            found.push(path);
        }
    }
    found.sort();
    found
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct Aggregate {
    id: Option<String>,
    date: Option<String>,
    counts: HashMap<String, usize>,
    first_cwd: Option<String>,
    last_cwd: Option<String>,
    excluded: bool,
}

impl Aggregate {
    fn line(&mut self, bytes: &[u8], kind: AgentKind) {
        if self.excluded {
            return;
        }
        #[derive(Deserialize)]
        struct Fields {
            cwd: Option<String>,
            timestamp: Option<String>,
            id: Option<String>,
            session_id: Option<String>,
            source: Option<serde_json::Value>,
        }
        #[derive(Deserialize)]
        struct Record<'a> {
            #[serde(rename = "type")]
            kind: Option<&'a str>,
            timestamp: Option<String>,
            #[serde(borrow)]
            payload: Option<&'a serde_json::value::RawValue>,
        }
        let fields = match kind {
            AgentKind::Claude => serde_json::from_slice::<Fields>(bytes).ok(),
            AgentKind::Codex => {
                let Ok(record) = serde_json::from_slice::<Record<'_>>(bytes) else {
                    return;
                };
                if !matches!(record.kind, Some("session_meta" | "turn_context")) {
                    return;
                }
                let Some(payload) = record.payload else {
                    return;
                };
                let Ok(mut fields) = serde_json::from_str::<Fields>(payload.get()) else {
                    return;
                };
                if record.kind == Some("session_meta") {
                    if fields
                        .source
                        .as_ref()
                        .is_some_and(|s| s.get("subagent").is_some())
                    {
                        self.excluded = true;
                        return;
                    }
                    self.id = fields.id.take().or_else(|| fields.session_id.take());
                    fields.timestamp = fields.timestamp.or(record.timestamp);
                }
                Some(fields)
            }
        };
        let Some(fields) = fields else { return };
        if self.date.is_none() {
            self.date = fields
                .timestamp
                .as_deref()
                .and_then(|t| t.get(..10))
                .map(str::to_string);
        }
        if let Some(cwd) = fields.cwd {
            *self.counts.entry(cwd.clone()).or_default() += 1;
            self.first_cwd.get_or_insert_with(|| cwd.clone());
            self.last_cwd = Some(cwd);
        }
    }

    fn session(&self, kind: AgentKind, modified: u64) -> Option<Session> {
        if self.excluded {
            return None;
        }
        let mut cwds: Vec<_> = self.counts.iter().map(|(p, n)| (p.clone(), *n)).collect();
        cwds.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        Some(Session {
            id: kind.session_key(self.id.as_deref()?),
            date: self.date.clone()?,
            cwds,
            first_cwd: self.first_cwd.clone(),
            last_cwd: self.last_cwd.clone(),
            modified,
        })
    }
}

fn identity(meta: &std::fs::Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (meta.dev(), meta.ino())
    }
    #[cfg(not(unix))]
    {
        (
            0,
            meta.created()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos() as u64),
        )
    }
}

/// Fingerprints the beginning and the previous complete-line boundary, without
/// reading the historical transcript again on an append.
fn fingerprint(file: &mut std::fs::File, offset: u64) -> std::io::Result<u64> {
    use std::io::{Read, Seek, SeekFrom};
    let mut hash = 0xcbf29ce484222325u64;
    for start in [0, offset.saturating_sub(4096)] {
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = vec![0; (offset - start).min(4096) as usize];
        file.read_exact(&mut bytes)?;
        for byte in bytes {
            hash = (hash ^ byte as u64).wrapping_mul(0x100000001b3);
        }
    }
    Ok(hash)
}

fn read_records(
    file: &mut std::fs::File,
    offset: u64,
    aggregate: &mut Aggregate,
    kind: AgentKind,
) -> std::io::Result<(u64, usize)> {
    use std::io::{BufRead, Seek, SeekFrom};
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = std::io::BufReader::new(file);
    let mut bytes = Vec::new();
    let mut complete = offset;
    let mut read = 0;
    loop {
        bytes.clear();
        let n = reader.read_until(b'\n', &mut bytes)?;
        read += n;
        if n == 0 || bytes.last() != Some(&b'\n') {
            break;
        }
        aggregate.line(&bytes, kind);
        complete += n as u64;
    }
    Ok((complete, read))
}

#[cfg(test)]
fn parse_codex(path: &Path) -> Option<Session> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut aggregate = Aggregate::default();
    read_records(&mut file, 0, &mut aggregate, AgentKind::Codex).ok()?;
    aggregate.session(AgentKind::Codex, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_matches_claude_project_dirs() {
        assert_eq!(
            slug(Path::new(
                "/home/dev/projects/demo/.claude/worktrees/a2i-config"
            )),
            "-home-dev-projects-demo--claude-worktrees-a2i-config"
        );
    }

    #[test]
    fn worktrees_from_cwds() {
        let session = Session {
            id: "x".into(),
            date: "2026-10-08".into(),
            cwds: vec![
                ("/r/.claude/worktrees/a2i/libraries/config".into(), 30),
                ("/r".into(), 20),
                ("/r/.claude/worktrees/b".into(), 5),
                ("/r/.claude/worktrees/a2i".into(), 3),
            ],
            first_cwd: None,
            last_cwd: None,
            modified: 0,
        };
        assert_eq!(
            session.worktrees_under(Path::new("/r/.claude/worktrees")),
            vec!["a2i", "b"]
        );
    }

    #[test]
    fn forgets_deleted_transcripts() {
        let dir = crate::table::fixtures::temp_dir();
        let kept = dir.join("kept.jsonl");
        std::fs::write(
            &kept,
            "{\"cwd\":\"/a\",\"timestamp\":\"2026-10-08T10:00:00Z\"}\n",
        )
        .unwrap();
        let mut cache = Cache::default();
        assert!(
            cache
                .session(&kept, AgentKind::Claude)
                .is_some_and(|s| s.modified > 0)
        );
        let gone = dir.join("gone.jsonl");
        std::fs::write(
            &gone,
            "{\"cwd\":\"/b\",\"timestamp\":\"2026-10-09T10:00:00Z\"}\n",
        )
        .unwrap();
        cache.session(&gone, AgentKind::Claude).unwrap();
        std::fs::remove_file(&gone).unwrap();
        cache.dirty = false;

        cache.prune();
        let files: Vec<&String> = cache.entries.keys().collect();
        assert_eq!(files, vec![&kept.to_string_lossy().to_string()]);
        assert!(cache.dirty);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn codex_metadata_and_turn_contexts_are_parsed_without_tool_output() {
        let dir = crate::table::fixtures::temp_dir();
        let file = dir.join("rollout-fixture.jsonl");
        let records = [
            serde_json::json!({"type": "session_meta", "timestamp": "2026-10-29T10:00:00Z",
                "payload": {"id": "same-id", "cwd": "/main", "source": "cli"}}),
            serde_json::json!({"type": "turn_context", "payload": {"cwd": "/main/.claude/worktrees/task with space"}}),
            serde_json::json!({"type": "response_item", "payload": {"cwd": "/wrong/tool-output"}}),
        ];
        let text = records
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&file, format!("{text}\n{{trailing-partial-record")).unwrap();
        let mut cache = Cache::default();
        let session = cache.session(&file, AgentKind::Codex).unwrap();
        assert_eq!(session.id, "codex:same-id");
        assert_eq!(session.date, "2026-10-29");
        assert_eq!(session.first_cwd.as_deref(), Some("/main"));
        assert_eq!(
            session.last_cwd.as_deref(),
            Some("/main/.claude/worktrees/task with space")
        );
        assert_eq!(session.cwds.len(), 2);
        assert_eq!(
            session.worktrees_under(Path::new("/main/.claude/worktrees")),
            ["task with space"]
        );
        assert!(session.modified > 0);
        // A parsed session reused from the cache keeps its backend-qualified id.
        assert_eq!(
            cache.session(&file, AgentKind::Codex).unwrap().id,
            session.id
        );
        let guardian = serde_json::json!({"type": "session_meta", "payload": {
            "id": "child", "timestamp": "2026-10-29T10:00:00Z", "cwd": "/main",
            "source": {"subagent": {"other": "guardian"}}}});
        std::fs::write(&file, guardian.to_string()).unwrap();
        assert!(parse_codex(&file).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reads_spaced_and_escaped_json_fields_without_message_contents() {
        let mut aggregate = Aggregate {
            id: Some("demo".into()),
            ..Default::default()
        };
        aggregate.line(br#"{"cwd": "/home/dev/two words", "timestamp": "2026-10-08T10:00:00Z", "message": "cwd fake"}"#, AgentKind::Claude);
        aggregate.line(br#"{"cwd":"/home/dev/quote\"dir"}"#, AgentKind::Claude);
        let session = aggregate.session(AgentKind::Claude, 0).unwrap();
        assert_eq!(session.cwds.len(), 2);
        assert_eq!(session.last_cwd.as_deref(), Some("/home/dev/quote\"dir"));
    }
}

#[cfg(test)]
mod incremental_tests {
    use super::*;
    use std::io::Write;

    fn records(kind: AgentKind, cwd: &str) -> String {
        let value = match kind {
            AgentKind::Claude => serde_json::json!({"cwd":cwd,"timestamp":"2026-11-01T00:00:00Z"}),
            AgentKind::Codex => {
                serde_json::json!({"type":"session_meta","timestamp":"2026-11-01T00:00:00Z","payload":{"id":"demo","cwd":cwd,"source":"cli"}})
            }
        };
        format!("{value}\n")
    }

    #[test]
    fn appends_resume_persisted_aggregates_and_wait_for_complete_lines() {
        for kind in [AgentKind::Claude, AgentKind::Codex] {
            let dir = crate::table::fixtures::temp_dir();
            let path = dir.join("demo.jsonl");
            let row = records(kind, "/home/dev/old");
            std::fs::write(&path, row.repeat(20_000)).unwrap();
            let mut cache = Cache::default();
            let original = cache.session(&path, kind).unwrap();
            assert_eq!(original.cwds[0].1, 20_000);
            cache.session(&path, kind).unwrap();
            assert_eq!(cache.bytes_read, 0);
            let saved = serde_json::to_vec(&cache.entries).unwrap();
            let mut cache = Cache {
                entries: serde_json::from_slice(&saved).unwrap(),
                ..Default::default()
            };
            let next = records(kind, "/home/dev/new");
            let mut output = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            output.write_all(next.trim_end().as_bytes()).unwrap();
            assert_eq!(cache.session(&path, kind).unwrap().cwds[0].1, 20_000);
            output.write_all(b"\n").unwrap();
            let appended = cache.session(&path, kind).unwrap();
            assert_eq!(appended.first_cwd, original.first_cwd);
            assert_eq!(appended.last_cwd.as_deref(), Some("/home/dev/new"));
            assert_eq!(appended.cwds.iter().map(|(_, n)| n).sum::<usize>(), 20_001);
            assert!(
                cache.bytes_read < 1024,
                "append reparsed {} bytes",
                cache.bytes_read
            );
        }
    }

    #[test]
    fn truncation_replacement_and_changed_prefix_reset_the_aggregate() {
        for kind in [AgentKind::Claude, AgentKind::Codex] {
            let dir = crate::table::fixtures::temp_dir();
            let path = dir.join("demo.jsonl");
            let mut cache = Cache::default();
            std::fs::write(&path, records(kind, "/home/dev/old").repeat(4)).unwrap();
            cache.session(&path, kind).unwrap();
            std::fs::write(&path, records(kind, "/home/dev/truncated")).unwrap();
            assert_eq!(
                cache.session(&path, kind).unwrap().first_cwd.as_deref(),
                Some("/home/dev/truncated")
            );
            let replacement = dir.join("replacement");
            std::fs::write(&replacement, records(kind, "/home/dev/replaced").repeat(8)).unwrap();
            std::fs::rename(replacement, &path).unwrap();
            assert_eq!(
                cache.session(&path, kind).unwrap().cwds,
                vec![("/home/dev/replaced".into(), 8)]
            );
            std::fs::write(&path, records(kind, "/home/dev/edited").repeat(16)).unwrap();
            assert_eq!(
                cache.session(&path, kind).unwrap().cwds,
                vec![("/home/dev/edited".into(), 16)]
            );
        }
    }
}
