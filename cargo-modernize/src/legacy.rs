//! Detectors for idioms that predate Rust 1.50: the long tail of "write it the
//! way Rust has let you for years". Same contract as `rewrite.rs`: match syntax,
//! never types, and build rewrites from slices of the original source.
//!
//! These are all `dangerous` unless noted in `rules/walk.toml`: without types, a
//! method named `filter` or `unwrap_or` might not be the std one, so `--dangerous`
//! lets `cargo check` have the last word.

use crate::rewrite::is_diverging;
use crate::scan::{Edit, Scanner, is_int, is_tight};
use proc_macro2::LineColumn;
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{BinOp, Expr, Lit, Member, Pat, Stmt, Type};

const EMPTY: [&str; 7] = [
    "Vec", "String", "HashMap", "HashSet", "BTreeMap", "BTreeSet", "VecDeque",
];

fn tokens(e: &impl quote::ToTokens) -> String {
    e.to_token_stream().to_string()
}

fn strip_parens(e: &Expr) -> &Expr {
    match e {
        Expr::Paren(p) => strip_parens(&p.expr),
        e => e,
    }
}

fn single_expr(b: &syn::Block) -> Option<&Expr> {
    match b.stmts.as_slice() {
        [Stmt::Expr(e, None)] => Some(e),
        _ => None,
    }
}

/// A closure body with one level of `{ … }` removed.
fn body_expr(e: &Expr) -> &Expr {
    let e = strip_parens(e);
    match e {
        Expr::Block(b) => single_expr(&b.block).unwrap_or(e),
        _ => e,
    }
}

fn path_idents(e: &Expr) -> Option<Vec<String>> {
    let Expr::Path(p) = e else { return None };
    Some(p.path.segments.iter().map(|s| s.ident.to_string()).collect())
}

/// Does `e` leave its enclosing function or loop (`?`, `try!`, `return`, `break`, `continue`)?
/// Moving such an expression into a closure changes what it returns from.
fn escapes(e: &Expr) -> bool {
    struct Find(bool);
    impl<'ast> Visit<'ast> for Find {
        fn visit_expr(&mut self, e: &'ast Expr) {
            match e {
                Expr::Try(_) | Expr::Return(_) | Expr::Break(_) | Expr::Continue(_) => self.0 = true,
                Expr::Macro(m) if m.mac.path.is_ident("try") => self.0 = true,
                // a nested closure or fn keeps its own control flow
                Expr::Closure(_) => return,
                _ => {}
            }
            syn::visit::visit_expr(self, e);
        }
    }
    let mut f = Find(false);
    f.visit_expr(e);
    f.0
}

fn is_ident(e: &Expr, name: &str) -> bool {
    matches!(e, Expr::Path(p) if p.path.is_ident(name))
}

/// `Vec::new()`, `String::new()`, `Default::default()`, `T::default()`.
fn is_default_call(e: &Expr) -> bool {
    let Expr::Call(c) = e else { return false };
    let Some(segs) = path_idents(&c.func) else {
        return false;
    };
    c.args.is_empty()
        && match segs.as_slice() {
            [.., last] if last == "default" => true,
            [.., ty, last] if last == "new" => EMPTY.contains(&ty.as_str()),
            _ => false,
        }
}

/// `Vec::new`, `String::new`, `Default::default` passed as a function.
fn is_default_fn(e: &Expr) -> bool {
    let Some(segs) = path_idents(e) else {
        return false;
    };
    match segs.as_slice() {
        [.., last] if last == "default" => segs.len() > 1,
        [.., ty, last] if last == "new" => EMPTY.contains(&ty.as_str()),
        _ => false,
    }
}

/// A function or closure that returns the default value: `Vec::new`, `|| Vec::new()`.
fn is_default_thunk(e: &Expr) -> bool {
    if is_default_fn(e) {
        return true;
    }
    matches!(e, Expr::Closure(c) if c.inputs.is_empty() && is_default_call(body_expr(&c.body)))
}

