use zed_extension_api::{self as zed, settings::LspSettings, LanguageServerId, Result};

struct XQueryExtension;

impl zed::Extension for XQueryExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        // Prefer whatever is in the shell PATH (handles system installs and nix/homebrew).
        if let Some(path) = worktree.which("xquery-lsp-server") {
            return Ok(zed::Command { command: path, args: vec![], env: Default::default() });
        }

        // Fall back to ~/.cargo/bin, which is where `cargo install` puts the binary.
        if let Ok(home) = std::env::var("HOME") {
            let cargo_bin = std::path::PathBuf::from(home)
                .join(".cargo")
                .join("bin")
                .join("xquery-lsp-server");
            if cargo_bin.exists() {
                return Ok(zed::Command {
                    command: cargo_bin.to_string_lossy().to_string(),
                    args: vec![],
                    env: Default::default(),
                });
            }
        }

        Err("xquery-lsp-server not found. \
             Install it with: cargo install --path lsp-server \
             (run from the extension source directory)."
            .to_string())
    }

    fn language_server_initialization_options(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<serde_json::Value>> {
        let settings = LspSettings::for_worktree(language_server_id.as_ref(), worktree)?;
        Ok(settings.initialization_options)
    }
}

zed::register_extension!(XQueryExtension);
