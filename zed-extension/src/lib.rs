use zed_extension_api::{self as zed, LanguageServerId, Result};

struct ModernizeExtension;

impl zed::Extension for ModernizeExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let command = worktree
            .which("cargo-modernize-lsp")
            .ok_or("cargo-modernize-lsp not on PATH; run `cargo install --path cargo-modernize-lsp`")?;
        Ok(zed::Command {
            command,
            args: vec![],
            env: worktree.shell_env(),
        })
    }
}

zed::register_extension!(ModernizeExtension);
