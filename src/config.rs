use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::cli::Cli;
use crate::commit::{CleanupMode, CommitOptions};

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_ENDPOINT: &str = "/v1/chat/completions";
pub const DEFAULT_STAGED_DIFF_COMMAND: &str =
    "git diff --cached --stat && git diff --cached --binary --find-renames";
pub const DEFAULT_UNSTAGED_DIFF_COMMAND: &str =
    "git diff --stat && git diff --binary --find-renames";
pub const DEFAULT_UNTRACKED_DIFF_COMMAND: &str = "git ls-files --others --exclude-standard | while IFS= read -r file; do \
printf '\\nUntracked file: %s\\n' \"$file\"; \
git diff --no-index -- /dev/null \"$file\" || true; \
done";
pub const DEFAULT_UNSTAGED_FILES_COMMAND: &str = "git diff --name-only";
pub const DEFAULT_UNTRACKED_FILES_COMMAND: &str = "git ls-files --others --exclude-standard";
pub const DEFAULT_DIFF_COMMAND: &str = DEFAULT_STAGED_DIFF_COMMAND;
pub const DEFAULT_MAX_INPUT_CHARS: usize = 500_000;
pub const DEFAULT_MAX_FILE_CHARS: usize = 80_000;
pub(crate) const CHAT_MESSAGE_CHAR_LIMIT: usize = 10_485_760;
const MIN_DIFF_BUDGET_CHARS: usize = 4_000;

pub const DEFAULT_PROMPT: &str = r#"Generate a clear commit message in English based only on the provided diff.

Use Conventional Commits format for the subject when appropriate, such as "feat:",
"fix:", "docs:", "style:", "refactor:", "test:", "chore:", or "perf:".
Write the subject in the imperative mood, for example "fix: handle empty response".
Keep the subject concise, preferably under 100 characters.

After the subject, add a blank line and include a brief body when the diff contains
meaningful details, multiple changes, or behavior changes. The body should explain
what changed and why, using 1-3 concise bullet points or short sentences.

Do not simply repeat the subject. Do not use personal pronouns such as "I" or
"we". Do not end the subject with a period. Avoid vague messages like "update
code", "fix bug", or "misc changes". Return only the commit message, without
Markdown fences or commentary."#;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum IncludeUnstagedMode {
    Ask,
    Always,
    Never,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ReasoningEffort {
    pub const VALUES: &'static str = "none, minimal, low, medium, high, xhigh, max";

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "minimal" => Some(Self::Minimal),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::Xhigh),
            "max" => Some(Self::Max),
            _ => None,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct FileConfig {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub endpoint: Option<String>,
    pub model: Option<String>,
    pub effort: Option<toml::Value>,
    pub prompt: Option<String>,
    pub prompt_file: Option<PathBuf>,
    pub diff_command: Option<String>,
    pub staged_diff_command: Option<String>,
    pub unstaged_diff_command: Option<String>,
    pub untracked_diff_command: Option<String>,
    pub unstaged_files_command: Option<String>,
    pub untracked_files_command: Option<String>,
    pub include_unstaged: Option<IncludeUnstagedMode>,
    pub max_input_chars: Option<usize>,
    pub max_file_chars: Option<usize>,
    pub include_lockfiles: Option<bool>,
    pub ignore_diff_paths: Option<Vec<String>>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub timeout_seconds: Option<u64>,
    pub proxy: Option<String>,
    pub headers: Option<Vec<String>>,
    pub stage_all: Option<bool>,
    pub confirm: Option<bool>,
    pub dry_run: Option<bool>,
    pub commit: Option<CommitConfig>,
}

