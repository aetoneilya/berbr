//! Files shared by all agents of a task, outside its disposable worktrees.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

// Legacy task IDs can be arbitrary note names. Encode separators and '%' so
// they remain distinct single path components, including '.' and '..'.
fn component(value: &str) -> String {
    if value.is_empty() {
        return "%".into();
    }
    let encoded: String = value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric()
                || b == b'-'
                || b == b'_'
                || (b == b'.' && value != "." && value != "..")
            {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    // Legacy identities can expand beyond NAME_MAX. '~' is always encoded in
    // short names, so the bounded form cannot alias a literal short identity.
    if encoded.len() > 200 {
        format!("{}~{:x}", &encoded[..96], Sha256::digest(value.as_bytes()))
    } else {
        encoded
    }
}

pub fn directory(project: &str, task: &str) -> PathBuf {
    crate::config::project_home(&component(project))
        .join("artifacts")
        .join(component(task))
}

pub fn ensure(project: &str, task: &str) -> Result<PathBuf, String> {
    // BERBR_HOME itself may be an intentional symlink. All managed namespace
    // components below that anchor must be real directories, before grants.
    let home = crate::config::berbr_home();
    std::fs::create_dir_all(&home).map_err(|e| format!("{}: {e}", home.display()))?;
    let mut path = home.canonicalize().map_err(|e| e.to_string())?;
    for part in [
        "projects".to_string(),
        component(project),
        "artifacts".into(),
        component(task),
    ] {
        path.push(part);
        match std::fs::create_dir(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
        let metadata = path.symlink_metadata().map_err(|e| e.to_string())?;
        if !metadata.is_dir() || metadata.is_symlink() {
            return Err(format!(
                "{}: artifact namespace must be a directory, not a symlink or file",
                path.display()
            ));
        }
    }
    Ok(path)
}

#[derive(Serialize)]
pub struct Entry {
    pub name: String,
    /// Relative to the task's artifact folder; passed back for lazy expansion.
    pub relative: PathBuf,
    pub path: PathBuf,
    pub kind: &'static str,
    pub executable: bool,
}

/// List one level only. Do not traverse symlinks or recurse into large trees.
pub fn list(root: &Path, relative: &Path) -> Result<Vec<Entry>, String> {
    if relative.is_absolute()
        || relative.components().any(|c| {
            !matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return Err("artifact directory must be relative, without '..'".into());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let folder = root
        .join(relative)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !folder.starts_with(&root) {
        return Err("artifact directory is outside this task's folder".into());
    }
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&folder).map_err(|e| format!("{}: {e}", folder.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let metadata = entry.path().symlink_metadata().map_err(|e| e.to_string())?;
        let kind = if metadata.is_symlink() {
            "symlink"
        } else if metadata.is_dir() {
            "directory"
        } else if metadata.is_file() {
            "file"
        } else {
            "other"
        };
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let executable = false;
        entries.push(Entry {
            name: entry.file_name().to_string_lossy().into_owned(),
            relative: relative.join(entry.file_name()),
            path: entry.path(),
            kind,
            executable,
        });
    }
    entries.sort_by(|a, b| {
        (a.kind != "directory")
            .cmp(&(b.kind != "directory"))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_names_cannot_escape_or_alias_another_task() {
        for name in ["../other", "a/b", "a\\b", ".", "..", "", "задача"] {
            let encoded = component(name);
            assert_eq!(Path::new(&encoded).components().count(), 1);
            assert_ne!(encoded, component(&encoded));
        }
        assert_eq!(component("berbr-7"), "berbr-7");
        assert_ne!(component("a/b"), component("a%2Fb"));
    }

    #[test]
    fn lists_any_files_and_lazy_directories_without_following_links() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("data")).unwrap();
        for name in [
            "plan.md",
            "page.html",
            "table.csv",
            "script.sh",
            "binary",
            ".hidden",
        ] {
            std::fs::write(root.path().join(name), "content").unwrap();
        }
        std::fs::write(root.path().join("data/nested.csv"), "a,b").unwrap();
        let entries = list(root.path(), Path::new("")).unwrap();
        assert_eq!(entries.len(), 7);
        assert_eq!(entries[0].name, "data");
        assert_eq!(entries[0].kind, "directory");
        let nested = list(root.path(), Path::new("data")).unwrap();
        assert_eq!(nested[0].relative, Path::new("data/nested.csv"));
        assert!(list(root.path(), Path::new("../")).is_err());
        assert!(list(root.path(), root.path()).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::{fs::PermissionsExt, fs::symlink};
            let outside = tempfile::tempdir().unwrap();
            symlink(outside.path(), root.path().join("link")).unwrap();
            symlink(root.path(), root.path().join("loop")).unwrap();
            assert!(list(root.path(), Path::new("link")).is_err());
            let entries = list(root.path(), Path::new("")).unwrap();
            assert_eq!(
                entries.iter().find(|e| e.name == "loop").unwrap().kind,
                "symlink"
            );
            let script = root.path().join("script.sh");
            std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(
                list(root.path(), Path::new(""))
                    .unwrap()
                    .iter()
                    .find(|e| e.name == "script.sh")
                    .unwrap()
                    .executable
            );
        }
    }

    #[test]
    fn long_legacy_identities_are_bounded_and_distinct() {
        let a = format!("{}a", "я".repeat(120));
        let b = format!("{}b", "я".repeat(120));
        assert!(component(&a).len() <= 200);
        assert!(component(&b).len() <= 200);
        assert_ne!(component(&a), component(&b));
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(component(&a))).unwrap();
        std::fs::create_dir(root.path().join(component(&b))).unwrap();
        assert_ne!(component(&a), component(&component(&a)));
    }
}
