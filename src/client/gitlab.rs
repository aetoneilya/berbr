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
        let start = std::time::Instant::now();
        let response = self.client.get(url.clone()).query(query).send();
        super::http::log("GET", url.as_str(), start, &response);
        response
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
        let mr = mr?;
        let pipeline_review = self.pipeline_review(project, &iid, &mr).ok().flatten();
        let pipeline = mr
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
            pipeline_review,
            approved_by,
            unresolved,
        })
    }

    /// External review contexts participate in GitLab's aggregate pipeline status.
    /// Inspect the entire head pipeline, including bridge jobs, before separating
    /// that human gate from running CI. Any missing evidence keeps the raw status.
    fn pipeline_review(
        &self,
        project: &str,
        iid: &str,
        mr: &serde_json::Value,
    ) -> Result<Option<PipelineReview>, String> {
        let Some(head) = pipeline_identity(mr) else {
            return Ok(None);
        };
        if !matches!(head.status.as_str(), "running" | "pending") {
            return Ok(None);
        }
        let id = head.pipeline_id.to_string();
        let (statuses, jobs, bridges) = std::thread::scope(|scope| {
            let jobs =
                scope.spawn(|| self.pipeline_items(project, &["pipelines", &id, "jobs"], &[]));
            let bridges =
                scope.spawn(|| self.pipeline_items(project, &["pipelines", &id, "bridges"], &[]));
            (
                self.pipeline_items(
                    project,
                    &["repository", "commits", &head.pipeline_sha, "statuses"],
                    &[("pipeline_id", &id)],
                ),
                joined(jobs, "GitLab pipeline jobs"),
                joined(bridges, "GitLab pipeline bridges"),
            )
        });
        if !only_review_pending(&head, &statuses?, &jobs?, &bridges?) {
            return Ok(None);
        }
        // The source head and synthetic merged-results head are distinct. Compare
        // each to itself after inspection, never require their SHAs to be equal.
        let (response, _permit) =
            self.get(self.project_url(project, &["merge_requests", iid])?, &[])?;
        let current = response.json().map_err(|e| e.to_string())?;
        Ok((pipeline_identity(&current).as_ref() == Some(&head)).then_some(head))
    }

    fn pipeline_items(
        &self,
        project: &str,
        path: &[&str],
        query: &[(&str, &str)],
    ) -> Result<Vec<serde_json::Value>, String> {
        pipeline_pages(|page| {
            let page = page.to_string();
            let mut query = query.to_vec();
            query.extend([("per_page", "100"), ("page", page.as_str())]);
            let (response, _permit) = self.get(self.project_url(project, path)?, &query)?;
            let next = response
                .headers()
                .get("x-next-page")
                .map(|v| {
                    let value = v.to_str().map_err(|e| e.to_string())?;
                    if value.is_empty() {
                        Ok(None)
                    } else {
                        value.parse::<u32>().map(Some).map_err(|e| e.to_string())
                    }
                })
                .transpose()?;
            let items: Vec<serde_json::Value> = response.json().map_err(|e| e.to_string())?;
            Ok((items, next))
        })
    }
}

/// Provenance for the exact external `codereview` context, not a job-name
/// substring heuristic. Absent in old caches and when inspection is incomplete.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PipelineReview {
    pub pipeline_id: u64,
    pub pipeline_sha: String,
    pub source_sha: String,
    pub status: String,
    pub context: String,
}

fn pipeline_identity(mr: &serde_json::Value) -> Option<PipelineReview> {
    let p = mr.get("head_pipeline")?;
    Some(PipelineReview {
        pipeline_id: p.get("id")?.as_u64()?,
        pipeline_sha: p.get("sha")?.as_str().filter(|s| !s.is_empty())?.into(),
        source_sha: mr.get("sha")?.as_str().filter(|s| !s.is_empty())?.into(),
        status: p.get("status")?.as_str()?.into(),
        context: "codereview".into(),
    })
}