#[derive(Debug, Default, Deserialize)]
pub struct CommitConfig {
    pub amend: Option<bool>,
    pub signoff: Option<bool>,
    pub no_verify: Option<bool>,
    pub allow_empty: Option<bool>,
    pub allow_empty_message: Option<bool>,
    pub author: Option<String>,
    pub date: Option<String>,
    pub cleanup: Option<CleanupMode>,
    pub gpg_sign: Option<Option<String>>,
    pub args: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppConfig {
    pub api_key: String,
    pub base_url: String,
    pub endpoint: String,
    pub model: String,
    pub effort: Option<ReasoningEffort>,
    pub prompt: String,
    pub diff_command: String,
    pub staged_diff_command: String,
    pub unstaged_diff_command: String,
    pub untracked_diff_command: String,
    pub unstaged_files_command: String,
    pub untracked_files_command: String,
    pub include_unstaged: IncludeUnstagedMode,
    pub max_input_chars: usize,
    pub max_file_chars: usize,
    pub include_lockfiles: bool,
    pub ignore_diff_paths: Vec<String>,
    pub temperature: f32,
    pub max_tokens: u32,
    pub timeout_seconds: u64,
    pub proxy: Option<String>,
    pub headers: Vec<String>,
    pub stage_all: bool,
    pub confirm: bool,
    pub assume_yes: bool,
    pub dry_run: bool,
    pub commit: CommitOptions,
}

pub fn default_config_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir().context("could not determine the user config directory")?;
    Ok(config_dir.join("gitcommitgenerator").join("config.toml"))
}

pub fn load_file_config(path: &Path) -> Result<FileConfig> {
    if !path.exists() {
        return Ok(FileConfig::default());
    }

    let config = fs::read_to_string(path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    toml::from_str(&config)
        .with_context(|| format!("failed to parse config file {}", path.display()))
}

pub fn resolve_config(cli: Cli) -> Result<AppConfig> {
    let config_path = cli.config.clone().unwrap_or(default_config_path()?);
    let file = load_file_config(&config_path)?;
    let file_effort = file.effort.map(|value| match value {
        toml::Value::String(value) => value,
        value => value.to_string(),
    });

    let api_key = first_some(cli.api_key, file.api_key)
        .or_else(|| env::var("OPENAI_API_KEY").ok())
        .ok_or_else(|| {
            anyhow!(
                "missing API key; set api_key in {}, pass --api-key, or set GCG_API_KEY/OPENAI_API_KEY",
                config_path.display()
            )
        })?;
    let model = first_some(cli.model, file.model).ok_or_else(|| {
        anyhow!(
            "missing model; set model in {}, pass --model, or set GCG_MODEL",
            config_path.display()
        )
    })?;

    let effort = resolve_effort(first_some(cli.effort, file_effort));
    let prompt = resolve_prompt(cli.prompt, cli.prompt_file, file.prompt, file.prompt_file)?;

    let commit = file.commit.unwrap_or_default();
    let legacy_diff_command = first_some(cli.diff_command, file.diff_command);
    let stage_all = if cli.no_stage_all {
        false
    } else {
        cli.stage_all.or(file.stage_all).unwrap_or(true)
    };

    Ok(AppConfig {
        api_key,
        base_url: first_some(cli.base_url, file.base_url)
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
        endpoint: first_some(cli.endpoint, file.endpoint)
            .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string()),
        model,
        effort,
        prompt,
        diff_command: legacy_diff_command
            .clone()
            .unwrap_or_else(|| DEFAULT_DIFF_COMMAND.to_string()),
        staged_diff_command: first_some(cli.staged_diff_command, file.staged_diff_command)
            .or(legacy_diff_command)
            .unwrap_or_else(|| DEFAULT_STAGED_DIFF_COMMAND.to_string()),
        unstaged_diff_command: first_some(cli.unstaged_diff_command, file.unstaged_diff_command)
            .unwrap_or_else(|| DEFAULT_UNSTAGED_DIFF_COMMAND.to_string()),
        untracked_diff_command: first_some(cli.untracked_diff_command, file.untracked_diff_command)
            .unwrap_or_else(|| DEFAULT_UNTRACKED_DIFF_COMMAND.to_string()),
        unstaged_files_command: first_some(cli.unstaged_files_command, file.unstaged_files_command)
            .unwrap_or_else(|| DEFAULT_UNSTAGED_FILES_COMMAND.to_string()),
        untracked_files_command: first_some(
            cli.untracked_files_command,
            file.untracked_files_command,
        )
        .unwrap_or_else(|| DEFAULT_UNTRACKED_FILES_COMMAND.to_string()),
        include_unstaged: cli
            .include_unstaged
            .or(file.include_unstaged)
            .unwrap_or(IncludeUnstagedMode::Ask),
        max_input_chars: resolve_char_limit(
            cli.max_input_chars.or(file.max_input_chars),
            DEFAULT_MAX_INPUT_CHARS,
            "max_input_chars",
        )?,
        max_file_chars: resolve_char_limit(
            cli.max_file_chars.or(file.max_file_chars),
            DEFAULT_MAX_FILE_CHARS,
            "max_file_chars",
        )?,
        include_lockfiles: cli
            .include_lockfiles
            .or(file.include_lockfiles)
            .unwrap_or(false),
        ignore_diff_paths: merge_vec(
            file.ignore_diff_paths.unwrap_or_default(),
            cli.ignore_diff_path,
        ),
        temperature: cli.temperature.or(file.temperature).unwrap_or(0.2),
        max_tokens: cli.max_tokens.or(file.max_tokens).unwrap_or(512),
        timeout_seconds: cli.timeout_seconds.or(file.timeout_seconds).unwrap_or(120),
        proxy: first_some(cli.proxy, file.proxy),
        headers: merge_vec(file.headers.unwrap_or_default(), cli.header),
        stage_all,
        confirm: if cli.yes {
            false
        } else {
            cli.confirm.or(file.confirm).unwrap_or(true)
        },
        assume_yes: cli.yes,
        dry_run: resolve_bool_flag(cli.dry_run, cli.no_dry_run, file.dry_run.unwrap_or(false)),
        commit: CommitOptions {
            amend: resolve_bool_flag(cli.amend, cli.no_amend, commit.amend.unwrap_or(false)),
            signoff: resolve_bool_flag(
                cli.signoff,
                cli.no_signoff,
                commit.signoff.unwrap_or(false),
            ),
            no_verify: resolve_bool_flag(
                cli.no_verify,
                cli.verify,
                commit.no_verify.unwrap_or(false),
            ),
            allow_empty: resolve_bool_flag(
                cli.allow_empty,
                cli.no_allow_empty,
                commit.allow_empty.unwrap_or(false),
            ),
            allow_empty_message: resolve_bool_flag(
                cli.allow_empty_message,
                cli.no_allow_empty_message,
                commit.allow_empty_message.unwrap_or(false),
            ),
            author: first_some(cli.author, commit.author),
            date: first_some(cli.date, commit.date),
            cleanup: cli.cleanup.or(commit.cleanup),
            gpg_sign: cli.gpg_sign.or(commit.gpg_sign),
            args: merge_vec(commit.args.unwrap_or_default(), cli.commit_arg),
        },
    })
}

