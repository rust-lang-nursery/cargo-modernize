//! Structural detectors ("new ways to be Rusty", std replacements) and the rewrites
//! that modernize them. Each rewrite is built from slices of the original source, so
//! comments and formatting inside the rewritten expression survive.
//!
//! Whether a rewrite is applied by `--fix` or only by `--dangerous` is decided by
//! the rule's `fix` field, not here.

use crate::scan::{Edit, EnumInfo, Scanner, is_int, is_tight, path_segs};
use proc_macro2::LineColumn;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Expr, Lit, Pat, Stmt};

const INTS: [&str; 12] = ["i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize"];

impl<'a> Scanner<'a> {
    pub fn collect_enums(&mut self, file: &syn::File) {
        let last_use = file.items.iter().rfind(|i| matches!(i, syn::Item::Use(_)));
        self.import_at = match (last_use, file.items.first()) {
            (Some(u), _) => Some((u.span().end(), true)),
            (None, Some(first)) => Some((first.span().start(), false)),
            _ => None,
        };
        struct Collect<'s, 'a>(&'s mut Scanner<'a>);
        impl<'ast> Visit<'ast> for Collect<'_, '_> {
            fn visit_item_enum(&mut self, e: &'ast syn::ItemEnum) {
                let derive_close = e.attrs.iter().find(|a| a.path().is_ident("derive")).and_then(|a| match &a.meta {
                    syn::Meta::List(l) => Some(l.delimiter.span().close().start()),
                    _ => None,
                });
                let variants = e.variants.iter().map(|v| (v.ident.to_string(), v.span().start())).collect();
                self.0.enums.insert(e.ident.to_string(), EnumInfo { derive_close, start: e.span().start(), variants });
                visit::visit_item_enum(self, e);
            }
        }
        Collect(self).visit_file(file);
    }

    // ------------------------------------------------------------ method calls

