use serde_yaml::{Mapping, Value};
use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NoteError {
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),

    #[error("{0}: invalid frontmatter: {1}")]
    Yaml(PathBuf, serde_yaml::Error),

    #[error("{0}: frontmatter update could not preserve its values, skipped")]
    UnsafeUpdate(PathBuf),

    #[error("{0}: changed while being updated, skipped")]
    Changed(PathBuf),

    #[error("{0}: snapshot preview cannot be saved; reload the full note first")]
    Preview(PathBuf),
}

pub(crate) type Result<T> = std::result::Result<T, NoteError>;

/// A markdown note with optional YAML frontmatter.
///
/// Keys are read through a YAML parser, but written line by line: `set` replaces
/// only the lines of the keys it touches, so the rest of the file stays exactly as
/// Obsidian or the user left it.
#[derive(Debug)]
pub struct Note {
    pub path: PathBuf,
    text: String,
    modified: SystemTime,
    frontmatter: Mapping,
    /// Snapshot-only notes are read-only, even when their body fits in full.
    preview_truncated: Option<bool>,
}

pub const BODY_PREVIEW_BYTES: usize = 2048;

impl Note {
    pub fn read(path: &Path) -> Result<Note> {
        let io_err = |err| NoteError::Io(path.to_path_buf(), err);
        let text = fs::read_to_string(path).map_err(io_err)?;
        let modified = fs::metadata(path)
            .and_then(|m| m.modified())
            .map_err(io_err)?;
        let frontmatter = parse_frontmatter(&text).map_err(|e| NoteError::Yaml(path.into(), e))?;
        Ok(Note {
            path: path.to_path_buf(),
            text,
            modified,
            frontmatter,
            preview_truncated: None,
        })
    }

    /// Read all metadata, but only a bounded prefix of the body. Used solely by
    /// app snapshots; commands that edit notes and the TUI still use `read`.
    pub fn read_preview(path: &Path) -> Result<Note> {
        let io_err = |err| NoteError::Io(path.to_path_buf(), err);
        let file = fs::File::open(path).map_err(io_err)?;
        let modified = file.metadata().and_then(|m| m.modified()).map_err(io_err)?;
        let (text, has_more) = preview_text(BufReader::new(file)).map_err(io_err)?;
        let frontmatter = parse_frontmatter(&text).map_err(|e| NoteError::Yaml(path.into(), e))?;
        let mut note = Note {
            path: path.to_path_buf(),
            text,
            modified,
            frontmatter,
            preview_truncated: None,
        };
        let body_start = note.text.len() - note.body().len();
        let (preview, truncated) = body_preview(note.body());
        let end = body_start + preview.len();
        note.text.truncate(end);
        note.text.shrink_to_fit();
        note.preview_truncated = Some(truncated || has_more);
        Ok(note)
    }

    /// The text after the frontmatter.
    pub fn body(&self) -> &str {
        match frontmatter_range(&self.text) {
            Some((_, end)) => {
                let rest = &self.text[end..];
                rest.split_once('\n').map_or("", |(_, body)| body)
            }
            None => &self.text,
        }
    }

    /// Human text before machine sections, at most 2 KiB of UTF-8. The flag
    /// covers both the byte cap and omitted machine history.
    pub fn body_preview(&self) -> (&str, bool) {
        let (body, truncated) = body_preview(self.body());
        (body, truncated || self.preview_truncated.unwrap_or(false))
    }

    /// The task's own text, before its append-only agent findings.
    pub fn description(&self) -> &str {
        let body = self.body();
        let end = agent_notes_start(body).unwrap_or(body.len());
        body[..end].trim()
    }

    /// Replace the task's description, keeping frontmatter and agent findings.
    pub fn set_description(&mut self, description: &str) {
        let body = self.body();
        let start = self.text.len() - body.len();
        let notes = agent_notes_start(body).map(|i| body[i..].to_string());
        self.text.truncate(start);
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
        self.text.push_str(description.trim());
        self.text.push('\n');
        if let Some(notes) = notes {
            self.text.push('\n');
            self.text.push_str(&notes);
        }
    }

