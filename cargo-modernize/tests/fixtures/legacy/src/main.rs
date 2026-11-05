#[macro_use]
extern crate error_chain;
#[macro_use]
extern crate lazy_static;

use byteorder::{BigEndian, ReadBytesExt};
use rand::distributions::{Distribution, Standard, Uniform};
use rand::Rng;
use regex::Regex;
use ring::signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use std::io::Cursor;

error_chain! {
    foreign_links {
        Io(std::io::Error);
    }
}

lazy_static! {
    static ref WORD: Regex = Regex::new(r"\w+").unwrap();
}

fn threads() -> usize {
    num_cpus::get()
}

fn dice() -> u8 {
    let mut rng = rand::thread_rng();
    let die = Uniform::from(1..7);
    let _coin: bool = rng.gen();
    let _x: f64 = Standard.sample(&mut rng);
    println!("{}", rng.gen_range(0..10));
    die.sample(&mut rng)
}

fn sign(pkcs8: &[u8]) {
    let _kp = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8);
}

fn main() -> Result<()> {
    let arr = &mut [1, 2, 3];
    crossbeam::scope(|s| {
        s.spawn(|_| println!("{:?}", arr));
    })
    .unwrap();
    let mut rdr = Cursor::new(vec![0, 0, 1, 0]);
    let n = rdr.read_u32::<BigEndian>()?;
    println!("{} {} {} {}", n, threads(), dice(), std::u32::MAX);
    let _ = WORD.is_match("hi");
    sign(&[]);
    Ok(())
}
