use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::diagnostics::{DiagnosticEngine, Finding};

pub struct ModernizeServer {
    client: Client,
    documents: Arc<RwLock<HashMap<Url, String>>>,
    engine: DiagnosticEngine,
}

impl ModernizeServer {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: Arc::new(RwLock::new(HashMap::new())),
            engine: DiagnosticEngine::new(),
        }
    }

    async fn publish_diagnostics(&self, url: &Url) {
        let docs = self.documents.read().await;
        let source = match docs.get(url) {
            Some(s) => s.clone(),
            None => return,
        };
        drop(docs);

        let findings = self.engine.scan(url.path(), &source);
        let diagnostics: Vec<Diagnostic> = findings
            .into_iter()
            .map(|f| self.finding_to_diagnostic(url, &source, &f))
            .collect();

        self.client
            .publish_diagnostics(url.clone(), diagnostics, None)
            .await;
    }

    fn finding_to_diagnostic(&self, url: &Url, source: &str, f: &Finding) -> Diagnostic {
        let range = byte_pos_to_lsp_range(&source, f.span.0, f.span.1);

        let severity = match f.level {
            cargo_modernize::Level::Deny => DiagnosticSeverity::ERROR,
            cargo_modernize::Level::Warn => DiagnosticSeverity::WARNING,
            cargo_modernize::Level::Suggest => DiagnosticSeverity::HINT,
            cargo_modernize::Level::Off => DiagnosticSeverity::INFORMATION,
        };

        let mut related: Option<Vec<DiagnosticRelatedInformation>> = None;
        if let Some(new_way) = &f.suggestion {
            related = Some(vec![DiagnosticRelatedInformation {
                location: Location {
                    uri: url.clone(),
                    range: range.clone(),
                },
                message: format!("New school: {new_way}"),
            }]);
        }

        Diagnostic {
            range,
            severity: Some(severity),
            code: Some(NumberOrString::String(f.rule_id.clone())),
            code_description: None,
            source: Some("cargo-modernize".into()),
            message: f.message.clone(),
            related_information: related,
            tags: None,
            data: serde_json::to_value(f).ok(),
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for ModernizeServer {
    async fn initialize(
        &self,
        _: InitializeParams,
    ) -> tower_lsp::jsonrpc::Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                diagnostic_provider: Some(DiagnosticServerCapabilities::Options(
                    DiagnosticOptions {
                        identifier: Some("cargo-modernize".into()),
                        inter_file_dependencies: false,
                        workspace_diagnostics: false,
                        ..Default::default()
                    },
                )),
                code_action_provider: Some(CodeActionProviderCapability::Options(
                    CodeActionOptions {
                        resolve_provider: Some(false),
                        code_action_kinds: Some(vec![CodeActionKind::QUICKFIX]),
                        ..Default::default()
                    },
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                ..ServerCapabilities::default()
            },
            ..InitializeResult::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        // nothing
    }

    async fn shutdown(&self) -> tower_lsp::jsonrpc::Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let DidOpenTextDocumentParams { text_document } = params;
        self.documents
            .write()
            .await
            .insert(text_document.uri.clone(), text_document.text);
        self.publish_diagnostics(&text_document.uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let DidChangeTextDocumentParams {
            text_document,
            content_changes,
        } = params;

        let mut docs = self.documents.write().await;
        let source = docs.get_mut(&text_document.uri);
        if let Some(doc) = source {
            for change in content_changes {
                let text = change.text;
                if let Some(range) = change.range {
                    // FULL sync kind means we replace the whole document; apply
                    // incremental changes if the client sends them.
                    *doc = apply_edit(doc, &range, &text);
                } else {
                    *doc = text;
                }
            }
        }
        drop(docs);
        self.publish_diagnostics(&text_document.uri).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        self.publish_diagnostics(&uri).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.write().await.remove(&uri);
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }

    async fn hover(&self, params: HoverParams) -> tower_lsp::jsonrpc::Result<Option<Hover>> {
        let HoverParams {
            text_document_position_params,
            ..
        } = params;
        let url = text_document_position_params.text_document.uri;
        let position = text_document_position_params.position;

        let docs = self.documents.read().await;
        let source = match docs.get(&url) {
            Some(s) => s,
            None => return Ok(None),
        };

        let byte_offset =
            line_col_to_byte_offset(source, position.line as usize, position.character as usize);
        let findings = self.engine.scan(url.path(), source);

        for f in findings {
            if byte_offset >= f.span.0 && byte_offset <= f.span.1 {
                let markup = MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: if let Some(new_way) = &f.suggestion {
                        format!(
                            "**{}**\n\nOld: {}\n\nNew: {}",
                            f.message, f.message, new_way
                        )
                    } else {
                        format!("**{}**\n\n{}", f.rule_id, f.message)
                    },
                };
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(markup),
                    range: Some(byte_pos_to_lsp_range(source, f.span.0, f.span.1)),
                }));
            }
        }

        Ok(None)
    }

    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> tower_lsp::jsonrpc::Result<Option<CodeActionResponse>> {
        let CodeActionParams {
            text_document: text_document_identifier,
            range,
            context,
            ..
        } = params;

        let mut actions = Vec::new();

        for diagnostic in context.diagnostics {
            let rule_id = match &diagnostic.code {
                Some(NumberOrString::String(s)) => s.clone(),
                Some(NumberOrString::Number(n)) => n.to_string(),
                None => continue,
            };

            // The `data` field carries the serialized `Finding`.
            let finding: Option<Finding> = diagnostic
                .data
                .as_ref()
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            if finding.is_none() {
                continue;
            }

            let finding = finding.unwrap();
            if finding.suggestion.is_none() {
                continue; // no mechanical fix available
            }

            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: format!("Apply cargo-modernize fix: {rule_id}"),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![diagnostic]),
                edit: Some(WorkspaceEdit {
                    changes: Some(HashMap::from([(
                        text_document_identifier.uri.clone(),
                        vec![TextEdit {
                            range,
                            new_text: finding.suggestion.unwrap_or_default(),
                        }],
                    )])),
                    document_changes: None,
                    change_annotations: None,
                }),
                command: None,
                data: None,
                is_preferred: Some(true),
                disabled: None,
            }));
        }

        Ok(Some(actions))
    }
}

