#![forbid(unsafe_code)]

//! Transport-independent synchronization primitives for A.P.C.
//!
//! Semantic projections contain merge-domain state only. Publication identities,
//! multipart indexes and transport revision markers remain envelope/bookkeeping
//! concerns and never participate in causal or merge ordering.

mod codec;
mod projection;
mod protected;
mod recovery;
mod session;
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
pub use recovery::{
    decode_durable_sync_record, encode_durable_sync_record, DurableOutboxEntry, DurableSyncRecord,
    SyncRecoveryError, TransportCursor,
};
pub use session::{
    commit_received, commit_reconciled_outbox, fetch_from_durable_cursor, publish_staged,
    stage_outbound, PersistTransitionError, SessionCommitError, SessionIoError, SyncRecordStore,
    TransportCursorCodec,
};
pub use transport::{FetchOutcome, OpaqueTransport, PublishOutcome};
pub use wire::{decode_protected_sync_part, encode_protected_sync_part, ProtectedPartCodecError};
