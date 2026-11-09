//! Everything the table is built from, read from Jira, GitLab, git, herdr and
//! agent transcripts; as much of it at once as the dependencies allow.

use super::matching::Inputs;
use super::{Fresh, Mr, Source, Ticket, Tree};
use crate::client::gitlab::GitLab;
use crate::client::jira::{JiraClient, JiraIssue};
use crate::client::worktree::{self, Worktree};
use crate::client::{herdr, joined, parallel, sessions};
use crate::config::Config;
use crate::trace;
use crate::vault::task::{Task, ticket_keys};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Merged and closed MRs older than this are not fetched.
const MR_LOOKBACK_DAYS: i64 = 90;
/// Requests to GitLab or Jira at once: enough to hide latency (a page of open
/// MRs in one round), few enough not to flood them.
const REQUESTS: usize = 16;
/// git runs at once in big worktrees: the disk is the limit.
const GIT_RUNS: usize = 8;

/// Tickets by key, or why Jira did not give them.
type Tickets = Result<HashMap<String, Ticket>, String>;

/// What Jira and GitLab said last time, kept between runs.
#[derive(serde::Serialize, serde::Deserialize)]
struct RemoteCache {
    tickets: HashMap<String, Ticket>,
    mrs: Vec<Mr>,
}

impl RemoteCache {
    /// One per project.
    fn path(project: &str) -> PathBuf {
        crate::config::cache_dir().join(format!("remote-{project}.json"))
    }

    fn load(project: &str) -> Option<RemoteCache> {
        serde_json::from_slice(&std::fs::read(RemoteCache::path(project)).ok()?).ok()
    }

    fn save(project: &str, tickets: &HashMap<String, Ticket>, mrs: &[Mr]) {
        #[derive(serde::Serialize)]
        struct Ref<'a> {
            tickets: &'a HashMap<String, Ticket>,
            mrs: &'a [Mr],
        }
        let path = RemoteCache::path(project);
        let tmp = path.with_extension("json.tmp");
        if let Ok(json) = serde_json::to_vec(&Ref { tickets, mrs })
            && std::fs::write(&tmp, json).is_ok()
        {
            let _ = std::fs::rename(tmp, path);
        }
    }
}

pub(super) struct Options {
    pub online: bool,
    pub cached: bool,
    pub persist: bool,
    pub with_diff: bool,
    pub closing: Option<String>,
}

struct Repository {
    main: PathBuf,
    project: Option<String>,
    trees: Vec<Worktree>,
    refs: Option<worktree::Refs>,
    default: Option<String>,
}

fn repositories(config: &Config, refs: bool) -> (Vec<Repository>, Vec<String>) {
    let mut found = Vec::new();
    let mut warnings = Vec::new();
    for repo in &config.repos {
        let result = (|| {
            let main = worktree::main_checkout(&repo.path).map_err(|e| e.to_string())?;
            let trees = worktree::linked_worktrees(&main).map_err(|e| e.to_string())?;
            let refs = if refs {
                Some(worktree::Refs::read(&main).map_err(|e| e.to_string())?)
            } else {
                None
            };
            let default = refs
                .as_ref()
                .and_then(|r| r.default_branch(repo.main_branch.as_deref()));
            Ok::<_, String>(Repository {
                main,
                trees,
                refs,
                default,
                project: repo
                    .github_repo
                    .as_ref()
                    .or_else(|| config.github.as_ref().map(|g| &g.repo))
                    .filter(|_| repo.gitlab_project.is_none())
                    .map(|r| format!("github.com/{r}"))
                    .or_else(|| repo.gitlab_project.clone())
                    .or_else(|| config.gitlab.as_ref().map(|g| g.project.clone())),
            })
        })();
        match result {
            Ok(repo) => found.push(repo),
            Err(e) => warnings.push(format!("Git {}: {e}", repo.path.display())),
        }
    }
    (found, warnings)
}

struct ClosingScope {
    tickets: Vec<String>,
    branches: HashSet<(String, String)>,
    /// The task's own checkouts: only they need `git status` to close it.
    paths: HashSet<PathBuf>,
}

impl ClosingScope {
    fn new(task: &Task, trees: &[Tree], space: Option<&str>) -> Self {
        let owned = super::matching::task_trees(task, trees, space);
        let mut tickets = task.tickets.clone();
        for key in owned.iter().flat_map(|&i| &trees[i].keys) {
            if !tickets.contains(key) {
                tickets.push(key.clone());
            }
        }
        let branches = owned
            .iter()
            .filter_map(|&i| Some((trees[i].project.clone()?, trees[i].wt.branch.clone()?)))
            .collect();
        let paths = owned.iter().map(|&i| trees[i].wt.path.clone()).collect();
        Self {
            tickets,
            branches,
            paths,
        }
    }

    fn select(&self, mrs: Vec<Mr>) -> Vec<Mr> {
        let mut branches = self.branches.clone();
        branches.extend(
            mrs.iter()
                .filter(|m| m.keys.iter().any(|k| self.tickets.contains(k)))
                .map(|m| {
                    (
                        m.source_project().to_string(),
                        m.mr.source_branch().to_string(),
                    )
                }),
        );
        mrs.into_iter()
            .filter(|m| {
                m.keys.iter().any(|k| self.tickets.contains(k))
                    || branches.contains(&(
                        m.source_project().to_string(),
                        m.mr.source_branch().to_string(),
                    ))
            })
            .collect()
    }
}

