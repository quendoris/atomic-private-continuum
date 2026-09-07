#![forbid(unsafe_code)]

//! Platform-neutral A.P.C. runtime composition.
//!
//! This crate is intentionally above the portable semantic/sync layers. It owns
//! adapter composition that should be shared by Android and desktop without
//! becoming portable format semantics.

mod catchup;
mod catchup_set;
mod foreground_recovery;
mod publication;
mod publication_set;
mod receive;
mod receive_set;
mod resume;
mod resume_batch;
mod resume_batch_cycle;
mod resume_cycle;
mod resume_recovery_batch;
mod resume_recovery_cycle;
mod trusted_state;
mod trusted_state_set;

use apc_sync::{TransportCursor, TransportCursorCodec};
use apc_transport_github::GitHubCommitOid;

pub use catchup::{
    catch_up_single_scalar_domain, ScalarCatchUpError, ScalarCatchUpOutcome, ScalarCatchUpResult,
    ScalarCatchUpSpec,
};
pub use catchup_set::{
    catch_up_scalar_recovery_state, ScalarRecoveryCatchUpError, ScalarRecoveryCatchUpOutcome,
    ScalarRecoveryCatchUpResult, ScalarRecoveryCatchUpSpec,
};
pub use foreground_recovery::{
    ForegroundRecoveryCycleResult, ForegroundRecoveryCycleSpec, ForegroundRecoveryRuntime,
};
pub use publication::{
    prepare_scalar_handoff, recover_scalar_domain, stage_prepared_scalar_handoff,
    PreparedScalarHandoff, ProtectedPublication, ScalarHandoffStageError,
    ScalarPublicationPrepareError, ScalarRecoveryError, TrustedStateCodec,
};
pub use publication_set::{
    prepare_recovery_handoff, stage_prepared_recovery_handoff, PreparedRecoveryHandoff,
    RecoveryHandoffStageError, RecoveryPublicationPrepareError,
};
pub use receive::{
    commit_received_scalar_domain, decode_complete_scalar_domain_objects,
    decode_single_scalar_domain_object, ReceivedScalarState, ScalarObjectDecodeError,
    ScalarReceiveCommitError, ScalarReceiveResult,
};
pub use receive_set::{
    commit_received_recovery_scalar_domain, ReceivedRecoveryScalarState,
    RecoveryScalarReceiveCommitError, RecoveryScalarReceiveResult,
};
pub use resume::{
    resume_single_scalar_domain, ScalarResumeError, ScalarResumeOutboxOutcome, ScalarResumeReport,
};
pub use resume_batch::{
    resume_scalar_outbox_set, ScalarBatchResumeAction, ScalarBatchResumeError,
    ScalarBatchResumeReport, ScalarBatchResumeResult,
};
pub use resume_batch_cycle::{resume_scalar_outbox_set_bounded, ScalarBatchResumeCycleReport};
pub use resume_cycle::{resume_single_scalar_domain_bounded, ScalarResumeCycleReport};
pub use resume_recovery_batch::{
    resume_scalar_recovery_outbox_set, PendingRecoveryPublicationDecodeError,
    ScalarRecoveryResumeAction, ScalarRecoveryResumeError, ScalarRecoveryResumeReport,
    ScalarRecoveryResumeResult, ScalarRecoveryResumeSpec,
};
pub use resume_recovery_cycle::{
    resume_scalar_recovery_outbox_set_bounded, ScalarRecoveryResumeCycleReport,
};
pub use trusted_state::{
    decode_local_scalar_snapshot, encode_local_scalar_snapshot, DevelopmentScalarTrustedStateCodec,
    TrustedStateCodecError,
};
pub use trusted_state_set::{
    decode_multi_scalar_recovery_state, encode_multi_scalar_recovery_state,
    DevelopmentMultiScalarTrustedStateCodec, LocalScalarRecoveryState,
    LocalScalarRecoveryStateError, MultiScalarTrustedStateCodecError,
};

/// Reversible local crash-recovery codec for GitHub transport revisions.
///
/// GitHub commit identities remain opaque transport bookkeeping. Encoding them as
/// UTF-8 bytes in a `TransportCursor` does not give their lexical or numeric form
/// any causal, temporal or merge meaning.
#[derive(Clone, Copy, Debug, Default)]
pub struct GitHubCursorCodec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitHubCursorCodecError {
    InvalidUtf8,
    InvalidCommitIdentity,
    InvalidTransportCursor,
}

impl core::fmt::Display for GitHubCursorCodecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidUtf8 => write!(f, "GitHub transport cursor is not valid UTF-8"),
            Self::InvalidCommitIdentity => {
                write!(f, "GitHub transport cursor is not a valid commit identity")
            }
            Self::InvalidTransportCursor => write!(
                f,
                "GitHub commit identity cannot be encoded as a transport cursor"
            ),
        }
    }
}

impl std::error::Error for GitHubCursorCodecError {}

impl TransportCursorCodec<GitHubCommitOid> for GitHubCursorCodec {
    type Error = GitHubCursorCodecError;

    fn encode(&self, revision: &GitHubCommitOid) -> Result<TransportCursor, Self::Error> {
        TransportCursor::new(revision.as_str().as_bytes().to_vec())
            .map_err(|_| GitHubCursorCodecError::InvalidTransportCursor)
    }

    fn decode(&self, cursor: &TransportCursor) -> Result<GitHubCommitOid, Self::Error> {
        let text = std::str::from_utf8(cursor.as_bytes())
            .map_err(|_| GitHubCursorCodecError::InvalidUtf8)?;
        GitHubCommitOid::new(text.to_owned())
            .map_err(|_| GitHubCursorCodecError::InvalidCommitIdentity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_cursor_round_trip_preserves_opaque_identity_exactly() {
        let codec = GitHubCursorCodec;
        let oid = GitHubCommitOid::new("0123456789abcdef0123456789abcdef01234567").unwrap();

        let cursor = codec.encode(&oid).unwrap();
        assert_eq!(cursor.as_bytes(), oid.as_str().as_bytes());
        assert_eq!(codec.decode(&cursor).unwrap(), oid);
    }

    #[test]
    fn non_utf8_cursor_fails_closed() {
        let codec = GitHubCursorCodec;
        let cursor = TransportCursor::new(vec![0xff, 0xfe]).unwrap();

        assert_eq!(
            codec.decode(&cursor),
            Err(GitHubCursorCodecError::InvalidUtf8)
        );
    }

    #[test]
    fn invalid_ascii_commit_identity_fails_closed() {
        let codec = GitHubCursorCodec;
        let cursor = TransportCursor::new("é".as_bytes().to_vec()).unwrap();

        assert_eq!(
            codec.decode(&cursor),
            Err(GitHubCursorCodecError::InvalidCommitIdentity)
        );
    }
}
