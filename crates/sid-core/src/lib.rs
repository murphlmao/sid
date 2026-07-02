//! Core abstractions for sid (GPUI rebuild).
//!
//! Every external library and OS integration hides behind a trait owned here;
//! concrete impls live in their own crates (`sid-ssh`, `sid-term`, ...). GPUI is
//! never named in this crate — it is the pure adapter seam the frontend and the
//! impl crates compile against.

pub mod db;
pub mod ssh;
pub mod sys;
pub mod term;
