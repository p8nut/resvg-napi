//! Runtime pieces a napi-rs binding needs when the library it wraps is driven
//! by untrusted input. Nothing here knows which library that is: the binding
//! keeps its own `#[napi]` surface and calls into these.
//!
//! - [`contained`]: a panic becomes a JS error instead of aborting the process.
//! - [`Thrown`]: an error carrying a `code` of the binding's choosing.
//! - [`logs`]: what the wrapped crates report through `log`, bounded,
//!   for the whole process.
//! - [`fs`]: reads of caller-named files, capped.
//! - [`Signal`]: an `AbortSignal` argument, checked before napi converts it.
//! - [`inspect`]: what `console.log` prints for a class instance.
//! - [`Projection`]: a value borrowed out of an `Arc`-held owner, kept alive
//!   with it.
//! - [`Slot`]: a class's value, until a consuming call or `dispose()`, and
//!   the [`Lease`] an async call takes of it.
//! - [`spawn`]: an async call, on a worker thread, as a promise.
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

/// What a generated method throws: a JS `Error` whose `code` is a string of
/// the binding's choosing (napi sets `error.code` from it), not napi's fixed
/// `Status`. A wrapped `Result<T, E>` throws with `E`'s variant name as the
/// code and its `Display` as the message.
pub type Thrown = Error<String>;

/// A [`Thrown`] with this `code` and `message`.
pub fn thrown(code: &str, message: impl std::fmt::Display) -> Thrown {
    Error::new(code.to_string(), message.to_string())
}

/// napi's own error as a [`Thrown`]: its `Status` becomes the code.
pub fn coded(e: Error) -> Thrown {
    Error::new(e.status.as_ref().to_string(), e.reason.clone())
}

/// The error for a number that does not fit the upstream integer: `name` is
/// the JS argument or field.
pub fn invalid(name: &str, value: impl std::fmt::Display) -> Thrown {
    thrown("InvalidArg", format!("invalid {name}: {value}"))
}

/// What a projection resolves from: its owner, type-erased.
type Owner = std::sync::Arc<dyn std::any::Any + Send + Sync>;

/// The way from an owner to a value of a projection.
type Path<T> =
    std::sync::Arc<dyn Fn(&(dyn std::any::Any + Send + Sync)) -> Option<&T> + Send + Sync>;

/// A value borrowed out of an `Arc`-held owner: a node of a tree, a field of
/// a field. A class cannot hold a borrow, so it holds the owner and the path
/// to the value instead, and re-resolves the path on each call. The owner is
/// shared, never written (a copy-on-write `Arc` copies before it writes), so a
/// path that resolved once always does.
pub struct Projection<T: 'static> {
    owner: Owner,
    path: Path<T>,
}

impl<T: 'static> Clone for Projection<T> {
    fn clone(&self) -> Self {
        Projection {
            owner: self.owner.clone(),
            path: self.path.clone(),
        }
    }
}

impl<T: Send + Sync + 'static> Projection<T> {
    /// The owner itself.
    pub fn of(owner: std::sync::Arc<T>) -> Self {
        Projection {
            owner,
            path: std::sync::Arc::new(|o| o.downcast_ref::<T>()),
        }
    }

    /// A value of its own, for a place that hands out a copy.
    pub fn owned(value: T) -> Self {
        Self::of(std::sync::Arc::new(value))
    }
}

impl<T: 'static> Projection<T> {
    /// The value, resolved from the owner.
    pub fn get(&self) -> &T {
        (self.path)(&*self.owner).expect("a projection resolves: its owner is never written")
    }

    /// One step further, or `None` when the step finds nothing.
    pub fn then<U: 'static>(
        &self,
        step: impl Fn(&T) -> Option<&U> + Send + Sync + 'static,
    ) -> Option<Projection<U>> {
        let parent = self.path.clone();
        let p = Projection {
            owner: self.owner.clone(),
            path: std::sync::Arc::new(move |o| parent(o).and_then(&step)),
        };
        (p.path)(&*p.owner).is_some().then_some(p)
    }

    /// The owner, when it is an `O`.
    pub fn owner<O: 'static>(&self) -> Option<&O> {
        self.owner.downcast_ref()
    }
}

