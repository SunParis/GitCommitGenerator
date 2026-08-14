use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use reqwest::Proxy;
use reqwest::StatusCode;
use reqwest::blocking::Client;
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize};

use crate::config::{AppConfig, CHAT_MESSAGE_CHAR_LIMIT, ReasoningEffort};

const EMPTY_LLM_RESPONSE_RETRIES: usize = 2;

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    temperature: f32,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'a str>,
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
    content: Option<String>,
}

pub fn generate_commit_message(config: &AppConfig, diff: &str) -> Result<String> {
    let client = build_client(config)?;
    let url = join_url(&config.base_url, &config.endpoint);
    let user_content = format!("{}\n\nGit diff:\n{}", config.prompt, diff);
    let user_content_chars = user_content.chars().count();
    if user_content_chars >= CHAT_MESSAGE_CHAR_LIMIT {
        bail!(
            "LLM request is too large after diff summarization: {user_content_chars} chars; limit is {CHAT_MESSAGE_CHAR_LIMIT}. Reduce prompt size or max_input_chars."
        );
    }

    let headers = config
        .headers
        .iter()
        .map(|header| parse_header(header))
        .collect::<Result<Vec<_>>>()?;
    let mut effort = config.effort;
    let mut used_effort_fallback = false;
    let mut empty_response_attempts = 0;

    loop {
        let request = ChatRequest {
            model: &config.model,
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            reasoning_effort: effort.map(ReasoningEffort::as_str),
            messages: vec![ChatMessage {
                role: "user",
                content: user_content.clone(),
            }],
        };

        let mut builder = client
            .post(&url)
            .bearer_auth(&config.api_key)
            .json(&request);
        for (name, value) in &headers {
            builder = builder.header(name, value);
        }

        let response = builder.send().context("failed to call LLM API")?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let body = response.text().context("failed to read LLM response")?;

        if !status.is_success() {
            if let Some(rejected_effort) = effort
                && !used_effort_fallback
                && should_retry_without_effort(status, &body)
            {
                eprintln!(
                    "Warning: LLM API rejected reasoning_effort={:?}; retrying without it.",
                    rejected_effort.as_str()
                );
                effort = None;
                used_effort_fallback = true;
                continue;
            }

            bail!(
                "LLM API request failed\nURL: {url}\nStatus: {status}\nContent-Type: {}\nBody preview:\n{}",
                content_type.as_deref().unwrap_or("<missing>"),
                response_preview(&body)
            );
        }

        match parse_commit_message_response(&body, &url, status.as_u16(), content_type.as_deref()) {
            Ok(message) => return Ok(message),
            Err(error)
                if is_empty_llm_response_error(&error)
                    && empty_response_attempts < EMPTY_LLM_RESPONSE_RETRIES =>
            {
                empty_response_attempts += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

fn should_retry_without_effort(status: StatusCode, body: &str) -> bool {
    if status != StatusCode::BAD_REQUEST && status != StatusCode::UNPROCESSABLE_ENTITY {
        return false;
    }

    let normalized = body.to_ascii_lowercase();
    let mentions_effort = normalized.contains("effort");
    let rejects_effort = [
        "unsupported",
        "not supported",
        "does not support",
        "unknown",
        "unrecognized",
        "invalid",
        "not allowed",
        "unavailable",
        "not available",
    ]
    .iter()
    .any(|marker| normalized.contains(marker));

    mentions_effort && rejects_effort
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
    let parsed: ChatResponse = serde_json::from_str(body).map_err(|error| {
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
        .unwrap_or_default()
        .trim()
        .trim_matches('`')
        .trim()
        .to_string();

    if content.is_empty() {
        bail!("LLM returned an empty commit message");
    }
    Ok(content)
}

fn is_empty_llm_response_error(error: &anyhow::Error) -> bool {
    error.to_string() == "LLM returned an empty commit message"
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

#[cfg(test)]
mod tests {
    use mockito::Matcher;
    use serde_json::json;

    use crate::commit::CommitOptions;
    use crate::config::{
        DEFAULT_MAX_FILE_CHARS, DEFAULT_MAX_INPUT_CHARS, DEFAULT_STAGED_DIFF_COMMAND,
        DEFAULT_UNSTAGED_DIFF_COMMAND, DEFAULT_UNSTAGED_FILES_COMMAND,
        DEFAULT_UNTRACKED_DIFF_COMMAND, DEFAULT_UNTRACKED_FILES_COMMAND, IncludeUnstagedMode,
    };

    use super::*;

    fn base_test_config(base_url: String) -> AppConfig {
        AppConfig {
            api_key: "key".to_string(),
            base_url,
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
    fn parses_openai_compatible_chat_response() {
        let body = r#"{
            "choices": [{"message": {"content": "Add configurable commit generation"}}]
        }"#;

        assert_eq!(
            parse_commit_message(body).unwrap(),
            "Add configurable commit generation"
        );
    }

    #[test]
    fn missing_chat_message_content_is_empty_commit_message() {
        let body = r#"{
            "choices": [
                {
                    "finish_reason": "stop",
                    "index": 0,
                    "message": {"role": "assistant"}
                }
            ]
        }"#;

        let error = parse_commit_message_response(
            body,
            "http://127.0.0.1:3002/v1/chat/completions",
            200,
            Some("application/json; charset=utf-8"),
        )
        .unwrap_err()
        .to_string();

        assert_eq!(error, "LLM returned an empty commit message");
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
    fn effort_rejection_retries_once_without_reasoning_effort() {
        let mut server = mockito::Server::new();
        let rejected = server
            .mock("POST", "/v1/chat/completions")
            .match_body(Matcher::PartialJson(json!({"reasoning_effort": "max"})))
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":{"message":"reasoning_effort is not supported"}}"#)
            .expect(1)
            .create();
        let accepted_body = json!({
            "model": "model",
            "messages": [{"role": "user", "content": "prompt\n\nGit diff:\ndiff"}],
            "temperature": 0.2,
            "max_tokens": 512
        });
        let accepted = server
            .mock("POST", "/v1/chat/completions")
            .match_body(Matcher::Json(accepted_body))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"content":"fix: retry without effort"}}]}"#)
            .expect(1)
            .create();
        let mut config = base_test_config(server.url());
        config.effort = Some(ReasoningEffort::Max);

        let message = generate_commit_message(&config, "diff").unwrap();

        assert_eq!(message, "fix: retry without effort");
        rejected.assert();
        accepted.assert();
    }

    #[test]
    fn generic_bad_request_does_not_trigger_effort_fallback() {
        assert!(!should_retry_without_effort(
            StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"invalid max_tokens"}}"#
        ));
    }

    #[test]
    fn unprocessable_effort_error_triggers_fallback() {
        assert!(should_retry_without_effort(
            StatusCode::UNPROCESSABLE_ENTITY,
            r#"{"error":{"message":"effort is invalid for this model"}}"#
        ));
    }

    #[test]
    fn join_url_deduplicates_openai_style_suffixes() {
        assert_eq!(
            join_url("http://127.0.0.1:3002/v1", "/v1/chat/completions"),
            "http://127.0.0.1:3002/v1/chat/completions"
        );
    }
}
