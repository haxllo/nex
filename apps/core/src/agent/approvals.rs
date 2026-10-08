//! Pending tool-approval mailbox: the agent loop parks a call id here,
//! the overlay UI answers via `agentApprove`/`agentDeny` IPC, and
//! `runtime_loop` forwards the decision back into the waiter.
//!
//! Single-shot: each id resolves at most once. Unknown or already-used
//! ids return `false` and never panic. The 120s default-deny timeout is
//! enforced by the waiter (`recv_timeout`), not here.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn mailbox() -> &'static Mutex<HashMap<String, std::sync::mpsc::Sender<bool>>> {
    static MAILBOX: OnceLock<Mutex<HashMap<String, std::sync::mpsc::Sender<bool>>>> =
        OnceLock::new();
    MAILBOX.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Park `call_id` and return the receiver the loop waits on.
/// Overwrites any previous waiter for the same id.
pub(crate) fn request_approval(call_id: String) -> std::sync::mpsc::Receiver<bool> {
    let (tx, rx) = std::sync::mpsc::channel();
    if let Ok(mut m) = mailbox().lock() {
        m.insert(call_id, tx);
    }
    rx
}

/// Deliver the UI decision; `false` for unknown/expired ids. Never panics.
pub(crate) fn resolve_approval(call_id: &str, approved: bool) -> bool {
    let tx = mailbox()
        .lock()
        .ok()
        .and_then(|mut m| m.remove(call_id));
    match tx {
        Some(tx) => tx.send(approved).is_ok(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn unique_id(prefix: &str) -> String {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        format!(
            "{prefix}-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            n
        )
    }

    #[test]
    fn request_then_resolve_true_delivers_true() {
        let id = unique_id("deliver");
        let rx = request_approval(id.clone());
        assert!(resolve_approval(&id, true));
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            true
        );
    }

    #[test]
    fn resolve_unknown_id_returns_false() {
        assert!(!resolve_approval(&unique_id("unknown"), true));
        assert!(!resolve_approval(&unique_id("unknown"), false));
    }

    #[test]
    fn second_resolve_of_same_id_returns_false() {
        let id = unique_id("single-shot");
        let rx = request_approval(id.clone());
        assert!(resolve_approval(&id, false));
        assert!(!resolve_approval(&id, true));
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            false
        );
    }
}
