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
//! Only the `dyn ` and `_: ` patches change the length of a line. They are remembered so
//! a position in the patched text can be mapped back to the original. Findings from a
//! repaired file carry no autofix: the rewrite would be built from patched text.
//! Syntax from before Rust 1.0 (`proc()`, `box`, `[T, ..N]`) is not repaired.

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

/// Parse `original`, patching old syntax as needed. `None` if some error is not one we know how to fix.
pub fn parse(original: &str) -> Option<(syn::File, Repaired)> {
    let mut text = original.to_owned();
    let mut inserts = Vec::new();
    for _ in 0..400 {
        match syn::parse_file(&text) {
            Ok(ast) => return Some((ast, Repaired { text, inserts })),
            Err(e) => {
                let off = byte_offset(&text, &line_starts(&text), e.span().start());
                if !fix_at(&mut text, &mut inserts, off, &e.to_string()) {
                    return None;
                }
            }
        }
    }
    None
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
    false
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
    fn syntax_it_cannot_fix_is_left_alone() {
        assert!(parse("fn main() { spawn(proc() { 1 }); }").is_none());
    }
}
