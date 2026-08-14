use crate::config::{AppConfig, CHAT_MESSAGE_CHAR_LIMIT};

const LARGE_BINARY_DELTA_THRESHOLD: u64 = 100_000;
const DEFAULT_IGNORED_DIFF_PATHS: &[&str] = &[
    "dist/**",
    "build/**",
    "coverage/**",
    "target/**",
    "*.map",
    "*.min.js",
    "*.min.css",
];
const DEFAULT_LOCKFILE_NAMES: &[&str] = &[
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "Cargo.lock",
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffPreparationReport {
    pub original_chars: usize,
    pub prepared_chars: usize,
    pub input_budget_chars: usize,
    pub omitted_files: Vec<String>,
    pub truncated_files: Vec<String>,
    pub reached_input_budget: bool,
}

impl DiffPreparationReport {
    fn was_summarized(&self) -> bool {
        self.original_chars != self.prepared_chars
            || !self.omitted_files.is_empty()
            || !self.truncated_files.is_empty()
            || self.reached_input_budget
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreparedDiff {
    pub(crate) content: String,
    pub(crate) report: DiffPreparationReport,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DiffPart {
    path: Option<String>,
    text: String,
}

pub(crate) fn print_diff_preparation_report(report: &DiffPreparationReport) {
    if !report.was_summarized() {
        return;
    }

    eprintln!(
        "Diff input summarized: {} chars -> {} chars (budget: {} chars).",
        report.original_chars, report.prepared_chars, report.input_budget_chars
    );

    if !report.truncated_files.is_empty() {
        eprintln!(
            "Truncated {} large diff section(s): {}",
            report.truncated_files.len(),
            summarize_report_items(&report.truncated_files)
        );
    }

    if !report.omitted_files.is_empty() {
        eprintln!(
            "Omitted {} diff section(s): {}",
            report.omitted_files.len(),
            summarize_report_items(&report.omitted_files)
        );
    }

    if report.reached_input_budget {
        eprintln!("Input budget was exhausted before all diff sections were included.");
    }
}

fn summarize_report_items(items: &[String]) -> String {
    const MAX_ITEMS: usize = 5;
    let mut summary = items
        .iter()
        .take(MAX_ITEMS)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > MAX_ITEMS {
        summary.push_str(&format!(", ... and {} more", items.len() - MAX_ITEMS));
    }
    summary
}

pub(crate) fn prepare_diff_for_llm(config: &AppConfig, diff: &str) -> PreparedDiff {
    let original_chars = diff.chars().count();
    let budget = config.max_input_chars.min(CHAT_MESSAGE_CHAR_LIMIT - 1);
    let max_file_chars = config.max_file_chars.min(budget);
    let mut remaining = budget;
    let mut content = String::new();
    let mut omitted_files = Vec::new();
    let mut truncated_files = Vec::new();
    let mut reached_input_budget = false;

    for part in split_diff_parts(diff) {
        let part_path = part.path.clone();
        let path = part_path.as_deref();
        let Some(prepared_part) = prepare_diff_part(
            part,
            path,
            max_file_chars,
            config.include_lockfiles,
            &config.ignore_diff_paths,
            &mut omitted_files,
            &mut truncated_files,
        ) else {
            continue;
        };

        if prepared_part.trim().is_empty() {
            continue;
        }

        let separator_chars = if content.is_empty() { 0 } else { 2 };
        let prepared_chars = prepared_part.chars().count();
        if prepared_chars + separator_chars <= remaining {
            if !content.is_empty() {
                content.push_str("\n\n");
                remaining -= 2;
            }
            content.push_str(&prepared_part);
            remaining -= prepared_chars;
            continue;
        }

        reached_input_budget = true;
        if let Some(path) = path {
            omitted_files.push(format!(
                "{} (omitted: remaining input budget {} chars)",
                path, remaining
            ));
        } else {
            omitted_files.push(format!(
                "<metadata> (omitted: remaining input budget {} chars)",
                remaining
            ));
        }
        break;
    }

    if content.trim().is_empty() && !diff.trim().is_empty() {
        content =
            truncate_to_char_boundary(diff, budget, "\n[diff omitted: input budget exhausted]\n");
        reached_input_budget = true;
    }

    let prepared_chars = content.chars().count();
    PreparedDiff {
        content,
        report: DiffPreparationReport {
            original_chars,
            prepared_chars,
            input_budget_chars: budget,
            omitted_files,
            truncated_files,
            reached_input_budget,
        },
    }
}

fn prepare_diff_part(
    part: DiffPart,
    path: Option<&str>,
    max_file_chars: usize,
    include_lockfiles: bool,
    ignore_diff_paths: &[String],
    omitted_files: &mut Vec<String>,
    truncated_files: &mut Vec<String>,
) -> Option<String> {
    if let Some(path) = path
        && should_ignore_diff_path(path, include_lockfiles, ignore_diff_paths)
    {
        omitted_files.push(format!("{path} (ignored by diff input policy)"));
        return Some(format!("Edit file {path} (diff omitted by input policy)"));
    }

    let text_chars = part.text.chars().count();
    if text_chars <= max_file_chars {
        return Some(part.text);
    }

    let path_label = path.unwrap_or("<metadata>");
    truncated_files.push(format!(
        "{} ({} chars -> {} chars)",
        path_label, text_chars, max_file_chars
    ));
    Some(truncate_to_char_boundary(
        &part.text,
        max_file_chars,
        &format!(
            "\n[omitted: {path_label}, original diff was {text_chars} chars, exceeded per-file budget]\n"
        ),
    ))
}

fn split_diff_parts(diff: &str) -> Vec<DiffPart> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut current_path = None;

    for line in diff.split_inclusive('\n') {
        if line.starts_with("diff --git ") || line.starts_with("Untracked file: ") {
            if !current.is_empty() {
                parts.push(DiffPart {
                    path: current_path.take(),
                    text: current,
                });
                current = String::new();
            }
            current_path = parse_diff_part_path(line);
        }

        current.push_str(line);
    }

    if !current.is_empty() {
        parts.push(DiffPart {
            path: current_path,
            text: current,
        });
    }

    parts
}

fn parse_diff_part_path(line: &str) -> Option<String> {
    if line.starts_with("diff --git ") {
        return parse_diff_git_path(line);
    }

    line.strip_prefix("Untracked file: ")
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
}

fn should_ignore_diff_path(
    path: &str,
    include_lockfiles: bool,
    ignore_diff_paths: &[String],
) -> bool {
    if !include_lockfiles && is_lockfile_path(path) {
        return true;
    }

    DEFAULT_IGNORED_DIFF_PATHS
        .iter()
        .any(|pattern| path_matches_pattern(path, pattern))
        || ignore_diff_paths
            .iter()
            .any(|pattern| path_matches_pattern(path, pattern))
}

fn is_lockfile_path(path: &str) -> bool {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    DEFAULT_LOCKFILE_NAMES.contains(&file_name)
}

fn path_matches_pattern(path: &str, pattern: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return false;
    }

    if let Some(prefix) = pattern.strip_suffix("/**") {
        let prefix = prefix.trim_end_matches('/');
        return path == prefix || path.starts_with(&format!("{prefix}/"));
    }

    if let Some(suffix) = pattern.strip_prefix("*.") {
        return path
            .rsplit('/')
            .next()
            .unwrap_or(path)
            .ends_with(&format!(".{suffix}"));
    }

    if let Some(suffix) = pattern.strip_prefix('*') {
        return path.ends_with(suffix);
    }

    path == pattern || path.starts_with(&format!("{pattern}/"))
}

fn truncate_to_char_boundary(text: &str, max_chars: usize, marker: &str) -> String {
    let marker_chars = marker.chars().count();
    if marker_chars >= max_chars {
        return marker.chars().take(max_chars).collect();
    }

    let keep_chars = max_chars.saturating_sub(marker_chars);
    let mut truncated = text.chars().take(keep_chars).collect::<String>();
    truncated.push_str(marker);
    truncated
}

pub(crate) fn sanitize_diff_for_llm(diff: &str) -> String {
    let mut sanitized = String::new();
    let mut current_section = String::new();

    for line in diff.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            sanitized.push_str(&sanitize_diff_section(&current_section));
            current_section.clear();
        }

        if current_section.is_empty() && !line.starts_with("diff --git ") {
            sanitized.push_str(line);
        } else {
            current_section.push_str(line);
        }
    }

    sanitized.push_str(&sanitize_diff_section(&current_section));
    sanitized
}

