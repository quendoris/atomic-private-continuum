use std::collections::BTreeMap;

use crate::PublicationId;

const MAGIC: &[u8; 8] = b"APCSREC1";
const VERSION: u16 = 1;
const PUBLICATION_ID_BYTES: usize = 32;

/// Opaque serialized transport cursor stored only for crash recovery.
///
/// The bytes may encode a Git commit identity or another adapter-specific cursor.
/// They are never compared, ordered or interpreted as A.P.C. causal state.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TransportCursor(Vec<u8>);

impl TransportCursor {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Result<Self, SyncRecoveryError> {
        let bytes = bytes.into();
        if bytes.is_empty() {
            return Err(SyncRecoveryError::EmptyTransportCursor);
        }
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// One already-protected outbound publication retained verbatim across crashes.
///
/// `objects` are complete encoded protected wire objects. Retrying a publication
/// must reuse these exact bytes instead of re-encrypting the same clear state with
/// a fresh nonce and thereby creating a second transport identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableOutboxEntry {
    publication_id: PublicationId,
    expected_cursor: Option<TransportCursor>,
    objects: Vec<Vec<u8>>,
}

impl DurableOutboxEntry {
    pub fn publication_id(&self) -> PublicationId {
        self.publication_id
    }

    pub fn expected_cursor(&self) -> Option<&TransportCursor> {
        self.expected_cursor.as_ref()
    }

    pub fn objects(&self) -> &[Vec<u8>] {
        &self.objects
    }
}

/// Crash-consistent transport bookkeeping paired with one opaque trusted local
/// state image.
///
/// `trusted_state` is produced and validated by the higher semantic/recovery
/// layer. The sync layer deliberately does not decode it. The important local
/// invariant is that semantic state, applied transport cursor and durable outbox
/// are committed as one recovery unit, so a cursor can never advance beyond the
/// state that was durably produced from that transport position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableSyncRecord {
    trusted_state: Vec<u8>,
    applied_cursor: Option<TransportCursor>,
    outbox: BTreeMap<PublicationId, DurableOutboxEntry>,
}

impl DurableSyncRecord {
    pub fn new(trusted_state: Vec<u8>, applied_cursor: Option<TransportCursor>) -> Self {
        Self {
            trusted_state,
            applied_cursor,
            outbox: BTreeMap::new(),
        }
    }

    pub fn trusted_state(&self) -> &[u8] {
        &self.trusted_state
    }

    pub fn applied_cursor(&self) -> Option<&TransportCursor> {
        self.applied_cursor.as_ref()
    }

    pub fn outbox(&self) -> &BTreeMap<PublicationId, DurableOutboxEntry> {
        &self.outbox
    }

    /// Adds an outbound publication together with the trusted state image in
    /// which the corresponding local identities have already crossed the
    /// exposure/handoff boundary.
    ///
    /// The returned in-memory state still must be durably committed before any
    /// network I/O is allowed. This type cannot validate semantic exposure on its
    /// own because it intentionally treats `trusted_state` as opaque bytes.
    pub fn prepare_outbox(
        &mut self,
        exposed_trusted_state: Vec<u8>,
        publication_id: PublicationId,
        expected_cursor: Option<TransportCursor>,
        objects: Vec<Vec<u8>>,
    ) -> Result<(), SyncRecoveryError> {
        validate_objects(&objects)?;
        let entry = DurableOutboxEntry {
            publication_id,
            expected_cursor,
            objects,
        };

        match self.outbox.get(&publication_id) {
            Some(existing) if existing == &entry => {
                self.trusted_state = exposed_trusted_state;
                Ok(())
            }
            Some(_) => Err(SyncRecoveryError::PublicationIdentityCollision),
            None => {
                self.trusted_state = exposed_trusted_state;
                self.outbox.insert(publication_id, entry);
                Ok(())
            }
        }
    }

    /// Advances incoming transport position only together with the trusted local
    /// state produced after authenticating, assembling, validating and merging
    /// the corresponding protected objects.
    ///
    /// Pending outbound publications are intentionally preserved. A crash before
    /// this updated record becomes durable therefore restores both the old state
    /// and the old cursor; a crash after durability restores both new values.
    pub fn apply_received(&mut self, merged_trusted_state: Vec<u8>, new_cursor: TransportCursor) {
        self.trusted_state = merged_trusted_state;
        self.applied_cursor = Some(new_cursor);
    }

