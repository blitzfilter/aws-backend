use proc_macro::TokenStream;
use quote::quote;
use syn::{Expr, ExprArray, ItemFn, parse_macro_input};

/// Attribute macro for running async integration tests with LocalStack services.
///
/// This macro wraps an async test function and automatically:
/// - Parses the `services = [ServiceA, ServiceB, ...]` attribute
/// - Spins up LocalStack with only the specified services
/// - Calls each service's `set_up().await` before the test
/// - Executes the test body
/// - Calls each service's `tear_down().await` after the test
/// - Shuts down LocalStack at the end
///
/// # Requirements
///
/// Each service provided:
/// - must be a valid identifier for a type implementing trait `IntegrationTestService`
/// - must define a constant `SERVICE_NAME: &str` who corresponds to a valid service-name for LocalStack
/// - must define an `async fn set_up()`
/// - may define an `async fn tear_down()`
///
/// Example:
///
/// ```rust
/// pub struct S3;
///
/// impl S3 {
///     pub const SERVICE_NAME: &'static str = "s3";
///
///     pub async fn set_up() {
///         // setup logic
///     }
///
///     pub async fn tear_down() {
///         // teardown logic
///     }
/// }
/// ```
///
/// # Notes
///
/// - Requires Tokio runtime (`#[tokio::test]`) test ex82ecution.
/// - The attribute must be in the format: `services = [ServiceA, ServiceB, ...]`.
/// - Malformed input will panic at compile time.
///
/// # See also
///
/// - [`test_api::localstack::spin_up_localstack_with_services`] for how LocalStack is started.
///
#[proc_macro_attribute]
pub fn localstack_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(item as ItemFn);
    let attr_expr = attr.to_string();

    // Parse the `services = [ServiceA, ServiceB]` syntax manually
    let services: Vec<syn::Ident> = if let Some(eq_pos) = attr_expr.find('=') {
        let expr_str = attr_expr[eq_pos + 1..].trim();
        let expr: Expr = syn::parse_str(expr_str).expect("Expected a Rust expression");
        if let Expr::Array(ExprArray { elems, .. }) = expr {
            elems
                .into_iter()
                .filter_map(|elem| match elem {
                    Expr::Path(path) => path.path.get_ident().cloned(),
                    _ => None,
                })
                .collect()
        } else {
            panic!("Expected array expression for `services = [...]`");
        }
    } else {
        panic!("Expected `services = [...]`");
    };

    let service_name_consts = services.iter().map(|ident| {
        quote! { #ident::SERVICE_NAME }
    });
    let mut setup_code = quote! {};
    let mut teardown_code = quote! {};

    for (i, ident) in services.iter().enumerate() {
        syn::Ident::new(&format!("__svc_{i}"), ident.span());

        setup_code = quote! {
            #setup_code
            #ident::set_up().await;
        };

        teardown_code = quote! {
            #teardown_code
            #ident::tear_down().await;
        };
    }

    let fn_name = &input_fn.sig.ident;
    let fn_block = &input_fn.block;

    let expanded = quote! {
        #[tokio::test]
        #[test_api::serial]
        async fn #fn_name() {
            let __services: &[&str] = &[
                #( #service_name_consts ),*
            ];
            let __localstack = test_api::localstack::spin_up_localstack_with_services(__services).await;

            #setup_code

            let __test_fn = async #fn_block;
            __test_fn.await;

            #teardown_code

            drop(__localstack);
        }
    };

    expanded.into()
}
