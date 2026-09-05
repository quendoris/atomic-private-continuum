use core::fmt;

use crate::{AtomId, ContinuumId, RevisionId, WorkingEpochId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    DuplicateRevisionConflict {
        revision_id: RevisionId,
    },
    RevisionAlreadyKnown {
        revision_id: RevisionId,
    },
    UnknownRevision {
        revision_id: RevisionId,
    },
    MissingParent {
        revision_id: RevisionId,
        parent_id: RevisionId,
    },
    CausalCycle {
        revision_id: RevisionId,
    },
    DuplicateAtomId {
        atom_id: AtomId,
    },
    ContinuumMismatch {
        left: ContinuumId,
        right: ContinuumId,
    },
    WorkingEpochAlreadyOpen {
        epoch_id: WorkingEpochId,
    },
    NoWorkingEpoch,
    DirtyObservationRequiresSeal,
    UnexpectedPreObservationRevision {
        revision_id: RevisionId,
    },
    InvalidWorkingSnapshot,
    UnknownLocalRevision {
        revision_id: RevisionId,
    },
    FinalizedStatementConflict {
        revision_id: RevisionId,
    },
    HandoffRequiresFinalizedRevision {
        revision_id: RevisionId,
    },
    InvalidFinalizationSnapshot,
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateRevisionConflict { revision_id } => {
                write!(f, "revision {revision_id:?} has conflicting statements")
            }
            Self::RevisionAlreadyKnown { revision_id } => {
                write!(f, "revision identity {revision_id:?} is already known")
            }
            Self::UnknownRevision { revision_id } => {
                write!(f, "revision {revision_id:?} is not present in causal state")
            }
            Self::MissingParent {
                revision_id,
                parent_id,
            } => write!(
                f,
                "revision {revision_id:?} depends on missing parent {parent_id:?}"
            ),
            Self::CausalCycle { revision_id } => {
                write!(f, "causal graph contains a cycle at {revision_id:?}")
            }
            Self::DuplicateAtomId { atom_id } => {
                write!(f, "atom identity {atom_id:?} already exists in this state")
            }
            Self::ContinuumMismatch { left, right } => write!(
                f,
                "cannot merge different continua: left {left:?}, right {right:?}"
            ),
            Self::WorkingEpochAlreadyOpen { epoch_id } => {
                write!(f, "working epoch {epoch_id:?} is already open")
            }
            Self::NoWorkingEpoch => write!(f, "no working epoch is open"),
            Self::DirtyObservationRequiresSeal => {
                write!(
                    f,
                    "dirty working state must be sealed before remote observation"
                )
            }
            Self::UnexpectedPreObservationRevision { revision_id } => write!(
                f,
                "pre-observation revision {revision_id:?} was supplied with no dirty working state"
            ),
            Self::InvalidWorkingSnapshot => write!(
                f,
                "working snapshot pending frontier does not match its causal recovery state"
            ),
            Self::UnknownLocalRevision { revision_id } => {
                write!(f, "revision {revision_id:?} is not registered as local")
            }
            Self::FinalizedStatementConflict { revision_id } => write!(
                f,
                "finalized statement for revision {revision_id:?} does not match"
            ),
            Self::HandoffRequiresFinalizedRevision { revision_id } => write!(
                f,
                "transport handoff depends on unfinalized local revision {revision_id:?}"
            ),
            Self::InvalidFinalizationSnapshot => {
                write!(f, "finalization snapshot contains inconsistent bookkeeping")
            }
        }
    }
}

impl std::error::Error for CoreError {}
