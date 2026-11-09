//! Parse files written in syntax that `syn` 3 rejects but rustc 2015/2018 accepted.
//!
//! When a file fails to parse, look at where the parser stopped and patch that spot in
//! a copy of the text, then parse again. The patches are small:
//!
//! - `Box<Fn(i32)>` → `Box<dyn Fn(i32)>` (bare trait objects)
//! - `fn f(&self, &T)` → `fn f(&self, _: &T)` (anonymous trait parameters, edition 2015)
//! - `await`, `async`, `dyn`, `try` used as names → the same name with a capital last letter
//! - `crate struct S;` → `pub   struct S;` (nightly `crate` visibility)
//!
//! Syntax from before Rust 1.0 is patched too, since 2015 snapshots still hold it:
//!
//! - `proc() { … }` → `    || { … }`, `box x` → `    x`, `#[deriving(..)]` → `#[derive  (..)]`
//! - `[T, ..N]` → `[T;   N]`, `extern crate "a-b" as c` → `extern crate  a_b  as c`
//! - `use a::{mod, B}` → `use a::{_mo, B}`, `pub type E { .. }` → `pub enum E { .. }`
//! - `|uint| -> T` (closure type) → `fn(uint) -> T`, `x as u32 << 8` → `x as (u32) << 8`
//! - `v[]` → `v[..]`, `v[mut]` → `v[.. ]`, `v[mut a..b]` → `v[    a..b]`, and a `;` after an item macro written with `( … )`
//!
//! Most patches keep the length of the line. The ones that do not (`dyn `, `_: `, and the
//! last two above) are remembered so a position in the patched text can be mapped back to
//! the original. Findings from a repaired file carry no autofix: a rewrite would be built
//! from patched text. A file whose brackets do not balance is beyond repair.

use crate::scan::{byte_offset, line_starts};
use proc_macro2::LineColumn;

/// Text added at (`line`, `col`) of the patched file, `len` chars long.
#[derive(Debug, Clone)]
struct Insert {
    line: usize,
    col: usize,
    len: usize,
}

pub struct Repaired {
    pub text: String,
    inserts: Vec<Insert>,
}

impl Repaired {
    /// A position in the patched text, as a position in the original file.
    pub fn to_original(&self, lc: LineColumn) -> LineColumn {
        let mut sub = 0;
        for i in self.inserts.iter().filter(|i| i.line == lc.line) {
            if lc.column >= i.col + i.len {
                sub += i.len;
            } else if lc.column > i.col {
                sub += lc.column - i.col;
            }
        }
        LineColumn { line: lc.line, column: lc.column - sub }
    }
}

/// Where and why a file could not be parsed even after patching.
#[derive(Debug)]
pub struct Failure {
    /// In the original file.
    pub line: usize,
    pub column: usize,
    pub message: String,
}

/// Parse `original`, patching old syntax as needed. `None` if some error is not one we know how to fix.
pub fn parse(original: &str) -> Option<(syn::File, Repaired)> {
    try_parse(original).ok()
}

/// Like [`parse`], but says where it gave up.
pub fn try_parse(original: &str) -> Result<(syn::File, Repaired), Failure> {
    let mut text = original.to_owned();
    let mut inserts = Vec::new();
    let mut last = None;
    prepass(&mut text);
    for _ in 0..400 {
        match syn::parse_file(&text) {
            Ok(ast) => return Ok((ast, Repaired { text, inserts })),
            Err(e) => {
                let at = e.span().start();
                let off = byte_offset(&text, &line_starts(&text), at);
                let message = e.to_string();
                let stuck = Repaired { text: String::new(), inserts: inserts.clone() };
                let pos = stuck.to_original(at);
                last = Some(Failure { line: pos.line, column: pos.column + 1, message: message.clone() });
                if !fix_at(&mut text, &mut inserts, off, &message) {
                    break;
                }
            }
        }
    }
    Err(last.unwrap_or(Failure { line: 1, column: 1, message: "too many repairs".into() }))
}

const FN_TRAITS: [&str; 4] = ["Fn", "FnMut", "FnOnce", "FnBox"];
const RESERVED: [&str; 4] = ["await", "async", "dyn", "try"];

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The identifier ending just before `off` (skipping whitespace), with its start offset.
fn word_before(text: &str, off: usize) -> Option<(usize, &str)> {
    let head = text[..off].trim_end();
    let start = head.rfind(|c: char| !is_word(c)).map_or(0, |i| i + head[i..].chars().next().map_or(1, char::len_utf8));
    (start < head.len()).then(|| (start, &head[start..]))
}

fn word_at(text: &str, off: usize) -> &str {
    let rest = &text[off..];
    &rest[..rest.find(|c: char| !is_word(c)).unwrap_or(rest.len())]
}

