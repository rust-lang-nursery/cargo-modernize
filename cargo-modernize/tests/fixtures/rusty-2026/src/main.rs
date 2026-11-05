// The 2026 spellings of everything in ../rusty-2019. Must stay finding-free.
use std::any::Any;
use std::io::{self, Write};
use std::mem;

#[derive(Debug, Default)]
pub enum ClickState {
    #[default]
    None,
    Click,
}

pub trait Plugin: Any {}

#[non_exhaustive]
pub enum ErrorKind {
    Regex(String),
}

struct Stats {
    stats: Vec<u32>,
}

fn is_ident(c: char) -> bool {
    matches!(c, 'a'..='z' | 'A'..='Z' | '_')
}

fn first(v: &[u32]) -> Option<u32> {
    let Some(x) = v.first() else { return None };
    Some(*x)
}

fn parse(line: &str, name: Option<String>, binary: Option<bool>) -> io::Result<()> {
    let _kv = line.split_once('=');
    let _n: Option<&str> = name.as_deref();
    let _b = binary.is_some_and(|b| b);
    let _c = binary.is_none_or(|b| b);
    let _t = line.trim_start();
    let _max = usize::MAX;
    if line.is_empty() {
        return Err(io::Error::other("empty line"));
    }
    if let Some(n) = name
        && n.len() > 3
    {
        println!("long name");
    }
    Ok(())
}

fn main() {
    let mut s = Stats { stats: vec![1] };
    let taken = mem::take(&mut s.stats);
    let e = "boom";
    let _ = writeln!(io::stdout(), "Unable to create log file: {e}");
    println!("{taken:?} {} {:?} {}", is_ident('a'), first(&[1]), ClickState::default() as u8);
    let _ = (ErrorKind::Regex(String::new()), parse("a=b", None, None));
}
