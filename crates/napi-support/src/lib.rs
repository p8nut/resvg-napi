//! Runtime pieces a napi-rs binding needs when the library it wraps is driven
//! by untrusted input. Nothing here knows which library that is: the binding
//! keeps its own `#[napi]` surface and calls into these.
//!
//! - [`contained`]: a panic becomes a JS error instead of aborting the process.
//! - [`logs`]: what the wrapped crates report through `log`, bounded, globally
//!   and per document.
//! - [`fs`]: reads of caller-named files and gzip data, capped.
//! - [`Signal`]: an `AbortSignal` argument, checked before napi converts it.
//!
//! No Rust tests: a test binary cannot link napi's symbols outside Node. The
//! binding's JS suite covers each piece through the API that uses it.

use napi::bindgen_prelude::*;

/// Runs a call into the wrapped library, turning a panic into a JS error. An
/// unwind reaching napi's `extern "C"` frame aborts the whole process, and
/// these are the paths an untrusted input drives. `what` names the caller in
/// the message: `"<what> panicked: <payload>"`.
pub fn contained<T>(what: &str, f: impl FnOnce() -> T) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|p| {
        // The payload is what a bug report needs; panic!() makes it a &str or
        // a String, anything else is left out.
        let why = p
            .downcast_ref::<&str>()
            .copied()
            .or(p.downcast_ref::<String>().map(String::as_str));
        Error::from_reason(match why {
            Some(why) => format!("{what} panicked: {why}"),
            None => format!("{what} panicked"),
        })
    })
}

pub mod logs {
    //! The wrapped crates report recoverable problems through `log`. Nothing
    //! consumes it by default, so those messages are lost; this buffers them,
    //! for the whole process ([`take`]) and per document ([`Logs`]).

    use napi::{Error, Result};
    use std::sync::{Arc, Mutex, OnceLock};

    /// Entries kept per buffer, the truncation marker included.
    pub const CAP: usize = 500;

    static LOGS: Mutex<Vec<String>> = Mutex::new(Vec::new());
    /// The name the truncation marker carries: the binding's, from [`set_level`].
    static OWNER: OnceLock<&'static str> = OnceLock::new();

    /// Appends `line` unless the buffer is full. The last slot is kept for a
    /// marker saying so: 500 lines with nothing after them would read as
    /// everything there was.
    fn push_capped(buf: &mut Vec<String>, line: String) {
        if buf.len() + 1 < CAP {
            buf.push(line);
        } else if buf.len() + 1 == CAP {
            let owner = OWNER.get().copied().unwrap_or("napi-support");
            buf.push(format!(
                "WARN {owner}: log buffer full ({CAP} entries), later messages dropped"
            ));
        }
    }

    thread_local! {
        /// Set while a document parses or renders on this thread, so its
        /// messages can be told apart from a concurrent render's.
        static SINK: std::cell::RefCell<Option<Vec<String>>> =
            const { std::cell::RefCell::new(None) };
    }

    /// One document's messages, shared with the clones its async work runs on.
    #[derive(Default, Clone)]
    pub struct Logs(Arc<Mutex<Vec<String>>>);

    impl Logs {
        /// Runs `f` with this thread's messages routed here as well.
        pub fn scope<T>(&self, f: impl FnOnce() -> T) -> T {
            let outer = SINK.with(|s| s.replace(Some(Vec::new())));
            let out = f();
            let got = SINK.with(|s| s.replace(outer)).unwrap_or_default();
            let mut buf = self.0.lock().unwrap_or_else(|e| e.into_inner());
            for line in got {
                push_capped(&mut buf, line);
            }
            out
        }