fn only_review_pending(
    head: &PipelineReview,
    statuses: &[serde_json::Value],
    jobs: &[serde_json::Value],
    bridges: &[serde_json::Value],
) -> bool {
    let settled = |s: &serde_json::Value| {
        matches!(s["status"].as_str(), Some("success" | "skipped"))
            || (s["status"] == "manual" && s["allow_failure"] == true)
    };
    // Empty/incomplete job evidence is insufficient. Running AI review jobs are
    // computation too; only a status absent from the jobs API is an external gate.
    if jobs.is_empty()
        || !jobs
            .iter()
            .chain(bridges)
            .all(|job| job["id"].as_u64().is_some() && settled(job))
    {
        return false;
    }
    // A successful trigger can mean only that GitLab created the downstream
    // pipeline. Require its own success; never infer completion from the bridge.
    // Skipped/unplayed optional triggers may legitimately have no downstream.
    if bridges.iter().any(|bridge| {
        let downstream = &bridge["downstream_pipeline"];
        (bridge["status"] == "success" || !downstream.is_null())
            && (downstream["id"].as_u64().is_none() || downstream["status"] != "success")
    }) {
        return false;
    }
    let mut gate = false;
    for status in statuses {
        if status["pipeline_id"].as_u64() != Some(head.pipeline_id)
            || status["sha"].as_str() != Some(head.pipeline_sha.as_str())
        {
            return false;
        }
        let external = status["id"].as_u64().is_some_and(|id| {
            jobs.iter()
                .chain(bridges)
                .all(|job| job["id"].as_u64() != Some(id))
        });
        if external
            && status["allow_failure"] == false
            && status["name"] == "codereview"
            && matches!(status["status"].as_str(), Some("pending" | "running"))
        {
            gate = true;
        } else if !settled(status) {
            return false;
        }
    }
    gate && jobs.iter().any(|job| job["status"] == "success")
}

fn pipeline_pages(
    mut fetch: impl FnMut(u32) -> Result<(Vec<serde_json::Value>, Option<Option<u32>>), String>,
) -> Result<Vec<serde_json::Value>, String> {
    let mut found = Vec::new();
    let mut page = 1;
    for _ in 0..100 {
        let (items, next) = fetch(page)?;
        let next = next.unwrap_or_else(|| (items.len() == 100).then_some(page + 1));
        if next.is_some() && items.is_empty() {
            return Err("incomplete GitLab pipeline page".into());
        }
        found.extend(items);
        match next {
            None => return Ok(found),
            Some(n) if n == page + 1 => page = n,
            _ => return Err("invalid GitLab pipeline pagination".into()),
        }
    }
    Err("GitLab pipeline pagination limit exceeded".into())
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline_review: Option<PipelineReview>,
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
    /// Absent in legacy GitLab caches. GitHub uses the same internal change model.
    #[serde(default)]
    pub github: bool,
    /// Qualified source repository, distinct from the target for fork PRs.
    #[serde(default)]
    pub head_repo: Option<String>,
    #[serde(default)]
    pub review_decision: Option<String>,
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
    pub fn number_label(&self) -> String {
        format!("{}{}", if self.github { "#" } else { "!" }, self.iid)
    }

    pub fn kind(&self) -> &'static str {
        if self.github { "PR" } else { "MR" }
    }

    pub fn provider(&self) -> &'static str {
        if self.github { "github" } else { "gitlab" }
    }
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

