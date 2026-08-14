use std::io::{self, Write};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::git::run_git;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitFix {
    AllowEmpty,
    AllowEmptyMessage,
    StageAll,
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

pub(crate) fn commit_with_retries(
    initial_options: &CommitOptions,
    message: &str,
    staged_for_commit: bool,
) -> Result<()> {
    let mut options = initial_options.clone();
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

    #[test]
    fn builds_commit_args_from_named_and_raw_options() {
        let options = CommitOptions {
            amend: true,
            signoff: true,
            no_verify: true,
            allow_empty: true,
            allow_empty_message: true,
            author: Some("Example User <user@example.com>".to_string()),
            date: Some("2025-01-01T00:00:00Z".to_string()),
            cleanup: Some(CleanupMode::Verbatim),
            gpg_sign: Some(Some("ABC123".to_string())),
            args: vec!["--trailer Reviewed-by=QA".to_string()],
        };

        let args = build_commit_args(&options, "test message").unwrap();

        assert_eq!(&args[..3], ["commit", "-m", "test message"]);
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
