//! Stable, read-only data boundary between the Rust model and the native mod.
use crate::client::agent::AgentKind;
use crate::client::gitlab::MrState;
use crate::config::Config;
use crate::table::{
    self, Table, look,
    view::{self, Kind, Node},
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub(super) fn snapshot(projects: &[Config], chosen: usize, cached: bool) -> Result<(), String> {
    let config = &projects[chosen];
    let (tasks, skipped) = crate::vault::task::load_previews(config);
    let mut table = table::build_app(config, tasks, cached);
    table
        .warnings
        .extend(skipped.into_iter().map(|e| format!("skipped {e}")));
    let roots = view::build(
        &table,
        &view::Options {
            all: true,
            active_loose: true,
            ..Default::default()
        },
    );
    let stopped = stopped_by_tree(&table);
    let mut rows: Vec<_> = roots
        .iter()
        .map(|n| row_with_history(&table, n, None, &stopped))
        .collect();
    add_artifact_paths(&mut rows, &config.name);
    println!(
        "{}",
        json!({
            "version": 3, "project": config.name,
            "projects": projects.iter().map(|c| json!({"name":c.name,"cwd":c.repos.first().map(|r|&r.path),"agent":c.workflow.agent,"integrations":{"jira":c.jira.is_some(),"github":c.has_github()}})).collect::<Vec<_>>(),
            "rows": rows, "warnings": table.warnings, "debug": crate::log::debug_enabled(),
            "settings": {"agent":config.workflow.agent, "generation_agent":config.workflow.generation_agent,
                "generation_model":config.workflow.generation_model},
            "store": if config.vault.own { "berbr" } else { "vault" },
            "sources": {"jira":format!("{:?}",table.jira.fresh),"gitlab":format!("{:?}",table.gitlab.fresh),"github":format!("{:?}",table.github.fresh)}
        })
    );
    Ok(())
}

/// Paths only: snapshots never create folders or scan arbitrary artifact trees.
fn add_artifact_paths(rows: &mut [Value], project: &str) {
    for row in rows {
        if row["kind"] == "task"
            && let Some(id) = row["task"].as_str()
        {
            row["artifacts"] = json!(crate::artifacts::directory(project, id));
        }
        if let Some(children) = row["children"].as_array_mut() {
            add_artifact_paths(children, project);
        }
    }
}

#[cfg(test)]
fn row(table: &Table, node: &Node, owner: Option<&str>) -> Value {
    row_with_history(table, node, owner, &stopped_by_tree(table))
}

/// Saved conversations outlive their terminals. Index once per snapshot, keeping
/// each conversation under its most recent worktree and omitting live sessions.
struct AgentHistory {
    stopped: HashMap<PathBuf, Vec<Value>>,
    bindings: HashMap<String, String>,
    uncertain: HashSet<String>,
}

fn stopped_by_tree(table: &Table) -> AgentHistory {
    let mut live: HashSet<_> = table
        .trees
        .iter()
        .flat_map(|t| &t.agents)
        .chain(table.tasks.iter().flat_map(|t| &t.agents))
        .flat_map(|a| a.session.iter().chain(&a.session_aliases).cloned())
        .collect();
    let mut saved = HashMap::new();
    for task in &table.tasks {
        for session in task.task.sessions.iter().chain(&task.new_sessions) {
            let trees: Vec<_> = task
                .trees
                .iter()
                .map(|&i| table.tree(i))
                .filter(|tree| session.matches_tree(&tree.wt.path))
                .collect();
            if trees.len() == 1 {
                saved.insert(
                    session.id.clone(),
                    (trees[0].wt.path.clone(), session.date.clone(), 0),
                );
            }
        }
    }
    for session in table.sessions.values() {
        let tree = session
            .last_cwd
            .iter()
            .map(String::as_str)
            .chain(session.cwds.iter().map(|(cwd, _)| cwd.as_str()))
            .find_map(|cwd| {
                table
                    .trees
                    .iter()
                    .filter(|t| Path::new(cwd).starts_with(&t.wt.path))
                    .max_by_key(|t| t.wt.path.components().count())
            });
        if let Some(tree) = tree {
            saved.insert(
                session.id.clone(),
                (tree.wt.path.clone(), session.date.clone(), session.modified),
            );
        }
    }
    let mut saved: Vec<_> = saved
        .into_iter()
        .filter(|(id, _)| !live.contains(id.as_str()))
        .collect();
    saved.sort_by(|(a, (_, ad, am)), (b, (_, bd, bm))| {
        bm.cmp(am).then_with(|| bd.cmp(ad)).then_with(|| a.cmp(b))
    });
    // A fresh CLI has no reported conversation ID. Match the transcript born
    // when its terminal started, rather than a newer conversation closed later.
    // Legacy backends lacking birth times retain select's newest-session rule.
    let mut bindings = HashMap::new();
    for tree in &table.trees {
        let mut unknown: Vec<_> = tree.agents.iter().filter(|a| a.session.is_none()).collect();
        unknown.sort_by_key(|a| std::cmp::Reverse(a.created_ms));
        for agent in unknown {
            let candidates: Vec<_> = saved
                .iter()
                .filter(|(id, (path, _, _))| {
                    path == &tree.wt.path
                        && Some(AgentKind::from_session_key(id)) == agent.kind
                        && !live.contains(id)
                })
                .collect();
            let chosen = if agent.created_ms == 0 {
                candidates.first().copied()
            } else {
                candidates
                    .into_iter()
                    .filter_map(|entry| {
                        let birth = table.sessions.get(&entry.0)?.created_ms;
                        (birth > 0
                            && birth.saturating_add(2000) >= agent.created_ms
                            && birth <= agent.created_ms.saturating_add(120_000))
                        .then_some((entry, birth.abs_diff(agent.created_ms)))
                    })
                    .min_by_key(|(_, distance)| *distance)
                    .map(|(entry, _)| entry)
            };
            if let Some((id, _)) = chosen {
                bindings.insert(agent.pane.clone(), id.clone());
                live.insert(id.clone());
            }
        }
    }
    let uncertain: HashSet<_> = saved
        .iter()
        .filter(|(id, (path, _, modified))| {
            !live.contains(id)
                && table
                    .trees
                    .iter()
                    .filter(|tree| &tree.wt.path == path)
                    .flat_map(|tree| &tree.agents)
                    .any(|agent| {
                        agent.session.is_none()
                            && !bindings.contains_key(&agent.pane)
                            && agent.kind == Some(AgentKind::from_session_key(id))
                            && (agent.created_ms == 0
                                || modified.saturating_mul(1000).saturating_add(1000)
                                    >= agent.created_ms)
                    })
        })
        .map(|(id, _)| id.clone())
        .collect();
    let mut result: HashMap<PathBuf, Vec<Value>> = HashMap::new();
    for (id, (path, date, _)) in saved {
        if live.contains(&id) {
            continue;
        }
        let kind = AgentKind::from_session_key(&id);
        let short: String = AgentKind::native_id(&id).chars().take(8).collect();
        result.entry(path.clone()).or_default().push(json!({
            "id":format!("session:{}:{id}", path.display()), "kind":"agent",
            "session":id, "agent":kind, "cwd":path, "state":"stopped", "terminal":null,
            "resume_blocked":uncertain.contains(&id), "label":format!("{kind} · {date} · {short}")
        }));
    }
    AgentHistory {
        stopped: result,
        bindings,
        uncertain,
    }
}

pub(crate) fn session_may_be_live(table: &Table, id: &str) -> bool {
    stopped_by_tree(table).uncertain.contains(id)
}

pub(crate) fn session_worktree<'a>(
    table: &'a Table,
    session: &crate::client::sessions::Session,
) -> Option<&'a Path> {
    session
        .last_cwd
        .iter()
        .map(String::as_str)
        .chain(session.cwds.iter().map(|(cwd, _)| cwd.as_str()))
        .find_map(|cwd| {
            table
                .trees
                .iter()
                .filter(|tree| Path::new(cwd).starts_with(&tree.wt.path))
                .max_by_key(|tree| tree.wt.path.components().count())
                .map(|tree| tree.wt.path.as_path())
        })
}