        /// Drains this document's messages.
        pub fn take(&self) -> Vec<String> {
            std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()))
        }
    }

    struct Collector;
    static COLLECTOR: Collector = Collector;

    impl log::Log for Collector {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }

        fn log(&self, record: &log::Record) {
            let line = format!("{} {}: {}", record.level(), record.target(), record.args());
            SINK.with(|s| {
                if let Some(buf) = s.borrow_mut().as_mut() {
                    push_capped(buf, line.clone());
                }
            });
            // Bounded: a pathological input must not grow this forever.
            // Poison-tolerant like every other diagnostics lock: a panic
            // elsewhere must not switch logging off for the process.
            push_capped(&mut LOGS.lock().unwrap_or_else(|e| e.into_inner()), line);
        }

        fn flush(&self) {}
    }

    /// Starts collecting at `level` (`off`, `error`, `warn`, `info`, `debug`,
    /// `trace`). Safe to call repeatedly: the logger is installed once, the
    /// level always applies. `owner` names the binding in the truncation
    /// marker; pass `env!("CARGO_PKG_NAME")`.
    pub fn set_level(level: &str, owner: &'static str) -> Result<()> {
        let filter: log::LevelFilter = level
            .parse()
            .map_err(|_| Error::from_reason(format!("unknown log level: {level:?}")))?;
        OWNER.get_or_init(|| owner);
        static INSTALLED: OnceLock<()> = OnceLock::new();
        INSTALLED.get_or_init(|| {
            let _ = log::set_logger(&COLLECTOR);
        });
        log::set_max_level(filter);
        Ok(())
    }

    /// Drains the messages collected since the last call, from every document
    /// and thread.
    pub fn take() -> Vec<String> {
        std::mem::take(&mut *LOGS.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

pub mod fs {
    //! Reads whose size the caller does not control: a file named by an
    //! untrusted document, and gzip data that inflates by a factor of 1000.

    /// Largest file read, or gzip data inflated: 256 MiB.
    pub const MAX_BYTES: u64 = 256 * 1024 * 1024;

    /// The bytes of `path` if it is a regular file of at most [`MAX_BYTES`].
    /// Opened once and judged on the descriptor, so a file swapped for a FIFO
    /// or a device after the caller's checks is refused instead of blocking
    /// (`O_NONBLOCK`), and a last component swapped for a symlink is not
    /// followed (`O_NOFOLLOW`). Unix only: on WASI and Windows the path is
    /// checked before the open instead, which leaves that window open.
    ///
    /// A directory higher up swapped for a symlink between the caller's check
    /// and the open is still followed: closing that needs
    /// `openat2(RESOLVE_BENEATH)` or cap-std.
    pub fn read_regular(path: &std::path::Path) -> Option<Vec<u8>> {
        use std::io::Read;
        let mut open = std::fs::OpenOptions::new();
        open.read(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::custom_flags(
            &mut open,
            libc::O_NOFOLLOW | libc::O_NONBLOCK,
        );
        // No O_NONBLOCK elsewhere (WASI's flag is unstable in std): judge the
        // path before opening -- a FIFO would block the open itself. Racy, so
        // a swap after this check still blocks there.
        #[cfg(not(unix))]
        if !path.is_file() {
            return None;
        }
        let file = open.open(path).ok()?;
        let meta = file.metadata().ok()?;
        if !meta.is_file() || meta.len() > MAX_BYTES {
            return None;
        }
        // The size is the descriptor's at open time; a file still growing is
        // cut at the cap plus one byte, and that byte refuses it.
        let mut data = Vec::with_capacity(meta.len() as usize);
        file.take(MAX_BYTES + 1).read_to_end(&mut data).ok()?;
        (data.len() as u64 <= MAX_BYTES).then_some(data)
    }

    /// Inflates gzip data, refusing it past [`MAX_BYTES`]: an unbounded
    /// `read_to_end` lets 20 MB grow to 20 GB and abort the process. Over the
    /// cap is `FileTooLarge`; malformed data is the decoder's own error.
    pub fn gunzip_capped(data: &[u8]) -> std::io::Result<Vec<u8>> {
        use std::io::Read;
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(data)
            .take(MAX_BYTES + 1)
            .read_to_end(&mut out)?;
        if out.len() as u64 > MAX_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::FileTooLarge,
                format!("expands past {MAX_BYTES} bytes"),
            ));
        }
        Ok(out)
    }
}

/// An `AbortSignal` argument, checked before napi converts it: it must be an
/// instance of the global `AbortSignal`, and not already aborted -- napi only
/// listens for the `abort` event, so a signal aborted before the call would
/// let the whole task run. Take `Option<Signal>` and hand `.0` to
/// `AsyncTask::with_optional_signal`.
pub struct Signal(pub AbortSignal);

impl FromNapiValue for Signal {
    unsafe fn from_napi_value(
        env: napi::sys::napi_env,
        value: napi::sys::napi_value,
    ) -> Result<Self> {
        let mut global = std::ptr::null_mut();
        napi::check_status!(unsafe { napi::sys::napi_get_global(env, &mut global) })?;
        let mut ctor = std::ptr::null_mut();
        napi::check_status!(unsafe {
            napi::sys::napi_get_named_property(env, global, c"AbortSignal".as_ptr(), &mut ctor)
        })?;
        let mut is = false;
        napi::check_status!(unsafe { napi::sys::napi_instanceof(env, value, ctor, &mut is) })?;
        if !is {
            return Err(Error::new(
                Status::InvalidArg,
                "signal must be an AbortSignal",
            ));
        }
        let mut aborted = std::ptr::null_mut();
        napi::check_status!(unsafe {
            napi::sys::napi_get_named_property(env, value, c"aborted".as_ptr(), &mut aborted)
        })?;
        let mut already = false;
        napi::check_status!(unsafe { napi::sys::napi_get_value_bool(env, aborted, &mut already) })?;
        if already {
            return Err(Error::new(
                Status::Cancelled,
                "AbortError: the signal was already aborted",
            ));
        }
        unsafe { AbortSignal::from_napi_value(env, value) }.map(Signal)
    }
}
