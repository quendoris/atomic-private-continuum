#![forbid(unsafe_code)]

//! Development Unix filesystem backend for the A.P.C. durability contract.
//!
//! This crate is intentionally not the final `.apc` format. It stores opaque
//! byte snapshots behind a tiny committed-root manifest so the durability
//! protocol can be exercised against a real filesystem before portable encoding
//! is frozen.

#[cfg(not(unix))]
compile_error!("apc-storage-fs currently implements the development Unix backend only");

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use apc_core::DurabilityBackend;

const OBJECTS_DIR: &str = "objects";
const ROOT_FILE: &str = "root";
const ROOT_NEXT_FILE: &str = "root.next";
const CANDIDATE_PREFIX: &str = "candidate-";
const CANDIDATE_SUFFIX: &str = ".bin";

#[derive(Debug)]
pub enum FsStorageError {
    Io(std::io::Error),
    InvalidRootManifest,
    CandidateIdExhausted,
}

impl fmt::Display for FsStorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "filesystem storage error: {error}"),
            Self::InvalidRootManifest => write!(f, "invalid committed-root manifest"),
            Self::CandidateIdExhausted => write!(f, "candidate identifier space exhausted"),
        }
    }
}

impl std::error::Error for FsStorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidRootManifest | Self::CandidateIdExhausted => None,
        }
    }
}

impl From<std::io::Error> for FsStorageError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsCandidate {
    file_name: String,
}

impl FsCandidate {
    pub fn file_name(&self) -> &str {
        &self.file_name
    }
}

/// Single-writer development backend using immutable candidate files and one
/// atomically replaced committed-root manifest.
///
/// Candidate numeric names are local physical bookkeeping only. They are never
/// exposed as A.P.C. logical identities and never participate in merge order.
pub struct UnixFsDurabilityBackend {
    root: PathBuf,
    next_candidate: u64,
}

impl UnixFsDurabilityBackend {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, FsStorageError> {
        let root = path.as_ref().to_path_buf();
        fs::create_dir_all(root.join(OBJECTS_DIR))?;

        let next_candidate = scan_next_candidate(&root.join(OBJECTS_DIR))?;
        Ok(Self {
            root,
            next_candidate,
        })
    }

    pub fn root_path(&self) -> &Path {
        &self.root
    }

    fn objects_dir(&self) -> PathBuf {
        self.root.join(OBJECTS_DIR)
    }

    fn root_manifest(&self) -> PathBuf {
        self.root.join(ROOT_FILE)
    }

    fn root_next_manifest(&self) -> PathBuf {
        self.root.join(ROOT_NEXT_FILE)
    }

    fn candidate_path(&self, candidate: &FsCandidate) -> PathBuf {
        self.objects_dir().join(&candidate.file_name)
    }

