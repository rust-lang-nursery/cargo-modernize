// Pre-1.76 spellings that `walk/*` rules step forward.
use std::collections::hash_map::{DefaultHasher, RandomState};
use std::future::Future;
use std::hash::{BuildHasher, Hasher};
use std::os::raw::c_int;
use std::sync::atomic::{AtomicBool, Ordering};

#[allow(dead_code)]
fn slow<F: Future<Output = ()>>(_f: F) {}

#[allow(deprecated)]
fn main() {
    let flag = AtomicBool::new(false);
    let _prev = flag.compare_and_swap(false, true, Ordering::SeqCst);
    let mut h = DefaultHasher::new();
    h.write_u8(1);
    let _ = RandomState::new().build_hasher();
    let _n: c_int = 0;
}
