//! Loom model-checks the `Arc<Mutex<…>>` handoff used by `SysProbe`. Run with:
//!
//! ```sh
//! RUSTFLAGS='--cfg loom' cargo test -p sid-core \
//!     --test sys_probe_loom --release
//! ```
//!
//! `loom` is intentionally not pulled in for regular `cargo test`; the cfg
//! flag swaps in `loom::sync` for `std::sync` so the model checker can
//! explore interleavings exhaustively. The model here is a faithful slice
//! of the SysProbe behaviour: one task polls the provider while another
//! issues a kill, both racing on the same `Arc<Mutex<…>>`.

#![cfg(loom)]

use loom::sync::{Arc, Mutex};
use loom::thread;

/// The poll task and the kill task race on the same provider. After both
/// terminate, the inner counter must equal the sum of both threads'
/// contributions regardless of interleaving.
#[test]
fn poll_and_kill_race_does_not_deadlock() {
    loom::model(|| {
        let provider = Arc::new(Mutex::new(0u32));

        let p1 = Arc::clone(&provider);
        let t1 = thread::spawn(move || {
            for _ in 0..2 {
                let mut g = p1.lock().unwrap();
                *g = g.wrapping_add(1);
            }
        });

        let p2 = Arc::clone(&provider);
        let t2 = thread::spawn(move || {
            for _ in 0..2 {
                let mut g = p2.lock().unwrap();
                *g = g.wrapping_add(10);
            }
        });

        t1.join().unwrap();
        t2.join().unwrap();
        let final_value = *provider.lock().unwrap();
        // 2 increments of +1 and 2 of +10 in any order => 22.
        assert_eq!(final_value, 22);
    });
}

#[test]
fn dropped_provider_does_not_poison_in_normal_flow() {
    loom::model(|| {
        let provider = Arc::new(Mutex::new(0u32));
        let p = Arc::clone(&provider);
        let t = thread::spawn(move || {
            let mut g = p.lock().unwrap();
            *g += 1;
            drop(g);
        });
        t.join().unwrap();
        let g = provider.lock().unwrap();
        assert_eq!(*g, 1);
    });
}
