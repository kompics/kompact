// Stable formats the output differently
#[allow(dead_code)]
#[rustversion::attr(nightly, test)]
#[ignore = "The format is just too unstable and constantly breaks the build"]
fn compile_test() {
    // Uncomment this to run the tests. Ignored tests still run on CI, which doesn't solve the issue.
    // let t = trybuild::TestCases::new();
    // t.compile_fail("tests/compile-fail/*.rs");
}