fn resolve_effort(value: Option<String>) -> Option<ReasoningEffort> {
    let value = value?;
    if let Some(effort) = ReasoningEffort::parse(&value) {
        return Some(effort);
    }

    eprintln!(
        "Warning: invalid effort {value:?}; omitting reasoning_effort. Supported values: {}.",
        ReasoningEffort::VALUES
    );
    None
}

fn first_some<T>(left: Option<T>, right: Option<T>) -> Option<T> {
    left.or(right)
}

fn resolve_bool_flag(enable: bool, disable: bool, fallback: bool) -> bool {
    match (enable, disable) {
        (true, false) => true,
        (false, true) => false,
        _ => fallback,
    }
}

fn merge_vec<T>(mut file_values: Vec<T>, cli_values: Vec<T>) -> Vec<T> {
    file_values.extend(cli_values);
    file_values
}

fn resolve_char_limit(value: Option<usize>, fallback: usize, name: &str) -> Result<usize> {
    let value = value.unwrap_or(fallback);
    if value < MIN_DIFF_BUDGET_CHARS {
        bail!("{name} must be at least {MIN_DIFF_BUDGET_CHARS}");
    }
    if value >= CHAT_MESSAGE_CHAR_LIMIT {
        bail!("{name} must be less than the chat message limit of {CHAT_MESSAGE_CHAR_LIMIT}");
    }
    Ok(value)
}

