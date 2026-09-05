#![forbid(unsafe_code)]

//! GitHub-specific opaque transport bookkeeping for A.P.C.
//!
//! This crate deliberately operates on already-protected wire objects only. It
//! does not import clear sync projections, scalar values or merge policy.
//!
//! The current `GitHubApi` trait is an injectable boundary for testing the GitHub
//! commit/ref protocol before a concrete HTTP client is selected. The transport
//! and wire formats remain pre-release and are not compatibility commitments.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use apc_sync::{FetchOutcome, OpaqueTransport, PublishOutcome};
use sha2::{Digest, Sha256};

const OBJECT_PREFIX: &str = "sync/objects/";
const OBJECT_SUFFIX: &str = ".apcs";
const COMMIT_MESSAGE: &str = "A.P.C. protected sync publication";
const DEFAULT_MAX_INCREMENTAL_COMMITS: usize = 128;

/// Opaque Git commit identity used only as a transport cursor.
///
/// The string is never parsed numerically and its lexical order has no semantic
/// meaning. Supporting a string rather than a fixed SHA-1 width also avoids
/// making Git object-hash width part of A.P.C. semantics.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitHubCommitOid(String);

impl GitHubCommitOid {
    pub fn new(value: impl Into<String>) -> Result<Self, GitHubOidError> {
        let value = value.into();
        if value.is_empty() || !value.is_ascii() {
            return Err(GitHubOidError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GitHubOidError;

impl core::fmt::Display for GitHubOidError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "GitHub transport object ID must be non-empty ASCII")
    }
}

impl std::error::Error for GitHubOidError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubFileAddition {
    pub path: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitHubFileChangeKind {
    Added,
    Modified,
    Removed,
    Renamed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubFileChange {
    pub path: String,
    pub kind: GitHubFileChangeKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubCommitInfo {
    pub oid: GitHubCommitOid,
    pub parents: Vec<GitHubCommitOid>,
    pub changes: Vec<GitHubFileChange>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitHubCreateCommitResult {
    Created {
        head: GitHubCommitOid,
    },
    HeadChanged {
        current_head: Option<GitHubCommitOid>,
    },
}

/// Narrow GitHub API surface required by the transport algorithm.
///
/// A concrete implementation may use GitHub GraphQL `createCommitOnBranch`
/// with `expectedHeadOid` for routine small publications, or an equivalent
/// commit/ref construction that preserves the same optimistic-head contract.
pub trait GitHubApi {
    type Error;

    fn branch_head(&mut self) -> Result<Option<GitHubCommitOid>, Self::Error>;

    fn create_commit_on_branch(
        &mut self,
        expected_head: &GitHubCommitOid,
        additions: &[GitHubFileAddition],
        message: &str,
    ) -> Result<GitHubCreateCommitResult, Self::Error>;

    fn commit_info(&mut self, oid: &GitHubCommitOid) -> Result<GitHubCommitInfo, Self::Error>;

    fn read_file_at(&mut self, oid: &GitHubCommitOid, path: &str) -> Result<Vec<u8>, Self::Error>;
}

#[derive(Debug)]
pub enum GitHubTransportError<E> {
    Api(E),
    EmptyPublication,
    BranchUninitialized,
    InvalidCommitIdentity,
    InvalidCreatedHead,
    ImmutableObjectViolation { path: String },
    ObjectPathMismatch { path: String },
}

impl<E: core::fmt::Display> core::fmt::Display for GitHubTransportError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Api(error) => write!(f, "GitHub transport API error: {error}"),
            Self::EmptyPublication => write!(f, "cannot publish an empty protected object set"),
            Self::BranchUninitialized => {
                write!(f, "GitHub transport branch has no initial commit")
            }
            Self::InvalidCommitIdentity => {
                write!(
                    f,
                    "GitHub API returned commit metadata for a different object ID"
                )
            }
            Self::InvalidCreatedHead => {
                write!(f, "GitHub publication did not advance the transport head")
            }
            Self::ImmutableObjectViolation { path } => {
                write!(f, "protected transport object path was mutated: {path}")
            }
            Self::ObjectPathMismatch { path } => {
                write!(
                    f,
                    "protected transport object bytes do not match path: {path}"
                )
            }
        }
    }
}

/// GitHub implementation of the generic opaque transport seam.
///
/// Object filenames are SHA-256 digests of the complete already-protected wire
/// bytes. This is transport deduplication/naming only; A.P.C. authenticity still
/// comes from the AEAD layer inside `apc-sync`.
pub struct GitHubTransport<A> {
    api: A,
    max_incremental_commits: usize,
}

impl<A> GitHubTransport<A> {
    pub fn new(api: A) -> Self {
        Self {
            api,
            max_incremental_commits: DEFAULT_MAX_INCREMENTAL_COMMITS,
        }
    }

