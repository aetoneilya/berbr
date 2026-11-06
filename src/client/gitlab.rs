use super::joined;
use reqwest::Url;
use reqwest::blocking::{Client, Response};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

/// GitLab's REST API with one HTTP client: requests share its connections
/// (HTTP/2 multiplexes them over one), so a dozen MRs cost one TLS handshake.
pub struct GitLab {
    client: Client,
    host: String,
}

impl GitLab {
    pub fn new(host: &str, token: &str) -> Result<GitLab, String> {
        let mut token = HeaderValue::from_str(token)
            .map_err(|e| format!("GITLAB_TOKEN can't be sent in a header: {e}"))?;
        token.set_sensitive(true);
        let headers = HeaderMap::from_iter([(HeaderName::from_static("private-token"), token)]);
        let client = Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(GitLab {
            client,
            host: host.to_string(),
        })
    }

    /// A client with the token from `GITLAB_TOKEN`.
    pub fn from_env(host: &str) -> Result<GitLab, String> {
        let token =
            std::env::var("GITLAB_TOKEN").map_err(|_| "GITLAB_TOKEN is not set".to_string())?;
        GitLab::new(host, &token)
    }

    /// `https://<host>/api/v4/projects/<project>/<path…>`; the project path is one
    /// segment, its slashes encoded.
    fn project_url(&self, project: &str, path: &[&str]) -> Result<Url, String> {
        let mut url = Url::parse(&format!("https://{}/api/v4/projects", self.host))
            .map_err(|e| e.to_string())?;
        url.path_segments_mut()
            .map_err(|_| format!("bad GitLab host {}", self.host))?
            .push(project)
            .extend(path);
        Ok(url)
    }

    fn get(
        &self,
        url: Url,
        query: &[(&str, &str)],
    ) -> Result<(Response, super::http::Permit<'static>), String> {
        let permit = super::http::acquire();
        self.client
            .get(url)
            .query(query)
            .send()
            .and_then(|r| r.error_for_status())
            .map(|response| (response, permit))
            .map_err(|e| e.to_string())
    }

    /// My open merge requests in `project`. Plain REST with `scope=created_by_me`:
    /// the gitlab crate spends two requests on the token check and the current
    /// user first.
    pub fn my_open_merge_requests(&self, project: &str) -> Result<Vec<MergeRequest>, String> {
        self.my_merge_requests(project, &[("state", "opened")])
    }

    /// My merge requests in `project` updated in the last `days` days, open or not.
    pub fn my_recent_merge_requests(
        &self,
        project: &str,
        days: i64,
    ) -> Result<Vec<MergeRequest>, String> {
        let since = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        self.my_merge_requests(project, &[("state", "all"), ("updated_after", &since)])
    }

    fn my_merge_requests(
        &self,
        project: &str,
        filters: &[(&str, &str)],
    ) -> Result<Vec<MergeRequest>, String> {
        let url = self.project_url(project, &["merge_requests"])?;
        merge_request_pages(|page| {
            let page = page.to_string();
            let mut query = vec![
                ("scope", "created_by_me"),
                ("per_page", "100"),
                ("page", page.as_str()),
            ];
            query.extend_from_slice(filters);
            let (response, _permit) = self.get(url.clone(), &query)?;
            let last = response
                .headers()
                .get("x-next-page")
                .is_none_or(|v| v.is_empty());
            let mrs = response
                .json::<Vec<MergeRequest>>()
                .map_err(|e| e.to_string())?;
            Ok((mrs, last))
        })
    }

