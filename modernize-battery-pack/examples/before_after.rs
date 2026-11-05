//! The "Now in std" rewrites `cargo modernize` suggests, in their modern form.
//! `cargo run --example before_after -p modernize-battery-pack`
use std::collections::HashMap;
use std::sync::LazyLock;

// was: lazy_static! { static ref TABLE: HashMap<&'static str, u32> = hashmap!{ "a" => 1 }; }
static TABLE: LazyLock<HashMap<&'static str, u32>> = LazyLock::new(|| HashMap::from([("a", 1), ("b", 2)]));

fn main() {
    // was: num_cpus::get()
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());

    // was: crossbeam::scope(|s| { s.spawn(|_| ...); }).unwrap();
    let data = [1, 2, 3];
    let sum: i32 = std::thread::scope(|s| s.spawn(|| data.iter().sum()).join().unwrap());

    // was: Cursor::new(buf).read_u32::<BigEndian>()
    let n = u32::from_be_bytes([0, 0, 1, 0]);

    println!("cpus={cpus} sum={sum} n={n} a={}", TABLE["a"]);
}