    pub fn with_max_incremental_commits(api: A, max_incremental_commits: usize) -> Self {
        Self {
            api,
            max_incremental_commits: max_incremental_commits.max(1),
        }
    }

    pub fn into_api(self) -> A {
        self.api
    }
}

impl<A: GitHubApi> OpaqueTransport for GitHubTransport<A> {
    type Revision = GitHubCommitOid;
    type Error = GitHubTransportError<A::Error>;

    fn head(&mut self) -> Result<Option<Self::Revision>, Self::Error> {
        self.api.branch_head().map_err(GitHubTransportError::Api)
    }

    fn fetch_since(
        &mut self,
        known_head: Option<&Self::Revision>,
    ) -> Result<FetchOutcome<Self::Revision>, Self::Error> {
        let current_head = self.api.branch_head().map_err(GitHubTransportError::Api)?;

        if current_head.as_ref() == known_head {
            return Ok(FetchOutcome::UpToDate { head: current_head });
        }

        let Some(known_head) = known_head else {
            return Ok(FetchOutcome::BaselineUnavailable { head: current_head });
        };
        let Some(head) = current_head.clone() else {
            return Ok(FetchOutcome::BaselineUnavailable { head: None });
        };

        let mut cursor = head.clone();
        let mut commits = Vec::new();

        while &cursor != known_head {
            if commits.len() >= self.max_incremental_commits {
                return Ok(FetchOutcome::BaselineUnavailable { head: current_head });
            }

            let info = self
                .api
                .commit_info(&cursor)
                .map_err(GitHubTransportError::Api)?;
            if info.oid != cursor {
                return Err(GitHubTransportError::InvalidCommitIdentity);
            }
            if info.parents.len() != 1 {
                return Ok(FetchOutcome::BaselineUnavailable { head: current_head });
            }

            cursor = info.parents[0].clone();
            commits.push(info);
        }

        // Delivery order remains semantically irrelevant, but returning objects
        // in transport ancestry order makes diagnostics and tests reproducible.
        commits.reverse();
        let mut seen_paths = BTreeSet::new();
        let mut objects = Vec::new();

        for commit in commits {
            for change in commit.changes {
                if !change.path.starts_with(OBJECT_PREFIX) {
                    continue;
                }
                if change.kind != GitHubFileChangeKind::Added {
                    return Err(GitHubTransportError::ImmutableObjectViolation {
                        path: change.path,
                    });
                }
                if !seen_paths.insert(change.path.clone()) {
                    return Err(GitHubTransportError::ImmutableObjectViolation {
                        path: change.path,
                    });
                }

                let bytes = self
                    .api
                    .read_file_at(&commit.oid, &change.path)
                    .map_err(GitHubTransportError::Api)?;
                if object_path(&bytes) != change.path {
                    return Err(GitHubTransportError::ObjectPathMismatch { path: change.path });
                }
                objects.push(bytes);
            }
        }

        Ok(FetchOutcome::Changed { head, objects })
    }

