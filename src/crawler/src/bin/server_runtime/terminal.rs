/// Terminal process fence. Callers must finish explicit output/cleanup BEFORE this call.
#[cfg(unix)]
pub(super) fn terminal_exit(code: i32) -> ! {
    // SAFETY: POSIX _exit accepts any integer and no pointers. It terminates the process
    // without Rust stdout cleanup, libc atexit handlers, unwinding, shared cleanup locks,
    // or SIGABRT core collection. No Rust-owned value is accessed afterward. This narrow
    // executable-boundary exception is shared by crawler server and local bootstrap.
    unsafe { libc::_exit(code) }
}

#[cfg(not(unix))]
pub(super) fn terminal_exit(code: i32) -> ! {
    // Only Unix provides the terminal-fence contract. Preserve successful CLI/help behavior
    // on other platforms; watchdog failure there is best-effort and can invoke core handling.
    if code == 0 {
        std::process::exit(0);
    }
    std::process::abort()
}