    pub fn method_patterns(&mut self, m: &syn::ExprMethodCall, name: &str) {
        let from_method = |s: &Self, new: String| vec![s.edit(m.method.span().start(), m.span().end(), new)];
        match (name, m.args.len()) {
            ("map_or", 2) => {
                let which = match &m.args[0] {
                    Expr::Lit(l) => match &l.lit {
                        Lit::Bool(b) if !b.value => Some(("is-some-and", "is_some_and")),
                        Lit::Bool(_) => Some(("is-none-or", "is_none_or")),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some((pat, method)) = which {
                    let edits = from_method(self, format!("{method}({})", self.src_of(&m.args[1])));
                    self.pattern(pat, m.method.span(), Some(edits));
                }
            }
            ("splitn", 2) if is_int(&m.args[0], 2) => self.pattern("split-once", m.method.span(), None),
            ("map", 1) if is_as_ref_map(m) => {
                let Expr::MethodCall(inner) = &*m.receiver else { return };
                let edits = vec![self.edit(inner.method.span().start(), m.span().end(), "as_deref()")];
                self.pattern("as-deref", inner.method.span(), Some(edits));
            }
            ("unwrap_or", 1) => {
                let Expr::MethodCall(inner) = &*m.receiver else { return };
                let strip = inner.method.to_string();
                if matches!(strip.as_str(), "strip_prefix" | "strip_suffix") && inner.args.len() == 1 && tokens(&m.args[0]) == tokens(&inner.receiver) {
                    let new = format!("{}({})", strip.replace("strip_", "trim_"), self.src_of(&inner.args[0]));
                    let edits = vec![self.edit(inner.method.span().start(), m.span().end(), new)];
                    self.pattern("trim-prefix", inner.method.span(), Some(edits));
                }
            }
            ("into_owned" | "to_string", 0) => {
                let Expr::Call(c) = &*m.receiver else { return };
                if path_ends_with(&c.func, &["String", "from_utf8_lossy"]) {
                    // Only when we hand over an owned Vec: `&v` → `v` (which moves `v`).
                    let edits = match c.args.first() {
                        Some(Expr::Reference(r)) if r.mutability.is_none() => {
                            Some(vec![self.replace(m, format!("{}_owned({})", self.src_of(&c.func), self.src_of(&r.expr)))])
                        }
                        _ => None,
                    };
                    self.pattern("lossy-owned", m.method.span(), edits);
                }
            }
            ("collect", 0) if is_zero_range_map(&m.receiver) && collects_vec_or_unknown(m) => {
                let Expr::MethodCall(map) = &*m.receiver else { return };
                let Expr::Range(r) = strip_parens(&map.receiver) else { return };
                let edits = r.end.as_ref().map(|end| vec![self.replace(m, format!("Vec::from_fn({}, {})", self.src_of(&**end), self.src_of(&map.args[0])))]);
                self.pattern("vec-from-fn", m.method.span(), edits);
            }
            ("with", 1) if is_cell_set_get(&m.args[0]) => {
                let edits = self.local_key_update(m);
                self.pattern("local-key-update", m.method.span(), edits);
            }
            ("unwrap" | "expect", _) => {
                if let Expr::Call(c) = &*m.receiver {
                    if self.is_crossbeam_scope(c) {
                        // std's scope returns the closure's value directly: drop the unwrap.
                        let edits = self.crossbeam_scope_text(c).map(|t| vec![self.replace(m, t)]);
                        let s = c.span().start();
                        self.handled.insert((s.line, s.column));
                        self.pattern("crossbeam-scope", c.func.span(), edits);
                    }
                }
            }
            _ => {}
        }
    }

    fn local_key_update(&self, m: &syn::ExprMethodCall) -> Option<Vec<Edit>> {
        let Expr::Closure(c) = &m.args[0] else { return None };
        let Some(Pat::Ident(p)) = c.inputs.first() else { return None };
        let body = strip_parens(&c.body);
        let body = block_expr(body).unwrap_or(body);
        let Expr::MethodCall(set) = body else { return None };
        let arg = self.src_of(set.args.first()?);
        let getter = format!("{}.get()", p.ident);
        arg.contains(&getter).then(|| {
            let new = format!("{}.update(|{p}| {})", self.src_of(&m.receiver), arg.replace(&getter, &p.ident.to_string()), p = p.ident);
            vec![self.replace(m, new)]
        })
    }

    // ------------------------------------------------------------ calls

    pub fn call_patterns(&mut self, c: &syn::ExprCall) {
        if let Some(f) = self.resolve_expr_path(&c.func) {
            let f: Vec<&str> = f.iter().map(String::as_str).collect();
            let func = self.src_of(&c.func);
            if matches!(f.as_slice(), ["std", "io", "Error", "new"]) && c.args.len() == 2 && path_ends_with(&c.args[0], &["Other"]) {
                let edits = func.strip_suffix("new").map(|p| vec![self.replace(c, format!("{p}other({})", self.src_of(&c.args[1])))]);
                self.pattern("io-error-other", c.func.span(), edits);
            }
            if matches!(f.as_slice(), ["std" | "core", "mem", "replace"]) && c.args.len() == 2 && is_default_value(&c.args[1]) {
                let edits = func.strip_suffix("replace").map(|p| vec![self.replace(c, format!("{p}take({})", self.src_of(&c.args[0])))]);
                self.pattern("mem-take", c.func.span(), edits);
            }
        }
        // `u32::max_value()` → `u32::MAX`
        if let Expr::Path(p) = &*c.func {
            let segs = path_segs(&p.path);
            if let [(ty, _), (f, _)] = segs.as_slice() {
                if c.args.is_empty() && INTS.contains(&ty.as_str()) && matches!(f.as_str(), "max_value" | "min_value") {
                    let k = if f == "max_value" { "MAX" } else { "MIN" };
                    self.pattern("max-value-fn", c.func.span(), Some(vec![self.replace(c, format!("{ty}::{k}"))]));
                }
            }
        }
        let s = c.span().start();
        if self.is_crossbeam_scope(c) && !self.handled.contains(&(s.line, s.column)) {
            let edits = self.crossbeam_scope_text(c).map(|t| vec![self.replace(c, t)]);
            self.pattern("crossbeam-scope", c.func.span(), edits);
        }
    }

    fn is_crossbeam_scope(&self, c: &syn::ExprCall) -> bool {
        self.wants("crossbeam-scope")
            && self.resolve_expr_path(&c.func).is_some_and(|f| {
                let f: Vec<&str> = f.iter().map(String::as_str).collect();
                matches!(f.as_slice(), ["crossbeam", "scope"] | ["crossbeam", "thread", "scope"] | ["crossbeam_utils", "thread", "scope"])
            })
    }

    /// `crossbeam::scope(|s| { s.spawn(|_| …); })` → `std::thread::scope(|s| { s.spawn(|| …); })`.
    fn crossbeam_scope_text(&self, c: &syn::ExprCall) -> Option<String> {
        let Some(Expr::Closure(cl)) = c.args.first() else { return None };
        let Some(Pat::Ident(scope)) = cl.inputs.first() else { return None };
        struct Spawns {
            scope: String,
            spans: Vec<(LineColumn, LineColumn)>,
            unsupported: bool,
        }
        impl<'ast> Visit<'ast> for Spawns {
            fn visit_expr_method_call(&mut self, m: &'ast syn::ExprMethodCall) {
                let on_scope = matches!(&*m.receiver, Expr::Path(p) if p.path.is_ident(&self.scope));
                if m.method == "spawn" && on_scope {
                    match m.args.first() {
                        Some(Expr::Closure(c)) if c.inputs.len() == 1 && matches!(c.inputs[0], Pat::Wild(_)) => {
                            self.spans.push((c.or1_token.span.start(), c.or2_token.span.end()));
                        }
                        _ => self.unsupported = true, // the closure uses its scope argument
                    }
                }
                visit::visit_expr_method_call(self, m);
            }
        }
        let mut finder = Spawns { scope: scope.ident.to_string(), spans: Vec::new(), unsupported: false };
        finder.visit_expr(&cl.body);
        if finder.unsupported {
            return None;
        }
        let base = self.off(cl.span().start());
        let mut text = self.src_of(cl).to_owned();
        finder.spans.sort_by_key(|(a, _)| std::cmp::Reverse((a.line, a.column)));
        for (a, b) in finder.spans {
            text.replace_range(self.off(a) - base..self.off(b) - base, "||");
        }
        Some(format!("std::thread::scope({text})"))
    }

    // ------------------------------------------------------------ statements and expressions

    pub fn local_patterns(&mut self, l: &syn::Local) {
        let Some(init) = l.init.as_ref().filter(|i| i.diverge.is_none()) else { return };
        let parts = match &*init.expr {
            Expr::Match(m) => unwrap_or_diverge_match(m),
            Expr::If(i) => unwrap_or_diverge_if(i),
            _ => None,
        };
        let Some((keep, scrutinee, bail)) = parts else { return };
        let edits = self.let_else_text(l, keep, scrutinee, bail).map(|t| vec![self.replace(l, t)]);
        self.pattern("let-else", l.let_token.span, edits);
    }

    fn let_else_text(&self, l: &syn::Local, keep: &Pat, scrutinee: &Expr, bail: &Expr) -> Option<String> {
        let Pat::Ident(local) = &l.pat else { return None }; // `let x: T = …` keeps its type: skip
        if !l.attrs.is_empty() || local.by_ref.is_some() || local.subpat.is_some() {
            return None;
        }
        let mut names = Vec::new();
        bindings(keep, &mut names);
        if names != [local.ident.to_string()] {
            return None; // `let y = match e { Some(x) => x, … }` would rename the binding
        }
        let mut pat = self.src_of(keep).to_owned();
        if local.mutability.is_some() {
            let at = find_binding(keep, &local.ident.to_string())?;
            pat.insert_str(self.off(at) - self.off(keep.span().start()), "mut ");
        }
        let mut scrut = self.src_of(scrutinee).to_owned();
        // let-else forbids a scrutinee ending in `}` or a top-level `&&`/`||`.
        if scrut.trim_end().ends_with('}') || matches!(scrutinee, Expr::Binary(b) if matches!(b.op, syn::BinOp::And(_) | syn::BinOp::Or(_))) {
            scrut = format!("({scrut})");
        }
        let els = match bail {
            Expr::Block(b) => self.src_of(b).to_owned(),
            other => format!("{{ {} }}", self.src_of(other)),
        };
        Some(format!("let {pat} = {scrut} else {els};"))
    }

    pub fn match_patterns(&mut self, m: &syn::ExprMatch) {
        let bools: Vec<bool> = m.arms.iter().filter_map(|a| bool_lit(&a.body)).collect();
        let n = m.arms.len();
        if n < 2 || bools.len() != n || !matches!(m.arms[n - 1].pat, Pat::Wild(_)) || bools.iter().all(|b| *b == bools[0]) {
            return;
        }
        let wild = bools[n - 1];
        let edits = (bools[..n - 1].iter().all(|b| *b != wild)).then(|| {
            let arms = &m.arms[..n - 1];
            let guards: Vec<_> = arms.iter().filter_map(|a| a.guard.as_ref()).collect();
            if !guards.is_empty() && arms.len() > 1 {
                return None; // `matches!` takes one guard
            }
            let pats: Vec<&str> = arms.iter().map(|a| self.src_of(&a.pat)).collect();
            let guard = guards.first().map_or(String::new(), |(_, g)| format!(" if {}", self.src_of(&**g)));
            let neg = if wild { "!" } else { "" };
            Some(vec![self.replace(m, format!("{neg}matches!({}, {}{guard})", self.src_of(&m.expr), pats.join(" | ")))])
        });
        self.pattern("matches-macro", m.match_token.span, edits.flatten());
    }

    pub fn if_patterns(&mut self, i: &syn::ExprIf) {
        if let Some((_, els)) = &i.else_branch {
            if let (Some(ok), Some(err)) = (single_expr(&i.then_branch), block_expr(els)) {
                if is_ok_unit(ok) && call_named(err, "Err") {
                    let Expr::Call(err_call) = err else { return };
                    let e = &err_call.args[0];
                    let cond = if is_tight(&i.cond) { self.src_of(&*i.cond).to_owned() } else { format!("({})", self.src_of(&*i.cond)) };
                    // Keep the error lazy unless it's trivially cheap.
                    let new = if matches!(e, Expr::Path(_) | Expr::Lit(_)) {
                        format!("{cond}.ok_or({})", self.src_of(e))
                    } else {
                        format!("{cond}.ok_or_else(|| {})", self.src_of(e))
                    };
                    self.pattern("bool-ok-or", i.if_token.span, Some(vec![self.replace(i, new)]));
                }
            }
        } else if matches!(*i.cond, Expr::Let(_)) {
            if let [Stmt::Expr(Expr::If(inner), _)] = i.then_branch.stmts.as_slice() {
                if inner.else_branch.is_none() {
                    let inner_cond = match &*inner.cond {
                        Expr::Binary(b) if matches!(b.op, syn::BinOp::Or(_)) => format!("({})", self.src_of(&*inner.cond)),
                        c => self.src_of(c).to_owned(),
                    };
                    let new = format!("if {} && {inner_cond} {}", self.src_of(&*i.cond), self.src_of(&inner.then_branch));
                    self.pattern("let-chains", i.if_token.span, Some(vec![self.replace(i, new)]));
                }
            }
        }
    }

    pub fn assign_patterns(&mut self, a: &syn::ExprAssign) {
        if let Expr::Unary(u) = &*a.right {
            if matches!(u.op, syn::UnOp::Not(_)) && tokens(&u.expr) == tokens(&a.left) {
                let edits = vec![self.replace(a, format!("{}.toggle()", self.src_of(&*a.left)))];
                self.pattern("bool-toggle", a.eq_token.span, Some(edits));
            }
        }
    }

    // ------------------------------------------------------------ items

    pub fn impl_patterns(&mut self, i: &syn::ItemImpl) {
        let is_default = i.trait_.as_ref().is_some_and(|(_, p, _)| p.segments.last().is_some_and(|s| s.ident == "Default"));
        if !is_default {
            return;
        }
        let [syn::ImplItem::Fn(f)] = i.items.as_slice() else { return };
        let Some(Expr::Path(p)) = single_expr(&f.block) else { return };
        if p.path.segments.len() < 2 || !i.generics.params.is_empty() {
            return;
        }
        let variant = p.path.segments.last().unwrap().ident.to_string();
        let ty = match &*i.self_ty {
            syn::Type::Path(t) => t.path.segments.last().map(|s| s.ident.to_string()),
            _ => None,
        };
        let edits = ty.and_then(|ty| self.enums.get(&ty)).and_then(|info| {
            let at = *info.variants.get(&variant)?;
            let derive = match info.derive_close {
                Some(close) => self.edit(close, close, ", Default"),
                None => self.edit(info.start, info.start, format!("#[derive(Default)]\n{}", self.indent_at(info.start))),
            };
            Some(vec![derive, self.edit(at, at, "#[default]\n".to_owned() + &self.indent_at(at)), self.delete_lines(i.span().start(), i.span().end())])
        });
        self.pattern("derive-default-enum", i.impl_token.span, edits);
    }

    pub fn enum_patterns(&mut self, e: &syn::ItemEnum) {
        for pair in e.variants.pairs() {
            let v = pair.value();
            if !v.ident.to_string().starts_with("__") {
                continue;
            }
            let already = e.attrs.iter().any(|a| a.path().is_ident("non_exhaustive"));
            let edits = (!already).then(|| {
                let end = pair.punct().map_or(v.span().end(), |p| p.span().end());
                let start = e.span().start();
                vec![self.edit(start, start, format!("#[non_exhaustive]\n{}", self.indent_at(start))), self.delete_lines(v.span().start(), end)]
            });
            self.pattern("non-exhaustive", v.ident.span(), edits);
        }
    }

    // ------------------------------------------------------------ macros

    pub fn macro_patterns(&mut self, mac: &syn::Macro, name: &str) {
        let args = mac.parse_body_with(Punctuated::<Expr, syn::Token![,]>::parse_terminated).ok();
        if let Some(args) = &args {
            if let Some(fmt_index) = format_index(name) {
                if has_uninlined_format_args(fmt_index, args) {
                    let edits = self.inline_format_text(args, fmt_index);
                    self.pattern("inline-format-args", mac.path.span(), edits);
                }
            }
        }
        match name {
            "try" if self.wants("try-macro") => {
                if let Ok(e) = mac.parse_body::<Expr>() {
                    let inner = self.src_of(&e);
                    let new = if is_tight(&e) { format!("{inner}?") } else { format!("({inner})?") };
                    self.pattern("try-macro", mac.path.span(), Some(vec![self.replace(mac, new)]));
                }
            }
            "lazy_static" if self.wants("lazy-static") => {
                let (name, import) = self.import("std::sync::LazyLock");
                let edits = self.lazy_static_text(mac, &name).map(|t| std::iter::once(self.replace(mac, t)).chain(import).collect());
                self.pattern("lazy-static", mac.path.span(), edits);
            }
            "addr_of" | "addr_of_mut" if self.wants("addr-of") => {
                let resolved = self.resolve_expr_path(&Expr::Path(syn::ExprPath { attrs: vec![], qself: None, path: mac.path.clone() }));
                let is_std = resolved.as_deref().is_some_and(|r| r.len() == 3 && matches!(r[0].as_str(), "std" | "core") && r[1] == "ptr");
                if let (true, Ok(e)) = (is_std, mac.parse_body::<Expr>()) {
                    let kw = if name == "addr_of" { "const" } else { "mut" };
                    let s = mac.span().start();
                    let mut new = format!("&raw {kw} {}", self.src_of(&e));
                    if self.receivers.contains(&(s.line, s.column)) {
                        new = format!("({new})");
                    }
                    self.pattern("addr-of", mac.path.span(), Some(vec![self.replace(mac, new)]));
                }
            }
            _ => {}
        }
    }

    /// `"{} and {:?}", a, b.c()` → `"{a} and {:?}", b.c()`
    fn inline_format_text(&self, args: &Punctuated<Expr, syn::Token![,]>, fmt_index: usize) -> Option<Vec<Edit>> {
        let Some(Expr::Lit(syn::ExprLit { lit: Lit::Str(lit), .. })) = args.iter().nth(fmt_index) else { return None };
        let lit_src = self.src(lit.span());
        if lit_src.contains("\\u{") || lit_src.contains("\\\n") {
            return None; // braces in escapes / line continuations: leave alone
        }
        let rest: Vec<&Expr> = args.iter().skip(fmt_index + 1).collect();
        let mut out = String::new();
        let mut keep: Vec<&Expr> = Vec::new();
        let mut implicit = 0;
        let mut inlined = false;
        let mut chars = lit_src.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '{' if chars.peek() == Some(&'{') => {
                    chars.next();
                    out.push_str("{{");
                }
                '{' => {
                    let spec: String = chars.by_ref().take_while(|&c| c != '}').collect();
                    if !(spec.is_empty() || spec.starts_with(':')) || spec.contains('$') || spec.contains('*') {
                        return None;
                    }
                    let arg = rest.get(implicit)?;
                    implicit += 1;
                    match arg {
                        Expr::Path(p) if p.qself.is_none() && p.path.get_ident().is_some() => {
                            out.push_str(&format!("{{{}{spec}}}", p.path.get_ident().unwrap()));
                            inlined = true;
                        }
                        other => {
                            out.push_str(&format!("{{{spec}}}"));
                            keep.push(other);
                        }
                    }
                }
                '}' if chars.peek() == Some(&'}') => {
                    chars.next();
                    out.push_str("}}");
                }
                c => out.push(c),
            }
        }
        if implicit != rest.len() || !inlined {
            return None;
        }
        let tail: String = keep.iter().map(|e| format!(", {}", self.src_of(*e))).collect();
        Some(vec![self.edit(lit.span().start(), rest.last()?.span().end(), format!("{out}{tail}"))])
    }

