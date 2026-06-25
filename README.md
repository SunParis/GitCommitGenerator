# GitCommitGenerator

GitCommitGenerator is an AI-powered Git commit message generator for
OpenAI-compatible chat completion APIs.

It detects staged changes first. If unstaged tracked files or untracked files are
present, it asks whether to include all, none, or a selected subset in the
commit message. Included files are staged only after the final commit
confirmation.

## Install

```bash
cargo install --path .
```

Or run from the repository:

```bash
cargo run -- --model gpt-5.4 --api-key "$GCG_API_KEY"
```

## Configuration

Configuration is loaded from:

```text
~/.config/gitcommitgenerator/config.toml
```

CLI arguments override the config file. `api_key` and `model` are required
unless supplied with `--api-key` / `--model` or environment variables.

Example:

```toml
api_key = "sk-..."
base_url = "http://127.0.0.1:3002"
endpoint = "/v1/chat/completions"
model = "gpt-5.4"
temperature = 0.2
max_tokens = 512
timeout_seconds = 120
proxy = "http://127.0.0.1:7890"
stage_all = true
confirm = true
dry_run = false
include_unstaged = "ask" # ask, always, or never
max_input_chars = 500000
max_file_chars = 80000
include_lockfiles = false
ignore_diff_paths = ["fixtures/**", "*.snap"]

# Legacy alias for staged_diff_command. Prefer the explicit commands below.
diff_command = "git diff --cached --stat && git diff --cached --binary --find-renames"
staged_diff_command = "git diff --cached --stat && git diff --cached --binary --find-renames"
unstaged_diff_command = "git diff --stat && git diff --binary --find-renames"
untracked_diff_command = "git ls-files --others --exclude-standard | while IFS= read -r file; do printf '\\nUntracked file: %s\\n' \"$file\"; git diff --no-index -- /dev/null \"$file\" || true; done"
unstaged_files_command = "git diff --name-only"
untracked_files_command = "git ls-files --others --exclude-standard"

# Inline prompt or prompt_file may be used. If omitted, the built-in prompt
# follows Conventional Commits when appropriate.
prompt = """
Generate a clear Conventional Commit-style message from this diff.
Return only the commit message.
"""

headers = [
  "X-Custom-Header: value"
]

[commit]
signoff = true
no_verify = false
amend = false
allow_empty = false
allow_empty_message = false
author = "Example User <user@example.com>"
cleanup = "strip"
args = ["--trailer Reviewed-by=QA"]
```

`base_url` may be either the gateway root or an OpenAI-style prefix such as
`http://127.0.0.1:3002/v1`; the tool avoids duplicating path segments when
joining it with `endpoint`.

Diff input controls:

| Config key | CLI option | Default | Description |
| --- | --- | --- | --- |
| `max_input_chars` | `--max-input-chars` | `500000` | Maximum characters from the prepared diff that can be sent in the LLM request. Must be at least `4000` and below the chat message limit. |
| `max_file_chars` | `--max-file-chars` | `80000` | Maximum characters kept from one diff section before it is truncated with an omission marker. |
| `include_lockfiles` | `--include-lockfiles true/false` | `false` | Include lock file diffs in full instead of summarizing them. |
| `ignore_diff_paths` | `--ignore-diff-path` | `[]` | Extra path patterns to summarize instead of sending in full. Repeat the CLI flag for multiple patterns. |

Supported `ignore_diff_paths` patterns are intentionally simple: exact paths,
directory prefixes such as `fixtures/**`, extension-style patterns such as
`*.snap`, and suffix patterns such as `*generated.json`.

Environment variables:

```bash
export GCG_API_KEY="sk-..."
export GCG_MODEL="gpt-5.4"
export GCG_BASE_URL="http://127.0.0.1:3002"
export GCG_ENDPOINT="/v1/chat/completions"
export GCG_CONFIG="$HOME/.config/gitcommitgenerator/config.toml"
export GCG_MAX_INPUT_CHARS="500000"
export GCG_MAX_FILE_CHARS="80000"
```

`OPENAI_API_KEY` is also accepted as a fallback for the API key.

## Usage

```bash
gitcommitgenerator --model gpt-5.4 --api-key "$GCG_API_KEY"
```

Common options:

```bash
gitcommitgenerator \
  --base-url http://127.0.0.1:3002 \
  --model gpt-5.4 \
  --api-key "$GCG_API_KEY" \
  --signoff \
  --no-verify
```

Skip confirmation and print the message without committing:

```bash
gitcommitgenerator --model gpt-5.4 --api-key "$GCG_API_KEY" --yes --dry-run
```

Use a custom prompt and staged diff command:

```bash
gitcommitgenerator \
  --model gpt-5.4 \
  --api-key "$GCG_API_KEY" \
  --prompt-file ./commit-prompt.txt \
  --staged-diff-command "git diff --cached --stat && git diff --cached"
```

Control whether unstaged tracked files and untracked files are included:

```bash
gitcommitgenerator --model gpt-5.4 --api-key "$GCG_API_KEY" --include-unstaged ask
gitcommitgenerator --model gpt-5.4 --api-key "$GCG_API_KEY" --include-unstaged always
gitcommitgenerator --model gpt-5.4 --api-key "$GCG_API_KEY" --include-unstaged never
```

