/// Backend contract required by the core durability coordinator.
///
/// The physical implementation may use copy-on-write pages, a journal, two
/// manifests, database transactions, or another mechanism. The semantic
/// requirement is only that the operations below provide the documented
/// durability boundaries.
pub trait DurabilityBackend<S> {
    type Candidate;
    type Error;

    /// Returns the last committed state that survived the backend's durability
    /// barrier. Unpublished candidates must not become visible through this
    /// method.
    fn load_committed(&self) -> Result<Option<S>, Self::Error>;

    /// Writes a complete candidate state without making it the committed root.
    fn write_candidate(&mut self, state: &S) -> Result<Self::Candidate, Self::Error>;

    /// Makes the candidate data durable before it can become the committed root.
    fn sync_candidate(&mut self, candidate: &Self::Candidate) -> Result<(), Self::Error>;

    /// Atomically switches the visible committed root to an already durable
    /// candidate. This publication may itself still require a durability barrier.
    fn publish_candidate(&mut self, candidate: &Self::Candidate) -> Result<(), Self::Error>;

    /// Makes the published root durable. Once this method succeeds, a later
    /// power loss must not restore the previous committed root.
    fn sync_committed_root(&mut self) -> Result<(), Self::Error>;
}

/// Commits one complete state using the minimum ordering required by the A.P.C.
/// acknowledgement contract.
///
/// Returning `Ok(())` is the acknowledgement boundary. Therefore every backend
/// implementation must guarantee that a crash after this function returns
/// success can recover `state` as the committed state.
pub fn commit_durable<B, S>(backend: &mut B, state: &S) -> Result<(), B::Error>
where
    B: DurabilityBackend<S>,
{
    let candidate = backend.write_candidate(state)?;
    backend.sync_candidate(&candidate)?;
    backend.publish_candidate(&candidate)?;
    backend.sync_committed_root()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::convert::Infallible;

    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct Handle(u64);

    #[derive(Clone, Debug)]
    struct CrashBackend<S> {
        next_handle: u64,
        volatile_candidates: BTreeMap<Handle, S>,
        durable_candidates: BTreeMap<Handle, S>,
        visible_root: Option<Handle>,
        durable_root: Option<Handle>,
        calls: Vec<&'static str>,
    }

    impl<S: Clone> CrashBackend<S> {
        fn with_committed(state: S) -> Self {
            let baseline = Handle(0);
            Self {
                next_handle: 1,
                volatile_candidates: BTreeMap::new(),
                durable_candidates: BTreeMap::from([(baseline, state)]),
                visible_root: Some(baseline),
                durable_root: Some(baseline),
                calls: Vec::new(),
            }
        }

        /// Simulates a power loss. When `publication_reached_media` is true, an
        /// unsynced root publication is allowed to survive; when false, it is
        /// lost. This models both legal outcomes before the root barrier.
        fn crash(&mut self, publication_reached_media: bool) {
            if publication_reached_media {
                if let Some(root) = self.visible_root {
                    if self.durable_candidates.contains_key(&root) {
                        self.durable_root = Some(root);
                    }
                }
            }
            self.visible_root = self.durable_root;
            self.volatile_candidates.clear();
        }
    }

    impl<S: Clone> DurabilityBackend<S> for CrashBackend<S> {
        type Candidate = Handle;
        type Error = Infallible;

        fn load_committed(&self) -> Result<Option<S>, Self::Error> {
            Ok(self
                .durable_root
                .and_then(|root| self.durable_candidates.get(&root).cloned()))
        }

        fn write_candidate(&mut self, state: &S) -> Result<Self::Candidate, Self::Error> {
            self.calls.push("write_candidate");
            let handle = Handle(self.next_handle);
            self.next_handle += 1;
            self.volatile_candidates.insert(handle, state.clone());
            Ok(handle)
        }

        fn sync_candidate(&mut self, candidate: &Self::Candidate) -> Result<(), Self::Error> {
            self.calls.push("sync_candidate");
            if let Some(state) = self.volatile_candidates.get(candidate).cloned() {
                self.durable_candidates.insert(*candidate, state);
            }
            Ok(())
        }

        fn publish_candidate(&mut self, candidate: &Self::Candidate) -> Result<(), Self::Error> {
            self.calls.push("publish_candidate");
            assert!(self.durable_candidates.contains_key(candidate));
            self.visible_root = Some(*candidate);
            Ok(())
        }

        fn sync_committed_root(&mut self) -> Result<(), Self::Error> {
            self.calls.push("sync_committed_root");
            self.durable_root = self.visible_root;
            Ok(())
        }
    }

    #[test]
    fn commit_protocol_uses_required_order_and_ack_is_durable() {
        let mut backend = CrashBackend::with_committed("old".to_owned());

        commit_durable(&mut backend, &"new".to_owned()).unwrap();

        assert_eq!(
            backend.calls,
            [
                "write_candidate",
                "sync_candidate",
                "publish_candidate",
                "sync_committed_root"
            ]
        );

        backend.crash(false);
        assert_eq!(backend.load_committed().unwrap().as_deref(), Some("new"));
    }

    #[test]
    fn crash_before_candidate_sync_recovers_only_old_committed_state() {
        for publication_reached_media in [false, true] {
            let mut backend = CrashBackend::with_committed("old".to_owned());
            let candidate = backend.write_candidate(&"new".to_owned()).unwrap();
            assert_eq!(candidate, Handle(1));

            backend.crash(publication_reached_media);
            assert_eq!(backend.load_committed().unwrap().as_deref(), Some("old"));
        }
    }

    #[test]
    fn durable_unpublished_candidate_does_not_replace_old_root() {
        for publication_reached_media in [false, true] {
            let mut backend = CrashBackend::with_committed("old".to_owned());
            let candidate = backend.write_candidate(&"new".to_owned()).unwrap();
            backend.sync_candidate(&candidate).unwrap();

            backend.crash(publication_reached_media);
            assert_eq!(backend.load_committed().unwrap().as_deref(), Some("old"));
        }
    }

    #[test]
    fn crash_after_publish_before_root_barrier_may_recover_old_or_new_but_never_hybrid() {
        let mut old_survives = CrashBackend::with_committed("old".to_owned());
        let candidate = old_survives.write_candidate(&"new".to_owned()).unwrap();
        old_survives.sync_candidate(&candidate).unwrap();
        old_survives.publish_candidate(&candidate).unwrap();
        old_survives.crash(false);
        assert_eq!(
            old_survives.load_committed().unwrap().as_deref(),
            Some("old")
        );

        let mut new_survives = CrashBackend::with_committed("old".to_owned());
        let candidate = new_survives.write_candidate(&"new".to_owned()).unwrap();
        new_survives.sync_candidate(&candidate).unwrap();
        new_survives.publish_candidate(&candidate).unwrap();
        new_survives.crash(true);
        assert_eq!(
            new_survives.load_committed().unwrap().as_deref(),
            Some("new")
        );
    }

    #[test]
    fn crash_after_root_barrier_before_caller_observes_ack_recovers_new_state() {
        for publication_reached_media in [false, true] {
            let mut backend = CrashBackend::with_committed("old".to_owned());
            let candidate = backend.write_candidate(&"new".to_owned()).unwrap();
            backend.sync_candidate(&candidate).unwrap();
            backend.publish_candidate(&candidate).unwrap();
            backend.sync_committed_root().unwrap();

            backend.crash(publication_reached_media);
            assert_eq!(backend.load_committed().unwrap().as_deref(), Some("new"));
        }
    }
}