    /// `lazy_static! { static ref X: T = init; }` → `static X: LazyLock<T> = LazyLock::new(|| init);`
    fn lazy_static_text(&self, mac: &syn::Macro, lazy: &str) -> Option<String> {
        struct Item {
            attrs: Vec<syn::Attribute>,
            vis: syn::Visibility,
            name: syn::Ident,
            ty: syn::Type,
            init: Expr,
        }
        let items = mac
            .parse_body_with(|input: syn::parse::ParseStream| {
                let mut out = Vec::new();
                while !input.is_empty() {
                    let attrs = input.call(syn::Attribute::parse_outer)?;
                    let vis = input.parse()?;
                    input.parse::<syn::Token![static]>()?;
                    input.parse::<syn::Token![ref]>()?;
                    let name = input.parse()?;
                    input.parse::<syn::Token![:]>()?;
                    let ty = input.parse()?;
                    input.parse::<syn::Token![=]>()?;
                    let init = input.parse()?;
                    input.parse::<syn::Token![;]>()?;
                    out.push(Item { attrs, vis, name, ty, init });
                }
                Ok(out)
            })
            .ok()?;
        let indent = self.indent_at(mac.span().start());
        let decls: Vec<String> = items
            .iter()
            .map(|it| {
                let attrs: String = it.attrs.iter().map(|a| format!("{}\n{indent}", self.src_of(a))).collect();
                let vis = match &it.vis {
                    syn::Visibility::Inherited => String::new(),
                    v => format!("{} ", self.src_of(v)),
                };
                format!(
                    "{attrs}{vis}static {}: {lazy}<{}> = {lazy}::new(|| {});",
                    it.name,
                    self.src_of(&it.ty),
                    self.src_of(&it.init)
                )
            })
            .collect();
        (!decls.is_empty()).then(|| decls.join(&format!("\n{indent}")))
    }
}

