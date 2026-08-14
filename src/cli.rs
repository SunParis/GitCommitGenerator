use std::path::PathBuf;

use clap::{ArgAction, Parser};

use crate::commit::CleanupMode;
use crate::config::IncludeUnstagedMode;

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

    #[arg(long, env = "GCG_EFFORT")]
    pub effort: Option<String>,

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

    #[arg(long, env = "GCG_MAX_INPUT_CHARS")]
    pub max_input_chars: Option<usize>,

    #[arg(long, env = "GCG_MAX_FILE_CHARS")]
    pub max_file_chars: Option<usize>,

    #[arg(long, action = ArgAction::Set)]
    pub include_lockfiles: Option<bool>,

    #[arg(long)]
    pub ignore_diff_path: Vec<String>,

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
