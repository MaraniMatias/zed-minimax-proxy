use serde::Deserialize;
use zed_extension_api::{self as zed, settings::LspSettings, LanguageServerId, Result};

const SERVER_BIN_NAME: &str = "minimax-proxy-server";
const SERVER_LANGUAGE_SERVER_NAME: &str = "minimax-proxy";

#[derive(Clone, Debug, Default, Deserialize)]
struct MiniMaxProxySettings {
    #[serde(default = "default_model")]
    model: String,
    #[serde(default = "default_max_tokens")]
    max_tokens: u64,
    #[serde(default)]
    api_token: String,
    #[serde(default = "default_temperature")]
    temperature: f64,
    #[serde(default = "default_top_p")]
    top_p: f64,
}

fn default_model() -> String {
    "MiniMax-M3".to_string()
}

fn default_max_tokens() -> u64 {
    256
}

fn default_temperature() -> f64 {
    0.2
}

fn default_top_p() -> f64 {
    0.95
}

fn resolve_settings(worktree: &zed::Worktree) -> MiniMaxProxySettings {
    let raw = LspSettings::for_worktree(SERVER_LANGUAGE_SERVER_NAME, worktree)
        .ok()
        .and_then(|s| s.settings);
    match raw {
        Some(value) => serde_json::from_value(value).unwrap_or_default(),
        None => MiniMaxProxySettings::default(),
    }
}

fn locate_server_binary(worktree: &zed::Worktree) -> Result<String> {
    if let Some(path) = worktree.which(SERVER_BIN_NAME) {
        return Ok(path);
    }

    let root = worktree.root_path();
    let candidates = [
        format!("{root}/server/target/release/{SERVER_BIN_NAME}"),
        format!("{root}/bin/{SERVER_BIN_NAME}"),
        format!("{root}/target/release/{SERVER_BIN_NAME}"),
    ];

    for candidate in candidates {
        if std::path::Path::new(&candidate).exists() {
            return Ok(candidate);
        }
    }

    Err(format!(
        "could not locate {SERVER_BIN_NAME}. Build it with `cargo build --release --manifest-path server/Cargo.toml`, copy it to `bin/{SERVER_BIN_NAME}`, or install it on PATH via `cargo install --path server`."
    ))
}

struct MiniMaxProxyExtension;

impl zed::Extension for MiniMaxProxyExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let server_path = locate_server_binary(worktree)?;
        let settings = resolve_settings(worktree);

        let mut env = worktree.shell_env();
        env.push(("MINIMAX_MODEL".to_string(), settings.model));
        env.push(("MINIMAX_MAX_TOKENS".to_string(), settings.max_tokens.to_string()));
        env.push(("MINIMAX_TEMPERATURE".to_string(), settings.temperature.to_string()));
        env.push(("MINIMAX_TOP_P".to_string(), settings.top_p.to_string()));
        env.push(("MINIMAX_API_KEY".to_string(), settings.api_token));

        Ok(zed::Command {
            command: server_path,
            args: vec![],
            env,
        })
    }
}

zed::register_extension!(MiniMaxProxyExtension);