// ---------------------------------------------------------------- helpers

fn tokens(e: &impl quote::ToTokens) -> String {
    e.to_token_stream().to_string()
}

fn strip_parens(e: &Expr) -> &Expr {
    match e {
        Expr::Paren(p) => strip_parens(&p.expr),
        e => e,
    }
}

fn bool_lit(e: &Expr) -> Option<bool> {
    match e {
        Expr::Lit(l) => match &l.lit {
            Lit::Bool(b) => Some(b.value),
            _ => None,
        },
        Expr::Block(b) => single_expr(&b.block).and_then(bool_lit),
        _ => None,
    }
}

fn path_ends_with(e: &Expr, tail: &[&str]) -> bool {
    let Expr::Path(p) = e else { return false };
    let segs: Vec<String> = p.path.segments.iter().map(|s| s.ident.to_string()).collect();
    segs.len() >= tail.len() && segs[segs.len() - tail.len()..].iter().zip(tail).all(|(a, b)| a == b)
}

/// The block's only content, if it is a single tail expression.
fn single_expr(b: &syn::Block) -> Option<&Expr> {
    match b.stmts.as_slice() {
        [Stmt::Expr(e, None)] => Some(e),
        _ => None,
    }
}

fn block_expr(e: &Expr) -> Option<&Expr> {
    match e {
        Expr::Block(b) => single_expr(&b.block),
        _ => None,
    }
}