    /// File name without `.md`.
    pub fn name(&self) -> String {
        self.path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    pub fn string(&self, key: &str) -> Option<String> {
        self.frontmatter.get(key).and_then(scalar_string)
    }

    /// A list value, or a single scalar as a one-element list.
    pub fn strings(&self, key: &str) -> Vec<String> {
        match self.frontmatter.get(key) {
            Some(Value::Sequence(items)) => items.iter().filter_map(scalar_string).collect(),
            Some(value) => scalar_string(value).into_iter().collect(),
            None => Vec::new(),
        }
    }

    pub fn flag(&self, key: &str) -> bool {
        match self.frontmatter.get(key) {
            Some(Value::Bool(b)) => *b,
            Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
            _ => false,
        }
    }

    /// Writes a new note with `keys` as frontmatter; refuses to overwrite.
    pub fn create(path: &Path, keys: &[(&str, Value)], body: &str) -> Result<Note> {
        let io_err = |err| NoteError::Io(path.to_path_buf(), err);
        let mut text = format!("---\n---\n{body}");
        for (key, value) in keys {
            text = set_key(&text, key, value);
        }
        parse_frontmatter(&text).map_err(|e| NoteError::Yaml(path.into(), e))?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(io_err)?;
        }
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, text.as_bytes()))
            .map_err(io_err)?;
        Note::read(path)
    }

    /// Appends `line` as the last item under `## Agent notes`, adding the heading
    /// at the end of the note on first use.
    pub fn append_agent_note(&mut self, line: &str) {
        self.text = append_under_heading(&self.text, AGENT_NOTES, line);
    }

    /// A `- line` item at the end of the note's own text: before the agents'
    /// section when there is one.
    pub fn append_line(&mut self, line: &str) {
        let body_start = frontmatter_range(&self.text)
            .and_then(|(_, end)| self.text[end..].find('\n').map(|n| end + n + 1))
            .unwrap_or(0);
        let cut = agent_notes_start(&self.text[body_start..])
            .map(|i| body_start + i)
            .unwrap_or(self.text.len());
        let (before, after) = self.text.split_at(cut);
        let mut text = before.trim_end_matches('\n').to_string();
        text.push('\n');
        text.push_str(&format!("- {line}\n"));
        if !after.is_empty() {
            text.push('\n');
            text.push_str(after);
        }
        self.text = text;
    }

    /// Re-reads the file, for a retry after `save` found it changed.
    pub fn reload(&mut self) -> Result<()> {
        *self = Note::read(&self.path)?;
        Ok(())
    }

    /// Promote a snapshot preview before a managed update, under the caller's
    /// vault lock. Body edits are preserved; changed metadata invalidates the
    /// facts derived from the preview, including identity and task ownership.
    pub fn reload_preview_for_update(&mut self) -> Result<()> {
        if self.preview_truncated.is_some() {
            let full = Note::read(&self.path)?;
            if full.frontmatter != self.frontmatter {
                return Err(NoteError::Changed(self.path.clone()));
            }
            *self = full;
        }
        Ok(())
    }

    /// Sets a top-level key in memory; `save` writes it.
    pub fn set(&mut self, key: &str, value: Value) {
        self.text = set_key(&self.text, key, &value);
        self.frontmatter
            .insert(Value::String(key.to_string()), value);
    }

    /// Writes the note if `set` changed it. Refuses when the file was modified
    /// since it was read; writes a temp file and renames it over the note.
    pub fn save(&mut self) -> Result<bool> {
        if self.preview_truncated.is_some() {
            return Err(NoteError::Preview(self.path.clone()));
        }
        let parsed =
            parse_frontmatter(&self.text).map_err(|e| NoteError::Yaml(self.path.clone(), e))?;
        if parsed != self.frontmatter {
            return Err(NoteError::UnsafeUpdate(self.path.clone()));
        }
        let io_err = |err| NoteError::Io(self.path.clone(), err);
        let on_disk = fs::read_to_string(&self.path).map_err(io_err)?;
        if on_disk == self.text {
            return Ok(false);
        }
        let modified = fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .map_err(io_err)?;
        if modified != self.modified {
            return Err(NoteError::Changed(self.path.clone()));
        }

        let file_name = self.path.file_name().unwrap_or_default().to_string_lossy();
        let tmp = self.path.with_file_name(format!(".{file_name}.berbr-tmp"));
        fs::write(&tmp, &self.text).map_err(io_err)?;
        fs::rename(&tmp, &self.path).map_err(io_err)?;
        self.modified = fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .map_err(io_err)?;
        Ok(true)
    }
}