Control diff input size for large commits:

```bash
gitcommitgenerator \
  --model gpt-5.4 \
  --api-key "$GCG_API_KEY" \
  --max-input-chars 500000 \
  --max-file-chars 80000 \
  --include-lockfiles false \
  --ignore-diff-path "fixtures/**"
```

Disable automatic staging:

```bash
gitcommitgenerator --model gpt-5.4 --api-key "$GCG_API_KEY" --no-stage-all
```

With `--no-stage-all`, extra files may still be used for message generation if
you include them, but the tool will not automatically stage them before
`git commit`.

Pass extra `git commit` arguments:

```bash
gitcommitgenerator \
  --model gpt-5.4 \
  --api-key "$GCG_API_KEY" \
  --commit-arg "--trailer Reviewed-by=QA"
```

## Commit behavior

By default, the tool runs side-effect-free commands before the final commit
confirmation:

```bash
git diff --cached --stat
git diff --cached --binary --find-renames
git diff --name-only
git ls-files --others --exclude-standard
```

If unstaged or untracked files are found, the default `include_unstaged = "ask"`
mode prompts:

```text
Include unstaged and untracked files? [Y/n/select files to add]
```

Use `Y` to include every unstaged/untracked file, `n` to ignore them, or
`select files to add` to choose a subset. The selection UI lists files in reverse
numeric order:

```text
:: 3 files...
3  docs/readme.md
2  src/lib.rs
1  new.txt
==> Files to exclude: (for example: "1 2 3", "1-3", "^4", or file names)
==>
```

The selector starts with all listed files included. Enter numbers, ranges, or
file-name fragments to exclude files; prefix a selector with `^` to re-include a
file after a broader exclusion.

If extra files are included, the tool also runs side-effect-free path-scoped diff
commands before the LLM request:

```bash
git diff --stat -- <selected unstaged files>
git diff --binary --find-renames -- <selected unstaged files>
git diff --no-index -- /dev/null <selected untracked file>
```

The configured `unstaged_diff_command` and `untracked_diff_command` are used
when `include_unstaged = "always"` includes every extra file. Interactive
selection uses path-scoped Git commands so excluded files do not appear in the
LLM prompt.

## Diff input limits

Before calling the LLM, the collected diff is sanitized and constrained by an
input budget. This prevents OpenAI-compatible gateways from rejecting the
request because one chat message is too large.

The tool first removes large Git binary patch payloads. It then splits the diff
into file-level sections, applies path policy, truncates sections above
`max_file_chars`, and stops adding sections once `max_input_chars` is exhausted.
If the final prompt still exceeds the hard chat message limit of `10485760`
characters, the request is rejected locally with a clear error instead of
sending a doomed API call.

By default, these paths are summarized instead of sent in full:

- lock files: `package-lock.json`, `pnpm-lock.yaml`, `yarn.lock`, `Cargo.lock`
- generated directories: `dist/**`, `build/**`, `coverage/**`, `target/**`
- generated/minified files: `*.map`, `*.min.js`, `*.min.css`

Set `include_lockfiles = true` or pass `--include-lockfiles true` to keep lock
file diffs. Add project-specific generated files with `ignore_diff_paths` or
repeated `--ignore-diff-path` flags.

Summarized sections remain visible to the model as file-level markers such as:

```text
Edit file Cargo.lock (diff omitted by input policy)
[omitted: src/large.rs, original diff was 90000 chars, exceeded per-file budget]
```

If the diff has to be summarized, the tool prints a short report before the LLM
request:

```text
Diff input summarized: 24355202 chars -> 500000 chars (budget: 500000 chars).
Truncated 3 large diff section(s): src/large.rs (90000 chars -> 80000 chars)
Omitted 8 diff section(s): Cargo.lock (ignored by diff input policy), ...
```

For very large commits, prefer excluding generated output or lowering
`max_file_chars` rather than raising `max_input_chars` close to the gateway
limit. Commit messages are usually better when the prompt contains a compact
summary of representative source changes instead of megabytes of repeated
generated content.

Before the final commit confirmation, the default workflow does not mutate the
repository. After the user confirms the generated commit message, included
unstaged and untracked paths are staged with `git add -- <path>...` when
`stage_all = true`, then the commit is created.

The generated message is passed to:

```bash
git commit -m "<generated message>"
```

Supported named commit flags include:

- `--amend`
- `--no-amend`
- `--signoff`
- `--no-signoff`
- `--no-verify`
- `--verify`
- `--allow-empty`
- `--no-allow-empty`
- `--allow-empty-message`
- `--no-allow-empty-message`
- `--author`
- `--date`
- `--cleanup`
- `--gpg-sign`
- `--commit-arg` for additional raw arguments

If `git commit` fails with a simple recoverable error, such as an empty commit,
an empty commit message, or unstaged changes while automatic staging is disabled,
the tool asks whether to retry with the appropriate fix.

## Development

```bash
cargo fmt
cargo test
cargo clippy --all-targets --all-features
```
