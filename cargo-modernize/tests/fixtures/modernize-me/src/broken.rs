// `.map_or(false, ..)` on a Result: the syntax-only rewrite to `is_some_and` doesn't
// compile here (Result has `is_ok_and`), so --dangerous must revert this file.
pub fn positive(r: Result<i32, ()>) -> bool {
    r.map_or(false, |x| x > 0)
}