fn sanitize_diff_section(section: &str) -> String {
    if section.is_empty() || !should_omit_diff_section(section) {
        return section.to_string();
    }

    format!(
        "Edit file {} (large binary diff omitted)\n",
        diff_section_file_path(section)
    )
}

fn should_omit_diff_section(section: &str) -> bool {
    section
        .lines()
        .any(|line| line == "GIT binary patch" || large_binary_delta_size(line).is_some())
}

fn large_binary_delta_size(line: &str) -> Option<u64> {
    let size = line.strip_prefix("delta ")?.split_whitespace().next()?;
    let size = size.parse::<u64>().ok()?;
    (size >= LARGE_BINARY_DELTA_THRESHOLD).then_some(size)
}

fn diff_section_file_path(section: &str) -> String {
    let Some(header) = section.lines().next() else {
        return "<unknown>".to_string();
    };

    parse_diff_git_path(header).unwrap_or_else(|| "<unknown>".to_string())
}

fn parse_diff_git_path(header: &str) -> Option<String> {
    let rest = header.strip_prefix("diff --git ")?;

    if let Some((_, path)) = rest.rsplit_once(" b/") {
        return Some(path.trim().trim_matches('"').to_string());
    }

    let parts = shell_words::split(rest).ok()?;
    parts.get(1).and_then(|path| {
        path.strip_prefix("b/")
            .or_else(|| path.strip_prefix("a/"))
            .map(ToOwned::to_owned)
    })
}

