use anyhow::{Context, Result, anyhow, bail};
use clap::{ArgAction, Parser, ValueEnum};
use reqwest::Proxy;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

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

#[derive(Debug, Parser)]
#[command(author, version, about)]
pub struct Cli {
    #[arg(long, env = "GCG_CONFIG")]
    pub config: Option<PathBuf>,

    #[arg(long, env = "GCG_API_KEY")]
    pub api_key: Option<String>,

    #[arg(long, env = "GCG_BASE_URL")]
    pub base_url: Option<String>,

    #[arg(long, env = "GCG_ENDPOINT")]
    pub endpoint: Option<String>,

    #[arg(long, env = "GCG_MODEL")]
    pub model: Option<String>,

    #[arg(long)]
    pub prompt: Option<String>,

    #[arg(long)]
    pub prompt_file: Option<PathBuf>,

    #[arg(long)]
    pub diff_command: Option<String>,

    #[arg(long)]
    pub staged_diff_command: Option<String>,

    #[arg(long)]
    pub unstaged_diff_command: Option<String>,

    #[arg(long)]
    pub untracked_diff_command: Option<String>,

    #[arg(long)]
    pub unstaged_files_command: Option<String>,

    #[arg(long)]
    pub untracked_files_command: Option<String>,

    #[arg(long, value_enum)]
    pub include_unstaged: Option<IncludeUnstagedMode>,

    #[arg(long)]
    pub temperature: Option<f32>,

    #[arg(long)]
    pub max_tokens: Option<u32>,

    #[arg(long)]
    pub timeout_seconds: Option<u64>,

    #[arg(long)]
    pub proxy: Option<String>,

    #[arg(long)]
    pub header: Vec<String>,

    #[arg(long, action = ArgAction::Set)]
    pub stage_all: Option<bool>,

    #[arg(long, action = ArgAction::SetTrue)]
    pub no_stage_all: bool,

    #[arg(long, action = ArgAction::Set)]
    pub confirm: Option<bool>,

    #[arg(long, action = ArgAction::SetTrue)]
    pub yes: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    pub dry_run: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    pub no_dry_run: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    pub amend: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    pub no_amend: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    pub signoff: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    pub no_signoff: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    pub no_verify: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    pub verify: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    pub allow_empty: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    pub no_allow_empty: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    pub allow_empty_message: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    pub no_allow_empty_message: bool,

    #[arg(long)]
    pub author: Option<String>,

    #[arg(long)]
    pub date: Option<String>,

    #[arg(long, value_enum)]
    pub cleanup: Option<CleanupMode>,

    #[arg(long)]
    pub gpg_sign: Option<Option<String>>,