pub(crate) fn session_terminal(table: &Table, id: &str) -> Option<String> {
    table
        .trees
        .iter()
        .flat_map(|tree| &tree.agents)
        .chain(table.tasks.iter().flat_map(|task| &task.agents))
        .find(|agent| {
            agent.session.as_deref() == Some(id)
                || agent.session_aliases.iter().any(|alias| alias == id)
        })
        .map(|agent| agent.pane.clone())
        .or_else(|| {
            stopped_by_tree(table)
                .bindings
                .into_iter()
                .find(|(_, session)| session == id)
                .map(|(terminal, _)| terminal)
        })
}

fn row_with_history(
    table: &Table,
    node: &Node,
    owner: Option<&str>,
    stopped: &AgentHistory,
) -> Value {
    let mut value = json!({"id": node.id,"task":owner});
    match &node.kind {
        Kind::Task(i) => {
            let r = &table.tasks[*i];
            value["kind"] = json!("task");
            value["task"] = json!(r.task.name);
            value["title"] = json!(r.task.title().unwrap_or_else(|| "Untitled task".into()));
            value["label"] = json!(r.task.display_name());
            value["state"] = json!(r.row_state().map(|s| s.as_str()));
            value["done"] = json!(r.task.done);
            // Keep `body` for older mods; only task bodies change semantics.
            let (body, truncated) = r.task.note.body_preview();
            value["body"] = json!(body);
            value["body_truncated"] = json!(truncated);
            value["note"] = json!(r.task.note.path);
            value["tags"] = json!(r.task.tags);
            value["space"] = json!(r.space);
        }
        Kind::Ticket(key) => ticket(table, &mut value, key),
        Kind::Loose(i) => ticket(table, &mut value, &table.loose[*i].key),
        Kind::Mr(i) => {
            let m = table.mr(*i);
            value["kind"] = json!("mr");
            value["label"] = json!(format!("{}  {}", m.mr.number_label(), look::mr_title(m)));
            value["state"] = json!(if m.mr.github && m.opened() {
                "open"
            } else {
                m.mr.state().as_str()
            });
            value["provider"] = json!(m.mr.provider());
            value["repository"] = json!(m.project);
            value["head_repository"] = json!(m.mr.head_repo);
            value["review_decision"] = json!(m.mr.review_decision);
            value["url"] = json!(m.mr.web_url());
            value["branch"] = json!(m.mr.source_branch());
            value["body"] = json!(
                look::review(m)
                    .iter()
                    .map(|p| p.text.as_str())
                    .collect::<String>()
            );
            if m.opened() {
                let review = m.review.as_ref();
                let ci = review.and_then(|r| r.pipeline.as_deref());
                let unresolved = review.map_or(0, |r| r.unresolved);
                let conflict =
                    m.mr.has_conflicts || m.mr.detailed_merge_status.as_deref() == Some("conflict");
                value["ci"] = json!(ci);
                value["approvals"] = json!(review.map_or(0, |r| r.approved_by.len()));
                value["unresolved"] = json!(unresolved);
                value["draft"] = json!(m.mr.draft);
                value["merge_status"] = json!(m.mr.detailed_merge_status);
                value["conflict"] = json!(conflict);
                value["ready"] = json!(
                    (!m.mr.github || table.github.fresh == table::Fresh::Live)
                        && !m.mr.draft
                        && ci == Some("success")
                        && !conflict
                        && unresolved == 0
                        && m.mr.detailed_merge_status.as_deref() == Some("mergeable")
                );
            }
        }
        Kind::Tree(i) => {
            let t = table.tree(*i);
            value["kind"] = json!("tree");
            value["label"] = json!(t.wt.name());
            value["tree"] = json!(t.wt.name());
            value["cwd"] = json!(t.wt.path);
            value["repo"] = json!(t.repo);
            value["branch"] = json!(t.wt.branch);
            value["diff"] = json!(t.diff);
            value["merged"] = json!(
                t.finished
                    || t.mr
                        .is_some_and(|m| table.mr(m).mr.state() == MrState::Merged)
            );
            value["mr_state"] = json!(t.mr.and_then(|m| match table.mr(m).mr.state() {
                state @ (MrState::Opened | MrState::Merged | MrState::Closed) =>
                    Some(state.as_str()),
                _ => None,
            }));
            value["state"] = json!(if t.finished { "finished" } else { "active" });
            value["body"] = json!(format!(
                "{}\n{}",
                t.wt.path.display(),
                look::tree_marks(t)
                    .iter()
                    .map(|p| p.text.as_str())
                    .collect::<String>()
            ));
        }
        Kind::Agent(at, i) => {
            let a = table.agent(*at, *i);
            value["kind"] = json!("agent");
            value["label"] = json!(a.name);
            value["state"] = json!(a.status.as_str());
            value["terminal"] = json!(a.pane);
            value["session"] = json!(a.session.as_ref().or_else(|| stopped.bindings.get(&a.pane)));
            value["agent"] = json!(a.kind);
            value["role"] = json!(a.role);
            value["role_explicit"] = json!(a.role_explicit);
            value["session_aliases"] = json!(a.session_aliases);
            value["space"] = json!(a.workspace);
            value["tab"] = json!(a.tab);
        }
        Kind::Header(section) => {
            value["kind"] = json!("header");
            value["label"] = json!(section.title_for(table));
        }
        Kind::NewTask => {
            value["kind"] = json!("new");
        }
    }
    let owner = value["task"].as_str();
    let mut children: Vec<_> = node
        .children
        .iter()
        .map(|n| row_with_history(table, n, owner, stopped))
        .collect();
    if let Kind::Tree(i) = node.kind {
        let path = &table.tree(i).wt.path;
        children.extend(
            stopped
                .stopped
                .get(path)
                .into_iter()
                .flatten()
                .cloned()
                .map(|mut row| {
                    row["task"] = json!(owner);
                    row
                }),
        );
        for child in &mut children {
            if child["kind"] == "agent" {
                child["cwd"] = json!(path);
            }
        }
    }
    value["children"] = json!(children);
    value
}