/// `online: false` skips Jira and GitLab and git status. `cached`: what Jira
/// and GitLab said last time stands in for what they do not say now, offline or
/// failing.
pub(super) fn fetch(config: &Config, tasks: Vec<Task>, options: Options) -> Inputs {
    let Options {
        online,
        cached,
        persist,
        with_diff,
        closing,
    } = options;
    let (repos, repository_warnings) = repositories(config, online || with_diff);
    let _operation = herdr::operation();
    let mut roots: Vec<_> = repos
        .iter()
        .flat_map(|r| worktree::worktree_roots(&r.main, &config.workflow.worktree_dir))
        .collect();
    roots.extend(
        repos
            .iter()
            .flat_map(|r| &r.trees)
            .filter_map(|wt| wt.path.parent().map(|p| p.to_path_buf())),
    );
    let mut seen = HashSet::new();
    roots.retain(|root| seen.insert(root.clone()));
    let mains: Vec<_> = repos.iter().map(|r| r.main.clone()).collect();
    let early_herdr = closing.as_ref().map(|_| live_herdr());
    let mut raw_trees = collect_trees(&repos, false, false, None);
    let early_sessions = closing
        .as_ref()
        .map(|_| load_sessions(&roots, &mains, &raw_trees, &tasks, false));
    let closing = closing
        .as_deref()
        .and_then(|name| tasks.iter().find(|t| t.name == name))
        .map(|task| {
            let (agents, spaces, _) = early_herdr.as_ref().expect("closing inventory");
            for tree in &mut raw_trees {
                tree.agents = super::matching::agents_in(
                    &tree.wt.path,
                    agents,
                    early_sessions.as_ref().expect("closing sessions"),
                );
            }
            let space = crate::vault::task::task_space_in(task, spaces, &tasks)
                .ok()
                .flatten();
            ClosingScope::new(task, &raw_trees, space.map(|s| s.workspace_id.as_str()))
        });
    // a project without Jira or GitLab asks them nothing, as offline
    let jira = (online && config.jira.is_some()).then(|| config.jira_client());
    let gitlab = config
        .gitlab
        .as_ref()
        .filter(|_| online)
        .map(|g| GitLab::from_env(&g.host));
    // Jira answers in about a second, so every ticket query starts at once: mine,
    // the keys of open tasks and worktree branches, and those of open MRs as soon
    // as GitLab lists them
    let known: Vec<String> = if online {
        trace::time("known keys", || {
            closing
                .as_ref()
                .map_or_else(|| known_keys(&repos, &tasks), |scope| scope.tickets.clone())
        })
    } else {
        Vec::new()
    };
    let (mine, by_key, mrs, trees, herdr, github) = std::thread::scope(|scope| {
        let github = scope.spawn(|| {
            fetch_github(
                config,
                &repos,
                &known,
                online,
                cached,
                persist,
                closing.as_ref(),
            )
        });
        let mine = scope.spawn(|| match &jira {
            Some(jira) => trace::time("jira: mine", || {
                fetch_my_tickets(
                    jira.as_ref()?,
                    closing.as_ref().map(|s| s.tickets.as_slice()),
                )
            }),
            None => Ok(HashMap::new()),
        });
        let by_key = scope.spawn(|| match &jira {
            Some(Ok(jira)) => trace::time(&format!("jira: {} known keys", known.len()), || {
                fetch_tickets_by_key(jira, &known)
            }),
            _ => Ok(HashMap::new()),
        });
        let mrs = scope.spawn(|| match &gitlab {
            Some(gitlab) => fetch_mrs(
                config,
                gitlab.as_ref()?,
                jira.as_ref(),
                &known,
                closing.as_ref(),
            ),
            None => Ok((Vec::new(), Ok(HashMap::new()))),
        });
        let herdr = scope.spawn(herdr::inherit(move || {
            early_herdr.unwrap_or_else(|| trace::time("herdr", live_herdr))
        }));
        let trees = trace::time("git: worktrees", || {
            collect_trees(
                &repos,
                online,
                with_diff,
                closing.as_ref().map(|scope| &scope.paths),
            )
        });
        (
            joined(mine, "the fetch"),
            joined(by_key, "the fetch"),
            joined(mrs, "the fetch"),
            trees,
            herdr.join().unwrap_or_else(|_| {
                (
                    Vec::new(),
                    Vec::new(),
                    vec!["Herdr inventory panicked".into()],
                )
            }),
            github.join().unwrap_or_else(|_| {
                let warning = "GitHub fetch worker panicked".to_string();
                (
                    HashMap::new(),
                    Vec::new(),
                    Source {
                        fresh: Fresh::Missing,
                        warning: Some(warning.clone()),
                    },
                    vec![warning],
                )
            }),
        )
    });
    let (agents, workspaces, mut warnings) = herdr;
    refresh_stale_indexes(&trees, &agents);
    warnings.extend(repository_warnings);
    let (mrs, of_mrs) = match mrs {
        Ok((mrs, tickets)) => (Ok(mrs), tickets),
        Err(err) => (Err(err), Ok(HashMap::new())),
    };
    // Jira references in GitHub PRs follow the same contract as GitLab MR keys.
    // Only newly discovered keys need this extra batch after the parallel fetch.
    let github_jira_keys: Vec<_> = github
        .1
        .iter()
        .flat_map(|m| m.keys.clone())
        .filter(|key| crate::client::issue::github_parts(key).is_none() && !known.contains(key))
        .collect();
    let by_key = match &jira {
        Some(Ok(client)) => by_key.and_then(|mut tickets| {
            tickets.extend(fetch_new_jira_tickets(
                client,
                &github_jira_keys,
                closing.is_some(),
            )?);
            Ok(tickets)
        }),
        _ => by_key,
    };
    let tickets = all_tickets(mine, by_key, of_mrs);

    let complete = online && tickets.is_ok() && mrs.is_ok();
    let mut cache = (cached && !complete)
        .then(|| RemoteCache::load(&config.name))
        .flatten();
    let (mut tickets, jira) = or_cached(
        "Jira",
        online.then_some(tickets),
        cache.as_mut().map(|c| std::mem::take(&mut c.tickets)),
        &mut warnings,
    );
    let (mut mrs, gitlab) = or_cached(
        "GitLab",
        online.then_some(mrs),
        cache.map(|c| c.mrs),
        &mut warnings,
    );
    if complete && persist {
        RemoteCache::save(&config.name, &tickets, &mrs);
    }

    let (github_tickets, github_mrs, github, github_warnings) = github;
    let complete = complete && (!config.has_github() || github.fresh == Fresh::Live);
    tickets.extend(github_tickets);
    mrs.extend(github_mrs);
    warnings.extend(github_warnings);

    let sessions = early_sessions.unwrap_or_else(|| {
        trace::time("sessions", || {
            load_sessions(&roots, &mains, &trees, &tasks, persist)
        })
    });
    Inputs {
        mains,
        tasks,
        tickets,
        mrs,
        trees,
        agents,
        workspaces,
        sessions,
        roots,
        complete,
        jira,
        gitlab,
        github,
        warnings,
        jira_host: config
            .jira
            .as_ref()
            .map(|j| j.host.clone())
            .unwrap_or_default(),
    }
}