    fn allocate_candidate(&mut self) -> Result<FsCandidate, FsStorageError> {
        loop {
            let id = self.next_candidate;
            self.next_candidate = self
                .next_candidate
                .checked_add(1)
                .ok_or(FsStorageError::CandidateIdExhausted)?;

            let file_name = format!("{CANDIDATE_PREFIX}{id:020}{CANDIDATE_SUFFIX}");
            let path = self.objects_dir().join(&file_name);
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(_) => return Ok(FsCandidate { file_name }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl DurabilityBackend<Vec<u8>> for UnixFsDurabilityBackend {
    type Candidate = FsCandidate;
    type Error = FsStorageError;

    fn load_committed(&self) -> Result<Option<Vec<u8>>, Self::Error> {
        let root_path = self.root_manifest();
        let mut manifest = match File::open(root_path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };

        let mut file_name = String::new();
        manifest.read_to_string(&mut file_name)?;
        let file_name = file_name.trim_end_matches('\n');
        if !valid_candidate_name(file_name) {
            return Err(FsStorageError::InvalidRootManifest);
        }

        let mut bytes = Vec::new();
        File::open(self.objects_dir().join(file_name))?.read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }

    fn write_candidate(&mut self, state: &Vec<u8>) -> Result<Self::Candidate, Self::Error> {
        let candidate = self.allocate_candidate()?;
        let path = self.candidate_path(&candidate);
        let mut file = OpenOptions::new().write(true).truncate(true).open(path)?;
        file.write_all(state)?;
        file.flush()?;
        Ok(candidate)
    }

    fn sync_candidate(&mut self, candidate: &Self::Candidate) -> Result<(), Self::Error> {
        File::open(self.candidate_path(candidate))?.sync_all()?;

        // A newly created candidate is not safely referencable after power loss
        // until its directory entry is durable as well as its file contents.
        File::open(self.objects_dir())?.sync_all()?;
        Ok(())
    }

    fn publish_candidate(&mut self, candidate: &Self::Candidate) -> Result<(), Self::Error> {
        if !valid_candidate_name(candidate.file_name()) {
            return Err(FsStorageError::InvalidRootManifest);
        }

        // The replacement manifest is itself synced before rename so a durable
        // root publication cannot intentionally point at an empty/partial name.
        let next_path = self.root_next_manifest();
        let mut next = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&next_path)?;
        next.write_all(candidate.file_name().as_bytes())?;
        next.write_all(b"\n")?;
        next.sync_all()?;
        drop(next);

        fs::rename(next_path, self.root_manifest())?;
        Ok(())
    }

    fn sync_committed_root(&mut self) -> Result<(), Self::Error> {
        // On Unix, syncing the containing directory is the durability barrier
        // for the rename that publishes the root manifest.
        File::open(&self.root)?.sync_all()?;
        Ok(())
    }
}

fn valid_candidate_name(file_name: &str) -> bool {
    let Some(number) = file_name
        .strip_prefix(CANDIDATE_PREFIX)
        .and_then(|name| name.strip_suffix(CANDIDATE_SUFFIX))
    else {
        return false;
    };

    number.len() == 20 && number.bytes().all(|byte| byte.is_ascii_digit())
}

fn scan_next_candidate(objects_dir: &Path) -> Result<u64, FsStorageError> {
    let mut maximum: Option<u64> = None;

    for entry in fs::read_dir(objects_dir)? {
        let entry = entry?;
        let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !valid_candidate_name(&file_name) {
            continue;
        }

        let digits = &file_name[CANDIDATE_PREFIX.len()..file_name.len() - CANDIDATE_SUFFIX.len()];
        let Ok(value) = digits.parse::<u64>() else {
            continue;
        };
        maximum = Some(maximum.map_or(value, |current| current.max(value)));
    }

    match maximum {
        None => Ok(0),
        Some(value) => value
            .checked_add(1)
            .ok_or(FsStorageError::CandidateIdExhausted),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use apc_core::commit_durable;

    use super::*;

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "apc-storage-fs-{}-{id}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn successful_commit_reopens_as_new_state() {
        let directory = TestDir::new();
        let mut backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();

        commit_durable(&mut backend, &b"first".to_vec()).unwrap();
        drop(backend);

        let reopened = UnixFsDurabilityBackend::open(directory.path()).unwrap();
        assert_eq!(reopened.load_committed().unwrap(), Some(b"first".to_vec()));
    }

    #[test]
    fn durable_but_unpublished_candidate_is_ignored_after_reopen() {
        let directory = TestDir::new();
        let mut backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
        commit_durable(&mut backend, &b"old".to_vec()).unwrap();

        let candidate = backend.write_candidate(&b"new".to_vec()).unwrap();
        backend.sync_candidate(&candidate).unwrap();
        drop(backend);

        let reopened = UnixFsDurabilityBackend::open(directory.path()).unwrap();
        assert_eq!(reopened.load_committed().unwrap(), Some(b"old".to_vec()));
    }

    #[test]
    fn later_commit_replaces_root_without_rewriting_old_candidate() {
        let directory = TestDir::new();
        let mut backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
        commit_durable(&mut backend, &b"old".to_vec()).unwrap();
        commit_durable(&mut backend, &b"new".to_vec()).unwrap();

        let objects: Vec<PathBuf> = fs::read_dir(directory.path().join(OBJECTS_DIR))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(objects.len(), 2);

        drop(backend);
        let reopened = UnixFsDurabilityBackend::open(directory.path()).unwrap();
        assert_eq!(reopened.load_committed().unwrap(), Some(b"new".to_vec()));
    }

    #[test]
    fn corrupt_root_manifest_fails_closed() {
        let directory = TestDir::new();
        let backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
        fs::write(backend.root_manifest(), b"../escape\n").unwrap();

        assert!(matches!(
            backend.load_committed(),
            Err(FsStorageError::InvalidRootManifest)
        ));
    }

    #[test]
    fn candidate_counter_recovers_without_clock_ordering() {
        let directory = TestDir::new();
        let mut backend = UnixFsDurabilityBackend::open(directory.path()).unwrap();
        let first = backend.write_candidate(&b"one".to_vec()).unwrap();
        assert_eq!(first.file_name(), "candidate-00000000000000000000.bin");
        drop(backend);

        let mut reopened = UnixFsDurabilityBackend::open(directory.path()).unwrap();
        let second = reopened.write_candidate(&b"two".to_vec()).unwrap();
        assert_eq!(second.file_name(), "candidate-00000000000000000001.bin");
    }
}