/// The error for an instance whose value an earlier call consumed (a method
/// taking `self` in Rust).
pub fn consumed(class: &str) -> Thrown {
    thrown(
        "Consumed",
        format!("this {class} was consumed by an earlier call"),
    )
}

/// The error for an instance `dispose()` emptied.
pub fn disposed(class: &str) -> Thrown {
    thrown("Disposed", format!("this {class} was disposed"))
}

/// The error for an instance whose value an async call holds: a [`Lease`]
/// taken and not back yet.
pub fn leased(class: &str) -> Thrown {
    thrown("Leased", format!("this {class} is in use by an async call"))
}

/// Where a [`Lease`] puts its value back: written on whichever thread the
/// lease ends, read by its slot on the main thread.
type Home<T> = std::sync::Arc<std::sync::Mutex<Option<T>>>;

/// What a class holds its value in: the value until a call consumes it (a
/// method taking `self` in Rust) or `dispose()` drops it. Every call after
/// either throws, `Consumed` or `Disposed`. Dropping a shared value (an
/// `Arc`, a [`Projection`]) drops this instance's reference only: the value
/// lives on while anything else holds it.
///
/// An async call borrows the value with [`Slot::lease`]: it leaves the slot
/// with the task, every call meanwhile throws `Leased`, and the first call
/// after the lease ended finds it back.
pub struct Slot<T> {
    /// A `OnceCell`, so that a `&self` call can put an ended lease's value
    /// back: it is only ever set while empty.
    value: std::cell::OnceCell<T>,
    /// The last lease's home, until its value is back here.
    lent: Option<Home<T>>,
    disposed: bool,
}

impl<T> Slot<T> {
    pub fn new(value: T) -> Self {
        Slot {
            value: std::cell::OnceCell::from(value),
            lent: None,
            disposed: false,
        }
    }

    /// Puts an ended lease's value back. Whether the slot holds a value now.
    fn back(&self) -> bool {
        if self.value.get().is_some() {
            return true;
        }
        let Some(home) = &self.lent else {
            return false;
        };
        // Held elsewhere still: by the lease, or by the call it went with,
        // which lets go once its promise settled ([`spawn`]).
        if std::sync::Arc::strong_count(home) > 1 {
            return false;
        }
        // Poison-tolerant: a lease ends in `Drop`, which a panic may reach.
        let v = home.lock().unwrap_or_else(|e| e.into_inner()).take();
        v.is_some_and(|v| self.value.set(v).is_ok())
    }

    /// Whether the value is out with an async call.
    fn out(&self) -> bool {
        !self.back() && self.lent.is_some()
    }

    fn gone(&self, class: &str) -> Thrown {
        if self.disposed {
            disposed(class)
        } else if self.out() {
            leased(class)
        } else {
            consumed(class)
        }
    }

    /// The value, or what `class` throws without it.
    pub fn get(&self, class: &str) -> std::result::Result<&T, Thrown> {
        self.back();
        self.value.get().ok_or_else(|| self.gone(class))
    }

    /// The value to write, or what `class` throws without it.
    pub fn get_mut(&mut self, class: &str) -> std::result::Result<&mut T, Thrown> {
        if !self.back() {
            return Err(self.gone(class));
        }
        self.lent = None;
        Ok(self.value.get_mut().expect("checked above"))
    }

    /// The value, taken out: every later call throws `Consumed`.
    pub fn take(&mut self, class: &str) -> std::result::Result<T, Thrown> {
        self.get_mut(class)?;
        Ok(self.value.take().expect("checked above"))
    }

    /// The value, lent to an async call until the [`Lease`] ends: every call
    /// meanwhile throws `Leased`.
    pub fn lease(&mut self, class: &str) -> std::result::Result<Lease<T>, Thrown> {
        let value = self.take(class)?;
        let home = Home::default();
        self.lent = Some(home.clone());
        Ok(Lease {
            value: Some(value),
            home,
        })
    }

