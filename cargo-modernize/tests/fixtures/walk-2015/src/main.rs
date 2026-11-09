// An edition 2015 crate: `extern crate` and `#[macro_use]` are still needed here.
#[macro_use]
extern crate log;
extern crate libc;
extern crate core;

fn main() {
    info!("{}", libc::EOF);
}
