use anyhow::Result;
use cargo_modernize_lsp::ModernizeServer;
use tower_lsp::{LspService, Server};

#[tokio::main]
async fn main() -> Result<()> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::build(|client| ModernizeServer::new(client)).finish();

    Server::new(stdin, stdout, socket).serve(service).await;
    Ok(())
}