    pub fn mr_review(&self, project: &str, iid: u64) -> Result<MrReview, String> {
        let iid = iid.to_string();
        let get = |tail: &[&str], query: &[(&str, &str)]| -> Result<serde_json::Value, String> {
            let path = [&["merge_requests", iid.as_str()], tail].concat();
            let (response, _permit) = self.get(self.project_url(project, &path)?, query)?;
            response.json().map_err(|e| e.to_string())
        };
        let (mr, approvals, discussions) = std::thread::scope(|scope| {
            let approvals = scope.spawn(|| get(&["approvals"], &[]));
            let discussions = scope.spawn(|| {
                discussion_pages(|page| {
                    let page_text = page.to_string();
                    let (response, _permit) = self.get(
                        self.project_url(project, &["merge_requests", &iid, "discussions"])?,
                        &[("per_page", "100"), ("page", &page_text)],
                    )?;
                    let next = match response.headers().get("x-next-page") {
                        Some(value) if value.is_empty() => Some(None),
                        Some(value) => Some(Some(
                            value
                                .to_str()
                                .map_err(|e| e.to_string())?
                                .parse::<u32>()
                                .map_err(|e| e.to_string())?,
                        )),
                        None => None,
                    };
                    let threads: Vec<serde_json::Value> =
                        response.json().map_err(|e| e.to_string())?;
                    let next = next.unwrap_or_else(|| (threads.len() == 100).then_some(page + 1));
                    Ok((threads, next))
                })
            });
            (
                get(&[], &[]),
                joined(approvals, "a GitLab request"),
                joined(discussions, "a GitLab request"),
            )
        });
        let pipeline = mr?
            .pointer("/head_pipeline/status")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let approved_by = approvals?
            .get("approved_by")
            .and_then(|v| v.as_array())
            .map(|users| {
                users
                    .iter()
                    .filter_map(|u| u.pointer("/user/username").and_then(|n| n.as_str()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let unresolved = discussions?
            .iter()
            .filter(|d| {
                d.pointer("/notes/0/resolvable").and_then(|v| v.as_bool()) == Some(true)
                    && d.pointer("/notes/0/resolved").and_then(|v| v.as_bool()) == Some(false)
            })
            .count();
        Ok(MrReview {
            pipeline,
            approved_by,
            unresolved,
        })
    }
}

fn discussion_pages(
    mut fetch: impl FnMut(u32) -> Result<(Vec<serde_json::Value>, Option<u32>), String>,
) -> Result<Vec<serde_json::Value>, String> {
    let mut found = Vec::new();
    let mut page = 1;
    loop {
        let (threads, next) = fetch(page)?;
        if next.is_some() && threads.is_empty() {
            return Err("incomplete GitLab discussions: empty page before next page".into());
        }
        found.extend(threads);
        match next {
            None => return Ok(found),
            Some(next) if next > page => page = next,
            _ => return Err("invalid GitLab discussions pagination".into()),
        }
    }
}

/// Follow every page in the bounded time window; a failed page must fail the
/// source, so callers use the cache rather than treating a partial list as live.
fn merge_request_pages(
    mut fetch: impl FnMut(u32) -> Result<(Vec<MergeRequest>, bool), String>,
) -> Result<Vec<MergeRequest>, String> {
    let mut mrs = Vec::new();
    for page in 1.. {
        let (listed, last) = fetch(page)?;
        mrs.extend(listed);
        if last {
            break;
        }
    }
    Ok(mrs)
}

/// Review state of an open MR: three more requests, so only for open ones.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MrReview {
    /// Head pipeline status: "running", "success", "failed", "pending"…
    pub pipeline: Option<String>,
    pub approved_by: Vec<String>,
    /// Resolvable discussions not resolved yet.
    pub unresolved: usize,
}

/// GitLab's merge request states; anything else is `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MrState {
    Opened,
    Merged,
    Closed,
    Locked,
    #[serde(other)]
    Other,
}

impl MrState {
    pub fn as_str(self) -> &'static str {
        match self {
            MrState::Opened => "opened",
            MrState::Merged => "merged",
            MrState::Closed => "closed",
            MrState::Locked => "locked",
            MrState::Other => "other",
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct MergeRequest {
    iid: u64,
    title: String,
    state: MrState,
    web_url: String,
    source_branch: String,
    sha: Option<String>,
    /// GitLab's verdict on what blocks the merge: "mergeable", "ci_still_running",
    /// "ci_must_pass", "discussions_not_resolved", "not_approved", "conflict"…
    #[serde(default)]
    pub detailed_merge_status: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub has_conflicts: bool,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

impl MergeRequest {
    pub fn iid(&self) -> u64 {
        self.iid
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn state(&self) -> MrState {
        self.state
    }

    pub fn web_url(&self) -> &str {
        &self.web_url
    }

    pub fn source_branch(&self) -> &str {
        &self.source_branch
    }

    /// Head commit of the source branch when the MR was last updated.
    pub fn sha(&self) -> Option<&str> {
        self.sha.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::GitLab;

    #[test]
    fn mr_pagination_has_no_three_page_truncation_and_propagates_errors() {
        let mut calls = Vec::new();
        let mrs = super::merge_request_pages(|page| {
            calls.push(page);
            let mr = serde_json::from_value(serde_json::json!({
                "iid": page, "title": "x", "state": "merged", "source_branch": "x",
                "web_url": "https://gitlab/mr/x", "sha": null
            }))
            .unwrap();
            Ok((vec![mr], page == 4))
        })
        .unwrap();
        assert_eq!(calls, vec![1, 2, 3, 4]);
        assert_eq!(mrs.len(), 4);
        assert!(super::merge_request_pages(|_| Err("unavailable".into())).is_err());
        let empty = super::merge_request_pages(|_| Ok((vec![], true))).unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn project_path_is_one_segment() {
        // URL construction needs no system proxy lookup or network access.
        let gitlab = GitLab {
            client: reqwest::blocking::Client::builder()
                .no_proxy()
                .build()
                .unwrap(),
            host: "gitlab.example".into(),
        };
        let url = gitlab
            .project_url("demo/service", &["merge_requests", "7", "approvals"])
            .unwrap();
        assert_eq!(
            url.as_str(),
            "https://gitlab.example/api/v4/projects/demo%2Fservice/merge_requests/7/approvals"
        );
        let empty: &[(&str, &str)] = &[];
        let request = gitlab.client.get(url.clone()).query(empty).build().unwrap();
        assert_eq!(request.url().query(), None);
        let request = gitlab
            .client
            .get(url)
            .query(&[("updated_after", "2026-10-01T00:00:00+00:00")])
            .build()
            .unwrap();
        assert_eq!(
            request.url().query(),
            Some("updated_after=2026-10-01T00%3A00%3A00%2B00%3A00")
        );
    }
}

#[cfg(test)]
mod discussion_tests {
    use super::*;

    #[test]
    fn follows_discussions_past_the_first_hundred_and_propagates_failure() {
        let mut calls = Vec::new();
        let discussions = discussion_pages(|page| {
            calls.push(page);
            Ok(if page == 1 {
                (
                    vec![serde_json::json!({"notes":[{"resolvable":true,"resolved":true}]}); 100],
                    Some(2),
                )
            } else {
                (
                    vec![serde_json::json!({"notes":[{"resolvable":true,"resolved":false}]})],
                    None,
                )
            })
        })
        .unwrap();
        assert_eq!(calls, [1, 2]);
        assert_eq!(discussions.len(), 101);
        assert_eq!(
            discussions
                .iter()
                .filter(|d| d.pointer("/notes/0/resolved").and_then(|v| v.as_bool()) == Some(false))
                .count(),
            1
        );
        assert!(
            discussion_pages(|page| if page == 1 {
                Ok((vec![serde_json::json!({})], Some(2)))
            } else {
                Err("page failed".into())
            })
            .is_err()
        );
        assert!(discussion_pages(|_| Ok((vec![serde_json::json!({})], Some(1)))).is_err());
    }
}