fn fetch_github(
    config: &Config,
    repos: &[Repository],
    known: &[String],
    online: bool,
    cached: bool,
    persist: bool,
    closing: Option<&ClosingScope>,
) -> (HashMap<String, Ticket>, Vec<Mr>, Source, Vec<String>) {
    use std::hash::{Hash, Hasher};
    let configured = config.github_repos();
    if configured.is_empty() {
        return (
            HashMap::new(),
            Vec::new(),
            Source {
                fresh: Fresh::Live,
                warning: None,
            },
            Vec::new(),
        );
    }
    // Configuration changes must not reuse data from another set of repositories.
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    configured.hash(&mut hash);
    let cache_name = format!("{}-github-{:x}", config.name, hash.finish());
    let mut warnings = Vec::new();
    let fetched = online.then(|| {
        let github = crate::client::github::GitHub::from_env()?;
        let mut branches: HashMap<String, HashSet<String>> = HashMap::new();
        for repo in repos {
            if let Some(project) = repo
                .project
                .as_deref()
                .and_then(|p| p.strip_prefix("github.com/"))
            {
                branches
                    .entry(project.into())
                    .or_default()
                    .extend(repo.trees.iter().filter_map(|t| t.branch.clone()));
            }
        }
        let mut mrs = github.pulls(&configured, &branches)?;
        if let Some(scope) = closing {
            mrs = scope.select(mrs);
        }
        warnings.extend(github.reviews(&mut mrs));
        let keys: Vec<_> = known
            .iter()
            .cloned()
            .chain(mrs.iter().flat_map(|m| m.keys.clone()))
            .filter(|key| {
                crate::client::issue::github_parts(key)
                    .is_some_and(|(r, _)| configured.iter().any(|c| c == r))
            })
            .collect();
        let tickets = github.tickets(&configured, known, &keys)?;
        confirmed_issue_references(&mut mrs, &tickets);
        Ok::<_, String>(RemoteCache { tickets, mrs })
    });
    let cache = cached.then(|| RemoteCache::load(&cache_name)).flatten();
    let fallback = cache.map(|c| (c.tickets, c.mrs));
    let fetched = fetched.map(|r| r.map(|c| (c.tickets, c.mrs)));
    let ((tickets, mrs), source) = or_cached("GitHub", fetched, fallback, &mut warnings);
    if persist && online && source.fresh == Fresh::Live {
        RemoteCache::save(&cache_name, &tickets, &mrs);
    }
    (tickets, mrs, source, warnings)
}

/// GitHub numbers are shared by issues and PRs. Only API-confirmed issues may
/// enter task notes through automatic discovery; otherwise a PR reference would
/// become a required issue on the next refresh and poison the source.
fn confirmed_issue_references(mrs: &mut [Mr], tickets: &HashMap<String, Ticket>) {
    for mr in mrs {
        mr.keys.retain(|key| {
            crate::client::issue::github_parts(key).is_none() || tickets.contains_key(key)
        });
    }
}