    /// Retires exactly one durable outbound publication after reconciliation has
    /// proved its transport outcome, while atomically recording the resulting
    /// trusted state and cursor in the same recovery unit.
    pub fn retire_outbox(
        &mut self,
        publication_id: PublicationId,
        reconciled_trusted_state: Vec<u8>,
        new_cursor: TransportCursor,
    ) -> Result<(), SyncRecoveryError> {
        if self.outbox.remove(&publication_id).is_none() {
            return Err(SyncRecoveryError::UnknownOutboxPublication);
        }
        self.trusted_state = reconciled_trusted_state;
        self.applied_cursor = Some(new_cursor);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncRecoveryError {
    InvalidMagic,
    UnsupportedVersion { version: u16 },
    UnexpectedEof,
    LengthOverflow,
    TrailingBytes,
    InvalidBoolean,
    EmptyTransportCursor,
    EmptyOutbox,
    EmptyOutboxObject,
    DuplicatePublication,
    PublicationIdentityCollision,
    UnknownOutboxPublication,
}

impl core::fmt::Display for SyncRecoveryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidMagic => write!(f, "invalid durable sync recovery magic"),
            Self::UnsupportedVersion { version } => {
                write!(f, "unsupported durable sync recovery version {version}")
            }
            Self::UnexpectedEof => write!(f, "truncated durable sync recovery record"),
            Self::LengthOverflow => write!(f, "durable sync recovery length overflows host limits"),
            Self::TrailingBytes => write!(f, "durable sync recovery record has trailing bytes"),
            Self::InvalidBoolean => write!(f, "invalid durable sync recovery boolean"),
            Self::EmptyTransportCursor => write!(f, "transport cursor must not be empty"),
            Self::EmptyOutbox => write!(f, "outbound publication must contain protected objects"),
            Self::EmptyOutboxObject => write!(f, "outbound protected object must not be empty"),
            Self::DuplicatePublication => {
                write!(
                    f,
                    "durable sync recovery contains duplicate publication IDs"
                )
            }
            Self::PublicationIdentityCollision => {
                write!(
                    f,
                    "publication ID reused for different durable outbox bytes"
                )
            }
            Self::UnknownOutboxPublication => {
                write!(f, "cannot retire an unknown durable outbox publication")
            }
        }
    }
}

impl std::error::Error for SyncRecoveryError {}

pub fn encode_durable_sync_record(
    record: &DurableSyncRecord,
) -> Result<Vec<u8>, SyncRecoveryError> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(MAGIC);
    encoded.extend_from_slice(&VERSION.to_be_bytes());
    write_bytes(&mut encoded, &record.trusted_state)?;
    write_optional_cursor(&mut encoded, record.applied_cursor.as_ref())?;
    write_len(&mut encoded, record.outbox.len())?;

    for entry in record.outbox.values() {
        encoded.extend_from_slice(entry.publication_id.as_bytes());
        write_optional_cursor(&mut encoded, entry.expected_cursor.as_ref())?;
        write_len(&mut encoded, entry.objects.len())?;
        for object in &entry.objects {
            write_bytes(&mut encoded, object)?;
        }
    }

    Ok(encoded)
}

pub fn decode_durable_sync_record(encoded: &[u8]) -> Result<DurableSyncRecord, SyncRecoveryError> {
    if encoded.len() < MAGIC.len() + 2 {
        return Err(SyncRecoveryError::UnexpectedEof);
    }
    if &encoded[..MAGIC.len()] != MAGIC {
        return Err(SyncRecoveryError::InvalidMagic);
    }

    let mut reader = Reader::new(&encoded[MAGIC.len()..]);
    let version = reader.read_u16()?;
    if version != VERSION {
        return Err(SyncRecoveryError::UnsupportedVersion { version });
    }

    let trusted_state = reader.read_bytes()?.to_vec();
    let applied_cursor = reader.read_optional_cursor()?;
    let outbox_len = reader.read_len()?;
    let mut outbox = BTreeMap::new();

    for _ in 0..outbox_len {
        let publication_id = PublicationId::from_bytes(
            reader
                .read_exact(PUBLICATION_ID_BYTES)?
                .try_into()
                .expect("fixed-length publication ID"),
        );
        let expected_cursor = reader.read_optional_cursor()?;
        let object_count = reader.read_len()?;
        if object_count == 0 {
            return Err(SyncRecoveryError::EmptyOutbox);
        }

        let mut objects = Vec::with_capacity(object_count);
        for _ in 0..object_count {
            let object = reader.read_bytes()?.to_vec();
            if object.is_empty() {
                return Err(SyncRecoveryError::EmptyOutboxObject);
            }
            objects.push(object);
        }

        let entry = DurableOutboxEntry {
            publication_id,
            expected_cursor,
            objects,
        };
        if outbox.insert(publication_id, entry).is_some() {
            return Err(SyncRecoveryError::DuplicatePublication);
        }
    }

    if reader.remaining() != 0 {
        return Err(SyncRecoveryError::TrailingBytes);
    }

    Ok(DurableSyncRecord {
        trusted_state,
        applied_cursor,
        outbox,
    })
}

