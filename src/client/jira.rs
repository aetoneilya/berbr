use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct JiraClient {
    client: Client,
    base_url: String,
}

#[derive(Debug, Error)]
pub enum JiraError {
    #[error("incomplete Jira search: {0}")]
    Incomplete(String),
    #[error("failed during request execution: {0}")]
    RequestExecutionError(#[from] reqwest::Error),

    /// The status and the start of the response body.
    #[error("Jira answered {0}: {1}")]
    RequestResultError(StatusCode, String),

    #[error("JIRA_TOKEN can't be sent in a header: {0}")]
    BadToken(#[from] reqwest::header::InvalidHeaderValue),
}

pub(crate) type Result<T> = std::result::Result<T, JiraError>;

impl JiraError {
    /// Jira refused the query itself (400), e.g. a `key in (…)` with an unknown key.
    pub fn bad_request(&self) -> bool {
        matches!(self, JiraError::RequestResultError(status, _) if *status == StatusCode::BAD_REQUEST)
    }

    /// No such issue, or not one I may see (404).
    pub fn not_found(&self) -> bool {
        matches!(self, JiraError::RequestResultError(status, _) if *status == StatusCode::NOT_FOUND)
    }
}

/// How much of an error response goes into the message: Jira answers with whole
/// HTML pages.
const ERROR_BODY_CHARS: usize = 300;

/// The response when it is a success, otherwise its status and body as an error.
fn checked(response: Response) -> Result<Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let text = response.text().unwrap_or_default();
    let mut body: String = text.trim().chars().take(ERROR_BODY_CHARS).collect();
    if body.len() < text.trim().len() {
        body.push('…');
    }
    Err(JiraError::RequestResultError(status, body))
}

impl JiraClient {
    /// The token is trimmed: one read from a file or a shell often ends in a newline.
    pub fn new(base_url: impl Into<String>, token: &str) -> Result<Self> {
        let mut headers = HeaderMap::new();
        let mut auth = HeaderValue::from_str(&format!("Bearer {}", token.trim()))?;
        auth.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let client = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(5))
            .build()?;
        Ok(JiraClient {
            client,
            base_url: base_url.into(),
        })
    }

    /// A client with the token from `JIRA_TOKEN`.
    pub fn from_env(base_url: &str) -> std::result::Result<Self, String> {
        let token = std::env::var("JIRA_TOKEN").map_err(|_| "JIRA_TOKEN is not set".to_string())?;
        JiraClient::new(base_url, &token).map_err(|err| err.to_string())
    }

    fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        query: &[(&str, &str)],
    ) -> Result<T> {
        let _permit = super::http::acquire();
        let response = self.client.get(url).query(query).send()?;
        Ok(checked(response)?.json()?)
    }

    /// The workflow transitions available on the issue now.
    pub fn transitions(&self, key: &str) -> Result<Vec<Transition>> {
        let url = format!("{}/rest/api/2/issue/{}/transitions", self.base_url, key);
        let result: Transitions = self.get_json(&url, &[])?;
        Ok(result.transitions)
    }

    pub fn transition(&self, key: &str, transition_id: &str) -> Result<()> {
        let url = format!("{}/rest/api/2/issue/{}/transitions", self.base_url, key);
        let _permit = super::http::acquire();
        let response = self
            .client
            .post(&url)
            .timeout(Duration::from_secs(10))
            .json(&serde_json::json!({ "transition": { "id": transition_id } }))
            .send()?;
        checked(response).map(|_| ())
    }

    /// Creates a ticket assigned to me; returns its key.
    pub fn create_issue(&self, project: &str, issue_type: &str, summary: &str) -> Result<String> {
        let me: Myself = self.get_json(&format!("{}/rest/api/2/myself", self.base_url), &[])?;
        let url = format!("{}/rest/api/2/issue", self.base_url);
        let fields = serde_json::json!({
            "project": { "key": project },
            "issuetype": { "name": issue_type },
            "summary": summary,
            "assignee": { "name": me.name },
        });
        let _permit = super::http::acquire();
        let response = self
            .client
            .post(&url)
            .timeout(Duration::from_secs(10))
            .json(&serde_json::json!({ "fields": fields }))
            .send()?;
        let created: Created = checked(response)?.json()?;
        Ok(created.key)
    }

    pub fn get_issue_by_key(&self, key: &str) -> Result<JiraIssue> {
        let url = format!("{}/rest/api/2/issue/{}", self.base_url, key);
        self.get_json(&url, &[])
    }

    /// Search returning only the fields `JiraIssue` needs, up to `max` issues.
    pub fn search_issues_brief(&self, jql: &str, max: usize) -> Result<Vec<JiraIssue>> {
        let url = format!("{}/rest/api/2/search", self.base_url);
        search_pages(max, |start, limit| {
            let start = start.to_string();
            let limit = limit.to_string();
            self.get_json(
                &url,
                &[
                    ("jql", jql),
                    ("startAt", &start),
                    ("maxResults", &limit),
                    ("fields", "summary,status,created,updated,assignee"),
                ],
            )
        })
    }
}

#[derive(Debug, Deserialize)]
struct Myself {
    name: String,
}