const AGENT_NOTES: &str = "## Agent notes";

/// Frontmatter is needed in full for task/session/worktree identity. Stop body
/// IO after 2 KiB plus UTF-8 lookahead, even for a single enormous history line.
fn preview_text(mut reader: impl BufRead) -> io::Result<(String, bool)> {
    let mut opening = Vec::new();
    reader.by_ref().take(5).read_until(b'\n', &mut opening)?;
    let has_frontmatter = opening == b"---\n" || opening == b"---\r\n";
    let mut reader = io::Cursor::new(opening).chain(reader);
    let mut text = String::new();
    if has_frontmatter {
        reader.read_line(&mut text)?;
        loop {
            let start = text.len();
            if reader.read_line(&mut text)? == 0 || text[start..].trim_end() == "---" {
                break;
            }
        }
    }
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take((BODY_PREVIEW_BYTES + 4) as u64)
        .read_to_end(&mut bytes)?;
    let has_more = !reader.fill_buf()?.is_empty();
    if let Err(error) = std::str::from_utf8(&bytes) {
        if has_more && error.error_len().is_none() {
            bytes.truncate(error.valid_up_to());
        } else {
            return Err(io::Error::new(io::ErrorKind::InvalidData, error));
        }
    }
    text.push_str(std::str::from_utf8(&bytes).expect("validated UTF-8"));
    Ok((text, has_more))
}

fn body_preview(body: &str) -> (&str, bool) {
    let mut end = body.len().min(BODY_PREVIEW_BYTES);
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    let prefix = &body[..end];
    for (offset, level, title) in markdown_headings(prefix) {
        let machine = level >= 2
            && matches!(
                title.to_lowercase().as_str(),
                "agent notes" | "sessions" | "сессии"
            );
        // A heading cut mid-line may be the start of a machine section. Leave
        // it out rather than publishing a misleading fragment of its title.
        let partial = end < body.len() && !prefix[offset..].contains('\n');
        if machine || partial {
            end = offset;
            break;
        }
    }
    (body[..end].trim_end(), end < body.len())
}

#[cfg(test)]
mod preview_tests {
    use super::*;

    #[test]
    fn previews_stop_at_machine_headings_outside_fences() {
        for heading in ["## Agent notes", "   ### SESSIONS ###", "###### СЕССИИ"] {
            let body = format!("Human requirements.\r\n\r\n{heading}\r\nMachine history");
            assert_eq!(body_preview(&body), ("Human requirements.", true));
            assert_eq!(body_preview(&format!("{heading}\nHistory")), ("", true));
        }
        for fence in ["```", "~~~~"] {
            let human = format!(
                "Example:\n{fence}md\n## Agent notes\n### Sessions\n{fence}\nHuman criteria."
            );
            let body = format!("{human}\n\n## Agent notes\nMachine history");
            assert_eq!(body_preview(&body), (human.as_str(), true));
        }
        for body in [
            "## Sessions overview\nHuman criteria",
            "    ## Agent notes\nIndented code",
            "# Sessions\nHuman heading",
        ] {
            assert_eq!(body_preview(body), (body, false));
        }
    }

