// Idioms from Rust 1.0–1.49 that the `walk/*` detectors step forward.
#![allow(dead_code, clippy::all)]
use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

const NAMES: &'static [&'static str] = &["a", "b"];

struct Config {
    name: String,
    port: u16,
}

struct Token<'a>(&'a str);

impl<'a> fmt::Display for Token<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn double(x: u32) -> u32 {
    return x * 2;
}

fn callbacks() -> Vec<Box<fmt::Display + Send>> {
    Vec::new()
}

fn build(name: String, port: u16) -> Config {
    Config { name: name, port: port }
}

fn checks(v: &[u32], o: Option<u32>, m: &HashMap<String, u32>, idx: i32, len: i32) -> i32 {
    if v.len() == 0 {
        return 0;
    }
    if o == None {
        return 1;
    }
    if let Some(_) = m.get("k") {
        return 2;
    }
    let first = v.iter().filter(|x| **x > 1).next();
    let _ = first;
    let _flat: Vec<char> = vec!["ab", "cd"].into_iter().map(|s| s.chars()).flatten().collect();
    let _opts: Vec<u32> = vec![Some(1), None].into_iter().filter_map(|x| x).collect();
    let _c: Vec<u32> = v.iter().map(|x| x.clone()).collect();
    let _p: Vec<u32> = v.iter().map(|&x| x).collect();
    let _d = o.map(|x| x).unwrap_or(0);
    let _s = Some(String::new()).unwrap_or(String::new());
    let _t = Some(vec![1]).unwrap_or_else(Vec::new);
    let _u = o.and_then(|x| Some(x + 1));
    let _mapped = match o {
        Some(x) => Some(x + 1),
        None => None,
    };
    let _n = match o {
        Some(n) => n,
        None => 0,
    };
    let mut counts: HashMap<&str, u32> = HashMap::new();
    *counts.entry("w").or_insert(0) += 1;
    let mut lists: HashMap<&str, Vec<u32>> = HashMap::new();
    lists.entry("w").or_insert_with(Vec::new).push(1);
    let mut stack = vec![1, 2, 3];
    loop {
        match stack.pop() {
            Some(x) => println!("{}", x),
            None => break,
        }
    }
    let _wrapped = ((idx % len) + len) % len;
    let _e = if idx > 0 { Some(idx) } else { None };
    let _z = Duration::from_secs(0);
    let _r: Vec<u8> = std::iter::repeat(0u8).take(3).collect();
    let _clamped = idx.max(0).min(10);
    3
}

fn main() {
    let _ = (double(1), callbacks().len(), build(String::new(), 1).port, NAMES.len());
    let Config { name: name, port } = build(String::new(), 1);
    let _ = (name, port, checks(&[], None, &HashMap::new(), 1, 2));
    let mut v = vec![1.0f64, 2.0];
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
}
