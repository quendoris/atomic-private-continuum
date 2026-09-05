#![forbid(unsafe_code)]

//! Portable A.P.C. core primitives.
//!
//! This crate is intentionally small at the start of implementation. It contains
//! only semantics that already have strong architectural support and keeps
//! unresolved format, hierarchy, lifecycle, sequence and cryptographic choices
//! behind future modules rather than freezing them accidentally.

pub mod error;
pub mod id;
pub mod scalar;

pub use error::CoreError;
pub use id::{AtomId, ContinuumId, ReplicaId, RevisionId};
pub use scalar::{ScalarRegister, ScalarRevision};
