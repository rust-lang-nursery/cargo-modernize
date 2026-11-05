// A folder whose .modernize turns opinions off but promotes one rule to deny.
pub fn spaces(s: &str) -> &str {
    s.trim_left()
}

pub fn n(v: Option<u8>) -> bool {
    v.map_or(false, |x| x > 1)
}

pub enum ErrorKind {
    Regex(String),
    #[doc(hidden)]
    __Nonexhaustive,
}