fn call_named(e: &Expr, name: &str) -> bool {
    matches!(e, Expr::Call(c) if c.args.len() == 1 && path_ends_with(&c.func, &[name]))
}

fn is_ok_unit(e: &Expr) -> bool {
    matches!(e, Expr::Call(c) if path_ends_with(&c.func, &["Ok"]) && matches!(c.args.first(), Some(Expr::Tuple(t)) if t.elems.is_empty()))
}

fn is_diverging(e: &Expr) -> bool {
    match e {
        Expr::Return(_) | Expr::Continue(_) | Expr::Break(_) => true,
        Expr::Macro(m) => is_diverging_macro(&m.mac),
        Expr::Block(b) => block_diverges(&b.block),
        _ => false,
    }
}

fn is_diverging_macro(m: &syn::Macro) -> bool {
    m.path.segments.last().is_some_and(|s| matches!(s.ident.to_string().as_str(), "panic" | "unreachable" | "todo" | "unimplemented" | "bail"))
}

fn block_diverges(b: &syn::Block) -> bool {
    match b.stmts.last() {
        Some(Stmt::Expr(e, _)) => is_diverging(e),
        Some(Stmt::Macro(m)) => is_diverging_macro(&m.mac),
        _ => false,
    }
}

fn bindings(p: &Pat, out: &mut Vec<String>) {
    match p {
        // `None`, `FOO`: syn can't tell a unit variant/constant from a binding; go by case.
        Pat::Ident(i) if i.subpat.is_none() && i.ident.to_string().starts_with(char::is_uppercase) => {}
        Pat::Ident(i) => {
            out.push(i.ident.to_string());
            if let Some((_, sub)) = &i.subpat {
                bindings(sub, out);
            }
        }
        Pat::TupleStruct(t) => t.elems.iter().for_each(|e| bindings(e, out)),
        Pat::Tuple(t) => t.elems.iter().for_each(|e| bindings(e, out)),
        Pat::Struct(s) => s.fields.iter().for_each(|f| bindings(&f.pat, out)),
        Pat::Reference(r) => bindings(&r.pat, out),
        _ => {}
    }
}

