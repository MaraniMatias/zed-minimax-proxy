use zed_extension_api::{self as zed, LanguageServerId, Result};

struct ZedProxyExtension;

impl zed::Extension for ZedProxyExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/matiasmarani".to_string());

        Ok(zed::Command {
            command: format!("{home}/.config/zed/extensions/zed-proxy/bin/zed-proxy-server"),
            args: vec![],
            env: worktree.shell_env(),
        })
    }
}

zed::register_extension!(ZedProxyExtension);