    #[test]
    fn preview_cap_is_utf8_safe_and_reports_omissions() {
        for body in [
            "a".repeat(BODY_PREVIEW_BYTES),
            "я".repeat(1500),
            "🙂".repeat(1000),
            format!("a{}", "🙂".repeat(1000)),
        ] {
            let mut reader = io::Cursor::new(body.as_bytes());
            let (prefix, more) = preview_text(&mut reader).unwrap();
            let (preview, truncated) = body_preview(&prefix);
            assert!(preview.len() <= BODY_PREVIEW_BYTES);
            assert!(body.starts_with(preview));
            assert_eq!(truncated || more, body.len() > BODY_PREVIEW_BYTES);
            assert!(reader.position() <= (BODY_PREVIEW_BYTES + 4) as u64);
        }
        let body = format!(
            "{}\n## Agent notes\nHistory",
            "x".repeat(BODY_PREVIEW_BYTES - 7)
        );
        let (preview, truncated) = body_preview(&body);
        assert_eq!(preview, "x".repeat(BODY_PREVIEW_BYTES - 7));
        assert!(truncated);
        assert!(preview_text(io::Cursor::new(b"invalid \xff UTF-8")).is_err());
    }

    #[test]
    fn snapshot_reader_bounds_body_io_preserves_metadata_and_refuses_saves() {
        let header = "---\r\nid: demo-1\r\ntitle: Demo\r\ntickets: [DEMO-1]\r\nsessions: ['codex:demo demo-tree 2026-01-01']\r\nworktrees: [/home/dev/demo]\r\n---\r\n";
        for body in [
            format!(
                "Requirements.\n\n## Agent notes\n{}",
                "x".repeat(256 * 1024)
            ),
            "x".repeat(256 * 1024),
        ] {
            let source = format!("{header}{body}");
            let mut reader = io::Cursor::new(source.as_bytes());
            let (prefix, more) = preview_text(&mut reader).unwrap();
            assert!(more);
            assert!(prefix.len() <= header.len() + BODY_PREVIEW_BYTES + 4);
            assert!(reader.position() <= (header.len() + BODY_PREVIEW_BYTES + 4) as u64);

            let dir = crate::table::fixtures::temp_dir();
            let path = dir.join("demo-1.md");
            fs::write(&path, &source).unwrap();
            let mut note = Note::read_preview(&path).unwrap();
            assert_eq!(note.string("id").as_deref(), Some("demo-1"));
            assert_eq!(
                note.strings("sessions"),
                ["codex:demo demo-tree 2026-01-01"]
            );
            assert_eq!(note.strings("worktrees"), ["/home/dev/demo"]);
            assert!(note.text.len() <= header.len() + BODY_PREVIEW_BYTES);
            assert!(note.body_preview().1);
            note.set("title", Value::String("Changed".into()));
            assert!(matches!(note.save(), Err(NoteError::Preview(_))));
            assert_eq!(fs::read_to_string(&path).unwrap(), source);
            note.reload().unwrap();
            assert_eq!(note.body(), body);
            assert!(note.preview_truncated.is_none());
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn short_preview_is_complete_but_still_read_only() {
        let dir = crate::table::fixtures::temp_dir();
        let path = dir.join("demo-2.md");
        fs::write(&path, "---\ntitle: Demo\n---\nShort requirements.\n").unwrap();
        let mut note = Note::read_preview(&path).unwrap();
        assert_eq!(note.body_preview(), ("Short requirements.", false));
        assert!(matches!(note.save(), Err(NoteError::Preview(_))));
        fs::remove_dir_all(dir).unwrap();
    }
}

/// ATX headings outside backtick/tilde fences (up to three leading spaces).
fn markdown_headings(body: &str) -> Vec<(usize, usize, &str)> {
    let mut headings = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        let text = line.trim_end_matches(['\n', '\r']);
        let trimmed = text.trim_start_matches(' ');
        let indent = text.len() - trimmed.len();
        if indent <= 3 {
            let marker = trimmed.chars().next().unwrap_or(' ');
            let count = trimmed.chars().take_while(|c| *c == marker).count();
            if let Some((open, n)) = fence {
                if marker == open && count >= n && trimmed[count..].trim().is_empty() {
                    fence = None;
                }
            } else if (marker == '`' || marker == '~') && count >= 3 {
                fence = Some((marker, count));
            } else if marker == '#' && count <= 6 && trimmed[count..].starts_with([' ', '\t']) {
                headings.push((
                    offset,
                    count,
                    trimmed[count..].trim().trim_end_matches('#').trim_end(),
                ));
            }
        }
        offset += line.len();
    }
    headings
}

fn agent_notes_start(body: &str) -> Option<usize> {
    markdown_headings(body)
        .into_iter()
        .find(|(_, level, title)| *level == 2 && *title == "Agent notes")
        .map(|(offset, _, _)| offset)
}

fn append_under_heading(text: &str, heading: &str, line: &str) -> String {
    let item = format!("- {line}\n");
    let Some(pos) = agent_notes_start(text) else {
        let sep = if text.is_empty() || text.ends_with("\n\n") {
            ""
        } else if text.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        return format!("{text}{sep}{heading}\n{item}");
    };
    // the section ends at the next heading of the same or a higher level
    let body_start = pos + heading.len();
    let section_end = markdown_headings(text)
        .into_iter()
        .find(|(offset, level, _)| *offset > body_start && *level <= 2)
        .map(|(offset, _, _)| offset)
        .unwrap_or(text.len());
    let mut section = text[..section_end].trim_end_matches('\n').to_string();
    section.push('\n');
    section.push_str(&item);
    let rest = &text[section_end..];
    if rest.is_empty() {
        section
    } else {
        format!("{section}\n{rest}")
    }
}

fn scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Byte range of the YAML between the opening `---` line and the closing one.
fn frontmatter_range(text: &str) -> Option<(usize, usize)> {
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;
    let start = text.len() - rest.len();
    let mut offset = start;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Some((start, offset));
        }
        offset += line.len();
    }
    None
}

