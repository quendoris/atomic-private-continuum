use crate::{CoreError, ScalarRegister};

/// Deterministic state merge for one semantic merge unit.
///
/// Implementations promoted into the portable core are expected to satisfy the
/// applicable A.P.C. merge laws: determinism, commutativity, associativity and
/// idempotence for valid states.
pub trait MergeState: Sized {
    fn merge_state(&self, other: &Self) -> Result<Self, CoreError>;
}

impl<T: Clone + Eq> MergeState for ScalarRegister<T> {
    fn merge_state(&self, other: &Self) -> Result<Self, CoreError> {
        self.merge(other)
    }
}
