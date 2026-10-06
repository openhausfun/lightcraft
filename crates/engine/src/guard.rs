//! Last-resort panic guard (never-crash standard, `AGENTS.md`): a panic that escapes a command
//! becomes that command's error and the session (catalog, library, undo history) stays usable.
//! It's a safety net, not a way to report errors: commands return `Err` themselves.

use std::any::Any;
use std::io::Write;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;

/// The text of a panic payload (`panic!("…")` gives a `&str` or a `String`).
pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    match (payload.downcast_ref::<&str>(), payload.downcast_ref::<String>()) {
        (Some(s), _) => (*s).to_string(),
        (_, Some(s)) => s.clone(),
        _ => "unknown panic".into(),
    }
}

/// Run `f`; a panic inside it becomes `Err("<what> failed unexpectedly: <message>")`. On wasm32
/// (built with `panic = "abort"`) nothing can be caught and this just runs `f`.
pub fn catch<T>(what: &str, f: impl FnOnce() -> T) -> Result<T, String> {
    panic::catch_unwind(AssertUnwindSafe(f)).map_err(|p| {
        let msg = format!("{what} failed unexpectedly: {}", panic_message(p.as_ref()));
        log::error!("{msg}");
        msg
    })
}

/// Install a panic hook that keeps the default report (stderr) and also appends the panic's
/// location and message to `log` (best effort), so a panic in a GUI session leaves a trace.
pub fn install_hook(log: PathBuf) {
    let default = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        default(info);
        let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
            let _ = writeln!(f, "{secs} [{thread}] panic at {at}: {}", panic_message(info.payload()));
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_becomes_an_error() {
        assert_eq!(catch("ok", || 7), Ok(7));
        let e = catch("`demo.cmd`", || -> u32 { panic!("boom {}", 1) }).unwrap_err();
        assert_eq!(e, "`demo.cmd` failed unexpectedly: boom 1");
        assert!(catch("x", || std::panic::panic_any(5u8)).unwrap_err().ends_with("unknown panic"));
    }
}
