//! A local log to find out afterwards what went wrong. Commands that change
//! something (done, select, start, …) are always written; with debug on
//! (`BERBR_DEBUG=1`, or `debug = true` in the config) also read-only commands,
//! every git and herdr call, every HTTP request and the trace spans.
//!
//! `~/.berbr/logs/berbr.log`; `BERBR_LOG_DIR` overrides. Kept to three files of about 1 MiB. Never holds
//! tokens or response bodies: commands, paths, statuses and timings only.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

const MAX_BYTES: u64 = 1024 * 1024;
const KEEP: usize = 3;

static DEBUG: AtomicBool = AtomicBool::new(false);
static ENV_DEBUG: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| flag(std::env::var_os("BERBR_DEBUG")));
/// One writer at a time within the process; lines are whole appends across them.
static WRITE: Mutex<()> = Mutex::new(());
/// The command being run, once `begin` named it.
static RUN: Mutex<Option<Run>> = Mutex::new(None);

struct Run {
    args: String,
    cwd: String,
    start: Instant,
    logged: bool,
}

fn flag(value: Option<std::ffi::OsString>) -> bool {
    value.is_some_and(|v| !v.is_empty() && v != "0")
}

/// Turns debug on for the rest of the process; the environment always can.
pub fn set_debug(on: bool) {
    if on {
        DEBUG.store(true, Ordering::Relaxed);
    }
}

pub fn debug_enabled() -> bool {
    *ENV_DEBUG || DEBUG.load(Ordering::Relaxed)
}

pub fn dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("BERBR_LOG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    // unit tests remove real worktrees; their lines stay out of the person's log
    if cfg!(test) {
        return std::env::temp_dir().join("berbr-test-logs");
    }
    crate::config::berbr_home().join("logs")
}

pub fn path() -> PathBuf {
    dir().join("berbr.log")
}

/// Appends one timestamped line; a log that cannot be written is no error.
pub fn write(text: &str) {
    let _lock = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let _ = append(text);
}

fn append(text: &str) -> std::io::Result<()> {
    let dir = dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("berbr.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() >= MAX_BYTES) {
        rotate(&path);
    }
    let line = format!(
        "{} [{}] {}\n",
        chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z"),
        std::process::id(),
        text.replace('\n', "\n    ")
    );
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?
        .write_all(line.as_bytes())
}

/// berbr.log → .1 → .2; the oldest goes.
fn rotate(path: &std::path::Path) {
    let numbered = |n: usize| PathBuf::from(format!("{}.{n}", path.display()));
    for n in (1..KEEP).rev() {
        let from = if n == 1 {
            path.to_path_buf()
        } else {
            numbered(n - 1)
        };
        let _ = std::fs::rename(from, numbered(n));
    }
}

/// Writes the line only with debug on; `text` is built only then.
pub fn debug(text: impl FnOnce() -> String) {
    if debug_enabled() {
        write(&text());
    }
}

/// Names the command now running. `change`: it may change something, so it is
/// logged always, and its start too — a run that hangs or is killed (Herdr.app
/// stops mod commands after 30 s) leaves at least that.
pub fn begin(change: bool) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let run = Run {
        args: format!("{args:?}"),
        cwd: std::env::current_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|e| format!("({e})")),
        start: Instant::now(),
        logged: change || debug_enabled(),
    };
    if run.logged {
        write(&format!("start {} in {}", run.args, run.cwd));
    }
    *RUN.lock().unwrap_or_else(|e| e.into_inner()) = Some(run);
}

/// How the command named by `begin` ended.
pub fn finish(result: &Result<(), String>) {
    let Some(run) = RUN.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    if !run.logged && !debug_enabled() {
        return;
    }
    let ms = run.start.elapsed().as_millis();
    match result {
        Ok(()) => write(&format!("ok {ms}ms {}", run.args)),
        Err(e) => write(&format!("failed {ms}ms {} in {}: {e}", run.args, run.cwd)),
    }
}

/// Panics are written before the default hook prints them.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let args: Vec<String> = std::env::args().skip(1).collect();
        write(&format!("panic in {args:?}: {info}"));
        default(info);
    }));
}

/// An external command, timed; logged with debug on.
pub fn command(
    program: &str,
    cmd: &mut std::process::Command,
) -> std::io::Result<std::process::Output> {
    if !debug_enabled() {
        return cmd.output();
    }
    let start = Instant::now();
    let output = cmd.output();
    let ms = start.elapsed().as_millis();
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    match &output {
        Ok(out) if out.status.success() => write(&format!("{program} {args:?} ok {ms}ms")),
        Ok(out) => write(&format!(
            "{program} {args:?} {} {ms}ms: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(e) => write(&format!("{program} {args:?} could not run {ms}ms: {e}")),
    }
    output
}

/// The last `lines` lines of `path`, or why there are none.
pub fn tail(path: &std::path::Path, lines: usize) -> String {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let all: Vec<&str> = text.lines().collect();
            all[all.len().saturating_sub(lines)..].join("\n")
        }
        Err(e) => format!("({e})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_keeps_three_files_and_the_newest_lines() {
        let dir = crate::table::fixtures::temp_dir();
        let path = dir.join("berbr.log");
        std::fs::write(&path, "first\n").unwrap();
        rotate(&path);
        std::fs::write(&path, "second\n").unwrap();
        rotate(&path);
        std::fs::write(&path, "third\n").unwrap();
        rotate(&path);
        std::fs::write(&path, "fourth\n").unwrap();
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();
        assert_eq!(read("berbr.log"), "fourth\n");
        assert_eq!(read("berbr.log.1"), "third\n");
        assert_eq!(read("berbr.log.2"), "second\n");
        assert!(!dir.join("berbr.log.3").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tail_gives_the_last_lines_or_the_reason() {
        let dir = crate::table::fixtures::temp_dir();
        let path = dir.join("berbr.log");
        std::fs::write(&path, "a\nb\nc\n").unwrap();
        assert_eq!(tail(&path, 2), "b\nc");
        assert_eq!(tail(&path, 10), "a\nb\nc");
        assert!(tail(&dir.join("missing"), 2).starts_with('('));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn debug_flag_values() {
        assert!(!flag(None));
        assert!(!flag(Some("".into())));
        assert!(!flag(Some("0".into())));
        assert!(flag(Some("1".into())));
    }
}