    /// Drops the value now, not at garbage collection. Harmless twice.
    /// Throws `Leased` while an async call holds the value.
    pub fn dispose(&mut self, class: &str) -> std::result::Result<(), Thrown> {
        if !self.disposed && self.out() {
            return Err(leased(class));
        }
        self.value.take();
        self.lent = None;
        self.disposed = true;
        Ok(())
    }

    /// Whether [`Slot::dispose`] ran.
    pub fn disposed(&self) -> bool {
        self.disposed
    }
}

/// A slot's value, lent to an async call by [`Slot::lease`]. It goes back
/// to the slot when this is dropped, however the call ends: run, failed,
/// panicked, or dropped unrun because its signal fired while it was queued.
/// The slot takes it back only once the call lets go of its [`Lease::hold`]
/// too, so the instance is in use until the promise settles, never less.
pub struct Lease<T> {
    value: Option<T>,
    home: Home<T>,
}

/// What an async call keeps of a [`Lease`] until its promise settled.
pub type Held = std::sync::Arc<dyn std::any::Any + Send + Sync>;

impl<T: Send + 'static> Lease<T> {
    /// A hold on the lease's way home, for [`spawn`].
    pub fn hold(&self) -> Held {
        self.home.clone()
    }
}

impl<T> std::ops::Deref for Lease<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.value
            .as_ref()
            .expect("a lease holds its value until dropped")
    }
}

impl<T> std::ops::DerefMut for Lease<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.value
            .as_mut()
            .expect("a lease holds its value until dropped")
    }
}

impl<T> Drop for Lease<T> {
    fn drop(&mut self) {
        if let Some(v) = self.value.take() {
            *self.home.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
        }
    }
}

/// The worker half of an async call.
type Run<R> = Box<dyn FnOnce() -> std::result::Result<R, Thrown> + Send>;

/// An async call: `run` on a worker thread, its result made JS by `js` on
/// the main thread.
struct Work<R, J> {
    run: Option<Run<R>>,
    js: fn(R) -> std::result::Result<J, Thrown>,
    /// The leases `run` holds, kept until the task is dropped on the main
    /// thread, after its promise settled.
    held: Vec<Held>,
}

impl<R: Send + 'static, J: ToNapiValue + TypeName> Task for Work<R, J> {
    type Output = std::result::Result<R, Thrown>;
    type JsValue = Settled<J>;

    fn compute(&mut self) -> Result<Self::Output> {
        // The closure ends here, and its leases with it, but the slots wait
        // for `held`.
        let run = self.run.take().expect("a task computes once");
        Ok(run())
    }

    fn resolve(&mut self, _: Env, out: Self::Output) -> Result<Self::JsValue> {
        Ok(Settled(out.and_then(self.js)))
    }

    /// Runs on the main thread once the promise settled: resolved, rejected
    /// or aborted while queued (`run` is dropped unrun then). The leases are
    /// back from here on, before any JS sees the settlement.
    fn finally(self, _: Env) -> Result<()> {
        drop(self.run);
        drop(self.held);
        Ok(())
    }
}

/// What a [`Work`] resolves its promise with: the value, or a promise
/// rejected with the [`Thrown`], which the outer one adopts. A rejection
/// through napi would keep only a `Status` for the code.
struct Settled<J>(std::result::Result<J, Thrown>);

impl<J: TypeName> TypeName for Settled<J> {
    fn type_name() -> &'static str {
        J::type_name()
    }

    fn value_type() -> ValueType {
        J::value_type()
    }
}

impl<J: ToNapiValue> ToNapiValue for Settled<J> {
    unsafe fn to_napi_value(env: napi::sys::napi_env, val: Self) -> Result<napi::sys::napi_value> {
        match val.0 {
            Ok(v) => unsafe { J::to_napi_value(env, v) },
            Err(e) => {
                let p = PromiseRaw::<()>::reject(&Env::from_raw(env), e)?;
                unsafe { PromiseRaw::to_napi_value(env, p) }
            }
        }
    }
}

