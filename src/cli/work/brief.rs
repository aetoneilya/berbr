//! The agent's brief and first prompt.

use super::common::err;
use crate::client::issue::Issue;
use crate::client::worktree;
use crate::vault::task::Task;
use std::path::{Path, PathBuf};

const DONE_DEFINITION: &str = "\
If you change code, you are done when:
- it compiles, and the tests of the affected modules ran and pass — include the output;
- nothing is committed or pushed: the person reviews the diff first;
- you end with a short summary: what changed, how it was checked, open questions.
If the task is ambiguous or touches more than a couple of files, start with a plan and agree on it.";

/// Reads the context first, then works on a supplied description or waits.
pub(super) fn agent_prompt(
    note: Option<&Path>,
    brief: &Path,
    description: Option<&str>,
) -> Option<String> {
    let mut context = Vec::new();
    if let Some(note) = note {
        context.push(format!("task note {}", note.display()));
    }
    if brief.exists() {
        context.push(format!("brief with the ticket {}", brief.display()));
    }
    let description = description.filter(|d| !d.trim().is_empty());
    if context.is_empty() {
        return description.map(str::to_string);
    }
    let mut prompt = format!("Context: {}. Read it", context.join(", "));
    if description.is_some() {
        // Keep the shell command on one line. The full description, including
        // newlines and quotes, is already in the brief's "What is needed".
        prompt.push_str(" first, then work on the task description in the brief.");
    } else {
        prompt.push_str(" and wait for my command.");
    }
    Some(prompt)
}

/// The agent's brief: task note, ticket text, what to do, definition of done.
pub(super) fn brief_text(
    tree: &Path,
    branch: &str,
    task: Option<&Task>,
    issue: Option<&Issue>,
    task_tickets: &[String],
    message: Option<&str>,
) -> String {
    let mut brief = String::new();
    match (task, issue) {
        (Some(task), _) => brief.push_str(&format!("# {}\n\n", task.display_name())),
        (None, Some(issue)) => brief.push_str(&format!("# {}: {}\n\n", issue.key, issue.summary)),
        (None, None) => {}
    }
    if let Some(task) = task {
        brief.push_str(&format!(
            "The task note is the shared context for all of the task's tickets — read it in full: {}\n",
            task.note.path.display()
        ));
    }
    brief.push_str(&format!(
        "You work in the worktree {}, branch {branch}.\n",
        tree.display()
    ));
    if let Some(issue) = issue {
        if let Some(url) = crate::client::issue::url(&issue.key) {
            brief.push_str(&format!("\nIssue: {url}\n"));
        }
        brief.push_str(&format!(
            "\n## {}: {}\n\n{}\n",
            issue.key,
            issue.summary,
            issue
                .description
                .as_deref()
                .unwrap_or("(no description)")
                .trim()
        ));
    }
    let others: Vec<&String> = task_tickets
        .iter()
        .filter(|k| issue.is_none_or(|i| &i.key != *k))
        .collect();
    if !others.is_empty() {
        let list: Vec<&str> = others.iter().map(|k| k.as_str()).collect();
        brief.push_str(&format!(
            "\nOther tickets of the task: {}.\n",
            list.join(", ")
        ));
    }
    let todo = match (message, issue) {
        (Some(m), _) => m,
        (None, Some(_)) => "See the ticket above.",
        (None, None) => "See the task note.",
    };
    brief.push_str(&format!("\n## What is needed\n\n{todo}\n"));
    if task.is_some() {
        brief.push_str(
            "\n## Findings\n\nRecord decisions and non-obvious findings — not progress — with \
             `berbr note \"<text>\"` from the worktree: it goes to the \"Agent notes\" section of \
             the task note, which agents on the task's other tickets read.\n",
        );
    }
    if let Some(task) = task {
        let id = crate::client::herdr::shell_quote(&task.name);
        let command = task
            .note
            .string("project")
            .map(|project| {
                format!(
                    "berbr -p {} task",
                    crate::client::herdr::shell_quote(&project)
                )
            })
            .unwrap_or_else(|| "berbr task".into());
        brief.push_str(&format!(
            "\n## Task tools\n\nThe stable task ID is {id}; its title is separate from its identity. \
             After the first substantive user message (not the startup context prompt), \
             record a concise task description and, if absent, a title in that message's language. \
             Do this before implementation, even when you need to agree on a plan first. \
             Use `{command} show {id}` to inspect it, then \
             `{command} update {id} --fill-empty --title '<concise title>' --description '<task description>'`. \
             For multiline text use `--description-file <UTF-8 file>` or `--description-file -` (stdin). \
             Fill missing fields only; preserve user-supplied titles, description and agent findings. \
             Use the tools to update the note; keep its file, workspace, branch and worktree names stable.\n"
        ));
        brief.push_str(&format!("For findings outside a recognised worktree/workspace, use `berbr note --task {id} '<finding>'` with this project selected.\n"));
    }
    brief.push_str(&format!("\n## Done when\n\n{DONE_DEFINITION}\n"));
    brief
}

/// The brief goes to the worktree's git dir: never committed, removed with the
/// worktree. herdr can't pass multi-line arguments, so the prompt points at it.
pub(super) fn write_brief(tree: &Path, brief: &str) -> Result<PathBuf, String> {
    let path = brief_path(tree)?;
    std::fs::write(&path, brief).map_err(err)?;
    Ok(path)
}

/// Where `write_brief` puts the brief of the worktree `tree`.
pub(super) fn brief_path(tree: &Path) -> Result<PathBuf, String> {
    Ok(worktree::git_dir(tree).map_err(err)?.join("task.md"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_starts_work_but_an_empty_description_still_waits() {
        let dir = crate::table::fixtures::temp_dir();
        let brief = dir.join("task.md");
        std::fs::write(&brief, "## What is needed\n\nFix 'quotes'\nand newlines\n").unwrap();
        let note = dir.join("note.md");
        let prompt = agent_prompt(Some(&note), &brief, Some("Fix 'quotes'\nand newlines")).unwrap();
        assert!(prompt.contains("work on the task description in the brief"));
        assert!(!prompt.contains("wait for my command"));
        assert!(!prompt.contains('\n'));
        for description in [None, Some("  \n")] {
            assert!(
                agent_prompt(Some(&note), &brief, description)
                    .unwrap()
                    .ends_with("wait for my command.")
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