fn is_zero_lit(e: &Expr) -> bool {
    match e {
        Expr::Lit(l) => match &l.lit {
            Lit::Int(i) => i.base10_parse::<u64>().ok() == Some(0),
            Lit::Float(f) => f.base10_parse::<f64>().ok() == Some(0.0),
            Lit::Bool(b) => !b.value,
            _ => false,
        },
        _ => false,
    }
}

/// `Some(p)` / `Ok(p)`: a one-field tuple-struct pattern named `name`.
fn is_ctor(p: &Pat, name: &str) -> bool {
    matches!(p, Pat::TupleStruct(ts) if ts.path.is_ident(name) && ts.elems.len() == 1)
}

impl<'a> Scanner<'a> {
    fn paren_if_loose(&self, e: &Expr) -> String {
        if is_tight(e) {
            self.src_of(e).to_owned()
        } else {
            format!("({})", self.src_of(e))
        }
    }

    fn edit_offsets(&self, start: usize, end: usize, new: impl Into<String>) -> Edit {
        Edit {
            path: self.path.to_path_buf(),
            start,
            end,
            old: self.text[start..end].to_owned(),
            new: new.into(),
        }
    }

    // ------------------------------------------------------------ operators

    pub fn legacy_binary(&mut self, b: &syn::ExprBinary) {
        // `x.len() == 0` → `x.is_empty()`
        if let Expr::MethodCall(m) = &*b.left
            && m.method == "len"
            && m.args.is_empty()
        {
            let empty = match b.op {
                BinOp::Eq(_) | BinOp::Le(_) if is_int(&b.right, 0) => Some(true),
                BinOp::Lt(_) if is_int(&b.right, 1) => Some(true),
                BinOp::Ne(_) | BinOp::Gt(_) if is_int(&b.right, 0) => Some(false),
                BinOp::Ge(_) if is_int(&b.right, 1) => Some(false),
                _ => None,
            };
            if let Some(empty) = empty {
                let neg = if empty { "" } else { "!" };
                let new = format!("{neg}{}.is_empty()", self.paren_if_loose(&m.receiver));
                self.pattern("len-zero", m.method.span(), Some(vec![self.replace(b, new)]));
            }
        }
        // `x == None` → `x.is_none()`
        if matches!(b.op, BinOp::Eq(_) | BinOp::Ne(_)) {
            let other = match (is_ident(&b.right, "None"), is_ident(&b.left, "None")) {
                (true, false) => Some(&*b.left),
                (false, true) => Some(&*b.right),
                _ => None,
            };
            if let Some(other) = other {
                let method = if matches!(b.op, BinOp::Eq(_)) {
                    "is_none"
                } else {
                    "is_some"
                };
                let new = format!("{}.{method}()", self.paren_if_loose(other));
                self.pattern("none-compare", b.op.span(), Some(vec![self.replace(b, new)]));
            }
        }
        // `((x % n) + n) % n` → `x.rem_euclid(n)`
        if matches!(b.op, BinOp::Rem(_))
            && let Expr::Paren(outer) = &*b.left
            && let Expr::Binary(add) = &*outer.expr
            && matches!(add.op, BinOp::Add(_))
            && let Expr::Paren(inner) = &*add.left
            && let Expr::Binary(rem) = &*inner.expr
            && matches!(rem.op, BinOp::Rem(_))
            && tokens(&rem.right) == tokens(&add.right)
            && tokens(&add.right) == tokens(&b.right)
        {
            let new = format!(
                "{}.rem_euclid({})",
                self.paren_if_loose(&rem.left),
                self.src_of(&*b.right)
            );
            self.pattern("rem-euclid", b.op.span(), Some(vec![self.replace(b, new)]));
        }
    }

    // ------------------------------------------------------------ method calls