fn insert(text: &mut String, inserts: &mut Vec<Insert>, off: usize, what: &str) {
    let line = text[..off].matches('\n').count() + 1;
    let col = text[..off].rsplit('\n').next().map_or(0, |l| l.chars().count());
    let len = what.chars().count();
    for i in inserts.iter_mut().filter(|i| i.line == line && i.col >= col) {
        i.col += len;
    }
    inserts.push(Insert { line, col, len });
    text.insert_str(off, what);
}

fn fix_at(text: &mut String, inserts: &mut Vec<Insert>, off: usize, msg: &str) -> bool {
    // A reserved word used as a name: same length, so no positions move.
    let here = word_at(text, off).to_owned();
    if (msg.contains("keyword") || msg.contains("identifier")) && RESERVED.contains(&here.as_str()) {
        let n = here.len();
        text.replace_range(off + n - 1..off + n, &here[n - 1..].to_uppercase());
        return true;
    }
    // `crate struct S;`: the old visibility. `pub` + two spaces keeps the length.
    let before = word_before(text, off).map(|(i, w)| (i, w.to_owned()));
    for (start, w) in [Some((off, here)), before.clone()].into_iter().flatten() {
        if w == "crate" && text[start + 5..].trim_start().starts_with(|c: char| c.is_alphabetic()) {
            text.replace_range(start..start + 5, "pub  ");
            return true;
        }
    }
    // `Box<Fn(i32)>`: the parser stops at the `(` after a bare `Fn`.
    if let Some((start, w)) = before
        && FN_TRAITS.contains(&w.as_str())
        && text[off..].starts_with('(')
        && !text[..start].trim_end().ends_with("dyn")
    {
        insert(text, inserts, start, "dyn ");
        return true;
    }
    // `fn f(&self, &T)`: the parser reads `&T` as a pattern and wants `:` after it. That can
    // be at the `,`/`)` that ends the parameter, or at a `<` inside it (`&Vec<T>`).
    if msg.contains("expected `:`")
        && !text[off..].starts_with(':')
        && let Some(start) = param_start(text, off)
    {
        insert(text, inserts, start, "_: ");
        return true;
    }
    // `[f32, ..3]`: the old array type. `,` + `..` become `;` + spaces.
    if msg.contains("expected `]`") || msg.contains("expected `;`") || msg.contains("expected `,`") {
        let comma = if text[off..].starts_with(',') { Some(off) } else { text[..off].rfind(',') };
        if let Some(c) = comma {
            let after = text[c + 1..].trim_start();
            if after.starts_with("..") && after[2..].starts_with(|ch: char| ch.is_alphanumeric() || ch == '_') {
                let end = text.len() - after.len() + 2;
                text.replace_range(c..end, &format!(";{}", " ".repeat(end - c - 1)));
                return true;
            }
        }
    }
    // `macro_rules! m( … )` / `thread_local!( … )` as an item: today it needs a `;`.
    if msg.contains("expected `;`") {
        // At the end of the file the parser has no position to give (it reports 1:1).
        let head = if off == 0 { text.trim_end() } else { text[..off].trim_end() };
        if head.ends_with(')') || head.ends_with(']') {
            let at = head.len();
            insert(text, inserts, at, ";");
            return true;
        }
    }
    // `v[mut]`: the old mutable full-range slice. `mut` and `.. ` are the same length.
    if text[off..].starts_with("mut]") && text[..off].ends_with('[') {
        text.replace_range(off..off + 3, ".. ");
        return true;
    }
    // `v[mut a..b]`: the old mutable range slice.
    if text[off..].starts_with("mut ") && text[..off].ends_with('[') {
        text.replace_range(off..off + 3, "   ");
        return true;
    }
    // `v[]`: the old full-range slice, now `v[..]`. The parser stops at the `]`.
    if text[off..].starts_with(']') && text[..off].ends_with('[') {
        insert(text, inserts, off, "..");
        return true;
    }
    // `x as u32 << 8`: `<<` (or `<`) reads as generic arguments of `u32`, and the parser
    // fails just after it. Parenthesize the type.
    let line_start = text[..off].rfind('\n').map_or(0, |n| n + 1);
    let line = &text[line_start..off];
    for op in ["<<", "<"] {
        if let Some(at) = line.rfind(op) {
            let head = &text[..line_start + at];
            if let Some((start, ty)) = word_before(head, head.len()).map(|(i, w)| (i, w.to_owned()))
                && head[..start].trim_end().ends_with(" as")
                && text[line_start + at + op.len()..off].trim_start_matches(|c: char| c.is_whitespace() || c == '(').is_empty()
            {
                insert(text, inserts, start + ty.len(), ")");
                insert(text, inserts, start, "(");
                return true;
            }
        }
    }
    // `f: |uint| -> T` / `f: ||`: the old closure type. Becomes a `fn` pointer type.
    if text[off..].starts_with('|') {
        return closure_type(text, inserts, off);
    }
    false
}

