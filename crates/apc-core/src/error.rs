use core::fmt;

use crate::RevisionId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    DuplicateRevisionConflict {
        revision_id: RevisionId,
    },
    MissingParent {
        revision_id: RevisionId,
        parent_id: RevisionId,
    },
    CausalCycle {
        revision_id: RevisionId,
    },
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateRevisionConflict { revision_id } => {
                write!(f, "revision {revision_id:?} has conflicting statements")
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
        }
    }
}

impl std::error::Error for CoreError {}
