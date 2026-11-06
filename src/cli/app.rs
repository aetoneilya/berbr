//! Stable, read-only data boundary between the Rust model and the native mod.
use crate::client::gitlab::MrState;
use crate::config::Config;
use crate::table::{
    self, Table, look,
    view::{self, Kind, Node},
};
use serde_json::{Value, json};

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
    let rows: Vec<_> = roots.iter().map(|n| row(&table, n, None)).collect();
    println!(
        "{}",
        json!({
            "version": 3, "project": config.name,
            "projects": projects.iter().map(|c| json!({"name":c.name,"cwd":c.repos.first().map(|r|&r.path),"agent":c.workflow.agent})).collect::<Vec<_>>(),
            "rows": rows, "warnings": table.warnings,
            "sources": {"jira":format!("{:?}",table.jira.fresh),"gitlab":format!("{:?}",table.gitlab.fresh)}
        })
    );
    Ok(())
}

fn row(table: &Table, node: &Node, owner: Option<&str>) -> Value {
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
            value["label"] = json!(format!("!{}  {}", m.mr.iid(), look::mr_title(m)));
            value["state"] = json!(m.mr.state().as_str());
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
                    !m.mr.draft
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
            value["space"] = json!(a.workspace);
            value["tab"] = json!(a.tab);
        }
        Kind::Header(section) => {
            value["kind"] = json!("header");
            value["label"] = json!(section.title());
        }
        Kind::NewTask => {
            value["kind"] = json!("new");
        }
    }
    let owner = value["task"].as_str();
    let children: Vec<_> = node.children.iter().map(|n| row(table, n, owner)).collect();
    value["children"] = json!(children);
    value
}

fn ticket(table: &Table, value: &mut Value, key: &str) {
    value["kind"] = json!("ticket");
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
            workflow: WorkflowConfig::default(),
            repos: vec![],
            vault: VaultConfig {
                path: dir.clone(),
                task_dirs: vec!["tasks".into()],
                new_task_dir: "tasks".into(),
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
}
