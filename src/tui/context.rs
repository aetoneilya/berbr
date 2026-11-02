//! Select the task that owns the pane from which the board was opened.

use crate::table::{Agent, Table};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Default)]
pub(super) struct Opening {
    pub(super) dir: Option<PathBuf>,
    pane: Option<String>,
    tab: Option<String>,
    workspace: Option<String>,
    label: Option<String>,
}

impl Opening {
    pub(super) fn capture() -> Self {
        let context = std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok());
        // Plugin context describes the caller, whereas inherited pane IDs can
        // identify the popup itself. Do not combine the two sets of IDs.
        let mut opening = match context {
            Some(context) => Self::from_context(&context),
            None => Self {
                pane: std::env::var("HERDR_PANE_ID").ok(),
                tab: std::env::var("HERDR_TAB_ID").ok(),
                workspace: std::env::var("HERDR_WORKSPACE_ID").ok(),
                ..Self::default()
            },
        };
        opening.dir = opening.dir.or_else(|| std::env::current_dir().ok());
        opening
    }

    fn from_context(context: &Value) -> Self {
        let text = |key: &str| {
            context
                .pointer(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        Self {
            dir: [
                "/focused_pane_cwd",
                "/worktree/checkout_path",
                "/workspace_cwd",
            ]
            .into_iter()
            .find_map(text)
            .map(PathBuf::from),
            pane: text("/focused_pane_id"),
            tab: text("/tab_id"),
            workspace: text("/workspace_id"),
            label: text("/workspace_label"),
        }
    }

    /// Pane and tab identity win over a shell cwd that may lag behind the agent.
    /// Cwd resolves shell-only tabs too; a workspace covers work in the main repo.
    pub(super) fn task(&self, table: &Table) -> Option<usize> {
        let has_agent = |row: &crate::table::TaskRow, matches: &dyn Fn(&Agent) -> bool| {
            row.agents
                .iter()
                .chain(row.trees.iter().flat_map(|&t| &table.trees[t].agents))
                .any(matches)
        };
        for (id, pane) in [(&self.pane, true), (&self.tab, false)] {
            if let Some(id) = id {
                let candidates = table
                    .tasks
                    .iter()
                    .enumerate()
                    .filter_map(|(i, row)| {
                        has_agent(row, &|a| if pane { &a.pane == id } else { &a.tab == id })
                            .then_some(i)
                    })
                    .collect::<Vec<_>>();
                if let Some(i) = self.choose(table, &candidates) {
                    return Some(i);
                }
            }
        }
        if let Some(dir) = &self.dir {
            let dir = dir.canonicalize().unwrap_or_else(|_| dir.clone());
            let tree = table
                .trees
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    (
                        i,
                        t.wt.path
                            .canonicalize()
                            .unwrap_or_else(|_| t.wt.path.clone()),
                    )
                })
                .filter(|(_, path)| dir.starts_with(path))
                .max_by_key(|(_, path)| path.components().count())
                .map(|(i, _)| i);
            if let Some(tree) = tree {
                let candidates = table
                    .tasks
                    .iter()
                    .enumerate()
                    .filter_map(|(i, row)| row.trees.contains(&tree).then_some(i))
                    .collect::<Vec<_>>();
                if let Some(i) = self.choose(table, &candidates) {
                    return Some(i);
                }
            }
        }
        let candidates = (0..table.tasks.len()).collect::<Vec<_>>();
        self.in_workspace(table, &candidates)
    }

    fn choose(&self, table: &Table, candidates: &[usize]) -> Option<usize> {
        self.in_workspace(table, candidates).or(match candidates {
            [i] => Some(*i),
            _ => None,
        })
    }

    fn in_workspace(&self, table: &Table, candidates: &[usize]) -> Option<usize> {
        let unique = |found: Vec<usize>| match found.as_slice() {
            [i] => Some(*i),
            _ => None,
        };
        if let Some(workspace) = &self.workspace {
            let found = candidates
                .iter()
                .copied()
                .filter(|&i| table.tasks[i].space.as_deref() == Some(workspace.as_str()))
                .collect();
            if let Some(i) = unique(found) {
                return Some(i);
            }
        }
        let label = self.label.as_deref()?;
        unique(
            candidates
                .iter()
                .copied()
                .filter(|&i| {
                    table.tasks[i]
                        .task
                        .name
                        .trim()
                        .eq_ignore_ascii_case(label.trim())
                })
                .collect(),
        )
    }
}