fn branch_keys(project: Option<&str>, branch: Option<&str>) -> Vec<String> {
    let Some(branch) = branch else {
        return Vec::new();
    };
    // A fork checkout cannot resolve a bare issue number to the target repo.
    // Persisted task/worktree bindings and qualified PR references provide ownership.
    if project.is_some_and(|p| p.starts_with("github.com/")) {
        return crate::vault::task::jira_keys(branch);
    }
    ticket_keys(branch)
}

/// Every ticket Jira was asked for, or why not all of them: a ticket missing for
/// a failed query would read as unknown, and so would the state of its task.
/// `mine` goes last: the same ticket fetched by key is not marked as mine.
fn all_tickets(mine: Tickets, by_key: Tickets, of_mrs: Tickets) -> Tickets {
    let mine = mine?;
    let mut tickets = by_key?;
    tickets.extend(of_mrs?);
    tickets.extend(mine);
    Ok(tickets)
}

/// A service that did not answer is shown as it was last time, not as empty,
/// with a warning either way; one not asked (`fetched: None`) as it was last
/// time, without one.
fn or_cached<T: Default>(
    service: &str,
    fetched: Option<Result<T, String>>,
    cached: Option<T>,
    warnings: &mut Vec<String>,
) -> (T, Source) {
    let fresh = if cached.is_some() {
        Fresh::Cached
    } else {
        Fresh::Missing
    };
    let warning = match fetched {
        Some(Ok(value)) => {
            let source = Source {
                fresh: Fresh::Live,
                warning: None,
            };
            return (value, source);
        }
        Some(Err(err)) => {
            let shown = if cached.is_some() {
                " — showing cached"
            } else {
                ""
            };
            let warning = format!("{service}: {err}{shown}");
            warnings.push(warning.clone());
            Some(warning)
        }
        None => None,
    };
    (cached.unwrap_or_default(), Source { fresh, warning })
}

/// Agent sessions launched in the main checkouts, in current worktrees and in
/// worktrees open tasks already list sessions for, by id.
fn load_sessions(
    roots: &[PathBuf],
    mains: &[PathBuf],
    trees: &[Tree],
    tasks: &[Task],
    persist: bool,
) -> HashMap<String, sessions::Session> {
    let mut dirs = mains.to_vec();
    dirs.extend(trees.iter().map(|t| t.wt.path.clone()));
    for task in tasks.iter().filter(|t| !t.done) {
        for known in &task.sessions {
            dirs.extend(roots.iter().map(|r| r.join(&known.worktree)));
        }
    }
    dirs.sort();
    dirs.dedup();

    let mut cache = sessions::Cache::load();
    let mut found = HashMap::new();
    for dir in dirs {
        for session in sessions::sessions_in(&dir, &mut cache) {
            found.insert(session.id.clone(), session);
        }
    }
    if persist {
        cache.save();
    }
    found
}

/// Ticket keys of open tasks and of worktree branches: `git worktree list` is
/// instant, unlike the worktrees' status.
fn known_keys(repos: &[Repository], tasks: &[Task]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let branches: Vec<_> = repos
        .iter()
        .flat_map(|r| &r.trees)
        .filter_map(|wt| wt.branch.as_deref())
        .collect();
    let task_keys = tasks
        .iter()
        .filter(|t| !t.done)
        .flat_map(|t| t.tickets.clone());
    for key in task_keys.chain(branches.iter().flat_map(|b| ticket_keys(b))) {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

/// My tickets that are not done (resolution alone misses Declined/Cancelled).
fn fetch_my_tickets(jira: &JiraClient, keys: Option<&[String]>) -> Tickets {
    let filtered = keys.map(|keys| {
        keys.iter()
            .filter(|k| crate::client::issue::github_parts(k).is_none())
            .cloned()
            .collect::<Vec<_>>()
    });
    let keys = filtered.as_deref();
    if keys.is_some_and(|keys| keys.is_empty()) {
        return Ok(HashMap::new());
    }
    let query = keys.map_or_else(
        || "assignee = currentUser() AND statusCategory != Done".to_string(),
        |keys| format!("assignee = currentUser() AND key in ({})", keys.join(",")),
    );
    let issues = jira
        .search_issues_brief(&query, keys.map_or(usize::MAX, |keys| keys.len()))
        .map_err(|e| e.to_string())?;
    Ok(issues
        .iter()
        .map(|issue| (issue.key.clone(), Ticket::from_issue(issue, true)))
        .collect())
}

/// The tickets of `keys` that exist; keys Jira does not know are left out, but a
/// query that fails otherwise fails them all.
fn fetch_tickets_by_key(jira: &JiraClient, keys: &[String]) -> Tickets {
    let keys: Vec<String> = keys
        .iter()
        .filter(|k| crate::client::issue::github_parts(k).is_none())
        .cloned()
        .collect();
    let keys = keys.as_slice();
    let mut tickets = HashMap::new();
    for chunk in keys.chunks(100) {
        let jql = format!("key in ({})", chunk.join(","));
        let issues = match jira.search_issues_brief(&jql, chunk.len()) {
            Ok(issues) => issues,
            // one unknown key fails the whole query: fall back to one request per
            // key, all at once — but not when Jira is down, that would wait out
            // every key
            Err(err) if err.bad_request() => {
                parallel(chunk, REQUESTS, |key| existing(jira.get_issue_by_key(key)))
                    .into_iter()
                    .map(|r| r.unwrap_or_else(|| Err("a Jira request panicked".to_string())))
                    .collect::<Result<Vec<_>, _>>()?
            }
            .into_iter()
            .flatten()
            .collect(),
            Err(err) => return Err(err.to_string()),
        };
        let mut issues = issues;
        for key in chunk
            .iter()
            .filter(|key| !issues.iter().any(|i| &i.key == *key))
            .cloned()
            .collect::<Vec<_>>()
        {
            match existing(jira.get_issue_by_key(&key))? {
                Some(issue) => issues.push(issue),
                None => {
                    return Err(format!(
                        "incomplete Jira data: {key} was not returned or is inaccessible"
                    ));
                }
            }
        }
        for issue in &issues {
            tickets.insert(issue.key.clone(), Ticket::from_issue(issue, false));
        }
    }
    require_known_keys(keys, &tickets)?;
    Ok(tickets)
}

fn require_known_keys(keys: &[String], tickets: &HashMap<String, Ticket>) -> Result<(), String> {
    let missing: Vec<_> = keys
        .iter()
        .filter(|key| !tickets.contains_key(*key))
        .cloned()
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "incomplete Jira data: missing {}",
            missing.join(", ")
        ))
    }
}

