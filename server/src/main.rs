use std::{env, net::SocketAddr, process, time::Duration};

use axum::{
    body::Bytes,
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    time,
};

const DEFAULT_PORT: u16 = 8787;

// Secuencias de corte que M3 tiende a generar y que no queremos
// que lleguen a Zed: tags FIM repetidos, fin de turno, doble salto
// de línea (típico final de bloque de código), y nuestros propios
// marcadores del prompt.
const DEFAULT_STOP_SEQUENCES: &[&str] = &[
    "\n\n",
    "<|fim_",
    "<|endoftext|>",
    "<|im_end|>",
    "</context>",
    "<contextAfterCursor>",
];

#[derive(Clone)]
struct Config {
    port: u16,
    minimax_api_key: String,
    default_model: String,
    default_max_tokens: u64,
    default_temperature: f64,
    default_top_p: f64,
    default_top_k: u32,
    request_timeout_ms: u64,
    timeout_base_ms: u64,
    timeout_per_k_tokens_ms: u64,
    max_input_chars: usize,
    large_log_threshold: usize,
    large_log_head: usize,
    large_log_tail: usize,
}

#[derive(Clone)]
struct AppState {
    config: Config,
    client: Client,
}

#[derive(Debug, Deserialize)]
struct CompletionRequest {
    model: Option<String>,
    prompt: Option<String>,
    max_tokens: Option<u64>,
    max_output_tokens: Option<u64>,
    temperature: Option<f64>,
    top_p: Option<f64>,
    top_k: Option<u32>,
    stop: Option<StopSequences>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum StopSequences {
    One(String),
    Many(Vec<String>),
}

#[tokio::main]
async fn main() {
    let port = read_env_u16("PORT", DEFAULT_PORT);

    let lsp_stdio = tokio::spawn(run_lsp_stub());

    if port_is_active(port).await {
        println!("MiniMax proxy already running on port {port}");
        let _ = lsp_stdio.await;
        return;
    }

    let config = match Config::from_env(port) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            process::exit(1);
        }
    };

    let client = match Client::builder()
        .timeout(Duration::from_millis(config.request_timeout_ms))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            eprintln!("Failed to build HTTP client: {error}");
            process::exit(1);
        }
    };

    let state = AppState {
        config: config.clone(),
        client,
    };

    let app = Router::new()
        .route("/v1/completions", post(handle_completion))
        .fallback(not_found)
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], config.port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("Failed to bind http://{addr}: {error}");
            process::exit(1);
        }
    };

    println!(
        "MiniMax completions proxy listening on http://localhost:{}",
        config.port
    );
    println!("Default model: {}", config.default_model);
    println!("Default max tokens: {}", config.default_max_tokens);
    println!("Default temperature: {}", config.default_temperature);
    println!("Default top_p: {}", config.default_top_p);
    println!("Default top_k: {}", config.default_top_k);
    println!(
        "Max input chars: {} (prefix {} / suffix {})",
        config.max_input_chars,
        config.max_input_chars / 2,
        config.max_input_chars - config.max_input_chars / 2
    );
    println!(
        "Timeout: adaptive base={}ms + {}ms per 1k est. tokens, cap={}ms",
        config.timeout_base_ms, config.timeout_per_k_tokens_ms, config.request_timeout_ms
    );

    let server_result = axum::serve(listener, app).await;
    lsp_stdio.abort();
    if let Err(error) = server_result {
        eprintln!("Server error: {error}");
        process::exit(1);
    }
}

impl Config {
    fn from_env(port: u16) -> Result<Self, String> {
        let minimax_api_key = env::var("MINIMAX_API_KEY").map_err(|_| {
            "Missing MINIMAX_API_KEY\nRun: export MINIMAX_API_KEY='your_key'".to_string()
        })?;

        Ok(Self {
            port,
            minimax_api_key,
            default_model: env::var("MINIMAX_DEFAULT_MODEL")
                .unwrap_or_else(|_| "MiniMax-M3".to_string()),
            default_max_tokens: read_env_u64("MINIMAX_MAX_TOKENS", 256),
            default_temperature: read_env_f64("MINIMAX_TEMPERATURE", 1.0),
            default_top_p: read_env_f64("MINIMAX_TOP_P", 0.95),
            default_top_k: read_env_u32("MINIMAX_TOP_K", 40),
            request_timeout_ms: read_env_u64("MINIMAX_TIMEOUT_MS", 60_000),
            timeout_base_ms: read_env_u64("MINIMAX_TIMEOUT_BASE_MS", 10_000),
            timeout_per_k_tokens_ms: read_env_u64("MINIMAX_TIMEOUT_PER_K_TOKENS_MS", 2_000),
            max_input_chars: read_env_usize("MINIMAX_MAX_INPUT_CHARS", 1_000_000),
            large_log_threshold: read_env_usize("MINIMAX_LARGE_LOG_THRESHOLD", 2_000),
            large_log_head: read_env_usize("MINIMAX_LARGE_LOG_HEAD", 300),
            large_log_tail: read_env_usize("MINIMAX_LARGE_LOG_TAIL", 300),
        })
    }
}

