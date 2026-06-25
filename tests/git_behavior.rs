use gitcommitgenerator::{
    AppConfig, ChangeSet, CommitFix, CommitOptions, DEFAULT_MAX_FILE_CHARS,
    DEFAULT_MAX_INPUT_CHARS, DEFAULT_STAGED_DIFF_COMMAND, DEFAULT_UNSTAGED_DIFF_COMMAND,
    DEFAULT_UNTRACKED_DIFF_COMMAND, DiffPreparationReport, IncludeUnstagedMode, build_commit_args,
    collect_changes, detect_commit_fix, run_shell_command,
};
use std::fs;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use tempfile::tempdir;

static CWD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[test]
fn default_commit_arguments_are_valid_for_git() {
    let args = build_commit_args(&CommitOptions::default(), "Add generated message").unwrap();
    assert_eq!(args, vec!["commit", "-m", "Add generated message"]);
}

fn with_current_dir<T>(path: &std::path::Path, run: impl FnOnce() -> T) -> T {
    let _guard = CWD_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    let old_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(path).unwrap();
    let result = run();
    std::env::set_current_dir(old_dir).unwrap();
    result
}

#[test]
fn diff_command_can_capture_staged_untracked_files() {
    let dir = tempdir().unwrap();
    Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    fs::write(dir.path().join("new.txt"), "hello\n").unwrap();
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let diff = with_current_dir(dir.path(), || {
        run_shell_command("git diff --cached --name-only").unwrap()
    });

    assert_eq!(diff.trim(), "new.txt");
}

fn sample_config(include_unstaged: IncludeUnstagedMode) -> AppConfig {
    AppConfig {
        api_key: "key".to_string(),
        base_url: "http://127.0.0.1:3002".to_string(),
        endpoint: "/v1/chat/completions".to_string(),
        model: "model".to_string(),
        prompt: "prompt".to_string(),
        diff_command: DEFAULT_STAGED_DIFF_COMMAND.to_string(),
        staged_diff_command: DEFAULT_STAGED_DIFF_COMMAND.to_string(),
        unstaged_diff_command: DEFAULT_UNSTAGED_DIFF_COMMAND.to_string(),
        untracked_diff_command: DEFAULT_UNTRACKED_DIFF_COMMAND.to_string(),
        unstaged_files_command: "git diff --name-only".to_string(),
        untracked_files_command: "git ls-files --others --exclude-standard".to_string(),
        include_unstaged,
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

fn repo_with_staged_unstaged_and_untracked() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    fs::write(dir.path().join("tracked.txt"), "base\n").unwrap();
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    fs::write(dir.path().join("staged.txt"), "staged\n").unwrap();
    Command::new("git")
        .args(["add", "staged.txt"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    fs::write(dir.path().join("tracked.txt"), "base\nunstaged\n").unwrap();
    fs::write(dir.path().join("untracked.txt"), "untracked\n").unwrap();

    dir
}

#[test]
fn collect_changes_includes_staged_unstaged_and_untracked_when_enabled() {
    let dir = repo_with_staged_unstaged_and_untracked();
    let changes = with_current_dir(dir.path(), || {
        collect_changes(&sample_config(IncludeUnstagedMode::Always)).unwrap()
    });

    assert!(changes.diff.contains("staged.txt"));
    assert!(changes.diff.contains("tracked.txt"));
    assert!(changes.diff.contains("untracked.txt"));
    assert!(changes.diff.contains("Untracked file: untracked.txt"));
    assert_eq!(changes.included_unstaged_paths, vec!["tracked.txt"]);
    assert_eq!(changes.included_untracked_paths, vec!["untracked.txt"]);
}

#[test]
fn collect_changes_ignores_unstaged_and_untracked_when_disabled() {
    let dir = repo_with_staged_unstaged_and_untracked();
    let changes = with_current_dir(dir.path(), || {
        collect_changes(&sample_config(IncludeUnstagedMode::Never)).unwrap()
    });

    assert!(changes.diff.contains("staged.txt"));
    assert!(!changes.diff.contains("tracked.txt"));
    assert!(!changes.diff.contains("untracked.txt"));
    assert!(changes.included_unstaged_paths.is_empty());
    assert!(changes.included_untracked_paths.is_empty());
}

#[test]
fn collect_changes_diffs_only_selected_extra_files() {
    let dir = repo_with_staged_unstaged_and_untracked();
    let mut config = sample_config(IncludeUnstagedMode::Always);
    config.unstaged_files_command = "printf '%s\\n' tracked.txt".to_string();
    config.untracked_files_command = "printf '%s\\n'".to_string();

    let changes = with_current_dir(dir.path(), || collect_changes(&config).unwrap());

    assert!(changes.diff.contains("staged.txt"));
    assert!(changes.diff.contains("tracked.txt"));
    assert!(!changes.diff.contains("untracked.txt"));
    assert_eq!(changes.included_unstaged_paths, vec!["tracked.txt"]);
    assert!(changes.included_untracked_paths.is_empty());
}

#[test]
fn change_set_reports_extra_paths_only_when_included() {
    let changes = ChangeSet {
        diff: "diff".to_string(),
        included_unstaged_paths: vec!["tracked.txt".to_string()],
        included_untracked_paths: Vec::new(),
        diff_report: DiffPreparationReport::default(),
    };

    assert_eq!(changes.included_unstaged_paths, vec!["tracked.txt"]);
}

#[test]
fn no_changes_added_suggests_staging_when_stage_all_is_disabled() {
    assert_eq!(
        detect_commit_fix(
            "no changes added to commit (use \"git add\" and/or \"git commit -a\")",
            false,
            &CommitOptions::default(),
        ),
        Some(CommitFix::StageAll)
    );
}