/// New references discovered after the initial Jira queries need an ownership
/// query too when closing; the initial "mine" query was scoped to known keys.
fn fetch_new_jira_tickets(jira: &JiraClient, keys: &[String], closing: bool) -> Tickets {
    let mut tickets = fetch_tickets_by_key(jira, keys)?;
    if closing {
        tickets.extend(fetch_my_tickets(jira, Some(keys))?);
    }
    Ok(tickets)
}

/// The issue Jira answered with, None for a key it does not know (404, or 400).
fn existing(answer: crate::client::jira::Result<JiraIssue>) -> Result<Option<JiraIssue>, String> {
    match answer {
        Ok(issue) => Ok(Some(issue)),
        Err(err) if err.bad_request() || err.not_found() => Ok(None),
        Err(err) => Err(err.to_string()),
    }
}

/// My MRs of every configured project. The open ones are listed first: their
/// review state and the tickets of their keys not in `known` load while GitLab
/// still lists the recently merged and closed ones, the large and slow list.
fn fetch_mrs(
    config: &Config,
    gitlab: &GitLab,
    jira: Option<&Result<JiraClient, String>>,
    known: &[String],
    closing: Option<&ClosingScope>,
) -> Result<(Vec<Mr>, Tickets), String> {
    let mut projects: Vec<_> = config
        .repos
        .iter()
        .filter_map(|r| r.gitlab_project.as_deref())
        .collect();
    if projects.is_empty()
        && let Some(gitlab) = &config.gitlab
    {
        projects.push(&gitlab.project);
    }
    projects.sort_unstable();
    projects.dedup();
    std::thread::scope(|scope| {
        let recent = scope.spawn(|| {
            let mut found = Vec::new();
            for &project in &projects {
                found.extend(
                    gitlab
                        .my_recent_merge_requests(project, MR_LOOKBACK_DAYS)?
                        .into_iter()
                        .map(|mr| Mr::new(mr, project)),
                );
            }
            Ok::<_, String>(found)
        });
        let mut found = Vec::new();
        for &project in &projects {
            found.extend(
                gitlab
                    .my_open_merge_requests(project)?
                    .into_iter()
                    .map(|mr| Mr::new(mr, project)),
            );
        }
        for mr in joined(recent, "GitLab MR listing")? {
            if !found
                .iter()
                .any(|m| m.project == mr.project && m.mr.iid() == mr.mr.iid())
            {
                found.push(mr);
            }
        }
        if let Some(closing) = closing {
            found = closing.select(found);
        }
        let keys: Vec<_> = found
            .iter()
            .flat_map(|m| m.keys.iter().cloned())
            .filter(|k| !known.contains(k))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let tickets = scope.spawn(move || match jira {
            Some(Ok(jira)) => fetch_new_jira_tickets(jira, &keys, closing.is_some()),
            _ => Ok(HashMap::new()),
        });
        let jobs: Vec<_> = found
            .iter()
            .enumerate()
            .filter(|(_, m)| m.opened())
            .map(|(i, _)| i)
            .collect();
        let reviews = parallel(&jobs, REQUESTS, |&i| {
            gitlab.mr_review(&found[i].project, found[i].mr.iid())
        });
        for (i, result) in jobs.into_iter().zip(reviews) {
            match result.unwrap_or_else(|| Err("GitLab review request panicked".into())) {
                Ok(review) => found[i].review = Some(review),
                Err(error) if closing.is_some() => {
                    return Err(format!(
                        "{} !{}: incomplete review: {error}",
                        found[i].project,
                        found[i].mr.iid()
                    ));
                }
                Err(_) => {}
            }
        }
        Ok((found, joined(tickets, "Jira MR keys")))
    })
}

