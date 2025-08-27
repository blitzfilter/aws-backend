use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, parse_macro_input};

#[proc_macro_attribute]
pub fn staging_test(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);
    let vis = &input.vis;
    let sig = &input.sig;
    let block = &input.block;

    let expanded = quote! {
        #[ignore]
        #[tokio::test]
        #[serial_test::serial]
        #vis #sig {
            use std::panic::{AssertUnwindSafe, catch_unwind};

            // Spawn the test body into its own task so we can unwind safely
            let result = catch_unwind(AssertUnwindSafe(|| {
                tokio::spawn(async #block)
            }));

            // Always run cleanup after the body (whether spawn panicked or not)
            staging_tests::reset().await;

            match result {
                Ok(handle) => {
                    // Await the task result
                    match handle.await {
                        Ok(_) => {} // test passed
                        Err(join_err) if join_err.is_panic() => {
                            // rethrow panic from inside the task
                            std::panic::resume_unwind(join_err.into_panic());
                        }
                        Err(_) => panic!("test task was cancelled"),
                    }
                }
                Err(panic) => {
                    // rethrow immediate panic before spawn
                    std::panic::resume_unwind(panic);
                }
            }
        }
    };

    TokenStream::from(expanded)
}
