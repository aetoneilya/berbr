//! Project the existing table into Herdr's sidebar without changing task notes.

use crate::client::herdr;
use crate::config::{self, Config};
use crate::table::{self, Agent, Fresh, Mr, RowState, Table, TaskRow};
use crate::vault::{self, task::TaskState};
use std::collections::BTreeMap;
use std::fs::File;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_secs(5);
const REFRESH: Duration = Duration::from_secs(60);
const KEYS: [&str; 11] = [
    "wt_project",
    "wt_task",
    "wt_state",
    "wt_ticket",
    "wt_mr",
    "wt_ci",
    "wt_review",
    "wt_role",
    "wt_fresh",
    "wt_workspace",
    "wt_cli",
];
type Tokens = BTreeMap<String, String>;
type Reports = BTreeMap<(&'static str, String), Tokens>;

fn empty_tokens() -> Tokens {
    KEYS.into_iter()
        .map(|key| (key.into(), String::new()))
        .collect()
}

fn tokens(table: &Table, row: Option<&TaskRow>, keys: &[String], mrs: &[usize]) -> Tokens {
    let mut out = empty_tokens();
    if let Some(row) = row {
        out.insert(
            "wt_task".into(),
            table::look::clean(&row.task.display_name()),
        );
        let state = match row.row_state() {
            Some(RowState::Task(TaskState::Idea)) => "todo",
            Some(RowState::Task(TaskState::Active)) => "in progress",
            Some(RowState::Task(TaskState::Review)) => "in review",
            Some(RowState::Task(TaskState::Finished) | RowState::Done) => "done",
            None => "",
        };
        out.insert("wt_state".into(), state.into());
    }
    out.insert("wt_ticket".into(), keys.join(" "));
    let mrs: Vec<_> = mrs
        .iter()
        .map(|&i| &table.mrs[i])
        .filter(|m| m.opened())
        .collect();
    out.insert(
        "wt_mr".into(),
        mrs.iter()
            .map(|m| m.mr.number_label())
            .collect::<Vec<_>>()
            .join(" "),
    );
    out.insert("wt_ci".into(), pipeline(&mrs).into());
    out.insert("wt_review".into(), attention(&mrs));
    if table.jira.fresh == Fresh::Cached
        || table.gitlab.fresh == Fresh::Cached
        || table.github.fresh == Fresh::Cached
    {
        out.insert("wt_fresh".into(), "cached".into());
    }
    out
}

/// One CI badge for the branch or task; failures take priority over pending and success.
fn pipeline(mrs: &[&Mr]) -> &'static str {
    mrs.iter()
        .filter_map(|mr| mr.review.as_ref()?.pipeline.as_deref())
        .map(|status| match status {
            "failed" => (5, "CI ✗"),
            "canceled" => (4, "CI –"),
            "manual" => (3, "CI Ⅱ"),
            "created"
            | "pending"
            | "preparing"
            | "running"
            | "waiting_for_resource"
            | "scheduled" => (2, "CI ↻"),
            "success" => (1, "CI ✓"),
            "skipped" => (0, "CI –"),
            _ => (3, "CI ?"),
        })
        .max_by_key(|(priority, _)| *priority)
        .map_or("", |(_, badge)| badge)
}

fn attention(mrs: &[&Mr]) -> String {
    let mut parts = Vec::new();
    let has = |status| {
        mrs.iter()
            .any(|m| m.mr.detailed_merge_status.as_deref() == Some(status))
    };
    if has("conflict") {
        parts.push("✗ конфликт".to_string());
    }
    if has("need_rebase") {
        parts.push("↻ нужен rebase".to_string());
    }
    if has("changes_requested") {
        parts.push("changes requested".into());
    }
    let threads: usize = mrs
        .iter()
        .filter_map(|m| m.review.as_ref())
        .map(|r| r.unresolved)
        .sum();
    if threads > 0 {
        // Nerd Fonts: nf-oct-comment_discussion (U+F442).
        parts.push(format!("\u{f442} обсуждений: {threads}"));
    } else if has("discussions_not_resolved") {
        parts.push("\u{f442} есть обсуждения".to_string());
    }
    // A zero approval count alone does not mean an approval is required.
    if mrs
        .iter()
        .any(|m| !m.mr.draft && m.mr.detailed_merge_status.as_deref() == Some("not_approved"))
    {
        parts.push("\u{f4af} ждёт ревью".to_string());
    }
    parts.join(" · ")
}