/// Linked worktrees of every configured repo; git status and the unpushed count
/// take ~0.5s per worktree in a big repo, so they run in parallel.
/// `status_only`: run `git status` only in these checkouts (closing a task);
/// the others keep `changes: None`, and the refs-based facts are read for all.
fn collect_trees(
    repos: &[Repository],
    with_status: bool,
    with_diff: bool,
    status_only: Option<&HashSet<PathBuf>>,
) -> Vec<Tree> {
    let found: Vec<_> = repos
        .iter()
        .flat_map(|repo| repo.trees.iter().map(move |wt| (repo, wt)))
        .collect();
    if !with_status && !with_diff {
        return found
            .iter()
            .map(|(repo, wt)| inspect_tree(repo, wt, false, false, false))
            .collect();
    }
    parallel(&found, GIT_RUNS, |(repo, wt)| {
        let with_changes = with_status && status_only.is_none_or(|paths| paths.contains(&wt.path));
        inspect_tree(repo, wt, with_status, with_changes, with_diff)
    })
    .into_iter()
    .flatten()
    .collect()
}

fn inspect_tree(
    repo: &Repository,
    wt: &Worktree,
    with_status: bool,
    with_changes: bool,
    with_diff: bool,
) -> Tree {
    std::thread::scope(|scope| {
        let diff = (with_diff && wt.branch.is_some())
            .then(|| scope.spawn(|| worktree::diff(&wt.path, repo.default.as_deref()?).ok()));
        let started = std::time::Instant::now();
        let changes = with_changes
            .then(|| worktree::changed_files(&wt.path).ok())
            .flatten();
        let stale_index = with_changes && started.elapsed() >= STALE_STATUS;
        let unpushed = with_status
            .then(|| {
                repo.refs
                    .as_ref()?
                    .unpushed(&wt.path, wt.branch.as_deref()?)
                    .ok()
            })
            .flatten();
        let head = if unpushed.is_some_and(|n| n > 0) {
            repo.refs
                .as_ref()
                .and_then(|r| r.head(wt.branch.as_deref()?))
        } else {
            None
        };
        let last_commit = with_status
            .then(|| repo.refs.as_ref()?.time(wt.branch.as_deref()?))
            .flatten();
        Tree {
            repo: repo.main.clone(),
            project: repo.project.clone(),
            wt: wt.clone(),
            keys: branch_keys(repo.project.as_deref(), wt.branch.as_deref()),
            changes,
            unpushed,
            head,
            last_commit,
            diff: diff.and_then(|h| h.join().ok()).flatten(),
            agents: Vec::new(),
            mr: None,
            finished: false,
            stale_index,
        }
    })
}

/// A lock-free `git status` this slow means the index's stat data is stale: it
/// never gets written back, so every later status rereads the changed files.
const STALE_STATUS: std::time::Duration = std::time::Duration::from_secs(1);

/// Starts a plain `git status` in the background in stale checkouts no agent is
/// working or waiting in, so it writes the index and the next look is fast.
/// A busy agent's own git commands refresh the index, and are not raced here.
fn refresh_stale_indexes(trees: &[Tree], agents: &[herdr::Agent]) {
    for tree in trees.iter().filter(|t| needs_refresh(t, agents)) {
        let _span = trace::span(format!("refresh index {}", tree.wt.path.display()));
        worktree::refresh_index_in_background(&tree.wt.path);
    }
}

fn needs_refresh(tree: &Tree, agents: &[herdr::Agent]) -> bool {
    tree.stale_index
        && !agents.iter().any(|agent| {
            !matches!(agent.agent_status.as_str(), "idle" | "done")
                && agent
                    .cwd
                    .as_deref()
                    .is_some_and(|cwd| std::path::Path::new(cwd).starts_with(&tree.wt.path))
        })
}