/// Where the binding `name` starts inside `p`.
fn find_binding(p: &Pat, name: &str) -> Option<LineColumn> {
    match p {
        Pat::Ident(i) if i.ident == name => Some(i.ident.span().start()),
        Pat::TupleStruct(t) => t.elems.iter().find_map(|e| find_binding(e, name)),
        Pat::Tuple(t) => t.elems.iter().find_map(|e| find_binding(e, name)),
        Pat::Struct(s) => s.fields.iter().find_map(|f| find_binding(&f.pat, name)),
        Pat::Reference(r) => find_binding(&r.pat, name),
        _ => None,
    }
}

fn binds_nothing(p: &Pat) -> bool {
    let mut names = Vec::new();
    bindings(p, &mut names);
    names.is_empty()
}

/// The arm/branch produces exactly the one variable its pattern binds.
fn yields_binding(pat: &Pat, body: &Expr) -> bool {
    let mut names = Vec::new();
    bindings(pat, &mut names);
    let body = block_expr(body).unwrap_or(body);
    matches!((names.as_slice(), body), ([n], Expr::Path(p)) if p.path.is_ident(n))
}

/// `match e { Some(x) => x, None => return }` → (keep pattern, scrutinee, diverging body).
/// Not `Err(e) => return Err(e.into())`: let-else's `else` can't use `e`.
fn unwrap_or_diverge_match(m: &syn::ExprMatch) -> Option<(&Pat, &Expr, &Expr)> {
    let [a, b] = m.arms.as_slice() else { return None };
    if a.guard.is_some() || b.guard.is_some() {
        return None;
    }
    let ok = |keep: &syn::Arm, bail: &syn::Arm| yields_binding(&keep.pat, &keep.body) && binds_nothing(&bail.pat) && is_diverging(&bail.body);
    if ok(a, b) {
        Some((&a.pat, &m.expr, &b.body))
    } else if ok(b, a) {
        Some((&b.pat, &m.expr, &a.body))
    } else {
        None
    }
}

