//! Explicit non-real legacy bootstrap or connect-only fresh initialization/verification.
mod bootstrap_runtime;

fn main() -> std::process::ExitCode {
    bootstrap_runtime::main()
}