fn live_herdr() -> (Vec<herdr::Agent>, Vec<herdr::Workspace>, Vec<String>) {
    if !herdr::available() && !std::env::var("BERBR_BACKEND").is_ok_and(|s| s == "herdr-app") {
        return (Vec::new(), Vec::new(), Vec::new());
    }
    let mut warnings = Vec::new();
    let agents = herdr::list_agents().unwrap_or_else(|e| {
        warnings.push(format!("Herdr agents unavailable: {e}"));
        Vec::new()
    });
    let spaces = herdr::list_workspaces().unwrap_or_else(|e| {
        warnings.push(format!("Herdr spaces unavailable: {e}"));
        Vec::new()
    });
    (agents, spaces, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::jira::JiraError;
    use crate::table::Category;
    use crate::table::fixtures::ticket;
    use reqwest::StatusCode;

    fn tickets(keys: &[&str]) -> Tickets {
        Ok(keys
            .iter()
            .map(|k| (k.to_string(), ticket(k, Category::New, false)))
            .collect())
    }

    fn agent_in(cwd: &str, status: &str) -> herdr::Agent {
        serde_json::from_value(serde_json::json!({
            "agent_status": status, "cwd": cwd, "pane_id": "p", "tab_id": "t", "workspace_id": "w",
        }))
        .unwrap()
    }

    #[test]
    fn a_stale_index_is_refreshed_only_where_no_agent_is_busy() {
        let mut stale = crate::table::fixtures::tree("stale", "demo/DEMO-1-a");
        stale.stale_index = true;
        let path = stale.wt.path.display().to_string();
        assert!(needs_refresh(&stale, &[]));
        assert!(needs_refresh(
            &stale,
            &[agent_in(&path, "idle"), agent_in(&path, "done")]
        ));
        // a sibling checkout sharing the name prefix is not this one
        assert!(needs_refresh(
            &stale,
            &[agent_in(&format!("{path}-2"), "working")]
        ));
        for status in ["working", "blocked", "unknown"] {
            assert!(!needs_refresh(
                &stale,
                &[agent_in(&format!("{path}/src"), status)]
            ));
        }
        stale.stale_index = false;
        assert!(!needs_refresh(&stale, &[]));
    }

    #[test]
    fn jira_answered_only_when_every_ticket_query_did() {
        let all = all_tickets(
            tickets(&["DEMO-1"]),
            tickets(&["DEMO-2"]),
            tickets(&["DEMO-3"]),
        );
        let mut keys: Vec<String> = all.unwrap().into_keys().collect();
        keys.sort();
        assert_eq!(keys, vec!["DEMO-1", "DEMO-2", "DEMO-3"]);
        let down = || Err("timed out".to_string());
        assert!(all_tickets(tickets(&["DEMO-1"]), down(), tickets(&["DEMO-3"])).is_err());
        assert!(all_tickets(tickets(&["DEMO-1"]), tickets(&["DEMO-2"]), down()).is_err());
    }

    #[test]
    fn a_failed_request_for_one_key_fails_the_query() {
        let refused = |status| JiraError::RequestResultError(status, String::new());
        assert!(matches!(
            existing(Err(refused(StatusCode::NOT_FOUND))),
            Ok(None)
        ));
        assert!(matches!(
            existing(Err(refused(StatusCode::BAD_REQUEST))),
            Ok(None)
        ));
        assert!(existing(Err(refused(StatusCode::BAD_GATEWAY))).is_err());
    }

    #[test]
    fn a_service_that_did_not_answer_shows_the_cache() {
        let mut warnings = Vec::new();
        let (value, source) = or_cached("GitLab", Some(Ok(1)), Some(2), &mut warnings);
        assert_eq!((value, source.fresh), (1, Fresh::Live));
        let (value, source) = or_cached("GitLab", Some(Err("down".into())), Some(2), &mut warnings);
        assert_eq!((value, source.fresh), (2, Fresh::Cached));
        assert_eq!(
            source.warning.as_deref(),
            Some("GitLab: down — showing cached")
        );
        let (value, source) =
            or_cached::<i32>("Jira", Some(Err("down".into())), None, &mut warnings);
        assert_eq!((value, source.fresh), (0, Fresh::Missing));
        assert_eq!(
            warnings,
            vec!["GitLab: down — showing cached", "Jira: down"]
        );
        // offline: the cache, silently
        let (value, source) = or_cached("Jira", None, Some(3), &mut warnings);
        assert_eq!(
            (value, source),
            (
                3,
                Source {
                    fresh: Fresh::Cached,
                    warning: None
                }
            )
        );
        assert_eq!(warnings.len(), 2);
    }
}

#[cfg(test)]
mod completeness_tests {
    use super::*;
    use crate::table::{Category, fixtures::ticket};

    #[test]
    fn every_known_jira_key_must_be_accounted_for() {
        let keys: Vec<_> = ["DEMO-1", "DEMO-2", "DEMO-3"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let mut tickets =
            HashMap::from([("DEMO-1".into(), ticket("DEMO-1", Category::Done, true))]);
        let error = require_known_keys(&keys, &tickets).unwrap_err();
        assert!(error.contains("DEMO-2, DEMO-3"));
        for key in &keys {
            tickets.insert(key.clone(), ticket(key, Category::Done, true));
        }
        assert!(require_known_keys(&keys, &tickets).is_ok());
    }
}

#[cfg(test)]
mod closing_scope_tests {
    use super::*;
    use crate::table::fixtures::{mr, task_note, tree};

    #[test]
    fn github_closing_scope_selects_own_fork_before_any_review_requests() {
        let note = task_note(
            "local",
            "worktrees: [/r/.claude/worktrees/own]",
            "2026-11-01",
        );
        let mut own = tree("own", "same");
        own.project = Some("github.com/fork/repo".into());
        let scope = ClosingScope::new(&note, &[own], None);
        let mut related = mr(1, "same", "Own fork PR", "opened");
        related.mr.github = true;
        related.mr.head_repo = Some("github.com/fork/repo".into());
        related.project = "github.com/upstream/repo".into();
        let mut unrelated = mr(
            2,
            "other",
            "Unrelated PR with missing review permissions",
            "opened",
        );
        unrelated.mr.github = true;
        unrelated.mr.head_repo = Some("github.com/other/repo".into());
        unrelated.project = "github.com/upstream/repo".into();
        let selected = scope.select(vec![related, unrelated]);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].mr.iid(), 1);
        let empty = ClosingScope::new(&note, &[], None);
        assert!(
            empty.select(selected).is_empty(),
            "a local task must trigger no unrelated PR review requests"
        );
    }

    #[test]
    fn closing_scope_uses_repository_branches_and_discovers_mr_ticket_branches() {
        let task = task_note(
            "demo-0",
            "tickets: [DEMO-1]\nworktrees: [/r/.claude/worktrees/own]",
            "2026-11-01",
        );
        let mut own = tree("own", "same");
        own.project = Some("demo/alpha".into());
        let scope = ClosingScope::new(&task, &[own], None);
        let mut a = mr(1, "same", "own", "opened");
        a.project = "demo/alpha".into();
        let mut b = mr(2, "same", "foreign", "opened");
        b.project = "demo/beta".into();
        let mut ticket = mr(3, "discovered", "DEMO-1 initial", "merged");
        ticket.project = "demo/alpha".into();
        let mut followup = mr(4, "discovered", "followup", "opened");
        followup.project = "demo/alpha".into();
        let selected = scope.select(vec![a, b, ticket, followup]);
        assert_eq!(
            selected.iter().map(|m| m.mr.iid()).collect::<Vec<_>>(),
            [1, 3, 4]
        );
    }
}