/// `|a, b| -> R` → `fn(a, b) -> R` and `||` → `fn()`.
fn closure_type(text: &mut String, inserts: &mut Vec<Insert>, off: usize) -> bool {
    if text[off..].starts_with("||") {
        text.replace_range(off..off + 2, "fn");
        insert(text, inserts, off + 2, "()");
        return true;
    }
    let Some(close) = text[off + 1..].find('|').map(|i| off + 1 + i) else {
        return false;
    };
    if text[off..close].contains('\n') {
        return false;
    }
    text.replace_range(close..close + 1, ")");
    text.replace_range(off..off + 1, "f");
    insert(text, inserts, off + 1, "n(");
    true
}

/// Replace `word` where it stands alone (not part of a longer name, path, or method call).
fn standalone(text: &str, word: &str) -> Vec<usize> {
    text.match_indices(word)
        .map(|(i, _)| i)
        .filter(|&i| {
            let before = text[..i].chars().next_back();
            let after = text[i + word.len()..].chars().next();
            !before.is_some_and(|c| is_word(c) || c == '.' || c == ':' || c == '#') && !after.is_some_and(is_word)
        })
        .collect()
}

/// Patches that are the same length as what they replace, and cannot be valid Rust 1.x
/// anyway, so they are safe to apply to any file that already failed to parse.
fn prepass(text: &mut String) {
    // `#[deriving(Clone)]` → `#[derive  (Clone)]`
    *text = text.replace("#[deriving(", "#[derive  (");
    // `box x` → `    x`: the old box expression.
    for i in standalone(text, "box").into_iter().rev() {
        if text[i + 3..].starts_with(char::is_whitespace) {
            text.replace_range(i..i + 3, "   ");
        }
    }
    // `proc(a, b) { … }` → `    |a, b| { … }`
    for i in standalone(text, "proc").into_iter().rev() {
        if !text[i + 4..].starts_with('(') {
            continue;
        }
        let Some(close) = matching_paren(text, i + 4) else { continue };
        if text[close + 1..].trim_start().starts_with('{') {
            text.replace_range(close..close + 1, "|");
            text.replace_range(i..i + 5, "    |");
        }
    }
    // `extern crate "a-b" as c;` → `extern crate  a_b  as c;`
    for i in standalone(text, "extern").into_iter().rev() {
        let rest = &text[i..];
        let Some(q) = rest.strip_prefix("extern").map(str::trim_start).and_then(|r| r.strip_prefix("crate")).map(str::trim_start) else {
            continue;
        };
        if let Some(body) = q.strip_prefix('"')
            && let Some(end) = body.find('"')
        {
            let start = text.len() - q.len() - (text.len() - i - rest.len());
            let name = body[..end].replace('-', "_");
            text.replace_range(start..start + end + 2, &format!(" {name} "));
        }
    }
    // `use a::{mod, B};` → `use a::{_mo, B};`
    for i in standalone(text, "mod").into_iter().rev() {
        let line_start = text[..i].rfind('\n').map_or(0, |n| n + 1);
        let line = &text[line_start..i];
        if line.trim_start().starts_with("use ") && line.contains('{') {
            text.replace_range(i..i + 3, "_mo");
        }
    }
    // `pub type Kind { A, B }` → `pub enum Kind { A, B }`
    for i in standalone(text, "type").into_iter().rev() {
        let rest = text[i + 4..].trim_start();
        let name_len = rest.find(|c: char| !is_word(c)).unwrap_or(0);
        if name_len > 0 && rest[name_len..].trim_start().starts_with('{') {
            text.replace_range(i..i + 4, "enum");
        }
    }
}

