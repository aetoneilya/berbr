//! One budget for actual requests across Jira and GitLab, including MR workers.
use std::sync::{Condvar, LazyLock, Mutex};

pub(crate) struct Budget {
    active: Mutex<usize>,
    changed: Condvar,
    limit: usize,
}

impl Budget {
    const fn new(limit: usize) -> Self {
        Self {
            active: Mutex::new(0),
            changed: Condvar::new(),
            limit,
        }
    }

    fn acquire(&self) -> Permit<'_> {
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        while *active >= self.limit {
            active = self.changed.wait(active).unwrap_or_else(|e| e.into_inner());
        }
        *active += 1;
        Permit(self)
    }
}

pub(crate) struct Permit<'a>(&'a Budget);
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut active = self.0.active.lock().unwrap_or_else(|e| e.into_inner());
        *active -= 1;
        self.0.changed.notify_one();
    }
}

pub(crate) fn acquire() -> Permit<'static> {
    static BUDGET: LazyLock<Budget> = LazyLock::new(|| Budget::new(16));
    BUDGET.acquire()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn nested_workers_share_one_request_budget_without_holding_outer_permits() {
        let budget = Budget::new(3);
        let peak = AtomicUsize::new(0);
        let active = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let budget = &budget;
                let active = &active;
                let peak = &peak;
                scope.spawn(move || {
                    std::thread::scope(|requests| {
                        for _ in 0..3 {
                            requests.spawn(|| {
                                let _permit = budget.acquire();
                                let n = active.fetch_add(1, Ordering::SeqCst) + 1;
                                peak.fetch_max(n, Ordering::SeqCst);
                                std::thread::sleep(std::time::Duration::from_millis(2));
                                active.fetch_sub(1, Ordering::SeqCst);
                            });
                        }
                    })
                });
            }
        });
        assert_eq!(peak.load(Ordering::SeqCst), 3);
        assert_eq!(*budget.active.lock().unwrap(), 0);
    }
}
