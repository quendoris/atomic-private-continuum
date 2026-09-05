/// Result of asking an opaque transport for protected objects after a known
/// transport revision.
///
/// Transport revisions are cursors/bookkeeping only. This type never compares,
/// orders or interprets them as A.P.C. causal state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchOutcome<R> {
    UpToDate {
        head: Option<R>,
    },
    Changed {
        head: R,
        objects: Vec<Vec<u8>>,
    },
    /// The transport can no longer provide a complete incremental path from the
    /// supplied cursor. The caller must rebootstrap from a retained baseline by
    /// policy outside this low-level adapter.
    BaselineUnavailable {
        head: Option<R>,
    },
}

/// Optimistic publication result for one transport mutation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublishOutcome<R> {
    Published { head: R },
    Conflict { current_head: Option<R> },
}

/// Minimum transport boundary consumed by a foreground A.P.C. sync session.
///
/// The adapter receives only opaque encoded protected objects. It must not need
/// clear `SyncProjection`, scalar values, block names or merge rules.
///
/// `Revision` is an opaque transport cursor (for example a Git commit identity),
/// not a logical `RevisionId` and not an ordering value.
pub trait OpaqueTransport {
    type Revision: Clone + Eq;
    type Error;

    /// Return the current transport head using the cheapest available marker.
    fn head(&mut self) -> Result<Option<Self::Revision>, Self::Error>;

    /// Fetch opaque protected objects introduced after `known_head`.
    ///
    /// Passing `None` means the caller has no transport cursor for the currently
    /// retained generation. A concrete adapter may return `BaselineUnavailable`
    /// instead of pretending an incomplete incremental history is sufficient.
    fn fetch_since(
        &mut self,
        known_head: Option<&Self::Revision>,
    ) -> Result<FetchOutcome<Self::Revision>, Self::Error>;

    /// Publish one set of opaque protected objects only if the current transport
    /// head still equals `expected_head`.
    fn publish(
        &mut self,
        expected_head: Option<&Self::Revision>,
        objects: &[Vec<u8>],
    ) -> Result<PublishOutcome<Self::Revision>, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Revision(u64);

    #[derive(Clone, Debug)]
    struct Commit {
        revision: Revision,
        parent: Option<Revision>,
        objects: Vec<Vec<u8>>,
    }

    #[derive(Default)]
    struct MemoryOpaqueTransport {
        commits: Vec<Commit>,
        next_revision: u64,
    }

    impl OpaqueTransport for MemoryOpaqueTransport {
        type Revision = Revision;
        type Error = core::convert::Infallible;

        fn head(&mut self) -> Result<Option<Self::Revision>, Self::Error> {
            Ok(self.commits.last().map(|commit| commit.revision))
        }

        fn fetch_since(
            &mut self,
            known_head: Option<&Self::Revision>,
        ) -> Result<FetchOutcome<Self::Revision>, Self::Error> {
            let current = self.commits.last().map(|commit| commit.revision);
            if current.as_ref() == known_head {
                return Ok(FetchOutcome::UpToDate { head: current });
            }

            let start = match known_head {
                None => 0,
                Some(known) => match self
                    .commits
                    .iter()
                    .position(|commit| &commit.revision == known)
                {
                    Some(index) => index + 1,
                    None => {
                        return Ok(FetchOutcome::BaselineUnavailable { head: current });
                    }
                },
            };

            let Some(head) = current else {
                return Ok(FetchOutcome::UpToDate { head: None });
            };
            let objects = self.commits[start..]
                .iter()
                .flat_map(|commit| commit.objects.iter().cloned())
                .collect();
            Ok(FetchOutcome::Changed { head, objects })
        }

        fn publish(
            &mut self,
            expected_head: Option<&Self::Revision>,
            objects: &[Vec<u8>],
        ) -> Result<PublishOutcome<Self::Revision>, Self::Error> {
            let current = self.commits.last().map(|commit| commit.revision);
            if current.as_ref() != expected_head {
                return Ok(PublishOutcome::Conflict {
                    current_head: current,
                });
            }

            self.next_revision += 1;
            let revision = Revision(self.next_revision);
            self.commits.push(Commit {
                revision,
                parent: current,
                objects: objects.to_vec(),
            });
            Ok(PublishOutcome::Published { head: revision })
        }
    }

    #[test]
    fn transport_moves_opaque_bytes_and_uses_revision_only_as_cursor() {
        let mut transport = MemoryOpaqueTransport::default();
        assert_eq!(transport.head().unwrap(), None);

        let first = vec![b"opaque-a".to_vec()];
        let head_a = match transport.publish(None, &first).unwrap() {
            PublishOutcome::Published { head } => head,
            other => panic!("unexpected publish outcome: {other:?}"),
        };

        assert_eq!(
            transport.fetch_since(Some(&head_a)).unwrap(),
            FetchOutcome::UpToDate { head: Some(head_a) }
        );

        let stale = transport.publish(None, &[b"opaque-b".to_vec()]).unwrap();
        assert_eq!(
            stale,
            PublishOutcome::Conflict {
                current_head: Some(head_a)
            }
        );

        let head_b = match transport
            .publish(Some(&head_a), &[b"opaque-b".to_vec()])
            .unwrap()
        {
            PublishOutcome::Published { head } => head,
            other => panic!("unexpected retry outcome: {other:?}"),
        };

        assert_eq!(
            transport.fetch_since(Some(&head_a)).unwrap(),
            FetchOutcome::Changed {
                head: head_b,
                objects: vec![b"opaque-b".to_vec()]
            }
        );
        assert_eq!(transport.commits[1].parent, Some(head_a));
    }

    #[test]
    fn unknown_cursor_requires_rebootstrap_instead_of_guessing() {
        let mut transport = MemoryOpaqueTransport::default();
        let head = match transport.publish(None, &[b"opaque".to_vec()]).unwrap() {
            PublishOutcome::Published { head } => head,
            other => panic!("unexpected publish outcome: {other:?}"),
        };

        assert_eq!(
            transport.fetch_since(Some(&Revision(999))).unwrap(),
            FetchOutcome::BaselineUnavailable { head: Some(head) }
        );
    }
}
