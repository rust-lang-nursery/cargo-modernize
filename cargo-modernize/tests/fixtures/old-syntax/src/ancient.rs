// Pre-1.0 syntax, still found in 2015 snapshots.
#[deriving(Clone)]
pub struct Frame {
    pub pos: [f32, ..3],
    pub on_done: |uint| -> u8,
}

pub fn run(v: &mut [u8], n: u32) -> u32 {
    let first = v[];
    let b = box Frame;
    spawn(proc() { first.len() });
    let empty = v.len() == 0;
    (n as u32 << 2) | empty as u32
}
