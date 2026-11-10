//! Ticket identity and the small provider-neutral context used by work commands.
use super::jira::JiraIssue;

pub struct Issue {
    pub key: String,
    pub summary: String,
    pub description: Option<String>,
}

impl From<JiraIssue> for Issue {
    fn from(issue: JiraIssue) -> Self {
        Self {
            key: issue.key,
            summary: issue.fields.summary,
            description: issue.fields.description,
        }
    }
}

/// Stored GitHub identity: github.com/owner/repo#number. Never store a bare #N.
pub fn github_key(text: &str) -> Option<String> {
    let text = text.trim();
    let (repo, number) = if let Ok(url) = reqwest::Url::parse(text) {
        if url.scheme() != "https"
            || url.host_str()? != "github.com"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
        {
            return None;
        }
        let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
        if parts.len() != 4 || parts[2] != "issues" {
            return None;
        }
        (format!("{}/{}", parts[0], parts[1]), parts[3].to_string())
    } else {
        let text = if text.to_ascii_lowercase().starts_with("github.com/") {
            &text["github.com/".len()..]
        } else {
            text
        };
        let (repo, number) = text.rsplit_once('#')?;
        (repo.to_string(), number.to_string())
    };
    let repo = github_repo(&repo).ok()?;
    if !number.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let number = number.parse::<u64>().ok().filter(|n| *n > 0)?;
    Some(format!("github.com/{repo}#{number}"))
}

pub fn github_repo(text: &str) -> Result<String, String> {
    let parts: Vec<_> = text.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|p| {
            p.is_empty()
                || *p == "."
                || *p == ".."
                || !p
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        })
    {
        return Err(format!(
            "invalid GitHub repository {text:?}: expected owner/repo"
        ));
    }
    Ok(text.to_ascii_lowercase())
}

pub fn github_parts(key: &str) -> Option<(&str, u64)> {
    let (repo, n) = key.strip_prefix("github.com/")?.rsplit_once('#')?;
    Some((repo, n.parse().ok()?))
}

pub fn url(key: &str) -> Option<String> {
    let (repo, number) = github_parts(key)?;
    Some(format!("https://github.com/{repo}/issues/{number}"))
}

/// Keys used in notes and command arguments. Jira extraction stays unchanged.
pub fn keys(text: &str) -> Vec<String> {
    if let Some(key) = github_key(text) {
        return vec![key];
    }
    crate::vault::task::jira_keys(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn github_identity_is_qualified_and_canonical() {
        for text in [
            "Owner/Repo#12",
            "github.com/Owner/Repo#12",
            "GitHub.com/Owner/Repo#12",
            "https://github.com/Owner/Repo/issues/12?x=1",
        ] {
            assert_eq!(
                github_key(text).as_deref(),
                Some("github.com/owner/repo#12")
            );
        }
        for text in [
            "#12",
            "owner/repo#0",
            "owner/repo#-1",
            "owner/repo#+1",
            "https://github.com/a/b/pull/12",
            "https://enterprise.invalid/a/b/issues/12",
            "https://user@github.com/a/b/issues/12",
        ] {
            assert!(github_key(text).is_none(), "{text}");
        }
        assert_ne!(github_key("a/one#1"), github_key("a/two#1"));
    }
}
