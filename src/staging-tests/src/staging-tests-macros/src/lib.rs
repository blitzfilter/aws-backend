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
            struct ResetGuard;

            impl Drop for ResetGuard {
                fn drop(&mut self) {
                    // Ensure cleanup always runs
                    let fut = staging_tests::reset();
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(fut);
                }
            }

            let _guard = ResetGuard;

            // Run the test body normally
            #block
        }
    };

    TokenStream::from(expanded)
}
