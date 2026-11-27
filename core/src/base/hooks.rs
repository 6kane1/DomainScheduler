//! Static hooks (the C# `public static Func<...> XHook` fields) and the test lock for them.
//!
//! A [`Hook<F>`] holds an optional `Arc<F>` (`F` is usually `dyn Fn(..) -> .. + Send + Sync`). C# tests that set a
//! static hook run in the xunit collection "static-hooks" (one at a time); Rust tests run in parallel threads, so a test
//! that sets any static hook or the settings backend takes [`static_hooks`] first and holds it to its end.

use std::sync::{Arc, Mutex, MutexGuard, RwLock};

pub struct Hook<F: ?Sized>(RwLock<Option<Arc<F>>>);

impl<F: ?Sized> Hook<F> {
    pub const fn new() -> Self {
        Hook(RwLock::new(None))
    }

    /// Sets (Some) or clears (None) the hook.
    pub fn set(&self, f: Option<Arc<F>>) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = f;
    }

    /// The hook, if set (cloned out, so it is called without holding the lock).
    pub fn get(&self) -> Option<Arc<F>> {
        self.0.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn is_set(&self) -> bool {
        self.0.read().unwrap_or_else(|e| e.into_inner()).is_some()
    }
}

impl<F: ?Sized> Default for Hook<F> {
    fn default() -> Self {
        Self::new()
    }
}

/// Locks a mutex and ignores poisoning (a panicking test must not break the next one; the product never unwinds).
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

static STATIC_HOOKS: Mutex<()> = Mutex::new(());

/// The xunit collection "static-hooks": a test that changes a static hook (Topo live hook, GameMask hooks, the settings
/// backend, Reservation's registry, AppLog's path ...) holds this guard for its whole run.
pub fn static_hooks() -> MutexGuard<'static, ()> {
    lock(&STATIC_HOOKS)
}
