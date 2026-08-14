use std::ffi::OsStr;
use std::io::{self, Write};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::config::{AppConfig, IncludeUnstagedMode};
use crate::diff::{DiffPreparationReport, prepare_diff_for_llm, sanitize_diff_for_llm};
use crate::git::{output_to_result, run_git, run_lines_command, run_shell_command};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeSet {
    pub diff: String,
    pub included_unstaged_paths: Vec<String>,
    pub included_untracked_paths: Vec<String>,
    pub diff_report: DiffPreparationReport,
}

impl ChangeSet {
    pub(crate) fn has_included_extra_paths(&self) -> bool {
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

    let raw_diff = parts.join("\n\n");
    let sanitized_diff = sanitize_diff_for_llm(&raw_diff);
    let prepared_diff = prepare_diff_for_llm(config, &sanitized_diff);

    Ok(ChangeSet {
        diff: prepared_diff.content,
        included_unstaged_paths,
        included_untracked_paths,
        diff_report: prepared_diff.report,
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
            _ => println!("Please enter y, n, or select files to add."),
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