fn ticket(table: &Table, value: &mut Value, key: &str) {
    value["kind"] = json!("ticket");
    if crate::client::issue::github_parts(key).is_some() {
        value["provider"] = json!("github");
    }
    value["label"] = json!(format!("{key}  {}", look::summary(table.tickets.get(key))));
    value["target"] = json!(key);
    value["state"] = json!(table.tickets.get(key).map(|t| t.status.as_str()));
    value["url"] = json!(table.ticket_url(key));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::gitlab::MrReview;
    use crate::table::fixtures::{mr, table, task, tree};

    #[test]
    fn preview_write_back_preserves_full_body_and_bounded_snapshot_json() {
        use crate::vault::{
            note::{BODY_PREVIEW_BYTES, Note},
            task::{SessionRef, Task, TaskState},
        };
        let dir = crate::table::fixtures::temp_dir();
        let path = dir.join("demo-1.md");
        let header = "---\nid: demo-1\ntitle: Demo\ntickets: ['[[DEMO-1]]']\ntags: [demo]\nstate: idea\nsessions: ['s0 demo-tree 2026-01-01']\n---\n";
        let body = format!(
            "Human requirements.\n\n{}\n\n## Agent notes\nMachine history.\n",
            "More human context. ".repeat(400)
        );
        std::fs::write(&path, format!("{header}{body}")).unwrap();
        let preview = Task::from_note(Note::read_preview(&path).unwrap());
        let mut table = table(
            vec![preview],
            vec![],
            vec![],
            vec![tree("demo-tree", "DEMO-1-DEMO-2")],
        );
        assert_eq!(table.tasks[0].state, Some(TaskState::Active));
        assert_eq!(table.tasks[0].found_tickets, ["DEMO-2"]);
        table.tasks[0].new_sessions.push(SessionRef {
            id: "codex:s1".into(),
            worktree: "demo-tree".into(),
            date: "2026-01-02".into(),
        });
        // An editor can change the body after the preview was loaded. Reload
        // under write_back's lock must preserve that current body byte-for-byte.
        let latest_body = format!("{body}\nLatest human edit.\n");
        std::fs::write(&path, format!("{header}{latest_body}")).unwrap();
        assert_eq!(table::write_back(&mut table), ["demo-1"]);
        assert!(table.warnings.is_empty(), "{:?}", table.warnings);
        let saved = Task::from_note(Note::read(&path).unwrap());
        assert_eq!(saved.note.body().as_bytes(), latest_body.as_bytes());
        assert_eq!(saved.state, Some(TaskState::Active));
        assert_eq!(saved.note.strings("tickets"), ["[[DEMO-1]]", "DEMO-2"]);
        assert_eq!(
            saved.note.strings("sessions"),
            ["s0 demo-tree 2026-01-01", "codex:s1 demo-tree 2026-01-02"]
        );
        assert_eq!(saved.tags, ["demo"]);
        assert_eq!(table.tasks[0].task.note.body(), latest_body);
        assert!(table.tasks[0].found_tickets.is_empty());
        assert!(table.tasks[0].new_sessions.is_empty());
        let node = Node {
            id: "task:demo-1".into(),
            kind: Kind::Task(0),
            children: vec![],
        };
        let value = row(&table, &node, None);
        assert!(value["body"].as_str().unwrap().len() <= BODY_PREVIEW_BYTES);
        assert_eq!(value["body_truncated"], true);
        assert!(serde_json::to_vec(&value).unwrap().len() < 3000);
        assert!(table::write_back(&mut table).is_empty());
        assert!(table.warnings.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn three_hundred_large_notes_load_as_bounded_snapshot_previews() {
        use crate::config::{VaultConfig, WorkflowConfig};
        let dir = crate::table::fixtures::temp_dir();
        std::fs::create_dir(dir.join("tasks")).unwrap();
        let history = "x".repeat(256 * 1024);
        for i in 0..300 {
            std::fs::write(dir.join(format!("tasks/demo-{i}.md")), format!(
                "---\nid: demo-{i}\ntitle: Demo {i}\ntickets: [DEMO-{}]\ntags: [demo]\nsessions: ['codex:demo demo-tree 2026-01-01']\n---\nHuman requirements.\n\n## Agent notes\n{history}", i + 1
            )).unwrap();
        }
        let config = Config {
            name: "demo".into(),
            jira: None,
            gitlab: None,
            github: None,
            workflow: WorkflowConfig::default(),
            repos: vec![],
            vault: VaultConfig {
                path: dir.clone(),
                task_dirs: vec!["tasks".into()],
                new_task_dir: "tasks".into(),
                own: false,
            },
        };
        let (tasks, errors) = crate::vault::task::load_previews(&config);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(tasks.len(), 300);
        for task in &tasks {
            assert_eq!(task.note.body_preview(), ("Human requirements.", true));
            assert_eq!(task.sessions.len(), 1);
            assert_eq!(task.tags, ["demo"]);
            assert_eq!(task.tickets.len(), 1);
        }
        let table = table(tasks, vec![], vec![], vec![]);
        let roots = view::build(
            &table,
            &view::Options {
                all: true,
                ..Default::default()
            },
        );
        let rows: Vec<_> = roots.iter().map(|n| row(&table, n, None)).collect();
        // 75 MiB of synthetic history must not leak into JSON. Short previews
        // leave room for metadata while staying under 512 bytes per task.
        assert!(serde_json::to_vec(&rows).unwrap().len() < 300 * 512);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn snapshot_task_payload_is_bounded_for_large_human_and_machine_sections() {
        use crate::vault::{
            note::{BODY_PREVIEW_BYTES, Note},
            task::Task,
        };
        let dir = crate::table::fixtures::temp_dir();
        let path = dir.join("demo-1.md");
        let node = Node {
            id: "task:demo-1".into(),
            kind: Kind::Task(0),
            children: vec![],
        };
        for body in [
            "x".repeat(256 * 1024),
            format!(
                "Human requirements.\n\n## Agent notes\n{}",
                "x".repeat(256 * 1024)
            ),
        ] {
            std::fs::write(
                &path,
                format!("---\nid: demo-1\ntitle: Demo\ntags: [demo]\n---\n{body}"),
            )
            .unwrap();
            // Bound serialization even if a future caller supplies full notes.
            for read in [Note::read, Note::read_preview] {
                let table = table(
                    vec![Task::from_note(read(&path).unwrap())],
                    vec![],
                    vec![],
                    vec![],
                );
                let value = row(&table, &node, None);
                assert!(value["body"].as_str().unwrap().len() <= BODY_PREVIEW_BYTES);
                assert_eq!(value["body_truncated"], true);
                assert_eq!(value["tags"], json!(["demo"]));
                assert!(serde_json::to_vec(&value).unwrap().len() < 3000);
                if body.starts_with("Human") {
                    assert_eq!(value["body"], "Human requirements.");
                }
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn snapshot_review_fields_and_each_readiness_blocker() {
        let node = Node {
            id: "mr:1".into(),
            kind: Kind::Mr(0),
            children: vec![],
        };
        for state in ["opened", "merged", "closed"] {
            for ci in [
                None,
                Some("success"),
                Some("running"),
                Some("pending"),
                Some("failed"),
                Some("canceled"),
                Some("skipped"),
                Some("manual"),
                Some("created"),
            ] {
                for status in [
                    None,
                    Some("mergeable"),
                    Some("conflict"),
                    Some("ci_still_running"),
                    Some("ci_must_pass"),
                    Some("discussions_not_resolved"),
                    Some("not_approved"),
                    Some("need_rebase"),
                    Some("draft_status"),
                ] {
                    for (draft, has_conflicts, unresolved) in [
                        (false, false, 0),
                        (true, false, 0),
                        (false, true, 0),
                        (false, false, 2),
                    ] {
                        let mut m = mr(1, "TEST-1-work", "work", state);
                        m.mr.draft = draft;
                        m.mr.has_conflicts = has_conflicts;
                        m.mr.detailed_merge_status = status.map(str::to_string);
                        m.review = Some(MrReview {
                            pipeline: ci.map(str::to_string),
                            approved_by: vec!["reviewer-a".into(), "reviewer-b".into()],
                            unresolved,
                        });
                        let table = table(vec![], vec![], vec![m], vec![]);
                        let value = row(&table, &node, None);
                        assert!(value["body"].is_string());
                        assert_eq!(value["branch"], "TEST-1-work");
                        if state != "opened" {
                            for field in [
                                "ci",
                                "approvals",
                                "unresolved",
                                "draft",
                                "merge_status",
                                "conflict",
                                "ready",
                            ] {
                                assert!(value[field].is_null(), "{field}: {value}");
                            }
                            continue;
                        }
                        assert_eq!(value["ci"], json!(ci));
                        assert_eq!(value["approvals"], 2);
                        assert_eq!(value["unresolved"], unresolved);
                        assert_eq!(value["draft"], draft);
                        assert_eq!(value["merge_status"], json!(status));
                        assert_eq!(
                            value["conflict"],
                            has_conflicts || status == Some("conflict")
                        );
                        assert_eq!(
                            value["ready"],
                            !draft
                                && !has_conflicts
                                && unresolved == 0
                                && ci == Some("success")
                                && status == Some("mergeable")
                        );
                    }
                }
            }
        }
        let table = table(
            vec![],
            vec![],
            vec![mr(1, "work", "work", "opened")],
            vec![],
        );
        let value = row(&table, &node, None);
        assert_eq!(value["ci"], Value::Null);
        assert_eq!(value["approvals"], 0);
        assert_eq!(value["unresolved"], 0);
        assert_eq!(value["ready"], false);
    }

    #[test]
    fn snapshot_tree_exposes_terminal_mr_and_merge_state() {
        for state in ["opened", "merged", "closed"] {
            let table = table(
                vec![task("work", &["DEMO-1"])],
                vec![],
                vec![mr(1, "DEMO-1-a", "a", state)],
                vec![tree("a", "DEMO-1-a")],
            );
            let roots = view::build(
                &table,
                &view::Options {
                    all: true,
                    active_loose: true,
                    ..Default::default()
                },
            );
            let task = row(&table, &roots[0], None);
            let tree = task["children"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["kind"] == "tree")
                .unwrap();
            assert_eq!(tree["mr_state"], state);
            assert_eq!(tree["merged"], state != "opened");
            assert_eq!(tree["children"][0]["kind"], "mr");
            assert_eq!(tree["children"][0]["state"], state);
        }
        let mut dirty = tree("loose", "DEMO-2-a");
        dirty.changes = Some(1);
        let loose = table(
            vec![],
            vec![],
            vec![mr(2, "DEMO-2-a", "merged", "merged")],
            vec![dirty],
        );
        let roots = view::build(
            &loose,
            &view::Options {
                all: true,
                active_loose: true,
                ..Default::default()
            },
        );
        let ticket = row(&loose, &roots[1], None);
        assert_eq!(ticket["children"][0]["kind"], "tree");
        assert_eq!(ticket["children"][0]["children"][0]["state"], "merged");
        let table = table(vec![], vec![], vec![], vec![tree("a", "misc")]);
        let roots = view::build(&table, &view::Options::default());
        let tree = row(&table, &roots[1], None);
        assert_eq!(tree["mr_state"], Value::Null);
        assert_eq!(tree["merged"], false);
    }
    #[test]
    fn snapshots_keep_all_closed_sessions_and_exclude_live_rollout_aliases() {
        use crate::client::{herdr::AgentSession, sessions::Session};
        use crate::table::fixtures;
        let mut input = fixtures::inputs(
            vec![task("task", &["DEMO-1"])],
            vec![],
            vec![],
            vec![tree("a", "DEMO-1-a"), tree("b", "DEMO-1-b")],
        );
        for (id, tree, modified) in [
            ("old", "a", 1),
            ("new", "a", 2),
            ("newest", "a", 3),
            ("fork", "a", 4),
            ("other", "b", 5),
        ] {
            let path = format!("{}/{}", fixtures::ROOT, tree);
            input.sessions.insert(
                format!("codex:{id}"),
                Session {
                    id: format!("codex:{id}"),
                    date: "2026-11-09".into(),
                    cwds: vec![(path.clone(), 1)],
                    first_cwd: Some(path.clone()),
                    last_cwd: Some(path),
                    modified,
                    created_ms: 0,
                },
            );
        }
        let mut active = fixtures::agent("active", "a", "space");
        active.extra.insert("agent".into(), json!("codex"));
        active
            .extra
            .insert("session_aliases".into(), json!(["fork"]));
        active.agent_session = Some(AgentSession {
            value: "old".into(),
        });
        input.agents.push(active);
        let table = fixtures::assemble(input);
        let history = stopped_by_tree(&table);
        let a = PathBuf::from(format!("{}/a", fixtures::ROOT));
        let b = PathBuf::from(format!("{}/b", fixtures::ROOT));
        assert_eq!(
            history.stopped[&a]
                .iter()
                .map(|row| row["session"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["codex:newest", "codex:new"]
        );
        assert_eq!(history.stopped[&b][0]["session"], "codex:other");
        assert_eq!(
            session_terminal(&table, "codex:fork"),
            Some("space:active".into())
        );
        let roots = view::build(
            &table,
            &view::Options {
                all: true,
                ..Default::default()
            },
        );
        let value = row(&table, &roots[0], None);
        let tree = value["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["tree"] == "a")
            .unwrap();
        let agents: Vec<_> = tree["children"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["kind"] == "agent")
            .collect();
        assert_eq!(agents.len(), 3);
        assert!(
            agents
                .iter()
                .all(|r| r["cwd"] == json!(a) && r["task"] == "task")
        );
        assert_eq!(agents.iter().filter(|r| r["state"] == "stopped").count(), 2);
    }

    #[test]
    fn a_fresh_idle_agent_is_bound_by_birth_not_a_newer_closed_conversation() {
        use crate::client::sessions::Session;
        use crate::table::fixtures;
        let mut input = fixtures::inputs(vec![], vec![], vec![], vec![tree("a", "misc")]);
        let path = format!("{}/a", fixtures::ROOT);
        for (id, birth, modified) in [("live", 1100, 1), ("closed", 20_000, 999)] {
            input.sessions.insert(
                format!("codex:{id}"),
                Session {
                    id: format!("codex:{id}"),
                    date: "2026-11-09".into(),
                    cwds: vec![(path.clone(), 1)],
                    first_cwd: Some(path.clone()),
                    last_cwd: Some(path.clone()),
                    modified,
                    created_ms: birth,
                },
            );
        }
        let mut active = fixtures::agent("active", "a", "space");
        active.extra.insert("agent".into(), json!("codex"));
        active.extra.insert("created_ms".into(), json!(1000));
        input.agents.push(active);
        let table = fixtures::assemble(input);
        let history = stopped_by_tree(&table);
        assert_eq!(history.bindings["space:active"], "codex:live");
        assert_eq!(
            history.stopped[&PathBuf::from(path)][0]["session"],
            "codex:closed"
        );
    }
    #[test]
    fn a_delayed_unidentified_session_is_not_safe_to_resume_until_its_agent_closes() {
        use crate::client::sessions::Session;
        use crate::table::fixtures;
        let mut input = fixtures::inputs(vec![], vec![], vec![], vec![tree("a", "misc")]);
        let path = format!("{}/a", fixtures::ROOT);
        input.sessions.insert(
            "conversation".into(),
            Session {
                id: "conversation".into(),
                date: "2026-11-09".into(),
                cwds: vec![(path.clone(), 1)],
                first_cwd: Some(path.clone()),
                last_cwd: Some(path),
                modified: 300,
                created_ms: 300_000,
            },
        );
        let mut active = fixtures::agent("active", "a", "space");
        active.extra.insert("agent".into(), json!("claude"));
        active.extra.insert("created_ms".into(), json!(1000));
        input.agents.push(active);
        let mut table = fixtures::assemble(input);
        assert_eq!(session_terminal(&table, "conversation"), None);
        assert!(session_may_be_live(&table, "conversation"));
        let history = stopped_by_tree(&table);
        assert_eq!(
            history.stopped.values().next().unwrap()[0]["resume_blocked"],
            true
        );
        table.trees[0].agents.clear();
        assert!(!session_may_be_live(&table, "conversation"));
    }
}

#[cfg(test)]
mod github_tests {
    use super::*;
    use crate::client::gitlab::MrReview;
    use crate::table::fixtures::{mr, table};

    #[test]
    fn snapshot_github_review_and_cached_or_missing_data_cannot_report_ready() {
        let mut pr = mr(7, "same", "Synthetic pull", "opened");
        pr.mr.github = true;
        pr.project = "github.com/demo/repo".into();
        pr.mr.head_repo = Some(pr.project.clone());
        pr.mr.detailed_merge_status = Some("mergeable".into());
        pr.mr.review_decision = Some("APPROVED".into());
        pr.review = Some(MrReview {
            pipeline: Some("success".into()),
            approved_by: vec!["reviewer".into()],
            unresolved: 0,
        });
        let mut table = table(vec![], vec![], vec![pr], vec![]);
        let node = Node {
            id: "pr".into(),
            kind: Kind::Mr(0),
            children: vec![],
        };
        let value = row(&table, &node, None);
        assert_eq!(value["provider"], "github");
        assert_eq!(value["state"], "open");
        assert_eq!(value["label"], "#7  Synthetic pull");
        assert_eq!(value["ready"], true);
        table.github.fresh = table::Fresh::Cached;
        assert_eq!(row(&table, &node, None)["ready"], false);
        table.github.fresh = table::Fresh::Live;
        table.mrs[0].review = None;
        assert_eq!(row(&table, &node, None)["ready"], false);
        assert!(row(&table, &node, None)["ci"].is_null());
    }
}
