use zed_extension_api::{self as zed, LanguageServerId, Result};

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
        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/matiasmarani".to_string());

        let mut env = worktree.shell_env();

        // El usuario puede setear el token con MINIMAX_API_KEY o con
        // el alias MINIMAX_API_TOKEN. Si está el alias pero no la
        // variable canónica, lo copiamos para que el server lo
        // encuentre sin cambios.
        let has_api_key = env.iter().any(|(k, _)| k == "MINIMAX_API_KEY");
        if !has_api_key {
            if let Some((_, token)) = env.iter().find(|(k, _)| k == "MINIMAX_API_TOKEN") {
                env.push(("MINIMAX_API_KEY".to_string(), token.clone()));
            }
        }

        Ok(zed::Command {
            command: format!(
                "{home}/.config/zed/extensions/minimax-proxy/bin/minimax-proxy-server"
            ),
            args: vec![],
            env,
        })
    }
}

zed::register_extension!(MiniMaxProxyExtension);
