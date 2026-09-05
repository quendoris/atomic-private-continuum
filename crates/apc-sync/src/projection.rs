use std::collections::{BTreeMap, BTreeSet};

use apc_core::{AtomId, CoreError, MergeState, ScalarRegister};

/// Pre-format key for one independently mergeable atom domain.
///
/// The domain bytes identify a semantic merge domain inside one atom. Their final
/// portable namespace/encoding is intentionally not frozen by this type.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DomainKey {
    pub atom_id: AtomId,
    pub domain: Vec<u8>,
}

impl DomainKey {
    pub fn new(atom_id: AtomId, domain: impl Into<Vec<u8>>) -> Result<Self, ProjectionError> {
        let domain = domain.into();
        if domain.is_empty() {
            return Err(ProjectionError::EmptyDomainIdentifier);
        }
        Ok(Self { atom_id, domain })
    }
}

/// Mergeable semantic projection containing domain state only.
///
/// It deliberately has no projection/publication identifier. Publication IDs are
/// envelope/transport bookkeeping and must never participate in semantic merge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncProjection<K, S> {
    domains: BTreeMap<K, S>,
}

impl<K, S> Default for SyncProjection<K, S> {
    fn default() -> Self {
        Self {
            domains: BTreeMap::new(),
        }
    }
}

impl<K: Ord, S> SyncProjection<K, S> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_domains(domains: BTreeMap<K, S>) -> Self {
        Self { domains }
    }

    pub fn domains(&self) -> &BTreeMap<K, S> {
        &self.domains
    }

    pub fn is_empty(&self) -> bool {
        self.domains.is_empty()
    }

    pub fn len(&self) -> usize {
        self.domains.len()
    }

    pub fn get(&self, key: &K) -> Option<&S> {
        self.domains.get(key)
    }
}

impl<K, S> SyncProjection<K, S>
where
    K: Clone + Ord,
    S: Clone + MergeState,
{
    pub fn merge(&self, other: &Self) -> Result<Self, CoreError> {
        let mut merged = self.domains.clone();
        for (key, incoming) in &other.domains {
            match merged.get(key) {
                Some(current) => {
                    merged.insert(key.clone(), current.merge_state(incoming)?);
                }
                None => {
                    merged.insert(key.clone(), incoming.clone());
                }
            }
        }
        Ok(Self { domains: merged })
    }
}

/// Local sync view which separates current domain state from publication dirtiness.
///
/// Importing remote state does not itself mark a clean domain dirty. If a domain
/// already has unpublished local state, importing a concurrent remote projection
/// leaves that dirty marker intact so the local contribution is not lost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirtyDomainState<K, S> {
    domains: BTreeMap<K, S>,
    dirty: BTreeSet<K>,
}

impl<K, S> Default for DirtyDomainState<K, S> {
    fn default() -> Self {
        Self {
            domains: BTreeMap::new(),
            dirty: BTreeSet::new(),
        }
    }
}

impl<K, S> DirtyDomainState<K, S>
where
    K: Clone + Ord,
    S: Clone + Eq + MergeState,
{
    pub fn new() -> Self {
        Self::default()
    }

    pub fn domains(&self) -> &BTreeMap<K, S> {
        &self.domains
    }

    pub fn dirty_keys(&self) -> &BTreeSet<K> {
        &self.dirty
    }

    pub fn get(&self, key: &K) -> Option<&S> {
        self.domains.get(key)
    }

    /// Replaces one locally edited domain state and marks that exact domain dirty.
    pub fn set_local(&mut self, key: K, state: S) {
        self.domains.insert(key.clone(), state);
        self.dirty.insert(key);
    }

    /// Merges an authenticated/validated remote projection without inventing a
    /// new local edit or clearing any pre-existing local dirty marker.
    pub fn import_projection(
        &mut self,
        projection: &SyncProjection<K, S>,
    ) -> Result<(), CoreError> {
        for (key, incoming) in projection.domains() {
            match self.domains.get(key) {
                Some(current) => {
                    self.domains
                        .insert(key.clone(), current.merge_state(incoming)?);
                }
                None => {
                    self.domains.insert(key.clone(), incoming.clone());
                }
            }
        }
        Ok(())
    }

    /// Captures exactly the currently dirty domain states.
    pub fn export_dirty(&self) -> Option<SyncProjection<K, S>> {
        if self.dirty.is_empty() {
            return None;
        }

        let domains = self
            .dirty
            .iter()
            .filter_map(|key| self.domains.get(key).map(|state| (key.clone(), state.clone())))
            .collect();
        Some(SyncProjection::from_domains(domains))
    }

    /// Clears a dirty marker only if the current domain state is still exactly
    /// the state represented by the acknowledged projection.
    pub fn acknowledge(&mut self, projection: &SyncProjection<K, S>) {
        for (key, exported) in projection.domains() {
            if self.domains.get(key) == Some(exported) {
                self.dirty.remove(key);
            }
        }
    }
}

