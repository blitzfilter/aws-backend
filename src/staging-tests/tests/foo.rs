use staging_tests::staging_test;

#[staging_test]
async fn succeeds() {
    println!("this test passes");
}

#[staging_test]
async fn fails() {
    panic!("oh no");
}
