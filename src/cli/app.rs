//! Stable, read-only data boundary between the Rust model and the native mod.
use crate::client::gitlab::MrState;
use crate::config::Config;
use crate::table::{
    self, Table, look,
    view::{self, Kind, Node},
};
use serde_json::{Value, json};

pub(super) fn snapshot(projects: &[Config], chosen: usize, cached: bool) -> Result<(), String> {
    if std::env::var("BERBR_BACKEND").is_ok_and(|s| s == "herdr-app") {
        // The normal table tolerates a missing multiplexer. The app board must
        // surface a broken backend instead of making every live agent disappear.
        crate::client::herdr::list_workspaces().map_err(|e| e.to_string())?;
    }
    let config = &projects[chosen];
    let tasks = super::load(config)?;
    let table = if cached {
        table::build_cached(config, tasks)
    } else {
        table::build(config, tasks, true)
    };
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
            value["body"] = json!(r.task.note.body());
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
