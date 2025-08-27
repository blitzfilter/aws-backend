pub use staging_tests_macros::staging_test;

// Called inside the macro
pub async fn reset() {
    println!("cleanup called!");
}