/// An async call: `run` on a worker thread (libuv's pool, or the WASI
/// host's), and the promise of its result, made JS by `js` on the main
/// thread. A [`Thrown`] from either rejects with its `code` and `message`,
/// as the sync call throws. `signal` drops the call while it is queued: it
/// rejects with an `AbortError`, and `run` is dropped unrun, its leases
/// with it. A call already running is not interrupted.
///
/// `held` are the [`Lease::hold`]s of the leases `run` captured: their
/// slots stay in use until the promise settled, even if `run` ended (and
/// dropped them) earlier on its thread.
pub fn spawn<R: Send + 'static, J: ToNapiValue + TypeName>(
    env: Env,
    held: Vec<Held>,
    run: impl FnOnce() -> std::result::Result<R, Thrown> + Send + 'static,
    js: fn(R) -> std::result::Result<J, Thrown>,
    signal: Option<Signal>,
) -> std::result::Result<PromiseRaw<'static, J>, Thrown> {
    let work = Work {
        run: Some(Box::new(run)),
        js,
        held,
    };
    let task = AsyncTask::with_optional_signal(work, signal.map(|s| s.0));
    // SAFETY: `env` is the calling function's, on the main thread.
    let p = unsafe { AsyncTask::to_napi_value(env.raw(), task) }.map_err(coded)?;
    Ok(PromiseRaw::new(env.raw(), p))
}

pub mod logs {
    //! The wrapped crates report recoverable problems through `log`. Nothing
    //! consumes it by default, so those messages are lost; this buffers them
    //! for the whole process ([`take`]).

    use napi::{Error, Result};
    use std::sync::Mutex;

    /// Entries kept, the truncation marker included.
    pub const CAP: usize = 500;

    static LOGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// Appends `line` unless the buffer is full. The last slot is kept for a
    /// marker saying so: 500 lines with nothing after them would read as
    /// everything there was.
    fn push_capped(buf: &mut Vec<String>, line: String) {
        if buf.len() + 1 < CAP {
            buf.push(line);
        } else if buf.len() + 1 == CAP {
            buf.push(format!(
                "WARN {}: log buffer full ({CAP} entries), later messages dropped",
                module_path!()
            ));
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
            // Bounded: a pathological input must not grow this forever.
            // Poison-tolerant like every other diagnostics lock: a panic
            // elsewhere must not switch logging off for the process.
            push_capped(&mut LOGS.lock().unwrap_or_else(|e| e.into_inner()), line);
        }

        fn flush(&self) {}
    }

    /// Starts collecting at `level` (`off`, `error`, `warn`, `info`, `debug`,
    /// `trace`). Safe to call repeatedly: the logger is installed once, the
    /// level always applies.
    pub fn set_level(level: &str) -> Result<()> {
        let filter: log::LevelFilter = level
            .parse()
            .map_err(|_| Error::from_reason(format!("unknown log level: {level:?}")))?;
        // Fails harmlessly after the first call: the logger installs once.
        let _ = log::set_logger(&COLLECTOR);
        log::set_max_level(filter);
        Ok(())
    }