fn resolve_prompt(
    cli_prompt: Option<String>,
    cli_prompt_file: Option<PathBuf>,
    file_prompt: Option<String>,
    file_prompt_file: Option<PathBuf>,
) -> Result<String> {
    if let Some(prompt) = cli_prompt {
        return Ok(prompt);
    }
    if let Some(path) = cli_prompt_file {
        return fs::read_to_string(&path)
            .with_context(|| format!("failed to read prompt file {}", path.display()));
    }
    if let Some(prompt) = file_prompt {
        return Ok(prompt);
    }
    if let Some(path) = file_prompt_file {
        return fs::read_to_string(&path)
            .with_context(|| format!("failed to read prompt file {}", path.display()));
    }
    Ok(DEFAULT_PROMPT.to_string())
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use tempfile::tempdir;

    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::parse_from(std::iter::once("gitcommitgenerator").chain(args.iter().copied()))
    }

    #[test]
    fn cli_values_override_file_config() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
api_key = "file-key"
base_url = "http://file"
model = "file-model"
effort = "low"
temperature = 0.7
stage_all = false

[commit]
signoff = true
author = "File User <file@example.com>"
args = ["--quiet"]
"#,
        )
        .unwrap();

        let config = resolve_config(parse(&[
            "--config",
            config_path.to_str().unwrap(),
            "--api-key",
            "cli-key",
            "--model",
            "cli-model",
            "--base-url",
            "http://cli",
            "--effort",
            "HIGH",
            "--temperature",
            "0.1",
            "--stage-all",
            "true",
            "--author",
            "Cli User <cli@example.com>",
            "--commit-arg",
            "--verbose",
        ]))
        .unwrap();

        assert_eq!(config.api_key, "cli-key");
        assert_eq!(config.model, "cli-model");
        assert_eq!(config.base_url, "http://cli");
        assert_eq!(config.effort, Some(ReasoningEffort::High));
        assert_eq!(config.temperature, 0.1);
        assert!(config.stage_all);
        assert!(config.commit.signoff);
        assert_eq!(
            config.commit.author,
            Some("Cli User <cli@example.com>".to_string())
        );
        assert_eq!(config.commit.args, vec!["--quiet", "--verbose"]);
    }

    #[test]
    fn invalid_effort_falls_back_to_omitting_request_field() {
        let config = resolve_config(parse(&[
            "--config",
            "/tmp/definitely-not-a-real-gcg-config.toml",
            "--api-key",
            "key",
            "--model",
            "model",
            "--effort",
            "turbo",
        ]))
        .unwrap();

        assert_eq!(config.effort, None);
    }

    #[test]
    fn non_string_file_effort_falls_back_to_omitting_request_field() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
api_key = "key"
model = "model"
effort = 42
"#,
        )
        .unwrap();

        let config = resolve_config(parse(&["--config", config_path.to_str().unwrap()])).unwrap();

        assert_eq!(config.effort, None);
    }

    #[test]
    fn max_is_a_supported_effort() {
        let config = resolve_config(parse(&[
            "--config",
            "/tmp/definitely-not-a-real-gcg-config.toml",
            "--api-key",
            "key",
            "--model",
            "model",
            "--effort",
            "max",
        ]))
        .unwrap();

        assert_eq!(config.effort, Some(ReasoningEffort::Max));
    }

    #[test]
    fn file_effort_is_resolved_case_insensitively() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
