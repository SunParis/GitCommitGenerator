use anyhow::{Result, bail};

use crate::changes::{collect_changes, stage_included_paths};
use crate::commit::{commit_with_retries, confirm};
use crate::config::AppConfig;
use crate::diff::print_diff_preparation_report;
use crate::git::ensure_inside_git_repo;
use crate::llm::generate_commit_message;

pub fn run(config: &AppConfig) -> Result<()> {
    ensure_inside_git_repo()?;

    let changes = collect_changes(config)?;
    if changes.diff.trim().is_empty() && !config.commit.allow_empty {
        bail!("no uncommitted changes detected");
    }
    print_diff_preparation_report(&changes.diff_report);

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

    commit_with_retries(&config.commit, &message, staged_for_commit)
}
