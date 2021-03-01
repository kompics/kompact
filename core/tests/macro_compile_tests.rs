// Stable formats the output differently
//#[rustversion::attr(not(nightly), ignore)]
//#[test]
#[rustversion::attr(nightly, test)]
fn compile_test() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile-fail/*.rs");
}
