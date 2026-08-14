use std::ffi::OsStr;
use std::process::{Command, Output};

use anyhow::{Context, Result, bail};

pub fn ensure_inside_git_repo() -> Result<()> {
    let inside = run_git(["rev-parse", "--is-inside-work-tree"])?;
    if inside.trim() != "true" {
        bail!("current directory is not inside a Git repository");
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
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()
        .with_context(|| format!("failed to run command: {command}"))?;
    output_to_result(output)
}

pub fn run_lines_command(command: &str) -> Result<Vec<String>> {
    let output = run_shell_command(command)?;
    Ok(output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub(crate) fn output_to_result(output: Output) -> Result<String> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("command failed: {}", stderr.trim());
    }
    String::from_utf8(output.stdout).context("command output was not valid UTF-8")
}
