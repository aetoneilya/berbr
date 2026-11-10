//! Read-only GitHub.com integration. API failures never become empty live data.
use super::gitlab::{MergeRequest, MrReview};
use super::issue::{Issue, github_parts};
use crate::table::{Category, Mr, Ticket};
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

pub struct GitHub {
    client: Client,
    base: String,
}

/// Preserve HTTP status internally so optional references can distinguish a
/// missing issue from authentication, rate-limit and network failures.
struct ApiError {
    status: Option<u16>,
    message: String,
}
impl ApiError {
    fn other(message: impl Into<String>) -> Self {
        Self {
            status: None,
            message: message.into(),
        }
    }
}

impl GitHub {
    pub fn from_env() -> Result<Self, String> {
        let token = ["GH_TOKEN", "GITHUB_TOKEN"].iter()
            .find_map(|name| std::env::var(name).ok().filter(|v| !v.trim().is_empty()))
            .or_else(|| std::process::Command::new("gh").args(["auth", "token", "--hostname", "github.com"])
                .output().ok().filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok()).filter(|v| !v.trim().is_empty()))
            .ok_or("GitHub authentication missing: run gh auth login --hostname github.com or set GH_TOKEN / GITHUB_TOKEN")?;
        Self::new("https://api.github.com", &token)
    }

    fn new(base: &str, token: &str) -> Result<Self, String> {
        let mut headers = HeaderMap::new();
        let mut auth = HeaderValue::from_str(&format!("Bearer {}", token.trim()))
            .map_err(|_| "invalid GitHub token header")?;
        auth.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth);
        headers.insert(
            "accept",
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "x-github-api-version",
            HeaderValue::from_static("2022-11-28"),
        );
        let client = Client::builder()
            .default_headers(headers)
            .user_agent("berbr")
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            client,
            base: base.into(),
        })
    }

    fn request(&self, path: &str, body: Option<Value>) -> Result<(Value, bool), String> {
        self.request_json(path, body).map_err(|e| e.message)
    }

    fn request_json(&self, path: &str, body: Option<Value>) -> Result<(Value, bool), ApiError> {
        let _permit = super::http::acquire();
        let url = format!("{}{}", self.base, path);
        let start = std::time::Instant::now();
        let method = if body.is_some() { "POST" } else { "GET" };
        let request = match body {
            Some(body) => self.client.post(&url).json(&body),
            None => self.client.get(&url),
        };
        let result = request.send();
        super::http::log(method, &url, start, &result);
        let response =
            result.map_err(|e| ApiError::other(format!("GitHub request failed: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            // Never echo server bodies or tokens. 404 includes private/restricted repositories.
            let hint = match status.as_u16() {
                401 => "authentication expired; run gh auth login or check GH_TOKEN",
                403 | 429 => {
                    "access denied or API rate limit reached; check token permissions and retry later"
                }
                404 => {
                    "repository/issue is missing or inaccessible; check owner/repo and token access"
                }
                _ => "API unavailable; retry later",
            };
            return Err(ApiError {
                status: Some(status.as_u16()),
                message: format!("GitHub HTTP {status}: {hint}"),
            });
        }
        let next = response
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(',').any(|s| s.contains("rel=\"next\"")));
        let value: Value = response
            .json()
            .map_err(|e| ApiError::other(format!("invalid GitHub response: {e}")))?;
        if value.get("errors").is_some() {
            return Err(ApiError::other(
                "GitHub GraphQL returned incomplete data; check token permissions",
            ));
        }
        Ok((value, next))
    }

    fn pages(&self, path: &str) -> Result<Vec<Value>, String> {
        self.pages_until(path, |_| false)
    }

    fn pages_until(
        &self,
        path: &str,
        stop: impl Fn(&[Value]) -> bool,
    ) -> Result<Vec<Value>, String> {
        pages(|page| {
            let separator = if path.contains('?') { '&' } else { '?' };
            let (value, next) =
                self.request(&format!("{path}{separator}per_page=100&page={page}"), None)?;
            let values = value
                .as_array()
                .ok_or("GitHub list is not an array")?
                .clone();
            let next = next && !stop(&values);
            Ok((values, next))
        })
    }

    pub fn issue(&self, key: &str) -> Result<Issue, String> {
        let (repo, number) = github_parts(key).ok_or("invalid qualified GitHub issue")?;
        let (value, _) = self.request(&format!("/repos/{repo}/issues/{number}"), None)?;
        if value.get("pull_request").is_some() {
            return Err(format!("{key} is a pull request, not an issue"));
        }
        Ok(Issue {
            key: key.into(),
            summary: string(&value, "title")?.into(),
            description: value["body"].as_str().map(str::to_string),
        })
    }

    /// Open issues in every configured repository, plus all explicitly linked closed ones.
    pub fn tickets(
        &self,
        repos: &[String],
        known: &[String],
        referenced: &[String],
    ) -> Result<HashMap<String, Ticket>, String> {
        let (user, _) = self.request("/user", None)?;
        let login = string(&user, "login")?;
        let mut tickets = HashMap::new();
        for repo in repos {
            for value in self.pages(&format!(
                "/repos/{repo}/issues?state=open&sort=updated&direction=desc"
            ))? {
                if value.get("pull_request").is_some() {
                    continue;
                }
                let key = format!("github.com/{repo}#{}", number(&value)?);
                tickets.insert(key.clone(), ticket(&key, &value, login)?);
            }
        }
        let mut requested = HashSet::new();
        for key in known
            .iter()
            .chain(referenced)
            .filter(|k| github_parts(k).is_some())
        {
            if tickets.contains_key(key) || !requested.insert(key) {
                continue;
            }
            let (repo, n) = github_parts(key).ok_or("invalid GitHub issue")?;
            if !repos.iter().any(|r| r == repo) {
                return Err(format!("linked GitHub repository {repo} is not configured"));
            }
            let value = match self.request_json(&format!("/repos/{repo}/issues/{n}"), None) {
                Ok((value, _)) => value,
                Err(error) if error.status == Some(404) && !known.contains(key) => continue,
                Err(error) => return Err(error.message),
            };
            if value.get("pull_request").is_some() {
                if known.contains(key) {
                    return Err(format!("{key} is a PR, not an issue"));
                }
                continue;
            }
            tickets.insert(key.clone(), ticket(key, &value, login)?);
        }
        Ok(tickets)
    }

    /// Pull lists are scoped to target repositories. Also fetch upstream targets of forks.
    pub fn pulls(
        &self,
        repos: &[String],
        branches: &HashMap<String, HashSet<String>>,
    ) -> Result<Vec<Mr>, String> {
        let mut targets: HashSet<String> = repos.iter().cloned().collect();
        for repo in repos {
            let (info, _) = self.request(&format!("/repos/{repo}"), None)?;
            if let Some(parent) = info.pointer("/parent/full_name").and_then(Value::as_str) {
                targets.insert(super::issue::github_repo(parent)?);
            }
        }
        let mut targets: Vec<_> = targets.into_iter().collect();
        targets.sort();
        let mut found = Vec::new();
        for repo in targets {
            // All open PRs, plus closed PRs of local branches regardless of age. Recent
            // closed PRs without a checkout remain visible for task/issue matching.
            for state in ["open", "closed"] {
                let mut listed = self.pages_until(
                    &format!("/repos/{repo}/pulls?state={state}&sort=updated&direction=desc"),
                    |values| state == "closed" && values.last().is_some_and(|v| !recent(v)),
                )?;
                if state == "closed" {
                    // Bounded history listing above must not lose an old PR for a
                    // checkout that still exists (including an upstream fork PR).
                    for (head_repo, names) in branches {
                        let owner = head_repo.split('/').next().ok_or("invalid head repo")?;
                        for branch in names {
                            if found.iter().any(|m: &Mr| {
                                m.source_project() == format!("github.com/{head_repo}")
                                    && m.mr.source_branch() == branch
                            }) || listed.iter().any(|v| {
                                v.pointer("/head/repo/full_name")
                                    .and_then(Value::as_str)
                                    .is_some_and(|r| r.eq_ignore_ascii_case(head_repo))
                                    && v.pointer("/head/ref").and_then(Value::as_str)
                                        == Some(branch.as_str())
                            }) {
                                continue;
                            }
                            let mut query =
                                reqwest::Url::parse("https://api.github.com").expect("API URL");
                            query
                                .query_pairs_mut()
                                .append_pair("state", "closed")
                                .append_pair("head", &format!("{owner}:{branch}"));
                            for value in self.pages(&format!(
                                "/repos/{repo}/pulls?{}",
                                query.query().unwrap_or_default()
                            ))? {
                                if !listed.iter().any(|v| v["number"] == value["number"]) {
                                    listed.push(value);
                                }
                            }
                        }
                    }
                }
                for value in listed {
                    let head_repo = value
                        .pointer("/head/repo/full_name")
                        .and_then(Value::as_str)
                        .map(super::issue::github_repo)
                        .transpose()?;
                    let head_branch = value
                        .pointer("/head/ref")
                        .and_then(Value::as_str)
                        .ok_or("GitHub PR missing head ref")?;
                    let own_target = repos.contains(&repo);
                    let own_head = head_repo.as_ref().is_some_and(|r| repos.contains(r));
                    if !own_target && !own_head {
                        continue;
                    }
                    let local = head_repo
                        .as_ref()
                        .and_then(|r| branches.get(r))
                        .is_some_and(|b| b.contains(head_branch));
                    if state == "closed" && !local && !recent(&value) {
                        continue;
                    }
                    let mr = normalize(&repo, &value)?;
                    found.push(mr);
                }
            }
        }
        Ok(found)
    }

    /// Fetch reviews only after the caller has selected the relevant PRs.
    pub fn reviews(&self, found: &mut [Mr]) -> Vec<String> {
        let mut warnings = Vec::new();
        let jobs: Vec<_> = found
            .iter()
            .enumerate()
            .filter(|(_, m)| m.opened())
            .map(|(i, _)| i)
            .collect();
        let results = super::parallel(&jobs, 8, |&i| {
            let mr = &found[i];
            let repo = mr
                .project
                .strip_prefix("github.com/")
                .ok_or("invalid GitHub PR identity")?;
            self.review(
                repo,
                mr.mr.iid(),
                mr.mr.sha().unwrap_or_default(),
                mr.mr.draft,
            )
        });
        for (i, result) in jobs.into_iter().zip(results) {
            let mr = &mut found[i];
            match result.unwrap_or_else(|| Err("review request panicked".into())) {
                Ok((review, status, decision, conflict)) => {
                    mr.review = Some(review);
                    mr.mr.detailed_merge_status = status;
                    mr.mr.review_decision = decision;
                    mr.mr.has_conflicts = conflict;
                }
                Err(e) => warnings.push(format!(
                    "{} #{}: review unavailable: {e}",
                    mr.project,
                    mr.mr.iid()
                )),
            }
        }
        warnings
    }

    fn review(
        &self,
        repo: &str,
        n: u64,
        sha: &str,
        draft: bool,
    ) -> Result<(MrReview, Option<String>, Option<String>, bool), String> {
        let (owner, name) = repo.split_once('/').ok_or("invalid repo")?;
        let mut cursor: Option<String> = None;
        let mut unresolved = 0;
        let mut seen = HashSet::new();
        loop {
            let (data, _) = self.request("/graphql", Some(json!({
                "query": "query($owner:String!,$name:String!,$number:Int!,$cursor:String){repository(owner:$owner,name:$name){pullRequest(number:$number){headRefOid isDraft state mergeable mergeStateStatus reviewDecision latestReviews(first:100){nodes{state author{login}} pageInfo{hasNextPage}} commits(last:1){nodes{commit{oid statusCheckRollup{state}}}} reviewThreads(first:100,after:$cursor){nodes{isResolved} pageInfo{hasNextPage endCursor}}}}}",
                "variables":{"owner":owner,"name":name,"number":n,"cursor":cursor}
            })))?;
            let pr = data
                .pointer("/data/repository/pullRequest")
                .filter(|v| v.is_object())
                .ok_or("GitHub PR review missing")?;
            if pr["state"].as_str() != Some("OPEN") || pr["isDraft"].as_bool() != Some(draft) {
                return Err(
                    "PR state or draft changed while fetching review; refresh again".into(),
                );
            }
            if pr["headRefOid"].as_str() != Some(sha) {
                return Err("PR head changed while fetching review; refresh again".into());
            }
            let threads = pr
                .pointer("/reviewThreads/nodes")
                .and_then(Value::as_array)
                .ok_or("missing review threads")?;
            for thread in threads {
                if !thread["isResolved"]
                    .as_bool()
                    .ok_or("unknown review thread state")?
                {
                    unresolved += 1;
                }
            }
            let more = pr
                .pointer("/reviewThreads/pageInfo/hasNextPage")
                .and_then(Value::as_bool)
                .ok_or("missing review pagination")?;
            if more {
                let next = pr
                    .pointer("/reviewThreads/pageInfo/endCursor")
                    .and_then(Value::as_str)
                    .ok_or("missing review cursor")?;
                if threads.is_empty() || !seen.insert(next.to_string()) {
                    return Err("incomplete review pagination".into());
                }
                cursor = Some(next.into());
                continue;
            }
            if pr
                .pointer("/latestReviews/pageInfo/hasNextPage")
                .and_then(Value::as_bool)
                != Some(false)
            {
                return Err("incomplete latest reviews".into());
            }
            let reviews = pr
                .pointer("/latestReviews/nodes")
                .and_then(Value::as_array)
                .ok_or("missing latest reviews")?;
            let approved_by = reviews
                .iter()
                .filter(|r| r["state"] == "APPROVED")
                .filter_map(|r| {
                    r.pointer("/author/login")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect();
            let commit = pr
                .pointer("/commits/nodes/0/commit")
                .ok_or("missing PR commit")?;
            if commit["oid"].as_str() != Some(sha) {
                return Err("checks refer to a different commit".into());
            }
            let pipeline = match commit
                .pointer("/statusCheckRollup/state")
                .and_then(Value::as_str)
            {
                Some("SUCCESS") => Some("success"),
                Some("FAILURE" | "ERROR") => Some("failed"),
                Some("PENDING" | "EXPECTED") => Some("pending"),
                _ => None,
            }
            .map(str::to_string);
            let decision = pr["reviewDecision"].as_str().map(str::to_string);
            let conflict = pr["mergeable"] == "CONFLICTING";
            let status = match (
                pr["mergeStateStatus"].as_str(),
                pr["mergeable"].as_str(),
                decision.as_deref(),
            ) {
                (_, _, Some("CHANGES_REQUESTED")) => "changes_requested",
                (_, _, Some("REVIEW_REQUIRED")) => "not_approved",
                (_, Some("CONFLICTING"), _) => "conflict",
                (Some("CLEAN"), Some("MERGEABLE"), None | Some("APPROVED")) => "mergeable",
                (Some("BEHIND"), _, _) => "need_rebase",
                _ => "unknown",
            };
            return Ok((
                MrReview {
                    pipeline,
                    approved_by,
                    unresolved,
                },
                Some(status.into()),
                decision,
                conflict,
            ));
        }
    }
}

fn recent(value: &Value) -> bool {
    value["updated_at"]
        .as_str()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .is_some_and(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_days() <= 90)
}

fn pages(
    mut fetch: impl FnMut(u32) -> Result<(Vec<Value>, bool), String>,
) -> Result<Vec<Value>, String> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    for page in 1.. {
        let (items, next) = fetch(page)?;
        if next && items.is_empty() {
            return Err("incomplete GitHub list: empty intermediate page".into());
        }
        for item in &items {
            if let Some(id) = item["id"].as_u64()
                && !seen.insert(id)
            {
                return Err("incomplete GitHub list: repeated item; refresh again".into());
            }
        }
        found.extend(items);
        if !next {
            return Ok(found);
        }
    }
    unreachable!()
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .ok_or_else(|| format!("GitHub response missing {key}"))
}
fn number(value: &Value) -> Result<u64, String> {
    value["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("GitHub response missing number".into())
}

fn ticket(key: &str, v: &Value, login: &str) -> Result<Ticket, String> {
    let state = string(v, "state")?;
    let mine = v["assignees"]
        .as_array()
        .is_some_and(|a| a.iter().any(|u| u["login"].as_str() == Some(login)));
    Ok(Ticket {
        key: key.into(),
        summary: string(v, "title")?.into(),
        status: state.into(),
        category: match state {
            "open" => Category::New,
            "closed" => Category::Done,
            _ => Category::Unknown,
        },
        mine,
        created: string(v, "created_at")?.into(),
    })
}

fn normalize(repo: &str, v: &Value) -> Result<Mr, String> {
    let n = number(v)?;
    let head_repo = v
        .pointer("/head/repo/full_name")
        .and_then(Value::as_str)
        .map(super::issue::github_repo)
        .transpose()?
        .map(|r| format!("github.com/{r}"));
    let state = match string(v, "state")? {
        "open" => "opened",
        "closed" if !v["merged_at"].is_null() => "merged",
        "closed" => "closed",
        _ => "other",
    };
    let mr: MergeRequest = serde_json::from_value(json!({
        "github":true,"head_repo":head_repo,"iid":n,"title":string(v,"title")?,"state":state,
        "web_url":format!("https://github.com/{repo}/pull/{n}"),
        "source_branch":v.pointer("/head/ref").and_then(Value::as_str).ok_or("missing head ref")?,
        "sha":v.pointer("/head/sha").and_then(Value::as_str).ok_or("missing head sha")?,
        "draft":v["draft"].as_bool().ok_or("missing draft state")?,"created_at":v["created_at"],"updated_at":v["updated_at"]
    })).map_err(|e| e.to_string())?;
    // Explicit #N references are scoped to the target repository; cross-repo refs
    // retain their own owner/repo. Do not extract Jira-like substrings from a URL.
    let text = format!(
        "{} {} {}",
        mr.title(),
        v["body"].as_str().unwrap_or_default(),
        mr.source_branch()
    );
    let re = regex::Regex::new(r"(?:https://github\.com/([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)/issues/([0-9]+)|(?:([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+))?#([0-9]+))").expect("issue references");
    let mut keys = crate::vault::task::jira_keys(&re.replace_all(&text, ""));
    for c in re.captures_iter(&text) {
        let repository = c.get(1).or_else(|| c.get(3)).map_or(repo, |m| m.as_str());
        let number = c
            .get(2)
            .or_else(|| c.get(4))
            .expect("reference number")
            .as_str();
        if let Some(key) = super::issue::github_key(&format!("{repository}#{number}"))
            && !keys.contains(&key)
        {
            keys.push(key);
        }
    }
    Ok(Mr {
        mr,
        keys,
        review: None,
        project: format!("github.com/{repo}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock(
        responses: Vec<(u16, &'static str, Value)>,
    ) -> (GitHub, std::thread::JoinHandle<Vec<String>>) {
        let (base, server) = crate::client::test_http::serve(responses);
        let mut client = GitHub::new(&base, "synthetic-token").unwrap();
        // Local fixture traffic must not use the machine's proxy.
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer synthetic-token"),
        );
        client.client = Client::builder()
            .no_proxy()
            .default_headers(headers)
            .timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        (client, server)
    }

    fn review_data() -> Value {
        json!({"data":{"repository":{"pullRequest":{
            "headRefOid":"head", "state":"OPEN", "isDraft":false, "mergeable":"MERGEABLE", "mergeStateStatus":"CLEAN", "reviewDecision":"APPROVED",
            "latestReviews":{"nodes":[{"state":"APPROVED","author":{"login":"reviewer"}}],"pageInfo":{"hasNextPage":false}},
            "commits":{"nodes":[{"commit":{"oid":"head","statusCheckRollup":{"state":"SUCCESS"}}}]},
            "reviewThreads":{"nodes":[{"isResolved":false}],"pageInfo":{"hasNextPage":false,"endCursor":null}}
        }}}})
    }

    #[test]
    fn review_follows_threads_and_rejects_changed_heads_and_partial_graphql() {
        let mut first = review_data();
        first["data"]["repository"]["pullRequest"]["reviewThreads"]["pageInfo"] =
            json!({"hasNextPage":true,"endCursor":"page2"});
        let (github, server) = mock(vec![(200, "", first), (200, "", review_data())]);
        let (review, status, decision, conflict) =
            github.review("demo/repo", 7, "head", false).unwrap();
        assert_eq!(review.pipeline.as_deref(), Some("success"));
        assert_eq!(review.unresolved, 2);
        assert_eq!(review.approved_by, ["reviewer"]);
        assert_eq!(status.as_deref(), Some("mergeable"));
        assert_eq!(decision.as_deref(), Some("APPROVED"));
        assert!(!conflict);
        let requests = server.join().unwrap();
        assert!(requests[1].contains("\"cursor\":\"page2\""));
        for data in [
            review_data(),
            json!({"data":{},"errors":[{"message":"denied"}]}),
        ] {
            let (github, server) = mock(vec![(200, "", data)]);
            assert!(github.review("demo/repo", 7, "other-head", false).is_err());
            server.join().unwrap();
        }
    }

    #[test]
    fn unknown_checks_and_review_requirements_never_become_mergeable() {
        for (field, value, expected) in [
            ("mergeStateStatus", json!("UNKNOWN"), "unknown"),
            (
                "reviewDecision",
                json!("CHANGES_REQUESTED"),
                "changes_requested",
            ),
            ("reviewDecision", json!("REVIEW_REQUIRED"), "not_approved"),
            ("mergeable", json!("CONFLICTING"), "conflict"),
        ] {
            let mut data = review_data();
            data["data"]["repository"]["pullRequest"][field] = value;
            let (github, server) = mock(vec![(200, "", data)]);
            assert_eq!(
                github
                    .review("demo/repo", 7, "head", false)
                    .unwrap()
                    .1
                    .as_deref(),
                Some(expected)
            );
            server.join().unwrap();
        }
        let mut data = review_data();
        data["data"]["repository"]["pullRequest"]["commits"]["nodes"][0]["commit"]["statusCheckRollup"] =
            Value::Null;
        let (github, server) = mock(vec![(200, "", data)]);
        assert!(
            github
                .review("demo/repo", 7, "head", false)
                .unwrap()
                .0
                .pipeline
                .is_none()
        );
        server.join().unwrap();
    }

    #[test]
    fn rest_pagination_auth_access_and_rate_limits_fail_without_leaking_bodies() {
        let (github, server) = mock(vec![
            (
                200,
                "Link: <https://api.github.com/next>; rel=\"next\"\r\n",
                json!([{"id":1}]),
            ),
            (200, "", json!([{"id":2}])),
        ]);
        assert_eq!(github.pages("/repos/demo/repo/issues").unwrap().len(), 2);
        let requests = server.join().unwrap();
        assert!(requests[1].contains("page=2"));
        assert!(
            requests[0]
                .to_lowercase()
                .contains("authorization: bearer synthetic-token")
        );
        for status in [401, 403, 404, 429, 500, 302] {
            let (github, server) = mock(vec![(
                status,
                "Location: https://example.invalid/\r\n",
                json!({"message":"synthetic-secret"}),
            )]);
            let error = github.issue("github.com/demo/repo#1").err().unwrap();
            assert!(error.contains(&status.to_string()));
            assert!(!error.contains("synthetic-secret"));
            server.join().unwrap();
        }
    }

    #[test]
    fn issue_lists_exclude_pulls_but_keep_closed_linked_issues() {
        let issue = |n, state| json!({"number":n,"title":"Synthetic issue","state":state,"created_at":"2026-11-01T00:00:00Z","assignees":[{"login":"me"}]});
        let (github, server) = mock(vec![
            (200, "", json!({"login":"me"})),
            (
                200,
                "",
                json!([issue(1,"open"),{"number":2,"pull_request":{}}]),
            ),
            (200, "", issue(3, "closed")),
        ]);
        let tickets = github
            .tickets(
                &["demo/repo".into()],
                &["github.com/demo/repo#3".into()],
                &[],
            )
            .unwrap();
        assert_eq!(tickets.len(), 2);
        assert!(tickets["github.com/demo/repo#1"].mine);
        assert!(tickets["github.com/demo/repo#3"].closed());
        server.join().unwrap();
    }
    #[test]
    fn fork_listing_retains_old_upstream_pr_for_local_branch_and_ignores_other_forks() {
        let old = |n, head_repo| {
            json!({"number":n,"title":"Old pull request","state":"closed","draft":false,
            "merged_at":"2025-01-01T00:00:00Z","updated_at":"2025-01-01T00:00:00Z",
            "head":{"ref":"old","sha":"abc","repo":{"full_name":head_repo}}})
        };
        let (github, server) = mock(vec![
            (200, "", json!({"parent":{"full_name":"upstream/repo"}})),
            (200, "", json!([])),
            (200, "", json!([])),
            (200, "", json!([])),
            (200, "", json!([])),
            (200, "", json!([])),
            (200, "", json!([old(7, "fork/repo"), old(8, "other/repo")])),
        ]);
        let mrs = github
            .pulls(
                &["fork/repo".into()],
                &HashMap::from([("fork/repo".into(), HashSet::from(["old".into()]))]),
            )
            .unwrap();
        assert_eq!(mrs.len(), 1);
        assert_eq!(mrs[0].project, "github.com/upstream/repo");
        assert_eq!(mrs[0].source_project(), "github.com/fork/repo");
        assert_eq!(mrs[0].mr.state().as_str(), "merged");
        let requests = server.join().unwrap();
        assert!(requests.last().unwrap().contains("head=fork%3Aold"));
    }

    #[test]
    fn missing_automatic_reference_keeps_valid_issues_but_explicit_links_and_api_failures_still_fail()
     {
        let valid = json!({"number":1,"title":"Synthetic issue","state":"open","created_at":"2026-11-01T00:00:00Z","assignees":[]});
        for (status, explicit, succeeds) in [
            (404, false, true),
            (404, true, false),
            (403, false, false),
            (500, false, false),
        ] {
            let (github, server) = mock(vec![
                (200, "", json!({"login":"me"})),
                (200, "", json!([valid.clone()])),
                (status, "", json!({"message":"missing"})),
            ]);
            let missing = vec!["github.com/demo/repo#123456".into()];
            let known = if explicit { missing.as_slice() } else { &[] };
            let result = github.tickets(&["demo/repo".into()], known, &missing);
            if succeeds {
                let tickets = result.unwrap();
                assert_eq!(tickets.len(), 1);
                assert!(tickets.contains_key("github.com/demo/repo#1"));
            } else {
                assert!(result.is_err());
            }
            server.join().unwrap();
        }
    }

    #[test]
    fn explicit_issue_validation_refuses_pull_request_objects() {
        let (github, server) = mock(vec![(
            200,
            "",
            json!({"number":2,"title":"A PR","pull_request":{}}),
        )]);
        assert!(
            github
                .issue("github.com/demo/repo#2")
                .err()
                .unwrap()
                .contains("not an issue")
        );
        server.join().unwrap();
        let (github, server) = mock(vec![]);
        assert!(
            github.reviews(&mut []).is_empty(),
            "closing an unrelated task makes no review requests"
        );
        assert!(server.join().unwrap().is_empty());
    }

    #[test]
    fn pages_follow_every_page_and_refuse_partial_data() {
        let values = pages(|p| Ok((vec![json!({"id":p})], p < 4))).unwrap();
        assert_eq!(values.len(), 4);
        assert!(
            pages(|p| if p == 1 {
                Ok((vec![json!({"id":1})], true))
            } else {
                Err("offline".into())
            })
            .is_err()
        );
        assert!(pages(|_| Ok((vec![], true))).is_err());
        assert!(pages(|_| Ok((vec![json!({"id":1})], true))).is_err());
    }
    #[test]
    fn github_pull_preserves_jira_references_in_title_and_branch() {
        let v = json!({"number":7,"title":"DEMO-42: Fix login and #1", "state":"open","draft":false,
            "head":{"ref":"DEMO-43-fix","sha":"head","repo":{"full_name":"demo/repo"}}});
        let m = normalize("demo/repo", &v).unwrap();
        assert_eq!(m.keys, ["DEMO-42", "DEMO-43", "github.com/demo/repo#1"]);
    }

    #[test]
    fn fork_and_merged_pull_keep_source_target_and_issue_identity() {
        let v = json!({"number":7,"title":"Fix #1 and other/repo#1","body":"https://github.com/third/repo/issues/2", "state":"closed","merged_at":"2026-11-01T00:00:00Z","draft":false,
            "head":{"ref":"same","sha":"abc","repo":{"full_name":"Fork/Repo"}}});
        let m = normalize("upstream/repo", &v).unwrap();
        assert_eq!(m.project, "github.com/upstream/repo");
        assert_eq!(m.mr.head_repo.as_deref(), Some("github.com/fork/repo"));
        assert_eq!(m.mr.state().as_str(), "merged");
        assert_eq!(
            m.keys,
            [
                "github.com/upstream/repo#1",
                "github.com/other/repo#1",
                "github.com/third/repo#2"
            ]
        );
    }
}
