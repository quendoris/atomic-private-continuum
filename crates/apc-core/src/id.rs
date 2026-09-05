use core::fmt;

/// Width used by the first implementation for opaque logical identities.
///
/// This is an implementation choice consistent with the current architecture,
/// not a statement that every future content identifier must use this exact
/// representation.
pub const LOGICAL_ID_BYTES: usize = 32;

macro_rules! opaque_id {
    ($name:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; LOGICAL_ID_BYTES]);

        impl $name {
            pub const fn from_bytes(bytes: [u8; LOGICAL_ID_BYTES]) -> Self {
                Self(bytes)
            }

            pub const fn as_bytes(&self) -> &[u8; LOGICAL_ID_BYTES] {
                &self.0
            }

            pub const fn into_bytes(self) -> [u8; LOGICAL_ID_BYTES] {
                self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "("))?;
                for byte in self.0.iter().take(4) {
                    write!(f, "{byte:02x}")?;
                }
                write!(f, "…)")
            }
        }
    };
}

opaque_id!(ContinuumId);
opaque_id!(AtomId);
opaque_id!(ReplicaId);
opaque_id!(RevisionId);

// Device-local crash-recovery identity for one pending working epoch. This is
// deliberately distinct from `RevisionId`: it is not portable causal identity
// and must never participate in merge ordering.
opaque_id!(WorkingEpochId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_order_is_unsigned_lexicographic_bytes() {
        let mut lower = [0_u8; LOGICAL_ID_BYTES];
        let mut higher = [0_u8; LOGICAL_ID_BYTES];
        lower[0] = 0x7f;
        higher[0] = 0x80;

        assert!(RevisionId::from_bytes(lower) < RevisionId::from_bytes(higher));
    }

    #[test]
    fn typed_ids_do_not_interchange_implicitly() {
        let bytes = [7_u8; LOGICAL_ID_BYTES];
        let atom = AtomId::from_bytes(bytes);
        let revision = RevisionId::from_bytes(bytes);
        let working_epoch = WorkingEpochId::from_bytes(bytes);

        assert_eq!(atom.into_bytes(), revision.into_bytes());
        assert_eq!(working_epoch.into_bytes(), revision.into_bytes());
    }
}
