//! Cancellable background work that isn't a package install: Composer runs and app
//! installs (ADR 0020). One task per target at a time (a project folder, say).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct Tasks {
    /// Operation id → (target, cancel flag).
    running: Mutex<HashMap<String, (String, Arc<AtomicBool>)>>,
}

impl Tasks {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (String, Arc<AtomicBool>)>> {
        self.running.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether something is already running for `target`.
    pub fn busy(&self, target: &str) -> bool {
        self.lock()
            .values()
            .any(|(t, _)| t.eq_ignore_ascii_case(target))
    }

    /// Registers a task; `None` when another one runs for the same target.
    pub fn register(&self, operation_id: &str, target: &str) -> Option<Arc<AtomicBool>> {
        let mut running = self.lock();
        if running
            .values()
            .any(|(t, _)| t.eq_ignore_ascii_case(target))
        {
            return None;
        }
        let flag = Arc::new(AtomicBool::new(false));
        running.insert(
            operation_id.to_owned(),
            (target.to_owned(), Arc::clone(&flag)),
        );
        Some(flag)
    }

    pub fn finish(&self, operation_id: &str) {
        self.lock().remove(operation_id);
    }

    /// Requests cancellation. False when the operation isn't one of these tasks.
    pub fn cancel(&self, operation_id: &str) -> bool {
        match self.lock().get(operation_id) {
            Some((_, flag)) => {
                flag.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_task_per_target_and_cancel_only_known_ones() {
        let t = Tasks::default();
        let flag = t.register("op1", r"C:\www\shop").unwrap();
        assert!(t.busy(r"c:\WWW\shop"));
        assert!(t.register("op2", r"C:\WWW\SHOP").is_none());
        assert!(t.register("op3", r"C:\www\blog").is_some());
        assert!(!t.cancel("nope"));
        assert!(t.cancel("op1") && flag.load(Ordering::SeqCst));
        t.finish("op1");
        assert!(!t.busy(r"C:\www\shop"));
    }
}
