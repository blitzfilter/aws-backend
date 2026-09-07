use proc_macro::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{Expr, ExprArray, ItemFn, parse_macro_input};

/// Attribute macro for running async integration tests with Aura test services.
///
/// This macro wraps an async test function and automatically:
/// - Parses the `services = [ServiceA, ServiceB, ...]` attribute
/// - Spins up LocalStack with only the specified LocalStack services
/// - Calls each service's `set_up().await` before the test
/// - Executes the test body
/// - Calls each service's `tear_down().await` after the test
/// - Keeps non-LocalStack services, like Postgres, managed by their service helper
///
/// # Requirements
///
/// Each service provided:
/// - must be a valid identifier for a type implementing trait `IntegrationTestService`
/// - must return LocalStack service names from `service_names()` or `&[]` for non-LocalStack services
/// - must define an `async fn set_up()`
/// - may define an `async fn tear_down()`
///
/// Example:
///
/// ```rust
/// pub struct S3;
///
/// impl S3 {
///     pub fn service_names() -> &'static [&'static str] {
///         &["s3"]
///     }
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
/// - Requires Tokio runtime (`#[tokio::test]`) test execution.
/// - The attribute must be in the format: `services = [ServiceA, ServiceB, ...]`.
/// - Malformed input will panic at compile time.
///
/// # See also
///
/// - [`test_api::localstack::spin_up_localstack_with_services`] for how LocalStack is started.
///
#[proc_macro_attribute]
pub fn aura_integration_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(item as ItemFn);
    let attr_expr = attr.to_string();

    // Parse `services = [ServiceA(...), ServiceB, ...]`
    let service_exprs: Vec<Expr> = if let Some(eq_pos) = attr_expr.find('=') {
        let expr_str = attr_expr[eq_pos + 1..].trim();
        let expr: Expr = syn::parse_str(expr_str).expect("Expected a Rust expression");
        if let Expr::Array(ExprArray { elems, .. }) = expr {
            elems.into_iter().collect()
        } else {
            panic!("Expected array expression for `services = [...]`");
        }
    } else {
        panic!("Expected `services = [...]`");
    };

    // Generate bindings like: `let mut __svc_0 = Service(...)`
    let service_bindings = service_exprs.iter().enumerate().map(|(i, expr)| {
        let ident = syn::Ident::new(&format!("__svc_{i}"), expr.span());
        quote! {
            let mut #ident = #expr;
        }
    });

    // Generate code to collect service names from each instance
    let service_names = (0..service_exprs.len()).map(|i| {
        let ident = syn::Ident::new(&format!("__svc_{i}"), proc_macro2::Span::call_site());
        quote! {
            #ident.service_names()
        }
    });

    // Generate code to collect env vars from each service
    let service_env_vars = (0..service_exprs.len()).map(|i| {
        let ident = syn::Ident::new(&format!("__svc_{i}"), proc_macro2::Span::call_site());
        quote! {
            #ident.env_vars()
        }
    });

    // Generate setup and teardown calls
    let setup_calls = (0..service_exprs.len()).map(|i| {
        let ident = syn::Ident::new(&format!("__svc_{i}"), proc_macro2::Span::call_site());
        quote! {
            #ident.set_up().await;
        }
    });

    let teardown_calls = (0..service_exprs.len()).rev().map(|i| {
        let ident = syn::Ident::new(&format!("__svc_{i}"), proc_macro2::Span::call_site());
        quote! {
            #ident.tear_down().await;
        }
    });

    let attributes = &input_fn.attrs;
    let fn_name = &input_fn.sig.ident;
    let fn_block = &input_fn.block;

    let expanded = quote! {
        #( #attributes )*
        #[tokio::test]
        #[test_api::serial]
        async fn #fn_name() {
            use std::collections::HashSet;

            #( #service_bindings )*

            let __services: Vec<&str> = {
                let mut set = HashSet::new();
                let mut result = Vec::new();
                for name in [ #( #service_names ),* ].concat() {
                    if set.insert(name) {
                        result.push(name);
                    }
                }
                result
            };

            let __env_vars: Vec<(&str, &str)> = {
                let mut result = Vec::new();
                for pair in [ #( #service_env_vars ),* ].concat() {
                    result.push(pair);
                }
                result
            };

            let __localstack = if !__services.is_empty() {
                Some(test_api::localstack::get_localstack(&__services, &__env_vars).await)
            } else {
                None
            };

            let __setup_started = std::time::Instant::now();
            #( #setup_calls )*
            test_api::tracing::debug!(
                elapsed_ms = __setup_started.elapsed().as_millis(),
                "Integration test setup complete."
            );

            let __body_started = std::time::Instant::now();
            let __body_result = test_api::FutureExt::catch_unwind(
                std::panic::AssertUnwindSafe(async #fn_block)
            ).await;
            test_api::tracing::debug!(
                elapsed_ms = __body_started.elapsed().as_millis(),
                "Integration test body complete."
            );

            let __teardown_started = std::time::Instant::now();
            let __teardown_result = test_api::FutureExt::catch_unwind(
                std::panic::AssertUnwindSafe(async {
                    #( #teardown_calls )*
                })
            ).await;
            test_api::tracing::debug!(
                elapsed_ms = __teardown_started.elapsed().as_millis(),
                "Integration test teardown complete."
            );

            drop(__localstack);

            match (__body_result, __teardown_result) {
                (Err(body_panic), Err(cleanup_panic)) => {
                    eprintln!("integration-test teardown panicked after test-body panic: {cleanup_panic:?}");
                    std::panic::resume_unwind(body_panic);
                }
                (Err(body_panic), Ok(())) => std::panic::resume_unwind(body_panic),
                (Ok(()), Err(cleanup_panic)) => std::panic::resume_unwind(cleanup_panic),
                (Ok(()), Ok(())) => {}
            }
        }
    };

    expanded.into()
}