#[derive(Debug, Deserialize)]
struct Created {
    key: String,
}

#[derive(Debug, Deserialize)]
struct Transitions {
    transitions: Vec<Transition>,
}

#[derive(Debug, Deserialize)]
pub struct Transition {
    pub id: String,
    pub name: String,
    pub to: TransitionTarget,
}

#[derive(Debug, Deserialize)]
pub struct TransitionTarget {
    pub name: String,
    #[serde(rename = "statusCategory")]
    pub status_category: StatusCategory,
}

#[derive(Debug, Deserialize)]
struct JiraSearchResult {
    issues: Vec<JiraIssue>,
    #[serde(default, rename = "startAt")]
    start_at: Option<usize>,
    #[serde(default)]
    total: Option<usize>,
}

fn search_pages(
    max: usize,
    mut fetch: impl FnMut(usize, usize) -> Result<JiraSearchResult>,
) -> Result<Vec<JiraIssue>> {
    let mut issues = Vec::new();
    let mut keys = std::collections::HashSet::new();
    while issues.len() < max {
        let start = issues.len();
        let limit = (max - start).min(100);
        let page = fetch(start, limit)?;
        if page.start_at.is_some_and(|offset| offset != start) {
            return Err(JiraError::Incomplete(format!(
                "expected startAt {start}, got {:?}",
                page.start_at
            )));
        }
        let count = page.issues.len();
        for issue in page.issues.into_iter().take(max - start) {
            if !keys.insert(issue.key.clone()) {
                return Err(JiraError::Incomplete(format!(
                    "repeated key {} on page at {start}",
                    issue.key
                )));
            }
            issues.push(issue);
        }
        if page.total.is_some_and(|total| issues.len() >= total) {
            break;
        }
        if page.total.is_none() && count < limit {
            break;
        }
        if count == 0 {
            return Err(JiraError::Incomplete(format!(
                "empty page at {start} before total {:?}",
                page.total
            )));
        }
    }
    Ok(issues)
}

#[derive(Debug, Deserialize)]
pub struct JiraIssue {
    pub key: String,
    pub fields: JiraIssueFields,
}

impl JiraIssue {
    pub fn status_category(&self) -> Category {
        self.fields
            .status
            .as_ref()
            .and_then(|s| s.status_category.as_ref())
            .map_or(Category::Unknown, |c| c.key)
    }

    pub fn status(&self) -> &str {
        self.fields
            .status
            .as_ref()
            .map(|s| s.name.as_str())
            .unwrap_or("unknown")
    }
}

#[derive(Debug, Deserialize)]
pub struct JiraIssueFields {
    pub summary: String,
    pub description: Option<String>,
    pub created: DateTime<FixedOffset>,
    pub status: Option<Status>,
}

#[derive(Debug, Deserialize)]
pub struct Status {
    pub name: String,
    #[serde(rename = "statusCategory")]
    pub status_category: Option<StatusCategory>,
}

#[derive(Debug, Deserialize)]
pub struct StatusCategory {
    pub key: Category,
}

/// Jira's coarse status, the `statusCategory` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Deserialize)]
pub enum Category {
    #[serde(rename = "new")]
    New,
    #[serde(rename = "indeterminate")]
    InProgress,
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "undefined", other)]
    Unknown,
}

#[cfg(test)]
mod pagination_tests {
    use super::*;

    fn page(start: usize, count: usize, total: usize) -> JiraSearchResult {
        serde_json::from_value(serde_json::json!({
            "startAt": start, "maxResults": count, "total": total,
            "issues": (start..start + count).map(|n| serde_json::json!({
                "key": format!("DEMO-{}", n + 1),
                "fields": {"summary":"Synthetic issue", "created":"2026-11-01T00:00:00Z"}
            })).collect::<Vec<_>>()
        }))
        .unwrap()
    }

    #[test]
    fn follows_server_caps_and_stops_at_the_requested_bound() {
        let mut starts = Vec::new();
        let issues = search_pages(80, |start, limit| {
            starts.push((start, limit));
            Ok(page(start, 30.min(80 - start), 120))
        })
        .unwrap();
        assert_eq!(starts, [(0, 80), (30, 50), (60, 20)]);
        assert_eq!(issues.len(), 80);
        let issues = search_pages(usize::MAX, |start, _| {
            Ok(page(start, 30.min(230 - start), 230))
        })
        .unwrap();
        assert_eq!(issues.len(), 230);
        assert!(
            search_pages(0, |_, _| panic!("no request for zero bound"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn incomplete_repeated_and_failed_pages_are_errors() {
        assert!(
            search_pages(80, |start, _| Ok(page(
                start,
                if start == 0 { 50 } else { 0 },
                80
            )))
            .is_err()
        );
        assert!(search_pages(80, |_, _| Ok(page(0, 50, 80))).is_err());
        assert!(
            search_pages(80, |start, _| {
                if start > 0 {
                    Err(JiraError::Incomplete("page failed".into()))
                } else {
                    Ok(page(0, 50, 80))
                }
            })
            .is_err()
        );
        assert!(
            search_pages(80, |_, _| Ok(page(0, 0, 0)))
                .unwrap()
                .is_empty()
        );
    }
}
