//! Keyed background engine tasks for surfaces that are not part of the
//! report re-analysis queue in [`crate::jobs`] (clock/control-set/SSN/IBIS
//! reports, vectored power, ECO, DFX, OOC, coverage).
//!
//! A task clones its inputs on the UI thread, runs `compute` on a worker
//! thread, and [`poll`] applies the result on the UI thread with `apply`.
//! One task per key runs at a time; [`busy`] lets a button render disabled
//! ("Running…") while its key is in flight.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use crate::state::AppState;

type Apply = Box<dyn FnOnce(&mut AppState) + Send>;

#[derive(Clone)]
struct Task {
    key: &'static str,
    rx: Arc<Mutex<mpsc::Receiver<Apply>>>,
}

/// Background-task slice of [`AppState`].
#[derive(Clone, Default)]
pub struct BgState {
    tasks: Vec<Task>,
}

impl std::fmt::Debug for BgState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.tasks.iter().map(|t| t.key)).finish()
    }
}

/// Directory for user-facing artifacts (IBIS files, partial bitstreams,
/// ECO bitstreams, OOC checkpoints): next to the open `.prj`, else under
/// the temp runs directory.
pub fn output_dir(s: &AppState) -> std::path::PathBuf {
    let prj_dir = s
        .runs
        .source
        .as_deref()
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("prj")))
        .and_then(|p| p.parent())
        .map(|d| d.join("helion_out"));
    prj_dir.unwrap_or_else(|| std::env::temp_dir().join("helion-rune-runs").join("out"))
}

/// Whether a task with `key` is running.
pub fn busy(s: &AppState, key: &str) -> bool {
    s.bg.tasks.iter().any(|t| t.key == key)
}

/// Run `compute` on a worker and fold its result with `apply`. Returns
/// false (and does nothing) when `key` is already running.
pub fn spawn<R: Send + 'static>(
    s: &mut AppState,
    key: &'static str,
    compute: impl FnOnce() -> R + Send + 'static,
    apply: impl FnOnce(&mut AppState, R) + Send + 'static,
) -> bool {
    if busy(s, key) {
        return false;
    }
    let (tx, rx) = mpsc::channel::<Apply>();
    let spawned = std::thread::Builder::new()
        .name(format!("helion-bg-{key}"))
        .spawn(move || {
            let r = compute();
            let _ = tx.send(Box::new(move |s: &mut AppState| apply(s, r)));
        });
    if spawned.is_err() {
        return false;
    }
    s.bg.tasks.push(Task {
        key,
        rx: Arc::new(Mutex::new(rx)),
    });
    true
}

/// Apply finished tasks. Returns true when state changed; keeps the frame
/// clock ticking while tasks are in flight.
pub fn poll(s: &mut AppState, tx: Option<&mut rune::Tx>) -> bool {
    if s.bg.tasks.is_empty() {
        return false;
    }
    let mut done: Vec<(usize, Option<Apply>)> = Vec::new();
    for (i, t) in s.bg.tasks.iter().enumerate() {
        let Ok(rx) = t.rx.lock() else { continue };
        match rx.try_recv() {
            Ok(apply) => done.push((i, Some(apply))),
            Err(mpsc::TryRecvError::Disconnected) => done.push((i, None)),
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
    let changed = !done.is_empty();
    for (i, apply) in done.into_iter().rev() {
        s.bg.tasks.remove(i);
        if let Some(apply) = apply {
            apply(s);
        }
    }
    if !s.bg.tasks.is_empty() {
        if let Some(tx) = tx {
            tx.wake_after(std::time::Duration::from_millis(30));
        }
    }
    changed
}

/// Block until every task finished and was applied (tests and scripted
/// Tcl, which must see the result of the verb it just ran).
pub fn wait_all(s: &mut AppState) {
    let start = std::time::Instant::now();
    while !s.bg.tasks.is_empty() && start.elapsed() < std::time::Duration::from_secs(300) {
        if !poll(s, None) {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_runs_off_thread_and_applies_on_poll() {
        let mut s = AppState::default();
        assert!(spawn(&mut s, "t", || 41 + 1, |s, v: i32| s.hello_on = v == 42));
        assert!(busy(&s, "t"));
        assert!(!spawn(&mut s, "t", || 0, |_, _: i32| {}), "one task per key");
        wait_all(&mut s);
        assert!(!busy(&s, "t"));
        assert!(s.hello_on);
    }
}
