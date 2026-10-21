//! A panic is a bug in sluice, so it ends the process with the exit code of `Bug` and a message in
//! the same shape as every other failure, and not with Rust's 101 and a stack trace.
//!
//! `catch` returns the panic to `run`, which prints it through the output module. That runs the
//! destructors on the way out, so a temporary file or a lock is cleaned up as on any other failure.
//! A hook that called `process::exit` itself would skip them. The cost is that only a panic on the
//! main thread is caught. Nothing here starts another thread yet.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};

/// Replaces the message Rust prints for a panic, because `run` prints it in sluice's own shape.
/// With `RUST_BACKTRACE` set the default message and backtrace are kept, so a developer still sees
/// where it happened.
pub fn install_hook() {
    let default = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if std::env::var_os("RUST_BACKTRACE").is_some_and(|value| value != "0") {
            default(info);
        }
    }));
}

/// Runs `work`, and if it panics returns the panic message.
pub fn catch<T>(work: impl FnOnce() -> T) -> Result<T, String> {
    panic::catch_unwind(AssertUnwindSafe(work)).map_err(|payload| message_of(payload.as_ref()))
}

fn message_of(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "the panic carried no message".to_string()
    }
}

/// Panics with the message in `SLUICE_TEST_PANIC`, so a test can drive the panic path of the real
/// binary. It exists only when debug assertions are on, which a release build never has, so no
/// user can reach it.
#[cfg(debug_assertions)]
pub fn panic_if_asked() {
    if let Some(message) = std::env::var_os("SLUICE_TEST_PANIC") {
        panic!("{}", message.to_string_lossy());
    }
}

#[cfg(test)]
mod tests {
    use super::catch;

    #[test]
    fn a_value_comes_back_and_a_panic_comes_back_as_its_message() {
        assert_eq!(catch(|| 7), Ok(7));
        assert_eq!(catch(|| panic!("a literal")).unwrap_err(), "a literal");
        let formatted = 3;
        assert_eq!(
            catch(|| panic!("number {formatted}")).unwrap_err(),
            "number 3"
        );
        assert_eq!(
            catch(|| std::panic::panic_any(5u8)).unwrap_err(),
            "the panic carried no message"
        );
    }
}
