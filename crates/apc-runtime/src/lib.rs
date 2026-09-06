#![forbid(unsafe_code)]

//! Platform-neutral A.P.C. runtime composition.
//!
//! This crate is intentionally above the portable semantic/sync layers. It owns
//! adapter composition that should be shared by Android and desktop without
//! becoming portable format semantics.

mod publication;
mod trusted_state;

use apc_sync::{TransportCursor, TransportCursorCodec};
use apc_transport_github::GitHubCommitOid;

pub use publication::{
    prepare_scalar_handoff, recover_scalar_domain, stage_prepared_scalar_handoff,
    PreparedScalarHandoff, ProtectedPublication, ScalarHandoffStageError,
    ScalarPublicationPrepareError, ScalarRecoveryError, TrustedStateCodec,
};
pub use trusted_state::{
    decode_local_scalar_snapshot, encode_local_scalar_snapshot, DevelopmentScalarTrustedStateCodec,
    TrustedStateCodecError,
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