#[cfg(test)]
mod tests {
    use crate::commit::CommitOptions;
    use crate::config::{
        DEFAULT_MAX_FILE_CHARS, DEFAULT_MAX_INPUT_CHARS, DEFAULT_STAGED_DIFF_COMMAND,
        DEFAULT_UNSTAGED_DIFF_COMMAND, DEFAULT_UNSTAGED_FILES_COMMAND,
        DEFAULT_UNTRACKED_DIFF_COMMAND, DEFAULT_UNTRACKED_FILES_COMMAND, IncludeUnstagedMode,
    };

    use super::*;

    fn base_test_config() -> AppConfig {
        AppConfig {
            api_key: "key".to_string(),
            base_url: "http://127.0.0.1:3002".to_string(),
            endpoint: "/v1/chat/completions".to_string(),
            model: "model".to_string(),
            effort: None,
            prompt: "prompt".to_string(),
            diff_command: DEFAULT_STAGED_DIFF_COMMAND.to_string(),
            staged_diff_command: DEFAULT_STAGED_DIFF_COMMAND.to_string(),
            unstaged_diff_command: DEFAULT_UNSTAGED_DIFF_COMMAND.to_string(),
            untracked_diff_command: DEFAULT_UNTRACKED_DIFF_COMMAND.to_string(),
            unstaged_files_command: DEFAULT_UNSTAGED_FILES_COMMAND.to_string(),
            untracked_files_command: DEFAULT_UNTRACKED_FILES_COMMAND.to_string(),
            include_unstaged: IncludeUnstagedMode::Ask,
            max_input_chars: DEFAULT_MAX_INPUT_CHARS,
            max_file_chars: DEFAULT_MAX_FILE_CHARS,
            include_lockfiles: false,
            ignore_diff_paths: Vec::new(),
            temperature: 0.2,
            max_tokens: 512,
            timeout_seconds: 120,
            proxy: None,
            headers: Vec::new(),
            stage_all: true,
            confirm: true,
            assume_yes: false,
            dry_run: false,
            commit: CommitOptions::default(),
        }
    }

    #[test]
    fn sanitizes_git_binary_patch_sections_for_llm() {
        let diff = "Note.pdf | Bin 1 -> 2 bytes\nsrc/lib.rs | 1 +\n\n\
diff --git a/Note.pdf b/Note.pdf\n\
index 2e1a645..0dd5894 100644\n\
GIT binary patch\n\
delta 1089611\n\
binary payload\n\
diff --git a/src/lib.rs b/src/lib.rs\n\
index 1111111..2222222 100644\n\
--- a/src/lib.rs\n\
+++ b/src/lib.rs\n\
@@ -1 +1 @@\n\
-old\n\
+new\n";

        let sanitized = sanitize_diff_for_llm(diff);

        assert!(sanitized.contains("Note.pdf | Bin 1 -> 2 bytes"));
        assert!(sanitized.contains("Edit file Note.pdf"));
        assert!(!sanitized.contains("GIT binary patch"));
        assert!(!sanitized.contains("binary payload"));
        assert!(sanitized.contains("diff --git a/src/lib.rs b/src/lib.rs"));
        assert!(sanitized.contains("+new"));
    }