#[cfg(test)]
mod github_reference_tests {
    use super::*;
    use crate::table::Category;
    use crate::table::fixtures::{mr, task_note, ticket, tree};
    #[test]
    fn a_jira_ticket_first_discovered_in_github_pr_keeps_its_owner_when_closing() {
        let issue = serde_json::json!({"key":"DEMO-42","fields":{"summary":"Synthetic issue","created":"2026-11-01T00:00:00Z", "status":{"name":"Open","statusCategory":{"key":"new"}}}});
        let page = serde_json::json!({"startAt":0,"total":1,"issues":[issue]});
        for ownership_status in [200, 403] {
            let (base, server) = crate::client::test_http::serve(vec![
                (200, "", page.clone()),
                (ownership_status, "", page.clone()),
            ]);
            let client = JiraClient::fixture(base);
            let result = fetch_new_jira_tickets(&client, &["DEMO-42".into()], true);
            if ownership_status == 200 {
                assert!(result.unwrap()["DEMO-42"].mine);
            } else {
                assert!(
                    result.is_err(),
                    "unknown ownership must block closing, not leave the ticket open silently"
                );
            }
            let requests = server.join().unwrap();
            let jql = |request: &str| {
                let target = request
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                reqwest::Url::parse(&format!("http://fixture{target}"))
                    .unwrap()
                    .query_pairs()
                    .find(|(key, _)| key == "jql")
                    .unwrap()
                    .1
                    .into_owned()
            };
            assert!(!jql(&requests[0]).contains("assignee"));
            assert!(jql(&requests[1]).contains("assignee = currentUser()"));
            assert!(jql(&requests[1]).contains("DEMO-42"));
        }
    }

    #[test]
    fn github_branches_retain_jira_keys_without_guessing_github_issue_repository() {
        assert_eq!(
            branch_keys(Some("github.com/demo/repo"), Some("DEMO-42-fix")),
            ["DEMO-42"]
        );
        assert!(branch_keys(Some("github.com/fork/repo"), Some("issue-42-fix")).is_empty());
    }

    #[test]
    fn pr_and_unconfigured_upstream_references_do_not_become_required_task_issues_on_refresh() {
        let issue = "github.com/demo/repo#1";
        let pull = "github.com/demo/repo#2";
        let mut pr = mr(3, "same", "See #1 and #2", "opened");
        pr.keys = vec![
            issue.into(),
            pull.into(),
            "github.com/upstream/repo#1".into(),
            "DEMO-42".into(),
        ];
        let mut mrs = vec![pr];
        let tickets = HashMap::from([(issue.into(), ticket(issue, Category::New, false))]);
        confirmed_issue_references(&mut mrs, &tickets);
        assert_eq!(mrs[0].keys, [issue, "DEMO-42"]);
        mrs[0].mr.github = true;
        mrs[0].mr.head_repo = Some("github.com/demo/repo".into());
        mrs[0].project = "github.com/demo/repo".into();
        let note = task_note(
            "owner",
            "worktrees: [/r/.claude/worktrees/own]",
            "2026-11-01",
        );
        let path = note.note.path.clone();
        let mut checkout = tree("own", "same");
        checkout.project = Some("github.com/demo/repo".into());
        let mut table = crate::table::fixtures::table(
            vec![note],
            tickets.into_values().collect(),
            mrs,
            vec![checkout],
        );
        assert_eq!(table.tasks[0].found_tickets, [issue, "DEMO-42"]);
        crate::table::write_back(&mut table);
        assert!(table.warnings.is_empty());
        let saved = Task::from_note(crate::vault::note::Note::read(&path).unwrap());
        assert_eq!(known_keys(&[], &[saved]), [issue, "DEMO-42"]);
    }
}