/// `if let Some(x) = e { x } else { return }`
fn unwrap_or_diverge_if(i: &syn::ExprIf) -> Option<(&Pat, &Expr, &Expr)> {
    let (Expr::Let(l), Some((_, els))) = (&*i.cond, &i.else_branch) else { return None };
    (single_expr(&i.then_branch).is_some_and(|e| yields_binding(&l.pat, e)) && is_diverging(els)).then_some((&*l.pat, &*l.expr, &**els))
}

/// `x.as_ref().map(|s| s.as_str())` or `.map(String::as_str)`
fn is_as_ref_map(m: &syn::ExprMethodCall) -> bool {
    let Expr::MethodCall(inner) = &*m.receiver else { return false };
    if inner.method != "as_ref" || !inner.args.is_empty() {
        return false;
    }
    const DEREFS: [&str; 4] = ["as_str", "as_slice", "as_path", "as_os_str"];
    match &m.args[0] {
        Expr::Path(p) => p.path.segments.last().is_some_and(|s| DEREFS.contains(&s.ident.to_string().as_str())),
        Expr::Closure(c) => match (c.inputs.first(), strip_parens(&c.body)) {
            (Some(Pat::Ident(param)), Expr::MethodCall(call)) => {
                call.args.is_empty() && DEREFS.contains(&call.method.to_string().as_str()) && matches!(&*call.receiver, Expr::Path(p) if p.path.is_ident(&param.ident))
            }
            _ => false,
        },
        _ => false,
    }
}