pub type ScalarSyncProjection = SyncProjection<DomainKey, ScalarRegister<Vec<u8>>>;
pub type ScalarDirtyDomainState = DirtyDomainState<DomainKey, ScalarRegister<Vec<u8>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionError {
    EmptyDomainIdentifier,
}

impl core::fmt::Display for ProjectionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyDomainIdentifier => write!(f, "sync domain identifier must not be empty"),
        }
    }
}

impl std::error::Error for ProjectionError {}

#[cfg(test)]
mod tests {
    use super::*;
    use apc_core::{RevisionId, ScalarRegister};

    fn bytes(value: u64) -> [u8; 32] {
        let mut bytes = [0_u8; 32];
        bytes[24..].copy_from_slice(&value.to_be_bytes());
        bytes
    }

    fn atom(value: u64) -> AtomId {
        AtomId::from_bytes(bytes(value))
    }

    fn rid(value: u64) -> RevisionId {
        RevisionId::from_bytes(bytes(value))
    }

    fn key(atom_value: u64, domain: &str) -> DomainKey {
        DomainKey::new(atom(atom_value), domain.as_bytes()).unwrap()
    }

    fn register(revision: u64, value: &str) -> ScalarRegister<Vec<u8>> {
        let mut register = ScalarRegister::new();
        register.assign(rid(revision), value.as_bytes().to_vec()).unwrap();
        register
    }

    #[test]
    fn independent_domains_merge_without_publication_identity() {
        let body = key(1, "body");
        let title = key(1, "title");
        let left = SyncProjection::from_domains(BTreeMap::from([(
            body.clone(),
            register(10, "body"),
        )]));
        let right = SyncProjection::from_domains(BTreeMap::from([(
            title.clone(),
            register(20, "title"),
        )]));

        let merged = left.merge(&right).unwrap();
        assert_eq!(merged.len(), 2);
        assert_eq!(merged.get(&body).unwrap().materialized(), Some(&b"body".to_vec()));
        assert_eq!(
            merged.get(&title).unwrap().materialized(),
            Some(&b"title".to_vec())
        );
    }

    #[test]
    fn acknowledgement_does_not_clear_a_domain_changed_in_flight() {
        let body = key(1, "body");
        let mut state = ScalarDirtyDomainState::new();
        state.set_local(body.clone(), register(10, "A"));
        let exported = state.export_dirty().unwrap();

        let mut changed = exported.get(&body).unwrap().clone();
        changed.assign(rid(20), b"B".to_vec()).unwrap();
        state.set_local(body.clone(), changed);
        state.acknowledge(&exported);

        assert!(state.dirty_keys().contains(&body));
    }

    #[test]
    fn importing_remote_state_preserves_existing_local_dirty_marker() {
        let body = key(1, "body");
        let mut base = ScalarRegister::new();
        base.assign(rid(1), b"base".to_vec()).unwrap();

        let mut local = base.clone();
        local.assign(rid(10), b"local".to_vec()).unwrap();
        let mut remote = base;
        remote.assign(rid(20), b"remote".to_vec()).unwrap();

        let mut state = ScalarDirtyDomainState::new();
        state.set_local(body.clone(), local);
        let projection = SyncProjection::from_domains(BTreeMap::from([(body.clone(), remote)]));
        state.import_projection(&projection).unwrap();

        assert!(state.dirty_keys().contains(&body));
        assert_eq!(
            state.get(&body).unwrap().frontier_ids(),
            BTreeSet::from([rid(10), rid(20)])
        );
    }

    #[test]
    fn clean_remote_import_does_not_become_a_local_publication() {
        let body = key(1, "body");
        let projection = SyncProjection::from_domains(BTreeMap::from([(
            body.clone(),
            register(10, "remote"),
        )]));
        let mut state = ScalarDirtyDomainState::new();
        state.import_projection(&projection).unwrap();

        assert!(state.dirty_keys().is_empty());
        assert!(state.export_dirty().is_none());
        assert_eq!(state.get(&body), projection.get(&body));
    }
}
