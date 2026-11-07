use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The CLI that owns a conversation. Bare historical session ids mean Claude.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    #[default]
    Claude,
    Codex,
}

#[derive(Debug, Serialize)]
pub struct ModelChoice {
    pub id: String,
    pub label: String,
}

/// Explicit picker options. Codex's own cache reflects this installation's catalog.
pub fn model_choices(kind: AgentKind) -> Vec<ModelChoice> {
    let mut choices = vec![ModelChoice {
        id: String::new(),
        label: "CLI default".into(),
    }];
    match kind {
        AgentKind::Claude => {
            for (id, label) in [
                ("haiku", "Haiku"),
                ("sonnet", "Sonnet"),
                ("opus", "Opus"),
                ("fable", "Fable"),
            ] {
                choices.push(ModelChoice {
                    id: id.into(),
                    label: label.into(),
                });
            }
        }
        AgentKind::Codex => {
            let home = std::env::var_os("CODEX_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".codex"));
            if let Ok(data) = std::fs::read(home.join("models_cache.json")) {
                choices.extend(codex_model_choices(&data));
            }
        }
    }
    choices
}

fn codex_model_choices(data: &[u8]) -> Vec<ModelChoice> {
    let mut choices: Vec<ModelChoice> = Vec::new();
    let Ok(data) = serde_json::from_slice::<serde_json::Value>(data) else {
        return choices;
    };
    for model in data["models"].as_array().into_iter().flatten() {
        if model["visibility"].as_str() != Some("list") {
            continue;
        }
        if let Some(id) = model["slug"].as_str().filter(|id| !id.is_empty()) {
            if choices.iter().any(|choice| choice.id == id) {
                continue;
            }
            choices.push(ModelChoice {
                id: id.into(),
                label: model["display_name"].as_str().unwrap_or(id).into(),
            });
        }
    }
    choices
}

pub fn validate_naming_model(kind: AgentKind, model: &str) -> Result<(), String> {
    if model_choices(kind).iter().any(|choice| choice.id == model) {
        Ok(())
    } else {
        Err(format!(
            "model {model:?} is not an available Task naming model for {kind}; choose a listed model or CLI default"
        ))
    }
}

impl AgentKind {
    pub fn binary(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    pub fn from_session_key(key: &str) -> Self {
        if key.starts_with("codex:") {
            Self::Codex
        } else {
            Self::Claude
        }
    }

    pub fn session_key(self, id: &str) -> String {
        match self {
            Self::Claude => id.to_string(),
            Self::Codex => format!("codex:{id}"),
        }
    }

    pub fn native_id(key: &str) -> &str {
        key.strip_prefix("codex:").unwrap_or(key)
    }

    pub fn resume_args(self, key: &str) -> Vec<String> {
        let flag = match self {
            Self::Claude => "--resume",
            Self::Codex => "resume",
        };
        vec![flag.into(), Self::native_id(key).into()]
    }

    /// Codex owns its runtime here instead of attaching to the shared daemon.
    pub fn shell_prefix(self) -> &'static str {
        match self {
            Self::Claude => "command claude",
            Self::Codex => "command codex --no-daemon",
        }
    }

    pub fn named_shell_prefix(self, label: &str) -> String {
        if self == Self::Claude {
            format!(
                "{} --name {}",
                self.shell_prefix(),
                super::herdr::shell_quote(label)
            )
        } else {
            self.shell_prefix().to_string()
        }
    }

    pub fn named_shell_command(self, label: &str, args: &[&str]) -> String {
        let command = self.shell_command(args);
        if self == Self::Claude && !args.contains(&"--name") {
            command.replacen(self.shell_prefix(), &self.named_shell_prefix(label), 1)
        } else {
            command
        }
    }

    pub fn shell_command(self, args: &[&str]) -> String {
        let mut command = self.shell_prefix().to_string();
        for arg in args {
            command.push(' ');
            command.push_str(&super::herdr::shell_quote(arg));
        }
        command
    }
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.binary())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_catalog_filters_hidden_and_duplicate_ids() {
        let choices = codex_model_choices(
            br#"{"models":[
            {"slug":"first", "display_name":"First", "visibility":"list"},
            {"slug":"hidden", "visibility":"hide"},
            {"slug":"first", "visibility":"list"},
            {"slug":"second", "visibility":"list"},
            {"visibility":"list"}
        ]}"#,
        );
        assert_eq!(
            choices.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["first", "second"]
        );
        assert_eq!(choices[0].label, "First");
        assert_eq!(choices[1].label, "second");
        assert!(codex_model_choices(b"broken cache").is_empty());
        assert!(codex_model_choices(br#"{}"#).is_empty());
    }

    #[test]
    fn naming_models_require_a_listed_choice_and_always_allow_cli_default() {
        assert!(validate_naming_model(AgentKind::Claude, "haiku").is_ok());
        assert!(validate_naming_model(AgentKind::Claude, "").is_ok());
        assert!(validate_naming_model(AgentKind::Codex, "").is_ok());
        assert!(validate_naming_model(AgentKind::Claude, "invented-model").is_err());
    }

    #[test]
    fn session_keys_keep_legacy_claude_ids_and_separate_codex() {
        assert_eq!(AgentKind::Claude.session_key("same"), "same");
        assert_eq!(AgentKind::Codex.session_key("same"), "codex:same");
        assert_eq!(AgentKind::from_session_key("same"), AgentKind::Claude);
        assert_eq!(AgentKind::from_session_key("codex:same"), AgentKind::Codex);
        assert_eq!(AgentKind::Claude.resume_args("same"), ["--resume", "same"]);
        assert_eq!(
            AgentKind::Codex.resume_args("codex:same"),
            ["resume", "same"]
        );
    }

    #[test]
    fn codex_start_and_resume_disable_the_shared_daemon() {
        assert_eq!(
            AgentKind::Codex.shell_command(&[]),
            "command codex --no-daemon"
        );
        assert_eq!(
            AgentKind::Codex.shell_command(&["prompt"]),
            "command codex --no-daemon 'prompt'"
        );
        let args = AgentKind::Codex.resume_args("codex:session-id");
        let args: Vec<_> = args.iter().map(String::as_str).collect();
        assert_eq!(
            AgentKind::Codex.shell_command(&args),
            "command codex --no-daemon 'resume' 'session-id'"
        );
        assert_eq!(
            AgentKind::Claude.shell_command(&["--resume", "session-id"]),
            "command claude '--resume' 'session-id'"
        );
    }

    #[test]
    fn commands_pass_prompt_and_session_as_literal_arguments() {
        for kind in [AgentKind::Claude, AgentKind::Codex] {
            let prompt = "read 'my note'; $(touch /tmp/do-not-create)";
            let command = kind.shell_command(&[prompt]);
            assert!(command.contains("'\\''my note'\\''"));
            assert!(command.starts_with(&format!("{} '", kind.shell_prefix())));
        }
    }
}

#[cfg(test)]
mod display_name_tests {
    use super::*;

    #[test]
    fn claude_display_names_are_literal_and_codex_has_no_name_flag() {
        assert_eq!(
            AgentKind::Claude.named_shell_command("Parser's task", &["prompt"]),
            "command claude --name 'Parser'\\''s task' 'prompt'"
        );
        assert_eq!(
            AgentKind::Claude.named_shell_command("default", &["--name", "explicit", "prompt"]),
            "command claude '--name' 'explicit' 'prompt'"
        );
        assert_eq!(
            AgentKind::Codex.named_shell_command("ignored", &["prompt"]),
            "command codex --no-daemon 'prompt'"
        );
    }
}
