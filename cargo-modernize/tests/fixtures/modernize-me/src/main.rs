use std::cell::Cell;
use std::convert::TryFrom;
use std::io;
use std::mem;

mod broken;

thread_local!(static HITS: Cell<u32> = Cell::new(0));

#[derive(Debug, PartialEq)]
enum Mode {
    Fast,
    Slow,
}

impl Default for Mode {
    fn default() -> Self {
        Mode::Slow
    }
}

fn first_word(s: &str) -> Option<&str> {
    let w = match s.split_whitespace().next() {
        Some(w) => w,
        None => return None,
    };
    Some(w)
}

fn is_digit(c: char) -> bool {
    match c {
        '0'..='9' => true,
        _ => false,
    }
}

fn check(len: usize) -> Result<(), String> {
    if len < 10 { Ok(()) } else { Err(format!("too long: {}", len)) }
}

fn main() -> io::Result<()> {
    let name: Option<String> = Some("x".into());
    let label: Option<&str> = name.as_ref().map(|s| s.as_str());
    let big = name.as_ref().map_or(false, |n| n.len() > 3);
    let mut v = vec![1u8, 2];
    let old = mem::replace(&mut v, Vec::new());
    let n = u8::try_from(300u32).unwrap_or(u8::max_value());
    let limit = std::u32::MAX;
    HITS.with(|c| c.set(c.get() + 1));
    if old.is_empty() {
        return Err(io::Error::new(io::ErrorKind::Other, "empty"));
    }
    println!("{:?} {} {:?} {} {} {} {:?}", label, big, old, n, limit, is_digit('7'), first_word("a b"));
    println!("{:?} {:?} {}", Mode::default(), check(3), broken::positive(Ok(2)));
    let _ = Mode::Fast;
    Ok(())
}