#[cfg(test)]
mod pipeline_review_tests {
    use super::*;
    use serde_json::{Value, json};

    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/gitlab-review-pipeline.json"
        ))
        .unwrap()
    }

    fn classified(f: &Value) -> bool {
        let head = pipeline_identity(&f["mr"]).unwrap();
        only_review_pending(
            &head,
            f["statuses"].as_array().unwrap(),
            f["jobs"].as_array().unwrap(),
            f["bridges"].as_array().unwrap(),
        )
    }

    #[test]
    fn external_review_gate_is_distinct_from_running_jobs_and_manual_actions() {
        let f = fixture();
        assert_ne!(f["mr"]["sha"], f["mr"]["head_pipeline"]["sha"]);
        assert!(classified(&f));
        let mut running_review = f.clone();
        running_review["statuses"][8]["status"] = json!("running");
        assert!(classified(&running_review));
        for state in [
            "running",
            "pending",
            "created",
            "preparing",
            "waiting_for_resource",
            "manual",
            "failed",
            "canceled",
            "unknown",
        ] {
            let mut changed = f.clone();
            changed["jobs"][0]["status"] = json!(state);
            changed["jobs"][0]["allow_failure"] = json!(false);
            assert!(!classified(&changed), "job {state}");
            let mut changed = f.clone();
            changed["statuses"][0]["status"] = json!(state);
            assert!(!classified(&changed), "status {state}");
            let mut changed = f.clone();
            changed["bridges"] =
                json!([{"id":99,"name":"downstream", "status":state,"allow_failure":false}]);
            assert!(!classified(&changed), "bridge {state}");
        }
        for name in ["ai-review", "review", "codereview-other"] {
            let mut changed = f.clone();
            changed["statuses"][8]["name"] = json!(name);
            assert!(!classified(&changed), "not the external context: {name}");
        }
        // An actual running job called codereview is still running computation.
        let mut changed = f.clone();
        let gate_job = changed["statuses"][8].clone();
        changed["jobs"].as_array_mut().unwrap().push(gate_job);
        assert!(!classified(&changed));
    }

    #[test]
    fn successful_bridge_requires_successful_downstream_evidence() {
        for (downstream, expected) in [
            (None, false),
            (Some(Value::Null), false),
            (Some(json!({"id":200})), false),
            (Some(json!({"status":"success"})), false),
            (Some(json!({"id":200,"status":"running"})), false),
            (Some(json!({"id":200,"status":"pending"})), false),
            (Some(json!({"id":200,"status":"failed"})), false),
            (Some(json!({"id":200,"status":"canceled"})), false),
            (Some(json!({"id":200,"status":"unknown"})), false),
            (Some(json!({"id":200,"status":"manual"})), false),
            (Some(json!({"id":200,"status":"skipped"})), false),
            (Some(json!({"id":200,"status":"success"})), true),
        ] {
            let mut f = fixture();
            let mut bridge = json!({"id":99,"status":"success","allow_failure":false});
            if let Some(value) = downstream {
                bridge["downstream_pipeline"] = value;
            }
            f["bridges"] = json!([bridge]);
            assert_eq!(classified(&f), expected, "bridge {}", f["bridges"]);
        }
        for status in ["manual", "skipped"] {
            let mut f = fixture();
            f["bridges"] = json!([{"id":99,"status":status,"allow_failure":true}]);
            assert!(classified(&f), "unplayed {status} trigger");
            f["bridges"][0]["downstream_pipeline"] = json!({"id":200,"status":"running"});
            assert!(
                !classified(&f),
                "existing active downstream behind {status} trigger"
            );
        }
    }

    #[test]
    fn missing_or_wrong_pipeline_evidence_cannot_claim_review_pending() {
        let f = fixture();
        for field in ["statuses", "jobs"] {
            let mut changed = f.clone();
            changed[field] = json!([]);
            assert!(!classified(&changed));
        }
        for (field, value) in [
            ("pipeline_id", json!(0)),
            ("pipeline_id", Value::Null),
            ("sha", json!("old-head")),
            ("sha", Value::Null),
            ("status", Value::Null),
            ("id", Value::Null),
        ] {
            let mut changed = f.clone();
            changed["statuses"][8][field] = value;
            assert!(!classified(&changed), "invalid {field}");
        }
        let original = pipeline_identity(&f["mr"]).unwrap();
        for path in [
            "/sha",
            "/head_pipeline/sha",
            "/head_pipeline/id",
            "/head_pipeline/status",
        ] {
            let mut changed = f["mr"].clone();
            *changed.pointer_mut(path).unwrap() = json!("changed");
            assert_ne!(pipeline_identity(&changed).as_ref(), Some(&original));
        }
    }

    #[test]
    fn pipeline_pagination_reads_every_page_and_fails_closed() {
        let mut calls = Vec::new();
        let items = pipeline_pages(|page| {
            calls.push(page);
            Ok((
                vec![json!({"status":"success"}); if page == 1 { 100 } else { 1 }],
                None,
            ))
        })
        .unwrap();
        assert_eq!(calls, [1, 2]);
        assert_eq!(items.len(), 101);
        assert!(
            pipeline_pages(|page| if page == 1 {
                Ok((vec![json!({})], Some(Some(2))))
            } else {
                Err("page failed".into())
            })
            .is_err()
        );
        assert!(pipeline_pages(|_| Ok((vec![], Some(Some(2))))).is_err());
        assert!(pipeline_pages(|_| Ok((vec![json!({})], Some(Some(1))))).is_err());
        assert!(pipeline_pages(|page| Ok((vec![json!({})], Some(Some(page + 1))))).is_err());
    }

    #[test]
    fn legacy_cache_roundtrip_preserves_raw_pipeline_and_optional_provenance() {
        let old = json!({"pipeline":"running","approved_by":[],"unresolved":0});
        let mut review: MrReview = serde_json::from_value(old.clone()).unwrap();
        assert!(review.pipeline_review.is_none());
        assert_eq!(serde_json::to_value(&review).unwrap(), old);
        review.pipeline_review = pipeline_identity(&fixture()["mr"]);
        let decoded: MrReview =
            serde_json::from_value(serde_json::to_value(&review).unwrap()).unwrap();
        assert_eq!(decoded.pipeline.as_deref(), Some("running"));
        assert_eq!(decoded.pipeline_review, review.pipeline_review);
    }
}
