//! Bridge between `cargo-modernize` findings and LSP diagnostics.

use serde::{Deserialize, Serialize};
use std::path::Path;

use cargo_modernize::config::Config;
use cargo_modernize::rules::{Level, Rule};
use cargo_modernize::scan;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub rule_id: String,
    pub message: String,
    pub level: Level,
    pub span: (usize, usize),       // byte start, byte end
    pub suggestion: Option<String>, // "new school" replacement text
}

/// Thin wrapper around `cargo-modernize`'s scanner.
pub struct DiagnosticEngine {
    rules: Vec<Rule>,
    config: Config,
}

impl DiagnosticEngine {
    pub fn new() -> Self {
        let rules = cargo_modernize::rules::builtin().unwrap_or_default();
        let config = Config::default();
        Self { rules, config }
    }

    /// Scan a single file's source text and return findings.
    pub fn scan(&self, path: &str, source: &str) -> Vec<Finding> {
        let path = Path::new(path);
        let findings = scan::scan_source(path, source, &self.rules, &self.config, None);

        findings
            .into_iter()
            .map(|f| self.convert(f, source))
            .collect()
    }

    fn convert(&self, f: cargo_modernize::scan::Finding, source: &str) -> Finding {
        let start =
            line_col_to_byte_offset(source, f.line.saturating_sub(1), f.column.saturating_sub(1));
        let end = line_col_to_byte_offset(
            source,
            f.end_line.saturating_sub(1),
            f.end_column.saturating_sub(1),
        );

        let message = if let Some(detail) = &f.detail {
            detail.clone()
        } else if let Some(note) = &f.note {
            note.clone()
        } else {
            f.source_line.trim().to_string()
        };

        let suggestion = f
            .fix
            .and_then(|fix| fix.edits.into_iter().next().map(|e| e.new));

        Finding {
            rule_id: f.rule.id,
            message,
            level: f.level,
            span: (start, end),
            suggestion,
        }
    }
}

/// Convert a 0-based line/column to a byte offset (UTF-8 aware).
fn line_col_to_byte_offset(source: &str, line: usize, col: usize) -> usize {
    source
        .lines()
        .take(line)
        .map(|l| l.len() + 1) // +1 for the newline
        .sum::<usize>()
        .saturating_add(col.min(source.len()))
}