api_key = "key"
model = "model"
effort = "XHIGH"
"#,
        )
        .unwrap();

        let config = resolve_config(parse(&["--config", config_path.to_str().unwrap()])).unwrap();

        assert_eq!(config.effort, Some(ReasoningEffort::Xhigh));
    }

    #[test]
    fn default_prompt_and_diff_include_unstaged_and_untracked_changes() {
        let config = resolve_config(parse(&[
            "--config",
            "/tmp/definitely-not-a-real-gcg-config.toml",
            "--api-key",
            "key",
            "--model",
            "model",
        ]))
        .unwrap();

        assert!(config.prompt.contains("Conventional Commits"));
        assert!(config.prompt.contains("fix: handle empty response"));
        assert_eq!(config.endpoint, DEFAULT_ENDPOINT);
        assert_eq!(config.effort, None);
        assert_eq!(config.diff_command, DEFAULT_DIFF_COMMAND);
        assert_eq!(config.staged_diff_command, DEFAULT_STAGED_DIFF_COMMAND);
        assert_eq!(config.unstaged_diff_command, DEFAULT_UNSTAGED_DIFF_COMMAND);
        assert_eq!(
            config.untracked_diff_command,
            DEFAULT_UNTRACKED_DIFF_COMMAND
        );
        assert_eq!(
            config.unstaged_files_command,
            DEFAULT_UNSTAGED_FILES_COMMAND
        );
        assert_eq!(
            config.untracked_files_command,
            DEFAULT_UNTRACKED_FILES_COMMAND
        );
        assert_eq!(config.include_unstaged, IncludeUnstagedMode::Ask);
        assert_eq!(config.max_input_chars, DEFAULT_MAX_INPUT_CHARS);
        assert_eq!(config.max_file_chars, DEFAULT_MAX_FILE_CHARS);
        assert!(!config.include_lockfiles);
        assert!(config.ignore_diff_paths.is_empty());
        assert!(config.stage_all);
        assert!(config.confirm);
    }

    #[test]
    fn resolves_diff_input_budget_config() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
api_key = "key"
model = "model"
max_input_chars = 12000
max_file_chars = 6000
include_lockfiles = true
ignore_diff_paths = ["fixtures/**"]
"#,
        )
        .unwrap();

        let config = resolve_config(parse(&[
            "--config",
            config_path.to_str().unwrap(),
            "--max-input-chars",
            "20000",
            "--ignore-diff-path",
            "*.snap",
        ]))
        .unwrap();

        assert_eq!(config.max_input_chars, 20_000);
        assert_eq!(config.max_file_chars, 6_000);
        assert!(config.include_lockfiles);
        assert_eq!(config.ignore_diff_paths, vec!["fixtures/**", "*.snap"]);
    }

    #[test]
    fn rejects_too_small_diff_input_budget() {
        let error = resolve_config(parse(&[
            "--config",
            "/tmp/definitely-not-a-real-gcg-config.toml",
            "--api-key",
            "key",
            "--model",
            "model",
            "--max-input-chars",
            "3999",
        ]))
        .unwrap_err()
        .to_string();

        assert!(error.contains("max_input_chars must be at least"));
    }

    #[test]
    fn no_stage_all_overrides_everything() {
        let config = resolve_config(parse(&[
            "--config",
            "/tmp/definitely-not-a-real-gcg-config.toml",
            "--api-key",
            "key",
            "--model",
            "model",
            "--stage-all",
            "true",
            "--no-stage-all",
        ]))
        .unwrap();

        assert!(!config.stage_all);
    }

    #[test]
    fn negative_commit_flags_override_file_config() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
api_key = "key"
model = "model"
dry_run = true

[commit]
amend = true
signoff = true
no_verify = true
allow_empty = true
allow_empty_message = true
"#,
        )
        .unwrap();

        let config = resolve_config(parse(&[
            "--config",
            config_path.to_str().unwrap(),
            "--no-dry-run",
            "--no-amend",
            "--no-signoff",
            "--verify",
            "--no-allow-empty",
            "--no-allow-empty-message",
        ]))
        .unwrap();

        assert!(!config.dry_run);
        assert!(!config.commit.amend);
        assert!(!config.commit.signoff);
        assert!(!config.commit.no_verify);
        assert!(!config.commit.allow_empty);
        assert!(!config.commit.allow_empty_message);
    }

    #[test]
    fn legacy_diff_command_is_used_as_staged_diff_command_fallback() {
        let config = resolve_config(parse(&[
            "--config",
            "/tmp/definitely-not-a-real-gcg-config.toml",
            "--api-key",
            "key",
            "--model",
            "model",
            "--diff-command",
            "git diff --cached --name-only",
        ]))
        .unwrap();

        assert_eq!(config.diff_command, "git diff --cached --name-only");
        assert_eq!(config.staged_diff_command, "git diff --cached --name-only");
    }
}