/// Replace `[range]` in `doc` with `text`. Simplistic whole-document replacement
/// when the range covers everything; otherwise does a naive splice.
fn apply_edit(doc: &str, range: &Range, text: &str) -> String {
    let start = line_col_to_byte_offset(
        doc,
        range.start.line as usize,
        range.start.character as usize,
    );
    let end = line_col_to_byte_offset(doc, range.end.line as usize, range.end.character as usize);
    let mut out = doc.to_string();
    out.replace_range(start..end, text);
    out
}

/// Convert a byte-offset span to an LSP `Range`.
fn byte_pos_to_lsp_range(source: &str, start: usize, end: usize) -> Range {
    let (start_line, start_col) = byte_to_line_col(source, start);
    let (end_line, end_col) = byte_to_line_col(source, end);
    Range {
        start: Position {
            line: start_line as u32,
            character: start_col as u32,
        },
        end: Position {
            line: end_line as u32,
            character: end_col as u32,
        },
    }
}

/// Convert a line/column to a byte offset (UTF-8 aware).
fn line_col_to_byte_offset(source: &str, line: usize, col: usize) -> usize {
    source
        .lines()
        .take(line)
        .map(|l| l.len() + 1) // +1 for the newline
        .sum::<usize>()
        .saturating_add(col.min(source[..].len()))
}

/// Convert a byte offset to (line, column).
fn byte_to_line_col(source: &str, byte: usize) -> (usize, usize) {
    let byte = byte.min(source.len());
    let (before, _) = source.split_at(byte);
    let line = before.lines().count().saturating_sub(1);
    let col = before.lines().last().map(|l| l.len()).unwrap_or(0);
    (line, col)
}