    #[arg(long, allow_hyphen_values = true)]
    pub commit_arg: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum CleanupMode {
    Strip,
    Whitespace,
    Verbatim,
    Scissors,
    Default,
}

impl CleanupMode {
    fn as_git_value(&self) -> &'static str {
        match self {
            Self::Strip => "strip",
            Self::Whitespace => "whitespace",
            Self::Verbatim => "verbatim",
            Self::Scissors => "scissors",
            Self::Default => "default",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum IncludeUnstagedMode {
    Ask,
    Always,
    Never,
}

#[derive(Debug, Default, Deserialize)]
pub struct FileConfig {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub endpoint: Option<String>,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub prompt_file: Option<PathBuf>,
    pub diff_command: Option<String>,
    pub staged_diff_command: Option<String>,
    pub unstaged_diff_command: Option<String>,
    pub untracked_diff_command: Option<String>,
    pub unstaged_files_command: Option<String>,
    pub untracked_files_command: Option<String>,
    pub include_unstaged: Option<IncludeUnstagedMode>,
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
    pub prompt: String,
    pub diff_command: String,
    pub staged_diff_command: String,
    pub unstaged_diff_command: String,
    pub untracked_diff_command: String,
    pub unstaged_files_command: String,
    pub untracked_files_command: String,
    pub include_unstaged: IncludeUnstagedMode,
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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommitOptions {
    pub amend: bool,
    pub signoff: bool,
    pub no_verify: bool,
    pub allow_empty: bool,
    pub allow_empty_message: bool,
    pub author: Option<String>,
    pub date: Option<String>,
    pub cleanup: Option<CleanupMode>,
    pub gpg_sign: Option<Option<String>>,
    pub args: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    temperature: f32,
    max_tokens: u32,
}

#[derive(Debug, Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct ChatChoiceMessage {
    content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitFix {
    AllowEmpty,
    AllowEmptyMessage,
    StageAll,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeSet {
    pub diff: String,
    pub included_unstaged_paths: Vec<String>,
    pub included_untracked_paths: Vec<String>,
}

impl ChangeSet {
    fn has_included_extra_paths(&self) -> bool {
        !self.included_unstaged_paths.is_empty() || !self.included_untracked_paths.is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtraChangeSelection {
    pub unstaged_paths: Vec<String>,
    pub untracked_paths: Vec<String>,
}

impl ExtraChangeSelection {
    fn all(unstaged_paths: &[String], untracked_paths: &[String]) -> Self {
        Self {
            unstaged_paths: unstaged_paths.to_vec(),
            untracked_paths: untracked_paths.to_vec(),
        }
    }

    fn is_empty(&self) -> bool {
        self.unstaged_paths.is_empty() && self.untracked_paths.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectablePath {
    pub path: String,
    pub kind: ExtraPathKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtraPathKind {
    Unstaged,
    Untracked,
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

pub fn run(config: &AppConfig) -> Result<()> {
    ensure_inside_git_repo()?;

    let changes = collect_changes(config)?;
    if changes.diff.trim().is_empty() && !config.commit.allow_empty {
        bail!("no uncommitted changes detected");
    }

    let message = generate_commit_message(config, &changes.diff)?;
    println!("\n{}\n", message);

    if config.confirm && !confirm("Commit with this message? [Y/n]")? {
        println!("Aborted.");
        return Ok(());
    }

    if config.dry_run {
        println!("Dry run enabled; commit was not created.");
        return Ok(());
    }

    let staged_for_commit = if config.stage_all && changes.has_included_extra_paths() {
        stage_included_paths(&changes)?;
        true
    } else {
        false
    };

    commit_with_retries(config, &message, staged_for_commit)
}

pub fn stage_included_paths(changes: &ChangeSet) -> Result<()> {
    let mut paths = changes.included_unstaged_paths.clone();
    paths.extend(changes.included_untracked_paths.clone());
    if paths.is_empty() {
        return Ok(());
    }

    let mut args = vec!["add".to_string(), "--".to_string()];
    args.extend(paths);
    run_git(args)?;
    Ok(())
}

pub fn collect_changes(config: &AppConfig) -> Result<ChangeSet> {
    let staged_diff = run_shell_command(&config.staged_diff_command)?;
    let unstaged_paths = run_lines_command(&config.unstaged_files_command)?;
    let untracked_paths = run_lines_command(&config.untracked_files_command)?;
    let has_extra_paths = !unstaged_paths.is_empty() || !untracked_paths.is_empty();

    let extra_selection = if has_extra_paths {
        select_extra_changes(config, &unstaged_paths, &untracked_paths)?
    } else {
        ExtraChangeSelection::default()
    };

    let mut parts = Vec::new();
    if !staged_diff.trim().is_empty() {
        parts.push(format!("Staged changes:\n{staged_diff}"));
    }

    let mut included_unstaged_paths = Vec::new();
    let mut included_untracked_paths = Vec::new();

    if !extra_selection.is_empty() {
        let unstaged_diff = diff_unstaged_paths(&extra_selection.unstaged_paths)?;
        let untracked_diff = diff_untracked_paths(&extra_selection.untracked_paths)?;

        if !unstaged_diff.trim().is_empty() {
            parts.push(format!("Unstaged tracked changes:\n{unstaged_diff}"));
            included_unstaged_paths = extra_selection.unstaged_paths;
        }

        if !untracked_diff.trim().is_empty() {
            parts.push(format!("Untracked files:\n{untracked_diff}"));
            included_untracked_paths = extra_selection.untracked_paths;
        }
    }

    Ok(ChangeSet {
        diff: parts.join("\n\n"),
        included_unstaged_paths,
        included_untracked_paths,
    })
}

fn select_extra_changes(
    config: &AppConfig,
    unstaged_paths: &[String],
    untracked_paths: &[String],
) -> Result<ExtraChangeSelection> {
    match config.include_unstaged {
        IncludeUnstagedMode::Always => {
            Ok(ExtraChangeSelection::all(unstaged_paths, untracked_paths))
        }
        IncludeUnstagedMode::Never => Ok(ExtraChangeSelection::default()),
        IncludeUnstagedMode::Ask if config.assume_yes => {
            Ok(ExtraChangeSelection::all(unstaged_paths, untracked_paths))
        }
        IncludeUnstagedMode::Ask => {
            print_extra_paths(unstaged_paths, untracked_paths);
            prompt_extra_change_selection(unstaged_paths, untracked_paths)
        }
    }
}

fn print_extra_paths(unstaged_paths: &[String], untracked_paths: &[String]) {
    println!("Found files that are not staged with git add:");
    for path in unstaged_paths {
        println!("  unstaged: {path}");
    }
    for path in untracked_paths {
        println!("  untracked: {path}");
    }
}

fn prompt_extra_change_selection(
    unstaged_paths: &[String],
    untracked_paths: &[String],
) -> Result<ExtraChangeSelection> {
    loop {
        print!("Include unstaged and untracked files? [Y/n/select files to add]");
        io::stdout().flush().context("failed to flush stdout")?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("failed to read selection")?;

        match input.trim().to_lowercase().as_str() {
            "" | "y" | "yes" => {
                return Ok(ExtraChangeSelection::all(unstaged_paths, untracked_paths));
            }
            "n" | "no" => return Ok(ExtraChangeSelection::default()),
            "s" | "select" | "select files" | "select files to add" => {
                return prompt_select_files_to_exclude(unstaged_paths, untracked_paths);
            }
            _ => {
                println!("Please enter y, n, or select files to add.");
            }
        }
    }
}

fn prompt_select_files_to_exclude(
    unstaged_paths: &[String],
    untracked_paths: &[String],
) -> Result<ExtraChangeSelection> {
    let paths = selectable_paths(unstaged_paths, untracked_paths);
    print_selection_ui(&paths);

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("failed to read file selection")?;

    let selected_indexes = parse_file_selection(input.trim(), &paths)?;
    Ok(selection_from_indexes(&paths, &selected_indexes))
}

pub fn selectable_paths(
    unstaged_paths: &[String],
    untracked_paths: &[String],
) -> Vec<SelectablePath> {
    let mut paths = Vec::with_capacity(unstaged_paths.len() + untracked_paths.len());
    for path in unstaged_paths {
        paths.push(SelectablePath {
            path: path.clone(),
            kind: ExtraPathKind::Unstaged,
        });
    }
    for path in untracked_paths {
        paths.push(SelectablePath {
            path: path.clone(),
            kind: ExtraPathKind::Untracked,
        });
    }
    paths
}

fn print_selection_ui(paths: &[SelectablePath]) {
    println!(":: {} files...", paths.len());
    for (index, path) in paths.iter().enumerate().rev() {
        println!("{}  {}", index + 1, path.path);
    }
    println!("==> Files to exclude: (for example: \"1 2 3\", \"1-3\", \"^4\", or file names)");
    print!("==> ");
    let _ = io::stdout().flush();
}

pub fn parse_file_selection(input: &str, paths: &[SelectablePath]) -> Result<Vec<usize>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }

    let mut included = vec![true; paths.len()];
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(indexes_from_mask(&included));
    }

    for token in trimmed.split_whitespace() {
        apply_selection_token(token, paths, &mut included)?;
    }

    Ok(indexes_from_mask(&included))
}

fn apply_selection_token(
    token: &str,
    paths: &[SelectablePath],
    included: &mut [bool],
) -> Result<()> {
    if let Some(rest) = token.strip_prefix('^') {
        for index in resolve_selection_token(rest, paths)? {
            included[index] = true;
        }
        return Ok(());
    }

    for index in resolve_selection_token(token, paths)? {
        included[index] = false;
    }
    Ok(())
}

fn resolve_selection_token(token: &str, paths: &[SelectablePath]) -> Result<Vec<usize>> {
    if let Some((start, end)) = token.split_once('-') {
        let start = parse_selection_number(start, paths.len())?;
        let end = parse_selection_number(end, paths.len())?;
        let range = if start <= end {
            start..=end
        } else {
            end..=start
        };
        return Ok(range.collect());
    }

    if let Ok(index) = parse_selection_number(token, paths.len()) {
        return Ok(vec![index]);
    }

    let matches: Vec<usize> = paths
        .iter()
        .enumerate()
        .filter_map(|(index, path)| {
            (path.path == token || path.path.contains(token)).then_some(index)
        })
        .collect();

    if matches.is_empty() {
        bail!("unknown file selector: {token}");
    }
    Ok(matches)
}

fn parse_selection_number(value: &str, len: usize) -> Result<usize> {
    let number: usize = value
        .parse()
        .with_context(|| format!("invalid file number: {value}"))?;
    if number == 0 || number > len {
        bail!("file number out of range: {number}");
    }
    Ok(number - 1)
}

fn indexes_from_mask(included: &[bool]) -> Vec<usize> {
    included
        .iter()
        .enumerate()
        .filter_map(|(index, included)| included.then_some(index))
        .collect()
}

pub fn selection_from_indexes(
    paths: &[SelectablePath],
    selected_indexes: &[usize],
) -> ExtraChangeSelection {
    let mut selection = ExtraChangeSelection::default();
    for index in selected_indexes {
        match paths[*index].kind {
            ExtraPathKind::Unstaged => selection.unstaged_paths.push(paths[*index].path.clone()),
            ExtraPathKind::Untracked => selection.untracked_paths.push(paths[*index].path.clone()),
        }
    }
    selection
}

fn diff_unstaged_paths(paths: &[String]) -> Result<String> {
    if paths.is_empty() {
        return Ok(String::new());
    }

    let stat = run_git_with_paths(["diff", "--stat", "--"], paths)?;
    let diff = run_git_with_paths(["diff", "--binary", "--find-renames", "--"], paths)?;
    Ok(format!("{stat}{diff}"))
}

fn diff_untracked_paths(paths: &[String]) -> Result<String> {
    let mut output = String::new();
    for path in paths {
        output.push_str(&format!("\nUntracked file: {path}\n"));
        output.push_str(&run_git_no_index(path)?);
    }
    Ok(output)
}

fn run_git_with_paths<I, S>(args: I, paths: &[String]) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("git");
    command.args(args);
    command.args(paths);
    let output = command.output().context("failed to run git")?;
    output_to_result(output)
}

fn run_git_no_index(path: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["diff", "--no-index", "--", "/dev/null", path])
        .output()
        .context("failed to run git diff --no-index")?;
    if output.status.success() || output.status.code() == Some(1) {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    output_to_result(output)
}

pub fn ensure_inside_git_repo() -> Result<()> {
    let output = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .context("failed to run git rev-parse")?;
    if !output.status.success() || String::from_utf8_lossy(&output.stdout).trim() != "true" {
        bail!("current directory is not inside a Git work tree");
    }
    Ok(())
}

pub fn run_git<I, S>(args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .args(args)
        .output()
        .context("failed to run git")?;
    output_to_result(output)
}

pub fn run_shell_command(command: &str) -> Result<String> {
    let output = if cfg!(windows) {
        Command::new("cmd").args(["/C", command]).output()
    } else {
        Command::new("sh").args(["-c", command]).output()
    }
    .with_context(|| format!("failed to run diff command: {command}"))?;
    output_to_result(output)
}

pub fn run_lines_command(command: &str) -> Result<Vec<String>> {
    Ok(run_shell_command(command)?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn output_to_result(output: Output) -> Result<String> {
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = format!("{stdout}{stderr}");
    bail!("{}", message.trim());
}

pub fn generate_commit_message(config: &AppConfig, diff: &str) -> Result<String> {
    let client = build_client(config)?;
    let url = join_url(&config.base_url, &config.endpoint);
    let user_content = format!("{}\n\nGit diff:\n{}", config.prompt, diff);
    let request = ChatRequest {
        model: &config.model,
        temperature: config.temperature,
        max_tokens: config.max_tokens,
        messages: vec![ChatMessage {
            role: "user",
            content: user_content,
        }],
    };

    let mut builder = client
        .post(&url)
        .bearer_auth(&config.api_key)
        .json(&request);

    for header in &config.headers {
        let (name, value) = parse_header(header)?;
        builder = builder.header(name, value);
    }

    let response = builder.send().context("failed to call LLM API")?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let body = response.text().context("failed to read LLM response")?;

    if !status.is_success() {
        bail!(
            "LLM API request failed\nURL: {url}\nStatus: {status}\nContent-Type: {}\nBody preview:\n{}",
            content_type.as_deref().unwrap_or("<missing>"),
            response_preview(&body)
        );
    }

    parse_commit_message_response(&body, &url, status.as_u16(), content_type.as_deref())
}

fn build_client(config: &AppConfig) -> Result<Client> {
    let mut builder = Client::builder().timeout(Duration::from_secs(config.timeout_seconds));
    if let Some(proxy_url) = &config.proxy {
        builder = builder.proxy(Proxy::all(proxy_url).context("invalid proxy URL")?);
    }
    builder.build().context("failed to build HTTP client")
}

pub fn parse_commit_message(body: &str) -> Result<String> {
    parse_commit_message_response(body, "<unknown>", 200, None)
}

pub fn parse_commit_message_response(
    body: &str,
    url: &str,
    status: u16,
    content_type: Option<&str>,
) -> Result<String> {
    let parsed: ChatResponse =
        serde_json::from_str(body).map_err(|error| {
            anyhow!(
                "failed to parse LLM response as an OpenAI-compatible chat completion\nURL: {url}\nStatus: {status}\nContent-Type: {}\nParse error: {error}\nBody preview:\n{}\nHint: verify --base-url and --endpoint. For New API/OpenAI-compatible gateways, the endpoint is commonly /v1/chat/completions.",
                content_type.unwrap_or("<missing>"),
                response_preview(body)
            )
        })?;
    let content = parsed
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("LLM response did not contain any choices"))?
        .message
        .content
        .trim()
        .trim_matches('`')
        .trim()
        .to_string();

    if content.is_empty() {
        bail!("LLM returned an empty commit message");
    }
    Ok(content)
}

fn response_preview(body: &str) -> String {
    const MAX_CHARS: usize = 600;
    let mut preview = body.trim().chars().take(MAX_CHARS).collect::<String>();
    if body.trim().chars().count() > MAX_CHARS {
        preview.push_str("\n...<truncated>");
    }
    if preview.is_empty() {
        "<empty body>".to_string()
    } else {
        preview
    }
}

fn parse_header(header: &str) -> Result<(String, String)> {
    let (name, value) = header
        .split_once(':')
        .ok_or_else(|| anyhow!("invalid header {header:?}; expected 'Name: value'"))?;
    Ok((name.trim().to_string(), value.trim().to_string()))
}

pub fn join_url(base_url: &str, endpoint: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let mut endpoint_segments: Vec<&str> = endpoint
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();

    if endpoint_segments.is_empty() {
        return base.to_string();
    }

    let base_segments: Vec<&str> = base
        .split('/')
        .skip(3)
        .filter(|segment| !segment.is_empty())
        .collect();

    let overlap = overlapping_segment_count(&base_segments, &endpoint_segments);
    endpoint_segments.drain(0..overlap);

    if endpoint_segments.is_empty() {
        base.to_string()
    } else {
        format!("{}/{}", base, endpoint_segments.join("/"))
    }
}

fn overlapping_segment_count(base_segments: &[&str], endpoint_segments: &[&str]) -> usize {
    let max_overlap = base_segments.len().min(endpoint_segments.len());
    (1..=max_overlap)
        .rev()
        .find(|count| base_segments[base_segments.len() - count..] == endpoint_segments[..*count])
        .unwrap_or(0)
}

pub fn build_commit_args(options: &CommitOptions, message: &str) -> Result<Vec<String>> {
    let mut args = vec!["commit".to_string(), "-m".to_string(), message.to_string()];

    if options.amend {
        args.push("--amend".to_string());
    }
    if options.signoff {
        args.push("--signoff".to_string());
    }
    if options.no_verify {
        args.push("--no-verify".to_string());
    }
    if options.allow_empty {
        args.push("--allow-empty".to_string());
    }
    if options.allow_empty_message {
        args.push("--allow-empty-message".to_string());
    }
    if let Some(author) = &options.author {
        args.push("--author".to_string());
        args.push(author.clone());
    }
    if let Some(date) = &options.date {
        args.push("--date".to_string());
        args.push(date.clone());
    }
    if let Some(cleanup) = &options.cleanup {
        args.push(format!("--cleanup={}", cleanup.as_git_value()));
    }
    if let Some(gpg_sign) = &options.gpg_sign {
        match gpg_sign {
            Some(key_id) => args.push(format!("--gpg-sign={key_id}")),
            None => args.push("--gpg-sign".to_string()),
        }
    }

    for raw_arg in &options.args {
        let parsed = shell_words::split(raw_arg)
            .with_context(|| format!("failed to parse commit argument {raw_arg:?}"))?;
        args.extend(parsed);
    }

    Ok(args)
}

fn commit_with_retries(config: &AppConfig, message: &str, staged_for_commit: bool) -> Result<()> {
    let mut options = config.commit.clone();
    let mut already_stage_all = staged_for_commit;

    loop {
        let args = build_commit_args(&options, message)?;
        let output = Command::new("git")
            .args(&args)
            .output()
            .context("failed to run git commit")?;

        if output.status.success() {
            print!("{}", String::from_utf8_lossy(&output.stdout));
            return Ok(());
        }

        let error = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        eprintln!("{}", error.trim());

        match detect_commit_fix(&error, already_stage_all, &options) {
            Some(CommitFix::AllowEmpty) => {
                if confirm("Retry with --allow-empty? [Y/n]")? {
                    options.allow_empty = true;
                    continue;
                }
            }
            Some(CommitFix::AllowEmptyMessage) => {
                if confirm("Retry with --allow-empty-message? [Y/n]")? {
                    options.allow_empty_message = true;
                    continue;
                }
            }
            Some(CommitFix::StageAll) => {
                if confirm("Stage all changes with git add -A and retry? [Y/n]")? {
                    run_git(["add", "-A"])?;
                    already_stage_all = true;
                    continue;
                }
            }
            None => {}
        }

        bail!("git commit failed");
    }
}

pub fn detect_commit_fix(
    error: &str,
    already_stage_all: bool,
    options: &CommitOptions,
) -> Option<CommitFix> {
    let normalized = error.to_lowercase();

    if !options.allow_empty
        && (normalized.contains("nothing to commit")
            || normalized.contains("no changes added to commit"))
    {
        if normalized.contains("no changes added to commit") && !already_stage_all {
            return Some(CommitFix::StageAll);
        }
        return Some(CommitFix::AllowEmpty);
    }

    if !options.allow_empty_message
        && (normalized.contains("empty commit message")
            || normalized.contains("aborting commit due to empty commit message"))
    {
        return Some(CommitFix::AllowEmptyMessage);
    }

    None
}

pub fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt}");
    io::stdout().flush().context("failed to flush stdout")?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("failed to read confirmation")?;

    let normalized = input.trim().to_lowercase();
    Ok(normalized.is_empty() || normalized == "y" || normalized == "yes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use tempfile::tempdir;

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
    fn default_prompt_and_diff_include_unstaged_and_untracked_changes() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("missing.toml");
        let config = resolve_config(parse(&[
            "--config",
            config_path.to_str().unwrap(),
            "--api-key",
            "key",
            "--model",
            "model",
        ]))
        .unwrap();

        assert!(config.prompt.contains("Conventional Commits"));
        assert!(config.prompt.contains("fix: handle empty response"));
        assert_eq!(config.endpoint, DEFAULT_ENDPOINT);
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
        assert!(config.stage_all);
        assert!(config.confirm);
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
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("missing.toml");
        let config = resolve_config(parse(&[
            "--config",
            config_path.to_str().unwrap(),
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

    #[test]
    fn builds_commit_args_from_named_and_raw_options() {
        let args = build_commit_args(
            &CommitOptions {
                amend: true,
                signoff: true,
                no_verify: true,
                allow_empty: true,
                allow_empty_message: true,
                author: Some("A User <a@example.com>".to_string()),
                date: Some("2026-06-15T12:00:00+08:00".to_string()),
                cleanup: Some(CleanupMode::Verbatim),
                gpg_sign: Some(Some("ABC123".to_string())),
                args: vec!["--trailer Reviewed-by=QA".to_string()],
            },
            "Add feature",
        )
        .unwrap();

        assert_eq!(args[0..3], ["commit", "-m", "Add feature"]);
        assert!(args.contains(&"--amend".to_string()));
        assert!(args.contains(&"--signoff".to_string()));
        assert!(args.contains(&"--no-verify".to_string()));
        assert!(args.contains(&"--allow-empty".to_string()));
        assert!(args.contains(&"--allow-empty-message".to_string()));
        assert!(args.contains(&"--cleanup=verbatim".to_string()));
        assert!(args.contains(&"--gpg-sign=ABC123".to_string()));
        assert!(args.contains(&"--trailer".to_string()));
        assert!(args.contains(&"Reviewed-by=QA".to_string()));
    }

    #[test]
    fn parses_openai_compatible_chat_response() {
        let body = r#"{
            "choices": [
                {
                    "message": {
                        "content": "Add configurable commit generation"
                    }
                }
            ]
        }"#;

        assert_eq!(
            parse_commit_message(body).unwrap(),
            "Add configurable commit generation"
        );
    }

    #[test]
    fn parse_error_includes_response_context_and_hint() {
        let error = parse_commit_message_response(
            "<!doctype html><title>New API</title>",
            "http://127.0.0.1:3002/chat/completions",
            200,
            Some("text/html; charset=utf-8"),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("URL: http://127.0.0.1:3002/chat/completions"));
        assert!(error.contains("Content-Type: text/html; charset=utf-8"));
        assert!(error.contains("<!doctype html>"));
        assert!(error.contains("/v1/chat/completions"));
    }

    #[test]
    fn join_url_deduplicates_openai_style_suffixes() {
        assert_eq!(
            join_url("http://127.0.0.1:3002", "/v1/chat/completions"),
            "http://127.0.0.1:3002/v1/chat/completions"
        );
        assert_eq!(
            join_url("http://127.0.0.1:3002/v1", "/v1/chat/completions"),
            "http://127.0.0.1:3002/v1/chat/completions"
        );
        assert_eq!(
            join_url(
                "http://127.0.0.1:3002/v1/chat/completions",
                "/v1/chat/completions",
            ),
            "http://127.0.0.1:3002/v1/chat/completions"
        );
        assert_eq!(
            join_url(DEFAULT_BASE_URL, DEFAULT_ENDPOINT),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn file_selection_excludes_numbers_ranges_and_file_names() {
        let paths = selectable_paths(
            &["tracked.txt".to_string(), "src/lib.rs".to_string()],
            &["new.txt".to_string(), "docs/readme.md".to_string()],
        );

        let selected = parse_file_selection("1 3 docs/readme.md", &paths).unwrap();
        let selection = selection_from_indexes(&paths, &selected);

        assert_eq!(selection.unstaged_paths, vec!["src/lib.rs"]);
        assert!(selection.untracked_paths.is_empty());
    }

    #[test]
    fn file_selection_supports_ranges_and_caret_reinclude() {
        let paths = selectable_paths(
            &["one.txt".to_string(), "two.txt".to_string()],
            &["three.txt".to_string(), "four.txt".to_string()],
        );

        let selected = parse_file_selection("1-4 ^2", &paths).unwrap();
        let selection = selection_from_indexes(&paths, &selected);

        assert_eq!(selection.unstaged_paths, vec!["two.txt"]);
        assert!(selection.untracked_paths.is_empty());
    }

    #[test]
    fn detects_simple_commit_fixes() {
        assert_eq!(
            detect_commit_fix(
                "nothing to commit, working tree clean",
                true,
                &CommitOptions::default()
            ),
            Some(CommitFix::AllowEmpty)
        );
        assert_eq!(
            detect_commit_fix(
                "no changes added to commit",
                false,
                &CommitOptions::default()
            ),
            Some(CommitFix::StageAll)
        );
        assert_eq!(
            detect_commit_fix(
                "Aborting commit due to empty commit message.",
                true,
                &CommitOptions::default()
            ),
            Some(CommitFix::AllowEmptyMessage)
        );
    }
}
