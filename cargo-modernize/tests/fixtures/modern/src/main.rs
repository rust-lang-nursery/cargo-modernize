use rand::RngExt;
use rand::distr::{Distribution, StandardUniform};
use regex::Regex;
use std::f64::consts::PI;
use std::sync::LazyLock;

static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\w+").unwrap());

struct Gen;
impl Gen {
    fn r#gen(&self) -> u8 { 4 }
}

fn main() {
    let mut rng = rand::rng();
    let x: f64 = StandardUniform.sample(&mut rng);
    let n = std::thread::available_parallelism().map_or(1, |n| n.get());
    let v = u32::from_be_bytes([0, 0, 1, 0]);
    let _ = Gen.r#gen(); // a local `r#gen` is fine: rand rules only flag `.gen()` calls
    std::thread::scope(|s| {
        s.spawn(|| println!("{x} {n} {v} {} {PI}", u32::MAX));
    });
    let _ = WORD.is_match("hi") && rng.random_bool(0.5) && dice::rng().random::<bool>();
}
