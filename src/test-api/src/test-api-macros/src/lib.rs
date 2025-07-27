use proc_macro::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
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
/// - Requires Tokio runtime (`#[tokio::test]`) test execution.
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

    // Generate setup and teardown calls
    let setup_calls = (0..service_exprs.len()).map(|i| {
        let ident = syn::Ident::new(&format!("__svc_{i}"), proc_macro2::Span::call_site());
        quote! {
            #ident.set_up().await;
        }
    });

    let teardown_calls = (0..service_exprs.len()).map(|i| {
        let ident = syn::Ident::new(&format!("__svc_{i}"), proc_macro2::Span::call_site());
        quote! {
            #ident.tear_down().await;
        }
    });

    let fn_name = &input_fn.sig.ident;
    let fn_block = &input_fn.block;

    let expanded = quote! {
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

            let __localstack = test_api::localstack::spin_up_localstack_with_services(&__services).await;

            #( #setup_calls )*

            let __test_fn = async #fn_block;
            __test_fn.await;

            #( #teardown_calls )*

            drop(__localstack);
        }
    };

    expanded.into()
}