    /// Drains the messages collected since the last call, from every call
    /// and thread.
    pub fn take() -> Vec<String> {
        std::mem::take(&mut *LOGS.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

pub mod fs {
    //! Reads whose size the caller does not control: a file named by an
    //! untrusted document.

    /// Largest file read: 256 MiB.
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
        let ctor = Env::from_raw(env)
            .get_global()?
            .get_named_property_unchecked::<Function>("AbortSignal")?;
        let is = unsafe { Unknown::from_napi_value(env, value) }?.instanceof(ctor)?;
        if !is {
            return Err(Error::new(
                Status::InvalidArg,
                "signal must be an AbortSignal",
            ));
        }
        // An instance, so an object: reading it as one is sound.
        let already: bool = Object::from_raw(env, value).get_named_property("aborted")?;
        if already {
            return Err(Error::new(
                Status::Cancelled,
                "AbortError: the signal was already aborted",
            ));
        }
        unsafe { AbortSignal::from_napi_value(env, value) }.map(Signal)
    }
}

/// Bytes behind any pointer to them -- `Vec<u8>`, `Arc<Vec<u8>>`,
/// `Arc<dyn AsRef<[u8]>>` -- copied into a `Buffer`: the one copy, made
/// when JS reads them.
pub fn copied<D: std::ops::Deref>(d: &D) -> Buffer
where
    D::Target: AsRef<[u8]>,
{
    Buffer::from(d.deref().as_ref())
}

/// The body of a class's `[Symbol.for('nodejs.util.inspect.custom')]`:
/// `Name { key: value, .. }`, formatted by the `inspect` Node passes in, so
/// colours, `depth` and nesting behave like any other object. Without it a
/// class instance prints as `Name {}`: its values live on the prototype.
///
/// `getters` are read, `methods` called with no argument, nulls left out; the
/// binding picks the cheap ones. The printed object stands for `this`, so it does not use a
/// level: it gets the remaining `depth`, and an instance among its values
/// comes back here one level lower.
#[allow(clippy::too_many_arguments)]
pub fn inspect<'e>(
    env: &'e Env,
    this: &Object<'e>,
    class: &str,
    getters: &[&str],
    methods: &[&str],
    depth: Option<f64>,
    options: Object<'e>,
    inspect: Function<'e, FnArgs<(Object<'e>, Object<'e>)>, String>,
) -> Result<String> {
    if depth.is_some_and(|d| d < 0.0) {
        return Ok(format!("[{class}]"));
    }
    match shown(env, this, class, getters, methods, depth, options, inspect) {
        Err(e) if e.status == Status::PendingException => {
            // A getter threw. `Leased` and `Consumed` are states, printed as
            // `disposed` is; anything else is thrown again.
            let mut raw = std::ptr::null_mut();
            unsafe { napi::sys::napi_get_and_clear_last_exception(env.raw(), &mut raw) };
            let thrown = unsafe { Unknown::from_raw_unchecked(env.raw(), raw) };
            let code = unsafe { thrown.cast::<Object>() }
                .and_then(|o| o.get_named_property::<String>("code"))
                .unwrap_or_default();
            if ["Leased", "Consumed"].contains(&code.as_str()) {
                return Ok(format!("{class} {{ {}: true }}", code.to_lowercase()));
            }
            unsafe { napi::sys::napi_throw(env.raw(), raw) };
            Err(e)
        }
        other => other,
    }
}

#[allow(clippy::too_many_arguments)]
fn shown<'e>(
    env: &'e Env,
    this: &Object<'e>,
    class: &str,
    getters: &[&str],
    methods: &[&str],
    depth: Option<f64>,
    options: Object<'e>,
    inspect: Function<'e, FnArgs<(Object<'e>, Object<'e>)>, String>,
) -> Result<String> {
    // `disposed` is printed only once true, and alone: every other getter throws then.
    if getters.contains(&"disposed") && this.get_named_property::<bool>("disposed")? {
        return Ok(format!("{class} {{ disposed: true }}"));
    }
    let mut shown = Object::new(env)?;
    // A null is a member that does not apply here -- a group's opacity on a
    // path -- and five of them bury the two values that do.
    let mut show = |key: &str, value: Unknown<'e>| match value.get_type()? {
        ValueType::Null => Ok(()),
        _ => shown.set_named_property(key, value),
    };
    for key in getters.iter().filter(|k| **k != "disposed") {
        show(key, this.get_named_property(key)?)?;
    }
    for key in methods {
        let method: Function<(), Unknown> = this.get_named_property_unchecked(key)?;
        show(key, method.apply(*this, ())?)?;
    }
    // Node builds `options` afresh for each call, so it is ours to change.
    let mut remaining = options;
    // `null` is Node's "no limit", in and out.
    remaining.set_named_property("depth", depth)?;
    Ok(format!(
        "{class} {}",
        inspect.call((shown, remaining).into())?
    ))
}