fn role(agent: &Agent) -> &'static str {
    // berbr review names its agents <worktree>-review.
    if agent.name.ends_with("-review") {
        // Nerd Fonts: nf-oct-code_review (U+F4AF).
        "\u{f4af} review"
    } else {
        ""
    }
}

fn agent_tokens(mut values: Tokens, agent: &Agent) -> Tokens {
    values.insert("wt_role".into(), role(agent).into());
    let cli = agent.kind.map(|kind| kind.binary()).unwrap_or("");
    // Nerd Fonts: nf-cod-layout (U+EBEB).
    values.insert(
        "wt_workspace".into(),
        if agent.workspace.is_empty() {
            String::new()
        } else {
            "\u{ebeb}".into()
        },
    );
    values.insert(
        "wt_cli".into(),
        if !agent.named() || agent.name.eq_ignore_ascii_case(cli) {
            ""
        } else {
            cli
        }
        .into(),
    );
    values
}

fn reports(table: &Table) -> Reports {
    let mut out = Reports::new();
    for row in table.tasks.iter().filter(|r| !r.task.done) {
        let keys: Vec<_> = row.tickets().cloned().collect();
        let values = tokens(table, Some(row), &keys, &row.mrs);
        if let Some(space) = &row.space {
            out.insert(("workspace", space.clone()), values.clone());
        }
        for agent in &row.agents {
            out.insert(
                ("pane", agent.pane.clone()),
                agent_tokens(values.clone(), agent),
            );
        }
    }
    for (i, tree) in table.trees.iter().enumerate() {
        for agent in &tree.agents {
            let owners: Vec<_> = table
                .tasks
                .iter()
                .filter(|r| !r.task.done && r.trees.contains(&i))
                .collect();
            let row = owners
                .iter()
                .find(|r| r.space.as_deref() == Some(&agent.workspace))
                .copied()
                .or_else(|| (owners.len() == 1).then(|| owners[0]));
            let values = tokens(
                table,
                row,
                &tree.keys,
                &tree.mr.into_iter().collect::<Vec<_>>(),
            );
            out.insert(("pane", agent.pane.clone()), agent_tokens(values, agent));
        }
    }
    out
}

