#![forbid(unsafe_code)]

//! Transport-independent synchronization primitives for A.P.C.
//!
//! Semantic projections contain merge-domain state only. Publication identities,
//! multipart indexes and transport revision markers remain envelope/bookkeeping
//! concerns and never participate in causal or merge ordering.

mod codec;
mod projection;
mod protected;
mod transport;
mod wire;

pub use codec::{decode_scalar_projection, encode_scalar_projection, SyncCodecError};
pub use projection::{
    DirtyDomainState, DomainKey, ProjectionError, ScalarDirtyDomainState, ScalarSyncProjection,
    SyncProjection,
};
pub use protected::{
    protect_scalar_part, unprotect_scalar_part, MultipartInbox, ProtectedSyncPart, PublicationId,
    SyncPartError,
};
pub use transport::{FetchOutcome, OpaqueTransport, PublishOutcome};
pub use wire::{decode_protected_sync_part, encode_protected_sync_part, ProtectedPartCodecError};