    fn publish(
        &mut self,
        expected_head: Option<&Self::Revision>,
        objects: &[Vec<u8>],
    ) -> Result<PublishOutcome<Self::Revision>, Self::Error> {
        if objects.is_empty() {
            return Err(GitHubTransportError::EmptyPublication);
        }

        let Some(expected_head) = expected_head else {
            let current = self.api.branch_head().map_err(GitHubTransportError::Api)?;
            return match current {
                Some(current_head) => Ok(PublishOutcome::Conflict {
                    current_head: Some(current_head),
                }),
                None => Err(GitHubTransportError::BranchUninitialized),
            };
        };

        let mut additions_by_path = BTreeMap::new();
        for bytes in objects {
            additions_by_path
                .entry(object_path(bytes))
                .or_insert_with(|| bytes.clone());
        }
        let additions: Vec<GitHubFileAddition> = additions_by_path
            .into_iter()
            .map(|(path, bytes)| GitHubFileAddition { path, bytes })
            .collect();

        match self
            .api
            .create_commit_on_branch(expected_head, &additions, COMMIT_MESSAGE)
            .map_err(GitHubTransportError::Api)?
        {
            GitHubCreateCommitResult::Created { head } => {
                if &head == expected_head {
                    return Err(GitHubTransportError::InvalidCreatedHead);
                }
                Ok(PublishOutcome::Published { head })
            }
            GitHubCreateCommitResult::HeadChanged { current_head } => {
                Ok(PublishOutcome::Conflict { current_head })
            }
        }
    }
}

fn object_path(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        write!(&mut hex, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!("{OBJECT_PREFIX}{hex}{OBJECT_SUFFIX}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug)]
    struct FakeCommit {
        info: GitHubCommitInfo,
        files: BTreeMap<String, Vec<u8>>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct FakeApiError(&'static str);

    impl core::fmt::Display for FakeApiError {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    #[derive(Default)]
    struct FakeGitHubApi {
        head: Option<GitHubCommitOid>,
        commits: BTreeMap<GitHubCommitOid, FakeCommit>,
        next_oid: u64,
    }

    impl FakeGitHubApi {
        fn seeded() -> Self {
            let root = oid(1);
            let info = GitHubCommitInfo {
                oid: root.clone(),
                parents: Vec::new(),
                changes: Vec::new(),
            };
            Self {
                head: Some(root.clone()),
                commits: BTreeMap::from([(
                    root,
                    FakeCommit {
                        info,
                        files: BTreeMap::new(),
                    },
                )]),
                next_oid: 1,
            }
        }

        fn force_modify_object(&mut self, path: String, bytes: Vec<u8>) {
            let parent = self.head.clone().unwrap();
            let mut files = self.commits.get(&parent).unwrap().files.clone();
            files.insert(path.clone(), bytes);
            self.next_oid += 1;
            let head = oid(self.next_oid);
            let info = GitHubCommitInfo {
                oid: head.clone(),
                parents: vec![parent],
                changes: vec![GitHubFileChange {
                    path,
                    kind: GitHubFileChangeKind::Modified,
                }],
            };
            self.commits
                .insert(head.clone(), FakeCommit { info, files });
            self.head = Some(head);
        }
    }

    impl GitHubApi for FakeGitHubApi {
        type Error = FakeApiError;

        fn branch_head(&mut self) -> Result<Option<GitHubCommitOid>, Self::Error> {
            Ok(self.head.clone())
        }

        fn create_commit_on_branch(
            &mut self,
            expected_head: &GitHubCommitOid,
            additions: &[GitHubFileAddition],
            message: &str,
        ) -> Result<GitHubCreateCommitResult, Self::Error> {
            assert_eq!(message, COMMIT_MESSAGE);
            if self.head.as_ref() != Some(expected_head) {
                return Ok(GitHubCreateCommitResult::HeadChanged {
                    current_head: self.head.clone(),
                });
            }

            let parent = expected_head.clone();
            let mut files = self
                .commits
                .get(&parent)
                .ok_or(FakeApiError("missing parent"))?
                .files
                .clone();
            let mut changes = Vec::new();
            for addition in additions {
                let kind = if files.contains_key(&addition.path) {
                    GitHubFileChangeKind::Modified
                } else {
                    GitHubFileChangeKind::Added
                };
                files.insert(addition.path.clone(), addition.bytes.clone());
                changes.push(GitHubFileChange {
                    path: addition.path.clone(),
                    kind,
                });
            }

            self.next_oid += 1;
            let head = oid(self.next_oid);
            let info = GitHubCommitInfo {
                oid: head.clone(),
                parents: vec![parent],
                changes,
            };
            self.commits
                .insert(head.clone(), FakeCommit { info, files });
            self.head = Some(head.clone());
            Ok(GitHubCreateCommitResult::Created { head })
        }

        fn commit_info(&mut self, oid: &GitHubCommitOid) -> Result<GitHubCommitInfo, Self::Error> {
            self.commits
                .get(oid)
                .map(|commit| commit.info.clone())
                .ok_or(FakeApiError("unknown commit"))
        }

        fn read_file_at(
            &mut self,
            oid: &GitHubCommitOid,
            path: &str,
        ) -> Result<Vec<u8>, Self::Error> {
            self.commits
                .get(oid)
                .and_then(|commit| commit.files.get(path))
                .cloned()
                .ok_or(FakeApiError("unknown file"))
        }
    }

    fn oid(value: u64) -> GitHubCommitOid {
        GitHubCommitOid::new(format!("{value:040x}")).unwrap()
    }

    #[test]
    fn protected_objects_publish_and_fetch_without_semantic_filenames() {
        let api = FakeGitHubApi::seeded();
        let mut transport = GitHubTransport::new(api);
        let seed = transport.head().unwrap().unwrap();
        let first = b"opaque protected object A".to_vec();
        let second = b"opaque protected object B".to_vec();

        let head = match transport
            .publish(Some(&seed), &[first.clone(), second.clone()])
            .unwrap()
        {
            PublishOutcome::Published { head } => head,
            other => panic!("unexpected publication result: {other:?}"),
        };

        let outcome = transport.fetch_since(Some(&seed)).unwrap();
        let FetchOutcome::Changed {
            head: fetched_head,
            objects,
        } = outcome
        else {
            panic!("expected changed outcome")
        };
        assert_eq!(fetched_head, head);
        assert_eq!(
            objects.into_iter().collect::<BTreeSet<_>>(),
            BTreeSet::from([first, second])
        );

        let api = transport.into_api();
        let commit = api.commits.get(&head).unwrap();
        for change in &commit.info.changes {
            assert!(change.path.starts_with(OBJECT_PREFIX));
            assert!(change.path.ends_with(OBJECT_SUFFIX));
            assert!(!change.path.contains("object A"));
            assert_eq!(change.kind, GitHubFileChangeKind::Added);
        }
    }

    #[test]
    fn stale_expected_head_returns_conflict_without_overwriting_winner() {
        let api = FakeGitHubApi::seeded();
        let mut transport = GitHubTransport::new(api);
        let seed = transport.head().unwrap().unwrap();

        let winner = match transport
            .publish(Some(&seed), &[b"winner".to_vec()])
            .unwrap()
        {
            PublishOutcome::Published { head } => head,
            other => panic!("unexpected winner result: {other:?}"),
        };

        assert_eq!(
            transport
                .publish(Some(&seed), &[b"stale".to_vec()])
                .unwrap(),
            PublishOutcome::Conflict {
                current_head: Some(winner.clone())
            }
        );
        assert_eq!(transport.head().unwrap(), Some(winner));
    }

    #[test]
    fn unknown_or_too_old_cursor_requires_rebootstrap_instead_of_guessing() {
        let api = FakeGitHubApi::seeded();
        let mut transport = GitHubTransport::with_max_incremental_commits(api, 2);
        let seed = transport.head().unwrap().unwrap();
        let head_a = match transport.publish(Some(&seed), &[b"a".to_vec()]).unwrap() {
            PublishOutcome::Published { head } => head,
            other => panic!("unexpected result: {other:?}"),
        };
        let _head_b = match transport.publish(Some(&head_a), &[b"b".to_vec()]).unwrap() {
            PublishOutcome::Published { head } => head,
            other => panic!("unexpected result: {other:?}"),
        };
        let before_c = transport.head().unwrap().unwrap();
        let current = match transport
            .publish(Some(&before_c), &[b"c".to_vec()])
            .unwrap()
        {
            PublishOutcome::Published { head } => head,
            other => panic!("unexpected result: {other:?}"),
        };

        assert_eq!(
            transport.fetch_since(Some(&seed)).unwrap(),
            FetchOutcome::BaselineUnavailable {
                head: Some(current.clone())
            }
        );
        assert_eq!(
            transport.fetch_since(Some(&oid(999))).unwrap(),
            FetchOutcome::BaselineUnavailable {
                head: Some(current)
            }
        );
    }

    #[test]
    fn mutation_of_content_addressed_object_path_fails_closed() {
        let api = FakeGitHubApi::seeded();
        let mut transport = GitHubTransport::new(api);
        let seed = transport.head().unwrap().unwrap();
        let protected = b"protected".to_vec();
        let first_head = match transport
            .publish(Some(&seed), std::slice::from_ref(&protected))
            .unwrap()
        {
            PublishOutcome::Published { head } => head,
            other => panic!("unexpected result: {other:?}"),
        };

        let path = object_path(&protected);
        transport
            .api
            .force_modify_object(path.clone(), b"tampered".to_vec());

        let error = transport.fetch_since(Some(&first_head)).unwrap_err();
        assert!(matches!(
            error,
            GitHubTransportError::ImmutableObjectViolation { path: actual } if actual == path
        ));
    }

    #[test]
    fn no_cursor_on_nonempty_branch_is_not_silently_treated_as_complete_state() {
        let api = FakeGitHubApi::seeded();
        let mut transport = GitHubTransport::new(api);
        assert!(matches!(
            transport.fetch_since(None).unwrap(),
            FetchOutcome::BaselineUnavailable { head: Some(_) }
        ));
    }
}