async fn handle_completion(State(state): State<AppState>, body: Bytes) -> Response {
    let started_at = std::time::Instant::now();

    println!("\n--- Incoming request ---");
    println!("Method: POST");
    println!("Path: /v1/completions");

    match handle_completion_inner(&state, &body, started_at).await {
        Ok(response) => response,
        Err(error) => {
            eprintln!("Proxy error: {error}");
            println!(
                "Finished with proxy error in {} ms",
                started_at.elapsed().as_millis()
            );
            json_response(
                json!({
                    "error": {
                        "message": error,
                        "type": "proxy_error",
                    }
                }),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        }
    }
}

struct UpstreamResponse {
    status: StatusCode,
    content_type: HeaderValue,
    text: String,
}

async fn call_upstream_with_retry(
    state: &AppState,
    payload: &Value,
    timeout_ms: u64,
) -> Result<UpstreamResponse, String> {
    // Un solo reintento: M3 en 1M de contexto puede tirar timeout
    // puntual por cold start, no compensa hacer más de uno.
    const MAX_ATTEMPTS: u32 = 2;

    let mut last_error = String::new();

    for attempt in 0..MAX_ATTEMPTS {
        let send_result = state
            .client
            .post("https://api.minimax.io/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .bearer_auth(&state.config.minimax_api_key)
            .timeout(Duration::from_millis(timeout_ms))
            .json(payload)
            .send()
            .await;

        match send_result {
            Ok(response) => {
                let status = response.status();
                let content_type = response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .cloned()
                    .unwrap_or_else(|| HeaderValue::from_static("application/json"));
                match response.text().await {
                    Ok(text) => {
                        return Ok(UpstreamResponse {
                            status,
                            content_type,
                            text,
                        });
                    }
                    Err(error) => {
                        last_error = format!("read body: {error}");
                    }
                }
            }
            Err(error) => {
                last_error = format!("network: {error}");
            }
        }

        if attempt + 1 < MAX_ATTEMPTS {
            time::sleep(Duration::from_millis(150)).await;
        }
    }

    Err(last_error)
}

async fn handle_completion_inner(
    state: &AppState,
    body: &[u8],
    started_at: std::time::Instant,
) -> Result<Response, String> {
    let request: CompletionRequest =
        serde_json::from_slice(body).map_err(|error| error.to_string())?;

    let prompt = request.prompt.unwrap_or_default();
    let model = request
        .model
        .unwrap_or_else(|| state.config.default_model.clone());
    let max_tokens = request
        .max_tokens
        .or(request.max_output_tokens)
        .unwrap_or(state.config.default_max_tokens);
    let temperature = request
        .temperature
        .unwrap_or(state.config.default_temperature);
    let top_p = request.top_p.unwrap_or(state.config.default_top_p);
    let top_k = request.top_k.unwrap_or(state.config.default_top_k);
    let fim = parse_qwen_fim_prompt(&prompt);
    let max_prefix_chars = state.config.max_input_chars / 2;
    let max_suffix_chars = state.config.max_input_chars - max_prefix_chars;
    let user_prompt = build_minuet_style_user_prompt(&prompt, max_prefix_chars, max_suffix_chars);

    println!("Model: {model}");
    println!("Max tokens: {max_tokens}");
    println!("Temperature: {temperature}");
    println!("Top P: {top_p}");
    println!("Top K: {top_k}");
    println!("Timeout cap ms: {}", state.config.request_timeout_ms);
    println!("Qwen FIM detected: {}", fim.is_some());
    println!("Prompt length: {}", prompt.len());
    println!("Prompt preview:");
    println!("{}", preview(&prompt, 1000));

    if let Some((prefix, suffix)) = fim {
        println!("Prefix length: {}", prefix.len());
        println!("Suffix length: {}", suffix.len());
    }

    println!("User prompt sent to MiniMax preview:");
    println!("{}", preview_large(&user_prompt, &state.config));

    let upstream_payload = json!({
        "model": model,
        "stream": false,
        "messages": [
            {
                "role": "system",
                "content": build_system_prompt(),
            },
            {
                "role": "user",
                "content": user_prompt,
            },
        ],
        "max_tokens": max_tokens,
        "temperature": temperature,
        "top_p": top_p,
        "top_k": top_k,
        // thinking disabled = no razonamiento interno. Crítico
        // para completion inline: sin esto cada predicción tarda
        // segundos extra y consume tokens de output.
        "thinking": { "type": "disabled" },
    });

    println!("Calling MiniMax...");

    let upstream_body =
        serde_json::to_string(&upstream_payload).map_err(|error| error.to_string())?;
    let upstream_timeout_ms = compute_timeout_ms(upstream_body.len(), &state.config);

    println!("Upstream body chars: {}", upstream_body.len());
    println!("Upstream timeout ms: {}", upstream_timeout_ms);

    let upstream = call_upstream_with_retry(state, &upstream_payload, upstream_timeout_ms).await?;

    println!("MiniMax status: {}", upstream.status.as_u16());
    println!("MiniMax response preview:");
    println!("{}", preview(&upstream.text, 1000));

    if !upstream.status.is_success() {
        println!(
            "Finished with upstream error in {} ms",
            started_at.elapsed().as_millis()
        );

        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, upstream.content_type);
        return Ok((upstream.status, headers, upstream.text).into_response());
    }

    let upstream_json: Value =
        serde_json::from_str(&upstream.text).map_err(|error| error.to_string())?;
    let raw_completion = upstream_json
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let completion = clean_completion(raw_completion, &prompt, request.stop.as_ref());
    let usage = upstream_json.get("usage").unwrap_or(&Value::Null);

    println!("Raw completion length: {}", raw_completion.len());
    println!("Raw completion preview:");
    println!("{}", preview(raw_completion, 1000));
    println!("Final completion length: {}", completion.len());
    println!("Final completion preview:");
    println!("{}", preview(&completion, 1000));

    let response_body = json!({
        "id": "cmpl-minimax-proxy",
        "object": "text_completion",
        "created": unix_timestamp_seconds(),
        "model": model,
        "choices": [
            {
                "text": completion,
                "index": 0,
                "logprobs": null,
                "finish_reason": "stop",
            }
        ],
        "usage": {
            "prompt_tokens": usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            "completion_tokens": usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            "total_tokens": usage
                .get("total_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        },
    });

    println!("Finished OK in {} ms", started_at.elapsed().as_millis());

    Ok(json_response(response_body, StatusCode::OK))
}

async fn not_found() -> Response {
    println!("Rejected: route not found");
    json_response(json!({ "error": "Not found" }), StatusCode::NOT_FOUND)
}

fn json_response(data: Value, status: StatusCode) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    (status, headers, data.to_string()).into_response()
}

async fn port_is_active(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    matches!(
        time::timeout(Duration::from_millis(250), TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}

async fn run_lsp_stub() {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);
    let mut buffer = Vec::new();

    loop {
        buffer.clear();

        let mut content_length: Option<usize> = None;
        loop {
            let mut header_line: Vec<u8> = Vec::new();
            let bytes = match reader.read_until(b'\n', &mut header_line).await {
                Ok(0) => return,
                Ok(bytes) => bytes,
                Err(_) => return,
            };

            if bytes == 0 {
                return;
            }

            let trimmed = std::str::from_utf8(&header_line)
                .map(|line| line.trim_end_matches(['\r', '\n']))
                .unwrap_or("");

            if trimmed.is_empty() {
                if content_length.is_some() {
                    break;
                }
                continue;
            }

            if let Some(value) = trimmed
                .strip_prefix("Content-Length:")
                .or_else(|| trimmed.strip_prefix("content-length:"))
            {
                if let Some(parsed) = value
                    .trim()
                    .parse::<usize>()
                    .ok()
                {
                    content_length = Some(parsed);
                }
            }
        }

        let length = match content_length {
            Some(length) => length,
            None => continue,
        };

        buffer.resize(length, 0);
        if reader.read_exact(&mut buffer).await.is_err() {
            return;
        }

        let message: Value = match serde_json::from_slice(&buffer) {
            Ok(value) => value,
            Err(_) => continue,
        };

        if let Some(response) = handle_lsp_message(&message) {
            write_lsp_message(&mut stdout, &response).await;
        }
    }
}

fn handle_lsp_message(message: &Value) -> Option<Value> {
    let method = message.get("method").and_then(Value::as_str)?;
    let id = message.get("id").cloned();

    match method {
        "initialize" => Some(json!({
            "jsonrpc": "2.0",
            "id": id?,
            "result": {
                "capabilities": {},
                "serverInfo": { "name": "zed-proxy", "version": env!("CARGO_PKG_VERSION") },
            }
        })),
        "shutdown" => Some(json!({ "jsonrpc": "2.0", "id": id?, "result": null })),
        _ => None,
    }
}

async fn write_lsp_message<W>(writer: &mut W, message: &Value)
where
    W: AsyncWriteExt + Unpin,
{
    let body = message.to_string();
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let _ = writer.write_all(header.as_bytes()).await;
    let _ = writer.write_all(body.as_bytes()).await;
    let _ = writer.flush().await;
}

fn read_env_u16(name: &str, default: u16) -> u16 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn read_env_u32(name: &str, default: u32) -> u32 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn read_env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn read_env_f64(name: &str, default: f64) -> f64 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn read_env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn compute_timeout_ms(payload_chars: usize, config: &Config) -> u64 {
    const CHARS_PER_TOKEN: f64 = 4.0;
    let est_tokens = payload_chars as f64 / CHARS_PER_TOKEN;
    let k_tokens = (est_tokens / 1000.0).ceil() as u64;
    let adaptive = config.timeout_base_ms + k_tokens * config.timeout_per_k_tokens_ms;
    adaptive.min(config.request_timeout_ms)
}

fn unix_timestamp_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn preview(value: &str, max_length: usize) -> String {
    if value.len() <= max_length {
        return value.to_string();
    }

    let mut end = max_length;
    while !value.is_char_boundary(end) {
        end -= 1;
    }

    format!(
        "{}\n...[truncated {} chars]",
        &value[..end],
        value.len() - end
    )
}

fn preview_large(value: &str, config: &Config) -> String {
    if value.len() <= config.large_log_threshold {
        return value.to_string();
    }

    let head_end = char_boundary_at(value, config.large_log_head);
    let tail_start = char_boundary_from_end(value, config.large_log_tail);

    format!(
        "{}\n...[truncated {} chars of {} total]...\n{}",
        &value[..head_end],
        value.len() - head_end - (value.len() - tail_start),
        value.len(),
        &value[tail_start..]
    )
}

fn char_boundary_at(value: &str, max_chars: usize) -> usize {
    let mut end = max_chars.min(value.len());
    while end < value.len() && !value.is_char_boundary(end) {
        end += 1;
    }
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    end
}

fn char_boundary_from_end(value: &str, max_chars: usize) -> usize {
    let mut start = value.len().saturating_sub(max_chars);
    while start < value.len() && !value.is_char_boundary(start) {
        start += 1;
    }
    start
}

fn head(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn tail(value: &str, max_chars: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    let start = chars.len().saturating_sub(max_chars);
    chars[start..].iter().collect()
}

fn parse_qwen_fim_prompt(prompt: &str) -> Option<(&str, &str)> {
    let prefix_marker = "<|fim_prefix|>";
    let suffix_marker = "<|fim_suffix|>";
    let middle_marker = "<|fim_middle|>";

    let prefix_start = prompt.find(prefix_marker)?;
    let suffix_start = prompt.find(suffix_marker)?;
    let middle_start = prompt.find(middle_marker)?;

    if !(prefix_start < suffix_start && suffix_start < middle_start) {
        return None;
    }

    let prefix = &prompt[prefix_start + prefix_marker.len()..suffix_start];
    let suffix = &prompt[suffix_start + suffix_marker.len()..middle_start];

    Some((prefix, suffix))
}

fn guess_language_from_prefix(prefix: &str) -> &'static str {
    let lower = prefix.to_lowercase();

    if lower.contains("export default") || lower.contains("function ") || lower.contains("const ") {
        return "javascript";
    }

    if lower.contains("interface ") || lower.contains(": string") || lower.contains(": number") {
        return "typescript";
    }

    if lower.contains("def ") || lower.contains("import ") {
        return "python";
    }

    if lower.contains("fn ") || lower.contains("let mut") {
        return "rust";
    }

    if lower.contains("package main") || lower.contains("func ") {
        return "go";
    }

    "unknown"
}

fn build_minuet_style_user_prompt(
    original_prompt: &str,
    max_prefix_chars: usize,
    max_suffix_chars: usize,
) -> String {
    if let Some((prefix, suffix)) = parse_qwen_fim_prompt(original_prompt) {
        let prefix = tail(prefix, max_prefix_chars);
        let suffix = head(suffix, max_suffix_chars);
        let language = guess_language_from_prefix(&prefix);

        return [
            format!("# language: {language}"),
            "<contextBeforeCursor>".to_string(),
            format!("{prefix}<cursorPosition>"),
            "<contextAfterCursor>".to_string(),
            suffix,
        ]
        .join("\n");
    }

    let prefix = tail(original_prompt, max_prefix_chars);
    let language = guess_language_from_prefix(&prefix);

    [
        format!("# language: {language}"),
        "<contextBeforeCursor>".to_string(),
        format!("{prefix}<cursorPosition>"),
        "<contextAfterCursor>".to_string(),
        String::new(),
    ]
    .join("\n")
}

fn strip_repeated_prompt(completion: &str, original_prompt: &str) -> String {
    if let Some(stripped) = completion.strip_prefix(original_prompt) {
        return stripped.to_string();
    }

    let trimmed_prompt = original_prompt.trim_end();
    if let Some(stripped) = completion.strip_prefix(trimmed_prompt) {
        return stripped.to_string();
    }

    if let Some((prefix, _)) = parse_qwen_fim_prompt(original_prompt) {
        if let Some(stripped) = completion.strip_prefix(prefix) {
            return stripped.to_string();
        }
    }

    completion.to_string()
}

fn remove_markdown_fences(text: &str) -> String {
    let without_start = if let Some(rest) = text.strip_prefix("```") {
        match rest.find('\n') {
            Some(index)
                if rest[..index]
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-') =>
            {
                &rest[index + 1..]
            }
            _ => rest,
        }
    } else {
        text
    };

    without_start
        .strip_suffix("```")
        .map(|value| value.strip_suffix('\n').unwrap_or(value))
        .unwrap_or(without_start)
        .to_string()
}

fn remove_prompt_artifacts(text: &str) -> String {
    text.replace("<|fim_prefix|>", "")
        .replace("<|fim_suffix|>", "")
        .replace("<|fim_middle|>", "")
        .replace("<contextBeforeCursor>", "")
        .replace("<contextAfterCursor>", "")
        .replace("<cursorPosition>", "")
        .replace("<cursor>", "")
        .replace("</cursor>", "")
}

fn apply_stop_sequences(text: &str, stop: Option<&StopSequences>) -> String {
    let mut result = text.to_string();

    // Encadena los stop sequences del usuario con los defaults del proxy.
    // Los del usuario van primero para que tengan prioridad si el caller
    // quiere cortar antes que los boundaries naturales de M3.
    let user_stops: Vec<&str> = match stop {
        Some(StopSequences::One(value)) => vec![value.as_str()],
        Some(StopSequences::Many(values)) => values.iter().map(String::as_str).collect(),
        None => Vec::new(),
    };

    for stop_sequence in user_stops
        .iter()
        .chain(DEFAULT_STOP_SEQUENCES.iter())
    {
        if stop_sequence.is_empty() {
            continue;
        }
        if let Some(index) = result.find(stop_sequence) {
            result.truncate(index);
        }
    }

    result
}

fn cut_obvious_over_generation(text: &str) -> String {
    let bad_boundaries = [
        "```",
        "<|fim_prefix|>",
        "<|fim_suffix|>",
        "<|fim_middle|>",
        "<contextBeforeCursor>",
        "<contextAfterCursor>",
        "<cursorPosition>",
        "</context>",
    ];

    let mut result = text.to_string();

    for boundary in bad_boundaries {
        if let Some(index) = result.find(boundary) {
            result.truncate(index);
        }
    }

    result
}

fn clean_completion(
    raw_completion: &str,
    original_prompt: &str,
    stop: Option<&StopSequences>,
) -> String {
    let mut completion = raw_completion.to_string();

    completion = strip_repeated_prompt(&completion, original_prompt);
    completion = remove_markdown_fences(&completion);
    completion = remove_prompt_artifacts(&completion);
    completion = apply_stop_sequences(&completion, stop);
    completion = cut_obvious_over_generation(&completion);

    completion.trim_end().to_string()
}

fn build_system_prompt() -> String {
    "You are a code completion engine. Output only the text to insert at <cursorPosition>. Preserve exact whitespace and indentation. No markdown fences, no explanation, no repetition of context before or after the cursor. Prefer one line or a few lines. If the natural completion is a comment, output only the comment."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_qwen_fim_prompt() {
        let prompt = "<|fim_prefix|>const a = <|fim_suffix|>;\n<|fim_middle|>";

        let (prefix, suffix) = parse_qwen_fim_prompt(prompt).expect("expected FIM prompt");

        assert_eq!(prefix, "const a = ");
        assert_eq!(suffix, ";\n");
    }

    #[test]
    fn rejects_invalid_qwen_fim_marker_order() {
        let prompt = "<|fim_suffix|>after<|fim_prefix|>before<|fim_middle|>";

        assert!(parse_qwen_fim_prompt(prompt).is_none());
    }

    #[test]
    fn builds_fallback_prompt_without_fim() {
        let prompt = "function greet() {\n  const name = ";

        let result = build_minuet_style_user_prompt(prompt, 500_000, 500_000);

        assert!(result.contains("# language: javascript"));
        assert!(result.contains("<contextBeforeCursor>"));
        assert!(result.contains("function greet()"));
        assert!(result.contains("<cursorPosition>"));
        assert!(result.ends_with("<contextAfterCursor>\n"));
    }

    #[test]
    fn builds_fim_prompt_with_suffix() {
        let prompt = "<|fim_prefix|>fn main() {\n<|fim_suffix|>}\n<|fim_middle|>";

        let result = build_minuet_style_user_prompt(prompt, 500_000, 500_000);

        assert!(result.contains("# language: rust"));
        assert!(result.contains("fn main() {\n<cursorPosition>"));
        assert!(result.ends_with("}\n"));
    }

    #[test]
    fn strips_repeated_original_prompt() {
        let result = strip_repeated_prompt("abc completion", "abc ");

        assert_eq!(result, "completion");
    }

    #[test]
    fn strips_repeated_fim_prefix() {
        let prompt = "<|fim_prefix|>let value = <|fim_suffix|>;\n<|fim_middle|>";
        let result = strip_repeated_prompt("let value = 42", prompt);

        assert_eq!(result, "42");
    }

    #[test]
    fn removes_markdown_fences() {
        let result = remove_markdown_fences("```rust\nlet x = 1;\n```");

        assert_eq!(result, "let x = 1;");
    }

    #[test]
    fn removes_prompt_artifacts() {
        let result = remove_prompt_artifacts("<cursor>abc</cursor><|fim_middle|>");

        assert_eq!(result, "abc");
    }

    #[test]
    fn applies_single_stop_sequence() {
        let stop = StopSequences::One("END".to_string());
        let result = apply_stop_sequences("abcENDdef", Some(&stop));

        assert_eq!(result, "abc");
    }

    #[test]
    fn applies_multiple_stop_sequences_in_order() {
        let stop = StopSequences::Many(vec!["DEF".to_string(), "BC".to_string()]);
        let result = apply_stop_sequences("ABCDEF", Some(&stop));

        assert_eq!(result, "A");
    }

    #[test]
    fn applies_default_stop_sequences_when_user_omits_them() {
        let result = apply_stop_sequences("let x = 1;\n\nnext line", None);

        assert_eq!(result, "let x = 1;");
    }

    #[test]
    fn user_stop_takes_priority_over_default() {
        let stop = StopSequences::One("|fim_".to_string());
        let result = apply_stop_sequences("abc<|fim_|xyz", Some(&stop));

        // User pidió cortar en "<|fim_" — el default "<|fim_" es el
        // mismo trigger, así que cortamos en el mismo sitio. Lo que
        // cuenta es que un stop del usuario (aunque sea substring de
        // uno default) se aplica primero.
        assert_eq!(result, "abc");
    }

    #[test]
    fn cuts_obvious_over_generation() {
        let result = cut_obvious_over_generation("abc<contextAfterCursor>def");

        assert_eq!(result, "abc");
    }

    #[test]
    fn cleans_completion_pipeline() {
        let stop = StopSequences::One("STOP".to_string());
        let result = clean_completion("```ts\nconst x = 1;STOP extra\n```", "", Some(&stop));

        assert_eq!(result, "const x = 1;");
    }

    #[test]
    fn default_stops_cut_at_double_newline() {
        let result = clean_completion("let x = 1;\n\nfn next() {}", "", None);

        assert_eq!(result, "let x = 1;");
    }
}
