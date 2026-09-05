#![forbid(unsafe_code)]

//! Portable A.P.C. core primitives.
//!
//! This crate is intentionally small at the start of implementation. It contains
//! only semantics that already have strong architectural support and keeps
//! unresolved format, hierarchy, lifecycle, sequence and cryptographic choices
//! behind future modules rather than freezing them accidentally.

pub mod error;
pub mod finalization;
pub mod id;
pub mod merge;
pub mod scalar;
pub mod state;
pub mod working;

pub use error::CoreError;
pub use finalization::{FinalizationLedger, FinalizationSnapshot, FinalizedStatement};
pub use id::{AtomId, ContinuumId, ReplicaId, RevisionId, WorkingEpochId};
pub use merge::MergeState;
pub use scalar::{ScalarRegister, ScalarRevision};
pub use state::{AtomMap, ContinuumState};
pub use working::{WorkingEpoch, WorkingScalar, WorkingSnapshot};
