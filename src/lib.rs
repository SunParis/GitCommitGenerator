mod app;
mod changes;
mod cli;
mod commit;
mod config;
mod diff;
mod git;
mod llm;

pub use app::run;
pub use changes::{
    ChangeSet, ExtraChangeSelection, ExtraPathKind, SelectablePath, collect_changes,
    parse_file_selection, selectable_paths, selection_from_indexes, stage_included_paths,
};
pub use cli::Cli;
pub use commit::{
    CleanupMode, CommitFix, CommitOptions, build_commit_args, confirm, detect_commit_fix,
};
pub use config::{
    AppConfig, CommitConfig, DEFAULT_BASE_URL, DEFAULT_DIFF_COMMAND, DEFAULT_ENDPOINT,
    DEFAULT_MAX_FILE_CHARS, DEFAULT_MAX_INPUT_CHARS, DEFAULT_PROMPT, DEFAULT_STAGED_DIFF_COMMAND,
    DEFAULT_UNSTAGED_DIFF_COMMAND, DEFAULT_UNSTAGED_FILES_COMMAND, DEFAULT_UNTRACKED_DIFF_COMMAND,
    DEFAULT_UNTRACKED_FILES_COMMAND, FileConfig, IncludeUnstagedMode, ReasoningEffort,
    default_config_path, load_file_config, resolve_config,
};
pub use diff::DiffPreparationReport;
pub use git::{ensure_inside_git_repo, run_git, run_lines_command, run_shell_command};
pub use llm::{
    generate_commit_message, join_url, parse_commit_message, parse_commit_message_response,
};