/// `(0..n).map(f).collect()`
fn is_zero_range_map(e: &Expr) -> bool {
    let Expr::MethodCall(map) = e else { return false };
    map.method == "map"
        && matches!(strip_parens(&map.receiver), Expr::Range(r)
            if matches!(r.limits, syn::RangeLimits::HalfOpen(_)) && r.end.is_some() && r.start.as_deref().is_some_and(|s| is_int(s, 0)))
}

/// `collect()` or `collect::<Vec<_>>()`, not `collect::<String>()` etc.
fn collects_vec_or_unknown(m: &syn::ExprMethodCall) -> bool {
    m.turbofish.as_ref().is_none_or(|t| {
        matches!(t.args.first(), Some(syn::GenericArgument::Type(syn::Type::Path(p))) if p.path.segments.last().is_some_and(|s| s.ident == "Vec"))
    })
}

/// `KEY.with(|c| c.set(c.get() + 1))`
fn is_cell_set_get(e: &Expr) -> bool {
    let Expr::Closure(c) = e else { return false };
    let (Some(Pat::Ident(p)), body) = (c.inputs.first(), strip_parens(&c.body)) else { return false };
    let body = block_expr(body).unwrap_or(body);
    let Expr::MethodCall(set) = body else { return false };
    set.method == "set"
        && matches!(&*set.receiver, Expr::Path(r) if r.path.is_ident(&p.ident))
        && set.args.first().is_some_and(|a| tokens(a).contains(&format!("{} . get ()", p.ident)))
}

/// `Default::default()`, `T::default()`, `Vec::new()`, `String::new()`, …
fn is_default_value(e: &Expr) -> bool {
    const EMPTY: [&str; 7] = ["Vec", "String", "HashMap", "HashSet", "BTreeMap", "BTreeSet", "VecDeque"];
    let Expr::Call(c) = e else { return false };
    let Expr::Path(p) = &*c.func else { return false };
    let segs: Vec<String> = p.path.segments.iter().map(|s| s.ident.to_string()).collect();
    c.args.is_empty()
        && match segs.as_slice() {
            [.., last] if last == "default" => true,
            [.., ty, last] if last == "new" => EMPTY.contains(&ty.as_str()),
            _ => false,
        }
}

fn format_index(mac: &str) -> Option<usize> {
    match mac {
        "write" | "writeln" => Some(1),
        "format" | "print" | "println" | "eprint" | "eprintln" | "panic" | "format_args" | "unreachable" | "todo" | "unimplemented"
        | "bail" | "anyhow" | "info" | "warn" | "error" | "debug" | "trace" => Some(0),
        _ => None,
    }
}

/// `println!("{}", x)` where an implicit `{}` is fed a plain identifier.
fn has_uninlined_format_args(fmt_index: usize, args: &Punctuated<Expr, syn::Token![,]>) -> bool {
    let Some(Expr::Lit(syn::ExprLit { lit: Lit::Str(s), .. })) = args.iter().nth(fmt_index) else { return false };
    let fmt = s.value();
    let mut implicit = 0;
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
            }
            '{' => {
                let spec: String = chars.by_ref().take_while(|&c| c != '}').collect();
                if spec.is_empty() || spec.starts_with(':') {
                    implicit += 1;
                } else {
                    return false; // already uses named/positional arguments
                }
            }
            _ => {}
        }
    }
    let rest: Vec<&Expr> = args.iter().skip(fmt_index + 1).collect();
    implicit > 0 && rest.len() == implicit && rest.iter().any(|e| matches!(e, Expr::Path(p) if p.qself.is_none() && p.path.get_ident().is_some()))
}