pub(crate) fn caller_dir() -> Option<PathBuf> {
    Opening::capture().dir
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::fixtures::{ROOT, table, task, tree};

    fn two_tasks() -> Table {
        table(
            vec![task("alpha", &["DEMO-1"]), task("beta", &["DEMO-2"])],
            vec![],
            vec![],
            vec![tree("alpha", "DEMO-1-alpha"), tree("beta", "DEMO-2-beta")],
        )
    }

    fn selected<'a>(opening: &Opening, table: &'a Table) -> Option<&'a str> {
        opening
            .task(table)
            .map(|i| table.tasks[i].task.name.as_str())
    }

    #[test]
    fn pane_and_tab_win_over_an_outdated_shell_directory() {
        let mut table = two_tasks();
        let mut agent = crate::table::fixtures::agent("beta", "beta", "w2");
        agent.pane_id = "w2:p3".into();
        agent.tab_id = "w2:t2".into();
        table.update_agents(&[agent]);
        for key in ["focused_pane_id", "tab_id"] {
            let mut context = serde_json::json!({"focused_pane_cwd": format!("{ROOT}/alpha")});
            context[key] = Value::String(if key == "tab_id" { "w2:t2" } else { "w2:p3" }.into());
            assert_eq!(
                selected(&Opening::from_context(&context), &table),
                Some("beta")
            );
        }
    }

    #[test]
    fn a_shell_tab_selects_its_task_from_a_subdirectory() {
        let table = two_tasks();
        let opening = Opening::from_context(&serde_json::json!({
            "focused_pane_cwd": format!("{ROOT}/beta/src"),
            "workspace_label": "alpha"
        }));
        assert_eq!(selected(&opening, &table), Some("beta"));
        let other = Opening::from_context(&serde_json::json!({
            "focused_pane_cwd": format!("{ROOT}/beta-other")
        }));
        assert_eq!(selected(&other, &table), None);
    }

    #[test]
    #[cfg(unix)]
    fn caller_and_git_paths_can_use_different_symlinks() {
        let dir = crate::table::fixtures::temp_dir();
        let real = dir.join("real");
        std::fs::create_dir_all(real.join("src")).unwrap();
        let alias = dir.join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let mut table = two_tasks();
        let beta = table.tree_index("beta");
        table.trees[beta].wt.path = real;
        let opening = Opening {
            dir: Some(alias.join("src")),
            ..Opening::default()
        };
        assert_eq!(selected(&opening, &table), Some("beta"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn workspace_resolves_work_in_the_main_checkout_or_an_unlinked_tree() {
        let table = two_tasks();
        let opening = Opening::from_context(&serde_json::json!({
            "focused_pane_cwd": "/main", "workspace_label": " BETA "
        }));
        assert_eq!(selected(&opening, &table), Some("beta"));
        assert_eq!(selected(&Opening::default(), &table), None);
    }

    #[test]
    fn shared_worktrees_need_unambiguous_workspace_context() {
        let table = table(
            vec![task("alpha", &["DEMO-1"]), task("beta", &["DEMO-2"])],
            vec![],
            vec![],
            vec![tree("shared", "DEMO-1-DEMO-2")],
        );
        let mut opening = Opening::from_context(&serde_json::json!({
            "focused_pane_cwd": format!("{ROOT}/shared")
        }));
        assert_eq!(selected(&opening, &table), None);
        opening.label = Some("beta".into());
        assert_eq!(selected(&opening, &table), Some("beta"));
    }

    #[test]
    fn invocation_cwd_falls_back_to_worktree_then_workspace() {
        let context = serde_json::json!({
            "focused_pane_cwd": "", "worktree": { "checkout_path": "/tree" },
            "workspace_cwd": "/main"
        });
        assert_eq!(
            Opening::from_context(&context).dir,
            Some(PathBuf::from("/tree"))
        );
        assert_eq!(
            Opening::from_context(&serde_json::json!({"workspace_cwd": "/main"})).dir,
            Some(PathBuf::from("/main"))
        );
    }
}