fn publish(
    tables: &BTreeMap<String, Table>,
    sent: &mut BTreeMap<(&'static str, String), (Tokens, Instant)>,
) {
    let current: Reports = tables
        .iter()
        .flat_map(|(project, table)| {
            reports(table).into_iter().map(move |(target, mut values)| {
                values.insert("wt_project".into(), table::look::clean(project));
                (target, values)
            })
        })
        .collect();
    // Clear our tokens on still-live targets that no longer belong to a task.
    let removed: Vec<_> = sent
        .keys()
        .filter(|target| !current.contains_key(*target))
        .cloned()
        .collect();
    for (scope, target) in removed {
        let _ = herdr::report_metadata(scope, &target, &empty_tokens());
        sent.remove(&(scope, target));
    }
    for ((scope, target), values) in current {
        if sent
            .get(&(scope, target.clone()))
            .is_some_and(|(old, at)| old == &values && at.elapsed() < REFRESH)
        {
            continue;
        }
        match herdr::report_metadata(scope, &target, &values) {
            Ok(()) => {
                sent.insert((scope, target), (values, Instant::now()));
            }
            Err(err) => eprintln!("sidebar {scope} {target}: {err}"),
        }
    }
}

fn load(config: &Config, cached: bool) -> Result<Table, String> {
    let (tasks, skipped) = vault::task::load(config)?;
    for err in skipped {
        eprintln!("sidebar skipped {err}");
    }
    Ok(if cached {
        table::build_cached(config, tasks)
    } else {
        table::build(config, tasks, true)
    })
}

fn state_path(extension: &str) -> Result<PathBuf, String> {
    let root = std::env::var_os("HERDR_PLUGIN_STATE_DIR")
        .map(PathBuf::from)
        .ok_or("HERDR_PLUGIN_STATE_DIR is missing")?;
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let mut hash = DefaultHasher::new();
    std::env::var_os("HERDR_SOCKET_PATH")
        .ok_or("HERDR_SOCKET_PATH is missing")?
        .hash(&mut hash);
    Ok(root.join(format!("metadata-{:x}.{extension}", hash.finish())))
}

/// A one-shot plugin entrypoint. The child holds a per-socket advisory lock.
pub fn start() -> Result<(), String> {
    config::load_projects().map_err(|e| e.to_string())?;
    let log = File::options()
        .create(true)
        .append(true)
        .open(state_path("log")?)
        .map_err(|e| e.to_string())?;
    Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .arg("plugin-metadata")
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .process_group(0)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn run(once: bool, cached: bool) -> Result<(), String> {
    let socket = std::env::var_os("HERDR_SOCKET_PATH").map(PathBuf::from);
    let socket_inode = socket
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.ino());
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let binary_stamp = std::fs::metadata(&exe).and_then(|m| m.modified()).ok();
    let _lock = if once {
        None
    } else {
        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(state_path("lock")?)
            .map_err(|e| e.to_string())?;
        // An old worker exits within POLL when an installation replaces its binary.
        let mut acquired = false;
        for _ in 0..24 {
            match lock.try_lock() {
                Ok(()) => {
                    acquired = true;
                    break;
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(250))
                }
                Err(err) => return Err(err.to_string()),
            }
        }
        if !acquired {
            return Ok(());
        }
        Some(lock)
    };
    let projects = config::load_projects().map_err(|e| e.to_string())?;
    let mut tables = BTreeMap::new();
    let mut sent = BTreeMap::new();
    for config in &projects {
        tables.insert(config.name.clone(), load(config, cached || !once)?);
        publish(&tables, &mut sent);
    }
    if once {
        return Ok(());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        loop {
            for config in &projects {
                match load(config, false) {
                    Ok(table) => {
                        if tx.send((config.name.clone(), table)).is_err() {
                            return;
                        }
                    }
                    Err(err) => eprintln!("sidebar {}: {err}", config.name),
                }
            }
            std::thread::sleep(REFRESH);
        }
    });
    loop {
        match rx.recv_timeout(POLL) {
            Ok((name, table)) => {
                tables.insert(name, table);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(err) => return Err(err.to_string()),
        }
        if socket
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.ino())
            != socket_inode
            || std::fs::metadata(&exe).and_then(|m| m.modified()).ok() != binary_stamp
        {
            return Ok(());
        }
        let agents = herdr::list_agents().map_err(|e| e.to_string())?;
        for table in tables.values_mut() {
            table.update_agents(&agents);
        }
        publish(&tables, &mut sent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::gitlab::MrReview;
    use crate::table::fixtures::{agent, mr, task, tree};

    #[test]
    fn panes_get_their_own_branch_mr_and_workspaces_get_the_task_summary() {
        let mut table = crate::table::fixtures::table(
            vec![task("platform", &["DEMO-1", "DEMO-2"])],
            vec![],
            vec![
                mr(11, "DEMO-1-first", "first", "opened"),
                mr(22, "DEMO-2-second", "second", "opened"),
            ],
            vec![
                tree("first", "DEMO-1-first"),
                tree("second", "DEMO-2-second"),
            ],
        );
        table.tasks[0].space = Some("w1".into());
        table.mrs[0].review = Some(MrReview {
            pipeline: Some("failed".into()),
            approved_by: vec!["reviewer".into()],
            unresolved: 2,
        });
        table.mrs[1].review = Some(MrReview {
            pipeline: Some("success".into()),
            ..Default::default()
        });
        table.update_agents(&[
            agent("first", "first", "w1"),
            agent("second-review", "second", "w1"),
        ]);
        let got = reports(&table);
        let first = &got[&("pane", "w1:first".into())];
        assert_eq!(first["wt_task"], "platform");
        assert_eq!(first["wt_ticket"], "DEMO-1");
        assert_eq!(first["wt_mr"], "!11");
        assert_eq!(first["wt_ci"], "CI ✗");
        assert_eq!(first["wt_review"], "\u{f442} обсуждений: 2");
        assert_eq!(first["wt_role"], "");
        assert_eq!(first["wt_workspace"], "\u{ebeb}");
        let second = &got[&("pane", "w1:second-review".into())];
        assert_eq!(second["wt_mr"], "!22");
        assert_eq!(second["wt_ci"], "CI ✓");
        assert_eq!(second["wt_review"], "");
        assert_eq!(second["wt_role"], "\u{f4af} review");
        let workspace = &got[&("workspace", "w1".into())];
        assert_eq!(workspace["wt_mr"], "!11 !22");
        assert_eq!(workspace["wt_ticket"], "DEMO-1 DEMO-2");
        assert_eq!(workspace["wt_role"], "");
        assert_eq!(workspace["wt_ci"], "CI ✗");
    }

    #[test]
    fn merged_mrs_clear_ci_and_review_and_cached_values_are_marked() {
        let mut table = crate::table::fixtures::table(
            vec![task("platform", &["DEMO-1"])],
            vec![],
            vec![mr(11, "DEMO-1-first", "first", "merged")],
            vec![tree("first", "DEMO-1-first")],
        );
        table.tasks[0].space = Some("w1".into());
        table.gitlab.fresh = Fresh::Cached;
        let got = reports(&table);
        let values = &got[&("workspace", "w1".into())];
        for key in ["wt_mr", "wt_ci", "wt_review"] {
            assert_eq!(values[key], "");
        }
        assert_eq!(values["wt_fresh"], "cached");
        table.tasks[0].task.done = true;
        assert!(!reports(&table).contains_key(&("workspace", "w1".into())));
    }

    #[test]
    fn review_attention_requires_gitlab_to_request_approval() {
        let mut mr = mr(1, "branch", "title", "opened");
        mr.review = Some(MrReview::default());
        assert_eq!(attention(&[&mr]), "");
        mr.mr.detailed_merge_status = Some("not_approved".into());
        assert_eq!(attention(&[&mr]), "\u{f4af} ждёт ревью");
        mr.mr.draft = true;
        assert_eq!(attention(&[&mr]), "");
        mr.mr.detailed_merge_status = Some("conflict".into());
        assert_eq!(attention(&[&mr]), "✗ конфликт");
    }

    #[test]
    fn workspace_is_only_a_badge_and_cli_does_not_repeat_the_agent_name() {
        let mut table =
            crate::table::fixtures::table(vec![], vec![], vec![], vec![tree("first", "branch")]);
        table.update_agents(&[agent("codex", "first", "w1")]);
        let agent = &mut table.trees[0].agents[0];
        agent.kind = Some(crate::client::agent::AgentKind::Codex);
        let mut values = empty_tokens();
        values.insert("wt_task".into(), "codex".into());
        let values = agent_tokens(values, agent);
        assert_eq!(values["wt_workspace"], "\u{ebeb}");
        assert_eq!(values["wt_task"], "codex");
        assert_eq!(values["wt_cli"], "");
        agent.name = "first".into();
        let mut values = empty_tokens();
        values.insert("wt_task".into(), "platform".into());
        let values = agent_tokens(values, agent);
        assert_eq!(values["wt_workspace"], "\u{ebeb}");
        assert_eq!(values["wt_task"], "platform");
        agent.workspace.clear();
        let values = agent_tokens(empty_tokens(), agent);
        assert_eq!(values["wt_workspace"], "");
        assert_eq!(values["wt_cli"], "codex");
        agent.name = agent.pane.clone();
        assert_eq!(agent_tokens(empty_tokens(), agent)["wt_cli"], "");
        agent.workspace = "w1".into();
        for name in [
            "open-codex-from-wt",
            "Open_Codex__from-WT",
            "open-codex-from-wt-2",
        ] {
            agent.name = name.into();
            let mut values = empty_tokens();
            values.insert("wt_task".into(), "open codex from wt".into());
            let values = agent_tokens(values, agent);
            assert_eq!(values["wt_task"], "open codex from wt");
            assert_eq!(values["wt_workspace"], "\u{ebeb}");
        }
    }
}