/// The `)` that closes the `(` at `open`, ignoring nothing fancy: strings and comments are rare in `proc(..)` heads.
fn matching_paren(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in text[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Where the parameter that ends at `off` begins: just after the previous `,` or `(` at the same depth.
fn param_start(text: &str, off: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let (mut depth, mut i) = (0i32, off);
    while i > 0 {
        i -= 1;
        match bytes[i] {
            b')' | b']' | b'}' => depth += 1,
            b'>' if i > 0 && bytes[i - 1] != b'-' => depth += 1,
            b'<' => depth -= 1,
            b'(' | b'[' | b'{' if depth > 0 => depth -= 1,
            b'(' | b',' if depth == 0 => {
                let rest = &text[i + 1..off];
                return Some(i + 1 + (rest.len() - rest.trim_start().len()));
            }
            b';' | b'{' if depth <= 0 => return None,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> Repaired {
        parse(src).unwrap_or_else(|| panic!("did not repair: {src}")).1
    }

    #[test]
    fn bare_fn_trait_objects_get_dyn() {
        let r = ok("type A = Box<Fn(i32) -> i32>;\nfn f(x: &mut FnMut(u8), y: Option<Rc<Fn()>>) {}\n");
        assert!(r.text.contains("Box<dyn Fn(i32)") && r.text.contains("&mut dyn FnMut(u8)") && r.text.contains("Rc<dyn Fn()>"), "{}", r.text);
    }

    #[test]
    fn anonymous_parameters_get_a_name() {
        let r = ok("trait T {\n    fn apply(&self, &Vec<Rc<M>>) {}\n    fn is_set(&self, Arg) -> bool;\n    fn exec<H: Host>(self, &H, &[&str]) -> R;\n}\n");
        assert!(r.text.contains("_: &Vec<Rc<M>>") && r.text.contains("(&self, _: Arg)") && r.text.contains("_: &H, _: &[&str]"), "{}", r.text);
    }

    #[test]
    fn reserved_names_and_crate_visibility() {
        let r = ok("mod await;\ncrate struct S;\npub fn await(&self) {}\n");
        assert!(r.text.contains("mod awaiT;") && r.text.contains("pub   struct S;") && r.text.contains("fn awaiT("), "{}", r.text);
    }

    #[test]
    fn reserved_names_in_paths_and_calls() {
        for src in ["mod await;", "pub use self::await::*;", "fn f(core: C) { core.await(); }", "mod await;\npub use self::await::*;"] {
            let e = syn::parse_file(src).err().map(|e| (e.span().start().line, e.span().start().column, e.to_string()));
            let fixed = parse(src).is_some();
            assert!(fixed, "{src}: syn says {e:?}");
        }
    }

    #[test]
    fn positions_map_back_to_the_original() {
        // `dyn ` (4 chars) is added before `Fn`, so a column after it moves back by 4.
        let src = "type A = Box<Fn(i32)>; type B = u8;\n";
        let r = ok(src);
        let patched = r.text.find("type B").unwrap();
        let orig = src.find("type B").unwrap();
        assert_eq!(r.to_original(LineColumn { line: 1, column: patched }).column, orig);
        // A position inside the inserted text maps to where it was inserted.
        assert_eq!(r.to_original(LineColumn { line: 1, column: r.text.find("dyn").unwrap() + 2 }).column, src.find("Fn").unwrap());
    }

    #[test]
    fn pre_1_0_syntax_is_patched() {
        let src = "#[deriving(Clone)]\nstruct S { p: [f32, ..3], f: |uint| -> u8, g: || }\n\
                   pub type Kind { A, B }\n\
                   extern crate \"lib-sys\" as ffi;\nuse a::{mod, B};\n\
                   fn main() {\n    let x = box S;\n    spawn(proc(a: u8) { a });\n    let y = (self.a as i32 << 24) | 1;\n}\n";
        let r = match try_parse(src) {
            Ok((_, r)) => r,
            Err(f) => panic!("failed at {}:{}: {}\n{src}", f.line, f.column, f.message),
        };
        for want in ["#[derive  (Clone)]", "[f32;   3]", "fn(uint) -> u8", "g: fn()", "pub enum Kind", "extern crate  lib_sys  as ffi", "{_mo, B}", "let x =     S;", "    |a: u8| { a }", "as (i32) << 24"] {
            assert!(r.text.contains(want), "missing `{want}` in:\n{}", r.text);
        }
    }

    #[test]
    fn old_slices_item_macros_and_shifts() {
        let r = ok("macro_rules! m(\n    () => (1)\n)\n\n#[macro_export]\nmacro_rules! n { () => (2) }\nthread_local!(static X: u8 = 0)\nfn f(v: &[u8], i: u32) -> u32 { let _ = &v[]; let _ = &mut v[mut]; let _ = v[mut 1..2]; *x |= b[i] as u32 << (8 * i); 1 }\n");
        for want in ["m(\n    () => (1)\n);", "thread_local!(static X: u8 = 0);", "&v[..]", "as (u32) << (8 * i)", "[.. ]"] {
            assert!(r.text.contains(want), "missing `{want}` in:\n{}", r.text);
        }
    }

    #[test]
    fn same_length_patches_do_not_move_positions() {
        let src = "fn main() { spawn(proc() { work(); }); }\n";
        let r = ok(src);
        assert_eq!(r.text.len(), src.len());
        let at = r.text.find("work").unwrap();
        assert_eq!(r.to_original(LineColumn { line: 1, column: at }).column, at);
    }

    #[test]
    fn syntax_it_cannot_fix_is_left_alone() {
        // Unbalanced brackets are in the source itself.
        assert!(parse("#[cfg(any(target_os=\"a\", target_os=\"b\")]\nfn main() {}\n").is_none());
    }
}
