// Pre-2020 idioms, adapted from real projects (see rules/rusty.toml for sources).
#![feature(let_chains, never_type)]
use std::cell::Cell;
use std::io::{self, Write};
use std::mem;

mod generated;
mod legacy_api;

thread_local!(static COUNT: Cell<u32> = Cell::new(0));

#[derive(Debug)]
pub enum ClickState {
    None,
    Click,
}

impl Default for ClickState {
    fn default() -> Self {
        ClickState::None
    }
}

pub trait Plugin {
    fn as_any(&self) -> &dyn std::any::Any;
}

#[allow(dead_code)]
struct Stats {
    stats: Vec<u32>,
}

fn is_ident(c: char) -> bool {
    match c {
        'a'..='z' | 'A'..='Z' | '_' => true,
        _ => false,
    }
}

fn first(v: &[u32]) -> Option<u32> {
    let x = match v.first() {
        Some(x) => x,
        None => return None,
    };
    Some(*x)
}

fn parse(line: &str, name: Option<String>, binary: Option<bool>) -> io::Result<()> {
    let mut parts = line.splitn(2, '=');
    let _k = parts.next();
    let _n: Option<&str> = name.as_ref().map(|s| s.as_str());
    let _b = binary.map_or(false, |b| b);
    let _c = binary.map_or(true, |b| b);
    let _rest = line.strip_prefix("./").unwrap_or(line);
    let bytes = vec![b'h', b'i'];
    let _s = String::from_utf8_lossy(&bytes).into_owned();
    let _sq: Vec<usize> = (0..10).map(|i| i * i).collect();
    let _max = usize::max_value();
    let _m32 = u32::max_value();
    if line.is_empty() {
        return Err(io::Error::new(io::ErrorKind::Other, "empty line"));
    }
    if let Some(n) = name {
        if n.len() > 3 {
            println!("long name");
        }
    }
    let ok = !line.is_empty();
    if ok { Ok(()) } else { Err(io::Error::other("x")) }
}

fn main() {
    let mut s = Stats { stats: vec![1] };
    let taken = mem::replace(&mut s.stats, Vec::new());
    let mut verbose = false;
    verbose = !verbose;
    COUNT.with(|c| c.set(c.get() + 1));
    let e = "boom";
    let _ = writeln!(io::stdout(), "Unable to create log file: {}", e);
    println!("{:?} {} {:?} {}", taken, is_ident('a'), first(&[1]), verbose);
    let _ = parse("a=b", None, None);
}