fn validate_objects(objects: &[Vec<u8>]) -> Result<(), SyncRecoveryError> {
    if objects.is_empty() {
        return Err(SyncRecoveryError::EmptyOutbox);
    }
    if objects.iter().any(Vec::is_empty) {
        return Err(SyncRecoveryError::EmptyOutboxObject);
    }
    Ok(())
}

fn write_optional_cursor(
    encoded: &mut Vec<u8>,
    cursor: Option<&TransportCursor>,
) -> Result<(), SyncRecoveryError> {
    match cursor {
        Some(cursor) => {
            encoded.push(1);
            write_bytes(encoded, cursor.as_bytes())?;
        }
        None => encoded.push(0),
    }
    Ok(())
}

fn write_bytes(encoded: &mut Vec<u8>, bytes: &[u8]) -> Result<(), SyncRecoveryError> {
    write_len(encoded, bytes.len())?;
    encoded.extend_from_slice(bytes);
    Ok(())
}

fn write_len(encoded: &mut Vec<u8>, len: usize) -> Result<(), SyncRecoveryError> {
    let len = u64::try_from(len).map_err(|_| SyncRecoveryError::LengthOverflow)?;
    encoded.extend_from_slice(&len.to_be_bytes());
    Ok(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8], SyncRecoveryError> {
        let end = self
            .position
            .checked_add(len)
            .ok_or(SyncRecoveryError::LengthOverflow)?;
        let slice = self
            .bytes
            .get(self.position..end)
            .ok_or(SyncRecoveryError::UnexpectedEof)?;
        self.position = end;
        Ok(slice)
    }

    fn read_u16(&mut self) -> Result<u16, SyncRecoveryError> {
        Ok(u16::from_be_bytes(
            self.read_exact(2)?.try_into().expect("fixed-length u16"),
        ))
    }

    fn read_u64(&mut self) -> Result<u64, SyncRecoveryError> {
        Ok(u64::from_be_bytes(
            self.read_exact(8)?.try_into().expect("fixed-length u64"),
        ))
    }

    fn read_len(&mut self) -> Result<usize, SyncRecoveryError> {
        usize::try_from(self.read_u64()?).map_err(|_| SyncRecoveryError::LengthOverflow)
    }

    fn read_bytes(&mut self) -> Result<&'a [u8], SyncRecoveryError> {
        let len = self.read_len()?;
        self.read_exact(len)
    }

    fn read_optional_cursor(&mut self) -> Result<Option<TransportCursor>, SyncRecoveryError> {
        match self.read_exact(1)?[0] {
            0 => Ok(None),
            1 => Ok(Some(TransportCursor::new(self.read_bytes()?.to_vec())?)),
            _ => Err(SyncRecoveryError::InvalidBoolean),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(value: u64) -> [u8; PUBLICATION_ID_BYTES] {
        let mut bytes = [0_u8; PUBLICATION_ID_BYTES];
        bytes[PUBLICATION_ID_BYTES - 8..].copy_from_slice(&value.to_be_bytes());
        bytes
    }

    fn pid(value: u64) -> PublicationId {
        PublicationId::from_bytes(bytes(value))
    }

    fn cursor(value: &str) -> TransportCursor {
        TransportCursor::new(value.as_bytes().to_vec()).unwrap()
    }

    #[test]
    fn outbox_is_prepared_with_exact_wire_bytes_before_network_io() {
        let mut record =
            DurableSyncRecord::new(b"state-before-exposure".to_vec(), Some(cursor("R0")));
        let objects = vec![b"protected-wire-A".to_vec(), b"protected-wire-B".to_vec()];

        record
            .prepare_outbox(
                b"state-with-exposure".to_vec(),
                pid(1),
                Some(cursor("R0")),
                objects.clone(),
            )
            .unwrap();

        assert_eq!(record.trusted_state(), b"state-with-exposure");
        let entry = record.outbox().get(&pid(1)).unwrap();
        assert_eq!(entry.expected_cursor(), Some(&cursor("R0")));
        assert_eq!(entry.objects(), objects.as_slice());
    }

    #[test]
    fn incoming_cursor_advances_only_with_merged_trusted_state() {
        let mut record = DurableSyncRecord::new(b"old-state".to_vec(), Some(cursor("R0")));
        record
            .prepare_outbox(
                b"old-state-exposed".to_vec(),
                pid(1),
                Some(cursor("R0")),
                vec![b"protected".to_vec()],
            )
            .unwrap();

        record.apply_received(b"merged-R1".to_vec(), cursor("R1"));

        assert_eq!(record.trusted_state(), b"merged-R1");
        assert_eq!(record.applied_cursor(), Some(&cursor("R1")));
        assert!(record.outbox().contains_key(&pid(1)));
    }

    #[test]
    fn unknown_ack_cannot_erase_a_different_pending_publication() {
        let mut record = DurableSyncRecord::new(b"state".to_vec(), Some(cursor("R0")));
        record
            .prepare_outbox(
                b"exposed-A".to_vec(),
                pid(1),
                Some(cursor("R0")),
                vec![b"wire-A".to_vec()],
            )
            .unwrap();
        record
            .prepare_outbox(
                b"exposed-A-B".to_vec(),
                pid(2),
                Some(cursor("R0")),
                vec![b"wire-B".to_vec()],
            )
            .unwrap();

        record
            .retire_outbox(pid(1), b"reconciled-A".to_vec(), cursor("R1"))
            .unwrap();

        assert!(!record.outbox().contains_key(&pid(1)));
        assert!(record.outbox().contains_key(&pid(2)));
        assert_eq!(record.applied_cursor(), Some(&cursor("R1")));
    }

    #[test]
    fn deterministic_codec_round_trips_state_cursor_and_exact_outbox_bytes() {
        let mut record = DurableSyncRecord::new(b"trusted-state".to_vec(), Some(cursor("HEAD-A")));
        record
            .prepare_outbox(
                b"trusted-state-exposed".to_vec(),
                pid(9),
                Some(cursor("HEAD-A")),
                vec![b"opaque-1".to_vec(), b"opaque-2".to_vec()],
            )
            .unwrap();

        let first = encode_durable_sync_record(&record).unwrap();
        let second = encode_durable_sync_record(&record).unwrap();
        assert_eq!(first, second);
        assert_eq!(decode_durable_sync_record(&first).unwrap(), record);
    }

    #[test]
    fn malformed_cursor_outbox_and_trailing_bytes_fail_closed() {
        let mut record = DurableSyncRecord::new(b"trusted".to_vec(), Some(cursor("R0")));
        record
            .prepare_outbox(
                b"exposed".to_vec(),
                pid(3),
                Some(cursor("R0")),
                vec![b"wire".to_vec()],
            )
            .unwrap();
        let encoded = encode_durable_sync_record(&record).unwrap();

        let mut bad_magic = encoded.clone();
        bad_magic[0] ^= 1;
        assert_eq!(
            decode_durable_sync_record(&bad_magic).unwrap_err(),
            SyncRecoveryError::InvalidMagic
        );

        let mut bad_version = encoded.clone();
        bad_version[MAGIC.len()..MAGIC.len() + 2].copy_from_slice(&2_u16.to_be_bytes());
        assert_eq!(
            decode_durable_sync_record(&bad_version).unwrap_err(),
            SyncRecoveryError::UnsupportedVersion { version: 2 }
        );

        assert_eq!(
            decode_durable_sync_record(&encoded[..encoded.len() - 1]).unwrap_err(),
            SyncRecoveryError::UnexpectedEof
        );

        let mut trailing = encoded;
        trailing.push(0);
        assert_eq!(
            decode_durable_sync_record(&trailing).unwrap_err(),
            SyncRecoveryError::TrailingBytes
        );
    }

    #[test]
    fn preparing_same_publication_is_idempotent_only_for_same_transport_bytes() {
        let mut record = DurableSyncRecord::new(b"state".to_vec(), Some(cursor("R0")));
        record
            .prepare_outbox(
                b"exposed".to_vec(),
                pid(4),
                Some(cursor("R0")),
                vec![b"wire-A".to_vec()],
            )
            .unwrap();
        record
            .prepare_outbox(
                b"exposed".to_vec(),
                pid(4),
                Some(cursor("R0")),
                vec![b"wire-A".to_vec()],
            )
            .unwrap();

        assert_eq!(record.outbox().len(), 1);
        assert_eq!(
            record
                .prepare_outbox(
                    b"exposed".to_vec(),
                    pid(4),
                    Some(cursor("R0")),
                    vec![b"wire-B".to_vec()],
                )
                .unwrap_err(),
            SyncRecoveryError::PublicationIdentityCollision
        );
    }
}
