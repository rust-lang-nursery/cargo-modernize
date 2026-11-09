// Edition 2015 code that rustc accepts and syn 3 does not.
use std::collections::HashMap;
use std::rc::Rc;

pub struct Handlers {
    pub by_name: HashMap<String, Box<Fn(i32) -> i32>>,
    pub shared: Option<Rc<Fn() + Send>>,
}

pub trait Backend {
    fn set_color(&mut self, &str);
    fn apply(&self, &Vec<Rc<String>>) {}
}

pub fn first(v: &[u32]) -> bool {
    v.len() == 0
}

pub fn await(x: u32) -> Option<u32> {
    let r = try!(Some(x));
    Some(r)
}
