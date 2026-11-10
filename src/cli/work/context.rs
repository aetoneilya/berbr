//! Local agent context: no remote services, live Herdr queries or cache writes.
use super::common::{err, find_task};
use crate::client::worktree;
use crate::config::Config;
use crate::vault::task::Task;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Shared with note/show/update: absolute bindings win over basename bindings.
pub(super) fn bound_tasks(tasks: &[&Task], root: &Path, roots: &[PathBuf]) -> (Vec<usize>, bool) {
    let root = canonical(root);
    for absolute in [true, false] {
        let matches: Vec<_> = tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.done)
            .filter_map(|(i, t)| {
                t.note
                    .strings("worktrees")
                    .into_iter()
                    .chain(t.note.strings_at(&["bindings", "worktrees"]))
                    .any(|reference| {
                        let path = Path::new(&reference);
                        if absolute {
                            path.is_absolute() && canonical(path) == root
                        } else {
                            !path.is_absolute()
                                && root.file_name().and_then(|n| n.to_str())
                                    == Some(reference.as_str())
                        }
                    })
                    .then_some(i)
            })
            .collect();
        if !matches.is_empty() {
            let ambiguous = !absolute && duplicate_name(&root, roots);
            return (matches, ambiguous);
        }
    }
    (Vec::new(), false)
}

fn duplicate_name(root: &Path, roots: &[PathBuf]) -> bool {
    let mut matches: Vec<_> = roots
        .iter()
        .filter(|p| p.file_name() == root.file_name())
        .map(|p| canonical(p))
        .collect();
    matches.sort();
    matches.dedup();
    matches.len() > 1
}

fn inferred(task: &Task, root: &Path, branch: Option<&str>, roots: &[PathBuf]) -> bool {
    let unique_name = !duplicate_name(root, roots);
    task.sessions.iter().any(|s| {
        if Path::new(&s.worktree).is_absolute() {
            canonical(Path::new(&s.worktree)) == canonical(root)
        } else {
            unique_name && s.matches_tree(root)
        }
    }) || (unique_name
        && task.note.string("id").as_deref() == Some(task.name.as_str())
        && root.file_name().and_then(|n| n.to_str()) == Some(task.name.as_str()))
        || branch.is_some_and(|branch| {
            crate::vault::task::ticket_keys(branch)
                .iter()
                .any(|k| task.tickets.contains(k))
        })
}

fn task_json(task: &Task) -> Value {
    json!({"id":task.name,"title":task.title(),"note":task.note.path,"done":task.done})
}