fn parse_frontmatter(text: &str) -> std::result::Result<Mapping, serde_yaml::Error> {
    let Some((start, end)) = frontmatter_range(text) else {
        return Ok(Mapping::new());
    };
    match serde_yaml::from_str::<Value>(&text[start..end])? {
        Value::Mapping(mapping) => Ok(mapping),
        _ => Ok(Mapping::new()),
    }
}

/// Renders `key: value` the way Obsidian writes properties: lists as block lists
/// indented by two spaces, scalars quoted only when YAML needs it.
fn render_key(key: &str, value: &Value) -> String {
    let key = render_scalar(&Value::String(key.into()));
    match value {
        Value::Sequence(items) if items.is_empty() => format!("{key}: []\n"),
        Value::Sequence(items) => {
            let mut out = format!("{key}:\n");
            for item in items {
                out.push_str(&format!("  - {}\n", render_scalar(item)));
            }
            out
        }
        other => format!("{key}: {}\n", render_scalar(other)),
    }
}

fn render_scalar(value: &Value) -> String {
    serde_yaml::to_string(value)
        .unwrap_or_default()
        .trim_end()
        .to_string()
}

/// True for the line that starts top-level `key`.
fn starts_key(line: &str, key: &str) -> bool {
    top_level_key(line).as_deref() == Some(key)
}

/// Parse the key token with YAML itself, including quoted/escaped keys. A
/// top-level property ends the previous property's entire multiline value.
fn top_level_key(line: &str) -> Option<String> {
    if line.starts_with([' ', '\t', '#', '-']) {
        return None;
    }
    for (i, _) in line.match_indices(':') {
        if !line[i + 1..].is_empty() && !line[i + 1..].starts_with(char::is_whitespace) {
            continue;
        }
        let key = serde_yaml::from_str::<Value>(&line[..i]).ok();
        if let Some(Value::String(key)) = key {
            return Some(key);
        }
    }
    None
}