    #[test]
    fn keeps_small_text_diff_sections_for_llm() {
        let diff = "diff --git a/src/lib.rs b/src/lib.rs\n-old\n+new\n";

        assert_eq!(sanitize_diff_for_llm(diff), diff);
    }

    #[test]
    fn sanitizes_large_delta_sections_for_llm() {
        let diff = "diff --git a/asset.bin b/asset.bin\n\
index 1111111..2222222 100644\n\
delta 100000\n\
huge payload\n";

        let sanitized = sanitize_diff_for_llm(diff);

        assert_eq!(
            sanitized,
            "Edit file asset.bin (large binary diff omitted)\n"
        );
    }

    #[test]
    fn omits_lockfiles_and_generated_diff_sections_by_default() {
        let mut config = base_test_config();
        config.max_input_chars = 12_000;
        config.max_file_chars = 8_000;
        let diff = "diff --git a/src/lib.rs b/src/lib.rs\n-old\n+new\n\
diff --git a/Cargo.lock b/Cargo.lock\n-old lock\n+new lock\n\
diff --git a/dist/app.js b/dist/app.js\n-old bundle\n+new bundle\n";

        let prepared = prepare_diff_for_llm(&config, diff);

        assert!(prepared.content.contains("+new"));
        assert!(
            prepared
                .content
                .contains("Cargo.lock (diff omitted by input policy)")
        );
        assert!(
            prepared
                .content
                .contains("dist/app.js (diff omitted by input policy)")
        );
        assert!(!prepared.content.contains("+new lock"));
        assert!(!prepared.content.contains("+new bundle"));
        assert_eq!(prepared.report.omitted_files.len(), 2);
    }

    #[test]
    fn can_include_lockfiles_when_configured() {
        let mut config = base_test_config();
        config.include_lockfiles = true;
        let diff = "diff --git a/Cargo.lock b/Cargo.lock\n-old lock\n+new lock\n";

        let prepared = prepare_diff_for_llm(&config, diff);

        assert!(prepared.content.contains("+new lock"));
        assert!(prepared.report.omitted_files.is_empty());
    }

    #[test]
    fn truncates_large_diff_sections_to_per_file_budget() {
        let mut config = base_test_config();
        config.max_input_chars = 12_000;
        config.max_file_chars = 4_200;
        let large_payload = "x".repeat(8_000);
        let diff = format!("diff --git a/src/large.rs b/src/large.rs\n+{large_payload}\n");

        let prepared = prepare_diff_for_llm(&config, &diff);

        assert!(prepared.content.chars().count() <= 4_200);
        assert!(prepared.content.contains("exceeded per-file budget"));
        assert_eq!(prepared.report.truncated_files.len(), 1);
    }

    #[test]
    fn stops_adding_sections_when_input_budget_is_exhausted() {
        let mut config = base_test_config();
        config.max_input_chars = 4_500;
        config.max_file_chars = 4_200;
        let first_payload = "a".repeat(3_600);
        let second_payload = "b".repeat(3_600);
        let diff = format!(
            "diff --git a/src/one.rs b/src/one.rs\n\
index 1111111..2222222 100644\n\
--- a/src/one.rs\n\
+++ b/src/one.rs\n\
@@ -1 +1 @@\n\
-old\n\
+{first_payload}\n\
diff --git a/src/two.rs b/src/two.rs\n\
index 1111111..2222222 100644\n\
--- a/src/two.rs\n\
+++ b/src/two.rs\n\
@@ -1 +1 @@\n\
-old\n\
+{second_payload}\n"
        );

        let prepared = prepare_diff_for_llm(&config, &diff);

        assert!(prepared.content.contains("src/one.rs"));
        assert!(!prepared.content.contains("src/two.rs"));
        assert!(prepared.report.reached_input_budget);
        assert_eq!(prepared.report.omitted_files.len(), 1);
    }
}