pub(in crate::cli) fn cli_context(
    projects: &[Config],
    project: Option<&str>,
    target: Option<&str>,
    as_json: bool,
) -> Result<(), String> {
    let cwd = canonical(&std::env::current_dir().map_err(err)?);
    let root = worktree::worktree_root(&cwd).ok().map(|p| canonical(&p));
    let main = worktree::main_checkout(&cwd).ok().map(|p| canonical(&p));
    let branch = root
        .as_deref()
        .and_then(|p| worktree::current_branch(p).ok());
    let matches: Vec<_> = if let Some(name) = project {
        let i = projects
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("no project {name}"))?;
        vec![i]
    } else {
        let exact: Vec<_> = projects
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                p.repos.iter().any(|r| {
                    main.as_ref().is_some_and(|main| {
                        worktree::main_checkout(&r.path)
                            .ok()
                            .is_some_and(|p| canonical(&p) == *main)
                    })
                })
            })
            .map(|(i, _)| i)
            .collect();
        if !exact.is_empty() {
            exact
        } else {
            let containing: Vec<_> = projects
                .iter()
                .enumerate()
                .filter_map(|(i, p)| {
                    p.repos
                        .iter()
                        .map(|r| canonical(&r.path))
                        .filter(|p| cwd.starts_with(p))
                        .map(|p| p.components().count())
                        .max()
                        .map(|n| (i, n))
                })
                .collect();
            let longest = containing.iter().map(|(_, n)| n).max();
            containing
                .iter()
                .filter(|(_, n)| Some(n) == longest)
                .map(|(i, _)| *i)
                .collect()
        }
    };
    let mut output = json!({
        "version":1, "status":"no_project", "project":null,
        "project_candidates":matches.iter().map(|&i| projects[i].name.as_str()).collect::<Vec<_>>(),
        "cwd":cwd, "repo":main, "worktree":root, "branch":branch, "in_project":false,
        "task":null, "brief":null, "source":null, "candidates":[], "warnings":[],
    });
    let [i] = matches.as_slice() else {
        if matches.is_empty() {
            output["project_candidates"] =
                json!(projects.iter().map(|p| p.name.as_str()).collect::<Vec<_>>());
        } else {
            output["status"] = json!("ambiguous_project");
        }
        return print_context(output, as_json);
    };
    let config = &projects[*i];
    output["project"] = json!(config.name);
    let (tasks, skipped) = crate::vault::task::load(config)?;
    let mut warnings: Vec<_> = skipped
        .into_iter()
        .map(|e| format!("skipped {e}"))
        .collect();
    let mut roots = Vec::new();
    for repo in &config.repos {
        match worktree::main_checkout(&repo.path) {
            Ok(main) => {
                roots.push(canonical(&main));
                match worktree::linked_worktrees(&main) {
                    Ok(trees) => roots.extend(trees.into_iter().map(|t| canonical(&t.path))),
                    Err(e) => warnings.push(e.to_string()),
                }
            }
            Err(e) => warnings.push(e.to_string()),
        }
    }
    roots.sort();
    roots.dedup();
    let in_project = root.as_ref().is_some_and(|p| roots.contains(p));
    output["in_project"] = json!(in_project);
    let refs: Vec<_> = tasks.iter().collect();
    let (mut candidates, mut ambiguous_name, mut source) = (Vec::new(), false, "none");
    if let Some(target) = target {
        match find_task(&tasks, target, true) {
            Ok(Some(i)) => {
                candidates.push(i);
                source = "explicit";
            }
            Ok(None) => warnings.push(format!("no task {target}")),
            Err(e) => {
                warnings.push(e);
                ambiguous_name = true;
            }
        }
    } else {
        if let Some(root) = root.as_deref().filter(|_| in_project) {
            (candidates, ambiguous_name) = bound_tasks(&refs, root, &roots);
            if !candidates.is_empty() {
                source = "binding";
            } else {
                candidates = tasks
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| !t.done && inferred(t, root, branch.as_deref(), &roots))
                    .map(|(i, _)| i)
                    .collect();
                if !candidates.is_empty() {
                    source = "stored_reference";
                }
            }
        }
        if candidates.is_empty() && !ambiguous_name {
            if let Ok(space) = std::env::var("HERDR_WORKSPACE_ID") {
                candidates = tasks
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| {
                        !t.done && t.note.string("space").as_deref() == Some(space.as_str())
                    })
                    .map(|(i, _)| i)
                    .collect();
                if !candidates.is_empty() {
                    source = "saved_workspace";
                }
            }
        }
    }
    if candidates.len() == 1 && !ambiguous_name {
        let task = &tasks[candidates[0]];
        output["status"] = json!("resolved");
        output["task"] = task_json(task);
        // Do not return another checkout's brief when --task selects a remote context.
        if let Some(root) = root.as_deref().filter(|_| in_project) {
            let (bound, ambiguous) = bound_tasks(&refs, root, &roots);
            let owns = if bound.is_empty() {
                let owners: Vec<_> = tasks
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| !t.done && inferred(t, root, branch.as_deref(), &roots))
                    .map(|(i, _)| i)
                    .collect();
                owners == candidates
            } else {
                !ambiguous && bound == candidates
            };
            if owns {
                let path = worktree::git_dir(root)
                    .ok()
                    .map(|p| p.join("task.md"))
                    .filter(|p| super::brief::belongs_to_task(p, task));
                let cache_brief = crate::config::cache_dir()
                    .join("briefs")
                    .join(format!("{}.md", task.name.replace('/', "-")));
                output["brief"] = json!(path.or_else(|| {
                    (main.as_deref() == Some(root)
                        && super::brief::belongs_to_task(&cache_brief, task))
                    .then_some(cache_brief)
                }));
            }
        }
    } else {
        output["status"] = json!(if ambiguous_name || candidates.len() > 1 {
            "ambiguous"
        } else {
            "unbound"
        });
    }
    if ambiguous_name {
        warnings.push("ambiguous task or worktree name; pass -p <project> and --task <id>".into());
    }
    output["source"] = json!(source);
    output["candidates"] = if candidates.is_empty() {
        json!(
            tasks
                .iter()
                .filter(|t| !t.done)
                .map(task_json)
                .collect::<Vec<_>>()
        )
    } else {
        json!(
            candidates
                .iter()
                .map(|&i| task_json(&tasks[i]))
                .collect::<Vec<_>>()
        )
    };
    output["warnings"] = json!(warnings);
    print_context(output, as_json)
}

fn print_context(value: Value, as_json: bool) -> Result<(), String> {
    if as_json {
        println!("{value}");
    } else {
        println!("context: {}", value["status"].as_str().unwrap_or("unknown"));
        if value["project"].is_null() {
            for project in value["project_candidates"].as_array().into_iter().flatten() {
                println!("project candidate: {}", project.as_str().unwrap_or(""));
            }
        }
        for field in ["project", "worktree", "branch", "brief"] {
            if let Some(s) = value[field].as_str() {
                println!("{field}: {s}");
            }
        }
        if let Some(id) = value["task"]["id"].as_str() {
            println!("task: {id}");
        } else {
            for task in value["candidates"].as_array().into_iter().flatten() {
                println!(
                    "candidate: {}  {}",
                    task["id"].as_str().unwrap_or(""),
                    task["title"].as_str().unwrap_or("")
                );
            }
            println!("Choose -p <project> and --task <id>; task list --json lists local IDs.");
        }
        for warning in value["warnings"].as_array().into_iter().flatten() {
            eprintln!("{}", warning.as_str().unwrap_or(""));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::fixtures::task_note;

    #[test]
    fn absolute_binding_wins_and_duplicate_basenames_do_not_choose_a_task() {
        let first = task_note("first", "id: first\nworktrees: [parser]", "2026-11-01");
        let second = task_note(
            "second",
            "id: second\nworktrees: [/home/dev/two/parser]",
            "2026-11-01",
        );
        let roots = vec![
            PathBuf::from("/home/dev/one/parser"),
            PathBuf::from("/home/dev/two/parser"),
        ];
        assert_eq!(
            bound_tasks(&[&first, &second], &roots[0], &roots),
            (vec![0], true)
        );
        assert_eq!(
            bound_tasks(&[&first, &second], &roots[1], &roots),
            (vec![1], false)
        );
    }
}
