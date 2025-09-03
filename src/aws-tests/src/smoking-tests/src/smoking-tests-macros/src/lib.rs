use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, parse_macro_input};

#[proc_macro_attribute]
pub fn smoking_test(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);
    let vis = &input.vis;
    let sig = &input.sig;
    let block = &input.block;

    let expanded = quote! {
        #[ignore]
        #[tokio::test]
        #[serial_test::serial]
        #vis #sig {
            use futures_util::future::FutureExt;

            // Run the test body and catch any panic
            let result = std::panic::AssertUnwindSafe(async { #block })
                .catch_unwind()
                .await;

            // Rethrow panic if the test body panicked
            if let Err(panic) = result {
                std::panic::resume_unwind(panic);
            }
        }
    };

    TokenStream::from(expanded)
}