    pub fn legacy_method(&mut self, m: &syn::ExprMethodCall, name: &str) {
        let tail = |s: &Self, from: LineColumn, new: String| vec![s.edit(from, m.span().end(), new)];
        match (name, m.args.len()) {
            ("next", 0) => {
                if let Expr::MethodCall(f) = &*m.receiver
                    && f.method == "filter"
                    && f.args.len() == 1
                {
                    let new = format!("find({})", self.src_of(&f.args[0]));
                    let edits = tail(self, f.method.span().start(), new);
                    self.pattern("filter-next", f.method.span(), Some(edits));
                }
            }
            ("flatten", 0) => {
                if let Expr::MethodCall(f) = &*m.receiver
                    && f.method == "map"
                    && f.args.len() == 1
                {
                    let new = format!("flat_map({})", self.src_of(&f.args[0]));
                    let edits = tail(self, f.method.span().start(), new);
                    self.pattern("map-flatten", f.method.span(), Some(edits));
                }
            }
            ("filter_map", 1) => {
                if let Expr::Closure(c) = &m.args[0]
                    && let [Pat::Ident(p)] = c.inputs.iter().collect::<Vec<_>>().as_slice()
                    && is_ident(body_expr(&c.body), &p.ident.to_string())
                {
                    let edits = tail(self, m.method.span().start(), "flatten()".into());
                    self.pattern("filter-map-identity", m.method.span(), Some(edits));
                }
            }
            ("map", 1) => {
                if let Expr::Closure(c) = &m.args[0]
                    && c.inputs.len() == 1
                {
                    let body = body_expr(&c.body);
                    let (copied, cloned) = match (&c.inputs[0], body) {
                        (Pat::Ident(p), Expr::MethodCall(mc))
                            if mc.method == "clone"
                                && mc.args.is_empty()
                                && is_ident(&mc.receiver, &p.ident.to_string()) =>
                        {
                            (false, true)
                        }
                        (Pat::Ident(p), Expr::Unary(u))
                            if matches!(u.op, syn::UnOp::Deref(_))
                                && is_ident(&u.expr, &p.ident.to_string()) =>
                        {
                            (true, false)
                        }
                        (Pat::Reference(r), body) => match &*r.pat {
                            Pat::Ident(p) if is_ident(body, &p.ident.to_string()) => (true, false),
                            _ => (false, false),
                        },
                        _ => (false, false),
                    };
                    if cloned {
                        let edits = tail(self, m.method.span().start(), "cloned()".into());
                        self.pattern("cloned", m.method.span(), Some(edits));
                    } else if copied {
                        let edits = tail(self, m.method.span().start(), "copied()".into());
                        self.pattern("copied", m.method.span(), Some(edits));
                    }
                }
            }
            ("unwrap_or", 1) if is_default_call(&m.args[0]) => {
                let edits = tail(self, m.method.span().start(), "unwrap_or_default()".into());
                self.pattern("unwrap-or-default", m.method.span(), Some(edits));
            }
            ("unwrap_or_else", 1) if is_default_thunk(&m.args[0]) => {
                let edits = tail(self, m.method.span().start(), "unwrap_or_default()".into());
                self.pattern("unwrap-or-default", m.method.span(), Some(edits));
            }
            ("or_insert_with", 1) | ("or_insert", 1) => {
                let is_entry = matches!(&*m.receiver, Expr::MethodCall(e) if e.method == "entry" && e.args.len() == 1);
                let arg = &m.args[0];
                let default = if name == "or_insert" {
                    is_default_call(arg) || is_zero_lit(arg)
                } else {
                    is_default_thunk(arg)
                };
                if is_entry && default {
                    let edits = tail(self, m.method.span().start(), "or_default()".into());
                    self.pattern("or-default", m.method.span(), Some(edits));
                }
            }
            ("and_then", 1) => {
                if let Expr::Closure(c) = &m.args[0]
                    && let Expr::Call(call) = body_expr(&c.body)
                    && call.args.len() == 1
                    && (is_ident(&call.func, "Some") || is_ident(&call.func, "Ok"))
                    && !escapes(&call.args[0])
                {
                    let edits = vec![
                        self.replace(&m.method, "map"),
                        self.replace(body_expr(&c.body), self.src_of(&call.args[0])),
                    ];
                    self.pattern("and-then-some", m.method.span(), Some(edits));
                }
            }
            ("max" | "min", 1) => {
                if let Expr::MethodCall(inner) = &*m.receiver
                    && inner.args.len() == 1
                    && ((name == "min" && inner.method == "max")
                        || (name == "max" && inner.method == "min"))
                {
                    let (lo, hi) = if name == "min" {
                        (&inner.args[0], &m.args[0])
                    } else {
                        (&m.args[0], &inner.args[0])
                    };
                    let new = format!(
                        "{}.clamp({}, {})",
                        self.paren_if_loose(&inner.receiver),
                        self.src_of(lo),
                        self.src_of(hi)
                    );
                    self.pattern("clamp", inner.method.span(), Some(vec![self.replace(m, new)]));
                }
            }
            ("take", 1) => {
                if let Expr::Call(c) = &*m.receiver
                    && c.args.len() == 1
                    && self
                        .resolve_expr_path(&c.func)
                        .is_some_and(|p| matches!(p.as_slice(), [a, b, c] if matches!(a.as_str(), "std" | "core") && b == "iter" && c == "repeat"))
                {
                    let func = self.src_of(&c.func);
                    let name = match func.strip_suffix("repeat") {
                        Some(p) if !p.is_empty() => format!("{p}repeat_n"),
                        _ => "std::iter::repeat_n".to_owned(),
                    };
                    let new = format!(
                        "{name}({}, {})",
                        self.src_of(&c.args[0]),
                        self.src_of(&m.args[0])
                    );
                    self.pattern("repeat-n", c.func.span(), Some(vec![self.replace(m, new)]));
                }
            }
            ("sort_by" | "sort_unstable_by" | "max_by" | "min_by", 1) => {
                if let Expr::Closure(c) = &m.args[0]
                    && c.inputs.len() == 2
                    && let Expr::MethodCall(un) = body_expr(&c.body)
                    && un.method == "unwrap"
                    && matches!(&*un.receiver, Expr::MethodCall(pc) if pc.method == "partial_cmp" && pc.args.len() == 1)
                {
                    self.pattern("total-cmp", m.method.span(), None);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ calls

    pub fn legacy_call(&mut self, c: &syn::ExprCall) {
        let Some(f) = self.resolve_expr_path(&c.func) else {
            return;
        };
        let f: Vec<&str> = f.iter().map(String::as_str).collect();
        // `Duration::from_secs(0)` / `Duration::new(0, 0)` → `Duration::ZERO`
        if let ["std" | "core", "time", "Duration", ctor] = f.as_slice() {
            let zero = match (*ctor, c.args.len()) {
                ("from_secs" | "from_millis" | "from_micros" | "from_nanos", 1) => {
                    is_int(&c.args[0], 0)
                }
                ("new", 2) => is_int(&c.args[0], 0) && is_int(&c.args[1], 0),
                _ => false,
            };
            if zero {
                let func = self.src_of(&c.func);
                let edits = func
                    .rsplit_once("::")
                    .map(|(ty, _)| vec![self.replace(c, format!("{ty}::ZERO"))]);
                self.pattern("duration-zero", c.func.span(), edits);
            }
        }
    }

    // ------------------------------------------------------------ if / match / loop

    pub fn legacy_if(&mut self, i: &syn::ExprIf) {
        // An `else if` can't become `else c.then(…)`: remember it and skip its `bool-then`.
        if let Some((_, els)) = &i.else_branch
            && let Expr::If(inner) = &**els
        {
            let s = inner.span().start();
            self.handled.insert((s.line, s.column));
        }
        let s = i.span().start();
        let is_else_if = self.handled.contains(&(s.line, s.column));
        match (&*i.cond, &i.else_branch) {
            // `if let Some(_) = x` → `x.is_some()`
            (Expr::Let(l), _) => {
                let method = match &*l.pat {
                    Pat::TupleStruct(ts)
                        if ts.elems.len() == 1 && matches!(ts.elems[0], Pat::Wild(_)) =>
                    {
                        match ts.path.get_ident().map(|i| i.to_string()).as_deref() {
                            Some("Some") => Some("is_some"),
                            Some("Ok") => Some("is_ok"),
                            Some("Err") => Some("is_err"),
                            _ => None,
                        }
                    }
                    Pat::Path(p) if p.path.is_ident("None") => Some("is_none"),
                    Pat::Ident(p) if p.ident == "None" && p.subpat.is_none() => Some("is_none"),
                    _ => None,
                };
                if let Some(method) = method {
                    let new = format!("{}.{method}()", self.paren_if_loose(&l.expr));
                    self.pattern(
                        "redundant-pattern",
                        l.let_token.span,
                        Some(vec![self.replace(l, new)]),
                    );
                }
            }
            // `if c { Some(x) } else { None }` → `c.then(|| x)`
            (cond, Some((_, els))) if !is_else_if => {
                if let Some(Expr::Call(some)) = single_expr(&i.then_branch)
                    && is_ident(&some.func, "Some")
                    && some.args.len() == 1
                    && matches!(body_expr(els), Expr::Path(p) if p.path.is_ident("None"))
                    && !escapes(&some.args[0])
                {
                    let new = format!(
                        "{}.then(|| {})",
                        self.paren_if_loose(cond),
                        self.src_of(&some.args[0])
                    );
                    self.pattern("bool-then", i.if_token.span, Some(vec![self.replace(i, new)]));
                }
            }
            _ => {}
        }
    }

    pub fn legacy_match(&mut self, m: &syn::ExprMatch) {
        if m.arms.len() != 2 || m.arms.iter().any(|a| !a.attrs.is_empty()) {
            return;
        }
        if matches!(&*m.expr, Expr::Reference(_)) {
            return; // `match &opt` binds by reference; `.map` would not
        }
        let (some, none) = if is_ctor(&m.arms[0].pat, "Some") && is_none_pat(&m.arms[1].pat) {
            (&m.arms[0], &m.arms[1])
        } else if is_none_pat(&m.arms[0].pat) && is_ctor(&m.arms[1].pat, "Some") {
            (&m.arms[1], &m.arms[0])
        } else {
            return;
        };
        let Pat::TupleStruct(ts) = &some.pat else { return };
        let bound = &ts.elems[0];
        let scrut = self.paren_if_loose(&m.expr);
        let some_body = body_expr(&some.body);
        let none_body = body_expr(&none.body);
        // Some(x) => Some(f(x)), None => None
        if let Expr::Call(c) = some_body
            && is_ident(&c.func, "Some")
            && c.args.len() == 1
            && is_ident(none_body, "None")
            && !escapes(&c.args[0])
        {
            let new = format!("{scrut}.map(|{}| {})", self.src_of(bound), self.src_of(&c.args[0]));
            self.pattern("option-map-match", m.match_token.span, Some(vec![self.replace(m, new)]));
            return;
        }
        // Some(x) => x, None => <value>
        if let Pat::Ident(p) = bound
            && p.subpat.is_none()
            && p.by_ref.is_none()
            && is_ident(some_body, &p.ident.to_string())
        {
            let new = if is_default_call(none_body) {
                Some(format!("{scrut}.unwrap_or_default()"))
            } else if matches!(none_body, Expr::Lit(_) | Expr::Path(_)) {
                Some(format!("{scrut}.unwrap_or({})", self.src_of(none_body)))
            } else if !is_diverging(none_body) && !escapes(none_body) {
                Some(format!("{scrut}.unwrap_or_else(|| {})", self.src_of(none_body)))
            } else {
                None // `None => return …` is `let … else`, not `unwrap_or`
            };
            if let Some(new) = new {
                self.pattern(
                    "match-unwrap-or",
                    m.match_token.span,
                    Some(vec![self.replace(m, new)]),
                );
            }
        }
    }

    /// `loop { match it.next() { Some(x) => …, None => break } }` → `while let`.
    pub fn legacy_loop(&mut self, l: &syn::ExprLoop) {
        if l.label.is_some() {
            return;
        }
        let [Stmt::Expr(Expr::Match(m), _)] = l.body.stmts.as_slice() else {
            return;
        };
        if m.arms.len() != 2 {
            return;
        }
        let is_break = |e: &Expr| matches!(body_expr(e), Expr::Break(b) if b.label.is_none() && b.expr.is_none());
        let (arm, wrapper) = match (&m.arms[0], &m.arms[1]) {
            (a, b) if is_break(&b.body) && is_none_pat(&b.pat) => (a, "Some"),
            (a, b) if is_break(&b.body) && is_err_wild(&b.pat) => (a, "Ok"),
            _ => return,
        };
        if !is_ctor(&arm.pat, wrapper) {
            return;
        }
        let Pat::TupleStruct(ts) = &arm.pat else { return };
        let body = match &*arm.body {
            Expr::Block(b) if b.label.is_none() => self.src_of(b).to_owned(),
            other => format!("{{ {}; }}", self.src_of(other)),
        };
        let new = format!(
            "while let {wrapper}({}) = {} {body}",
            self.src_of(&ts.elems[0]),
            self.src_of(&*m.expr)
        );
        self.pattern("while-let-loop", l.loop_token.span, Some(vec![self.replace(l, new)]));
    }

    // ------------------------------------------------------------ items

    pub fn legacy_fn(&mut self, block: &syn::Block) {
        let Some(Stmt::Expr(Expr::Return(r), semi)) = block.stmts.last() else {
            return;
        };
        let Some(value) = &r.expr else { return };
        let stmt_end = match semi {
            Some(s) => s.span().end(),
            None => r.span().end(),
        };
        let edits = vec![self.edit(r.span().start(), stmt_end, self.src_of(&**value))];
        self.pattern("needless-return", r.return_token.span, Some(edits));
    }

    pub fn legacy_struct_expr(&mut self, s: &syn::ExprStruct) {
        for f in &s.fields {
            if let (Some(_), Member::Named(id), Expr::Path(p)) = (&f.colon_token, &f.member, &f.expr)
                && p.path.is_ident(id)
                && f.attrs.is_empty()
            {
                let edits = vec![self.edit(id.span().end(), f.expr.span().end(), "")];
                self.pattern("field-shorthand", f.span(), Some(edits));
            }
        }
    }

    pub fn legacy_struct_pat(&mut self, s: &syn::PatStruct) {
        for f in &s.fields {
            if let (Some(_), Member::Named(id), Pat::Ident(p)) = (&f.colon_token, &f.member, &*f.pat)
                && p.ident == *id
                && p.by_ref.is_none()
                && p.mutability.is_none()
                && p.subpat.is_none()
                && f.attrs.is_empty()
            {
                let edits = vec![self.edit(id.span().end(), f.pat.span().end(), "")];
                self.pattern("field-shorthand", f.span(), Some(edits));
            }
        }
    }

    /// `const X: &'static str` → `&str` (1.17). Only plain consts and statics:
    /// associated consts keep their lifetimes.
    pub fn legacy_static_ty(&mut self, ty: &Type) {
        let mut spans = Vec::new();
        collect_static_refs(ty, &mut spans);
        for (lt_start, lt_end) in spans {
            let (a, mut b) = (self.off(lt_start), self.off(lt_end));
            b += self.text[b..].len() - self.text[b..].trim_start().len();
            let edit = self.edit_offsets(a, b, "");
            self.pattern_at("static-lifetime", lt_start, lt_end, Some(vec![edit]));
        }
    }

    /// `Box<Trait + Send>` → `Box<dyn Trait + Send>`. A lone `Box<Trait>` is
    /// indistinguishable from a type without types, and syn 3 rejects bare `Fn(…)`
    /// types outright, so only the `+` form is seen.
    pub fn legacy_type(&mut self, ty: &Type) {
        if let Type::TraitObject(t) = ty
            && t.dyn_token.is_none()
        {
            let at = ty.span().start();
            let edits = vec![self.edit(at, at, "dyn ")];
            self.pattern("bare-trait-object", ty.span(), Some(edits));
        }
    }

    /// `impl<'a> Foo<'a>` → `impl Foo<'_>` when `'a` is named nowhere else (1.31).
    pub fn legacy_impl(&mut self, i: &syn::ItemImpl) {
        use syn::GenericParam;
        if i.generics.params.is_empty() || i.generics.where_clause.is_some() {
            return;
        }
        let mut names = Vec::new();
        for p in &i.generics.params {
            match p {
                GenericParam::Lifetime(l) if l.bounds.is_empty() => {
                    names.push(l.lifetime.ident.to_string())
                }
                _ => return,
            }
        }
        struct Lts(Vec<(String, LineColumn, LineColumn)>);
        impl<'ast> Visit<'ast> for Lts {
            fn visit_lifetime(&mut self, l: &'ast syn::Lifetime) {
                self.0.push((l.ident.to_string(), l.span().start(), l.span().end()));
            }
        }
        let mut all = Lts(Vec::new());
        all.visit_item_impl(i);
        let mut edits = Vec::new();
        let (self_start, self_end) = (i.self_ty.span().start(), i.self_ty.span().end());
        let within = |p: LineColumn| (p.line, p.column) >= (self_start.line, self_start.column)
            && (p.line, p.column) < (self_end.line, self_end.column);
        for name in &names {
            let uses: Vec<_> = all.0.iter().filter(|(n, _, _)| n == name).collect();
            // the declaration plus exactly one use, inside the self type
            let [_, (_, a, b)] = uses.as_slice() else { return };
            if !within(*a) {
                return;
            }
            edits.push(self.edit(*a, *b, "'_"));
        }
        edits.push(self.edit(
            i.generics.lt_token.map_or(i.impl_token.span.end(), |t| t.span().start()),
            i.generics.gt_token.map_or(i.impl_token.span.end(), |t| t.span().end()),
            "",
        ));
        self.pattern("impl-lifetime-elision", i.impl_token.span, Some(edits));
    }
}

fn is_none_pat(p: &Pat) -> bool {
    match p {
        Pat::Path(p) => p.path.is_ident("None"),
        Pat::Ident(p) => p.ident == "None" && p.subpat.is_none(),
        _ => false,
    }
}

fn is_err_wild(p: &Pat) -> bool {
    matches!(p, Pat::TupleStruct(ts) if ts.path.is_ident("Err") && ts.elems.len() == 1 && matches!(ts.elems[0], Pat::Wild(_)))
}

/// Every `&'static` in `ty` that elision would infer on its own: references nested in
/// references, slices, arrays, tuples, and generic arguments. Function pointer and
/// trait-object types are skipped, where elision means something different.
fn collect_static_refs(ty: &Type, out: &mut Vec<(LineColumn, LineColumn)>) {
    match ty {
        Type::Reference(r) => {
            if let Some(lt) = &r.lifetime
                && lt.ident == "static"
            {
                out.push((lt.span().start(), lt.span().end()));
            }
            collect_static_refs(&r.elem, out);
        }
        Type::Slice(s) => collect_static_refs(&s.elem, out),
        Type::Array(a) => collect_static_refs(&a.elem, out),
        Type::Paren(p) => collect_static_refs(&p.elem, out),
        Type::Tuple(t) => t.elems.iter().for_each(|e| collect_static_refs(e, out)),
        Type::Path(p) => {
            for seg in &p.path.segments {
                if let syn::PathArguments::AngleBracketed(a) = &seg.arguments {
                    for arg in &a.args {
                        if let syn::GenericArgument::Type(t) = arg {
                            collect_static_refs(t, out);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}
