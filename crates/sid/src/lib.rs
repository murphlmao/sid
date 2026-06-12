//! Library shim — exposes the `sid` binary's internal modules to integration
//! tests, doc tests, and benchmarks.
//!
//! All business logic lives in [`runtime`] and [`wire`].  This file is a
//! pure re-export; it contains no logic of its own.

pub mod runtime;
pub mod settings_undo;
pub mod toast;
pub mod wire;