fn set_key(text: &str, key: &str, value: &Value) -> String {
    let rendered = render_key(key, value);
    let Some((start, end)) = frontmatter_range(text) else {
        return format!("---\n{rendered}---\n{text}");
    };

    let yaml = &text[start..end];
    let mut out = String::with_capacity(text.len() + rendered.len());
    out.push_str(&text[..start]);

    let mut lines = yaml.split_inclusive('\n').peekable();
    let mut replaced = false;
    while let Some(line) = lines.next() {
        if !replaced && starts_key(line, key) {
            let mut comments = String::new();
            while lines
                .peek()
                .is_some_and(|next| top_level_key(next).is_none())
            {
                let next = lines.next().unwrap();
                if next.trim().is_empty() || next.trim_start().starts_with('#') {
                    comments.push_str(next);
                }
            }
            out.push_str(&rendered);
            out.push_str(&comments);
            replaced = true;
        } else {
            out.push_str(line);
        }
    }
    if !replaced {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&rendered);
    }
    out.push_str(&text[end..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Value {
        Value::Sequence(items.iter().map(|s| Value::String(s.to_string())).collect())
    }

    #[test]
    fn replaces_block_list_and_keeps_other_lines() {
        let text = "---\ndone: false\ntags:\n  - a\n  - b\nproject: x # mine\n---\nbody\n";
        let out = set_key(text, "tags", &list(&["c"]));
        assert_eq!(
            out,
            "---\ndone: false\ntags:\n  - c\nproject: x # mine\n---\nbody\n"
        );
    }

    #[test]
    fn replaces_flow_list_and_column_zero_items() {
        let flow = "---\nsessions: [a, b]\ndone: false\n---\n";
        assert_eq!(
            set_key(flow, "sessions", &list(&["x"])),
            "---\nsessions:\n  - x\ndone: false\n---\n"
        );
        let col0 = "---\nsessions:\n- a\n- b\ndone: false\n---\n";
        assert_eq!(
            set_key(col0, "sessions", &list(&["x"])),
            "---\nsessions:\n  - x\ndone: false\n---\n"
        );
    }

    #[test]
    fn appends_missing_key_before_closing_fence() {
        let text = "---\ndone: false\n---\nbody";
        let out = set_key(text, "state", &Value::String("active".into()));
        assert_eq!(out, "---\ndone: false\nstate: active\n---\nbody");
    }

    #[test]
    fn creates_frontmatter_when_absent() {
        let out = set_key("# title\n", "tickets", &list(&["DEMO-1"]));
        assert_eq!(out, "---\ntickets:\n  - DEMO-1\n---\n# title\n");
        assert_eq!(parse_frontmatter(&out).unwrap().len(), 1);
    }

    #[test]
    fn does_not_match_key_prefixes() {
        let text = "---\nstate_old: x\n---\n";
        let out = set_key(text, "state", &Value::String("idea".into()));
        assert_eq!(out, "---\nstate_old: x\nstate: idea\n---\n");
    }

    #[test]
    fn quotes_only_when_needed() {
        assert_eq!(
            render_scalar(&Value::String("a9c0 wt 2026-10-08".into())),
            "a9c0 wt 2026-10-08"
        );
        assert_eq!(
            render_scalar(&Value::String("2026-10-08".into())),
            "2026-10-08"
        );
        assert_eq!(render_scalar(&Value::String("true".into())), "'true'");
    }

    #[test]
    fn agent_notes_section() {
        let first = append_under_heading("---\n---\nbody\n", AGENT_NOTES, "a");
        assert_eq!(first, "---\n---\nbody\n\n## Agent notes\n- a\n");
        let second = append_under_heading(&first, AGENT_NOTES, "b");
        assert_eq!(second, "---\n---\nbody\n\n## Agent notes\n- a\n- b\n");
        let later = format!("{second}\n## Mine\ntext\n");
        assert_eq!(
            append_under_heading(&later, AGENT_NOTES, "c"),
            "---\n---\nbody\n\n## Agent notes\n- a\n- b\n- c\n\n## Mine\ntext\n"
        );
        let nested = "## Agent notes\n- a\n### sub\nx\n";
        assert_eq!(
            append_under_heading(nested, AGENT_NOTES, "b"),
            "## Agent notes\n- a\n### sub\nx\n- b\n"
        );
    }

    #[test]
    fn empty_frontmatter() {
        let out = set_key("---\n---\nbody", "state", &Value::String("idea".into()));
        assert_eq!(out, "---\nstate: idea\n---\nbody");
    }

    #[test]
    fn appends_my_line_before_agent_notes() {
        let mut note = Note {
            path: PathBuf::from("x.md"),
            text: "---\ndone: false\n---\nGoal.\n\n## Agent notes\n- a\n".into(),
            modified: SystemTime::now(),
            frontmatter: Mapping::new(),
            preview_truncated: None,
        };
        note.append_line("2026-10-13: mine");
        assert_eq!(
            note.text,
            "---\ndone: false\n---\nGoal.\n- 2026-10-13: mine\n\n## Agent notes\n- a\n"
        );
        let mut bare = Note {
            text: "---\ndone: false\n---\n".into(),
            ..note
        };
        bare.append_line("x");
        assert_eq!(bare.text, "---\ndone: false\n---\n- x\n");
    }
}

#[cfg(test)]
mod regressions {
    use super::*;
    #[test]
    fn quoted_yaml_property_must_be_updated_without_duplicate() {
        let text = "---\n\"state\": active\nother: preserved\n---\nBody\n";
        let out = set_key(text, "state", &Value::String("review".into()));
        assert!(
            parse_frontmatter(&out).is_ok(),
            "valid note becomes unreadable: {out}"
        );
    }
    #[test]
    fn commented_multiline_list_must_remain_valid_yaml() {
        let text =
            "---\nsessions:\n  - first\n# user comment\n  - second\nother: preserved\n---\nBody\n";
        assert!(parse_frontmatter(text).is_ok());
        let out = set_key(
            text,
            "sessions",
            &Value::Sequence(vec![Value::String("new".into())]),
        );
        assert_eq!(
            parse_frontmatter(&out).unwrap()["sessions"]
                .as_sequence()
                .unwrap()
                .len(),
            1,
            "old list item survives replacement: {out}"
        );
    }
    #[test]
    fn description_must_preserve_heading_inside_code_fence() {
        let text = "---\n---\nExample:\n```md\n## Agent notes\nexample only\n```\n\nReal description end\n\n## Agent notes\n- actual finding\n";
        let mut note = Note {
            path: "dummy.md".into(),
            text: text.into(),
            modified: SystemTime::now(),
            frontmatter: Mapping::new(),
            preview_truncated: None,
        };
        note.set_description("New description");
        assert_eq!(
            note.body().trim(),
            "New description\n\n## Agent notes\n- actual finding"
        );
    }
}

#[cfg(test)]
mod preservation_tests {
    use super::*;

    #[test]
    fn quoted_keys_and_comments_inside_lists_keep_other_source_text() {
        for key in ["'sessions'", "\"sessions\""] {
            let text = format!(
                "---\n# before\n{key}:\n  - old\n\n# inside\n  - stale\n# after\nother: yes # unchanged\n---\nBody\n"
            );
            let value = Value::Sequence(vec![Value::String("new".into())]);
            let out = set_key(&text, "sessions", &value);
            let parsed = parse_frontmatter(&out).unwrap();
            assert_eq!(parsed["sessions"], value);
            assert!(out.contains("# before"));
            assert!(out.contains("# inside"));
            assert!(out.contains("# after\nother: yes # unchanged\n---\nBody\n"));
        }
    }

    #[test]
    fn unrepresentable_source_edit_is_rejected_before_writing() {
        let root = crate::table::fixtures::temp_dir();
        let path = root.join("demo.md");
        let text = "---\n{state: active, other: preserved}\n---\nBody\n";
        fs::write(&path, text).unwrap();
        let mut note = Note::read(&path).unwrap();
        note.set("state", Value::String("review".into()));
        assert!(note.save().is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn fenced_headings_never_become_agent_note_boundaries() {
        for fence in ["```", "~~~~"] {
            let body = format!(
                "Example:\n{fence}md\n## Agent notes\n## Another example\n{fence}\nStill description\n\n## Agent notes\n- real\n{fence}md\n## Sample\n{fence}\n\n## Personal\nKeep\n"
            );
            let out = append_under_heading(&body, AGENT_NOTES, "new");
            assert!(out.contains(&format!("{fence}\n- new\n\n## Personal\nKeep")));
            let note = Note {
                path: "demo.md".into(),
                text: body,
                modified: SystemTime::now(),
                frontmatter: Mapping::new(),
                preview_truncated: None,
            };
            assert!(note.description().ends_with("Still description"));
        }
    }
}
