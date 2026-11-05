//! Applies identifier renames and version bumps in place.

use crate::scan::{Edit, PackageReport};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub fn apply(reports: &[PackageReport]) -> Result<usize> {
    let mut by_file: BTreeMap<PathBuf, Vec<&Edit>> = BTreeMap::new();
    for f in reports.iter().flat_map(|r| &r.findings) {
        if let Some(e) = &f.fix {
            by_file.entry(e.path.clone()).or_default().push(e);
        }
    }
    let mut applied = 0;
    for (path, mut edits) in by_file {
        // Right to left, so earlier columns on the same line stay valid.
        edits.sort_by_key(|e| std::cmp::Reverse((e.line, e.col)));
        edits.dedup_by(|a, b| (a.line, a.col) == (b.line, b.col));
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_owned).collect();
        for e in edits {
            let Some(line) = lines.get_mut(e.line - 1) else { continue };
            let Some(byte) = line.char_indices().nth(e.col).map(|(b, _)| b) else { continue };
            if line[byte..].starts_with(&e.old) {
                line.replace_range(byte..byte + e.old.len(), &e.new);
                applied += 1;
            }
        }
        std::fs::write(&path, lines.concat()).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(applied)
}
