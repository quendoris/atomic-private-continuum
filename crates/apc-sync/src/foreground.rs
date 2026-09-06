use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::{FetchOutcome, OpaqueTransport, PublishOutcome};

/// Shared application-lifecycle gate for foreground-only synchronization.
///
/// The gate starts backgrounded (fail closed). The platform binding must mark it
/// foreground before a sync session may begin transport I/O, and background it
/// as soon as the application leaves the foreground.
///
/// This type does not create timers, workers or background tasks. It only gates
/// transport calls made by the caller.
#[derive(Clone, Debug)]
pub struct ForegroundSyncLifecycle {
    foreground: Arc<AtomicBool>,
}

impl Default for ForegroundSyncLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl ForegroundSyncLifecycle {
    /// Create a fail-closed lifecycle in the backgrounded state.
    pub fn new() -> Self {
        Self {
            foreground: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Allow new synchronization transport calls.
    pub fn enter_foreground(&self) {
        self.foreground.store(true, Ordering::Release);
    }

    /// Prevent any new synchronization transport calls.
    ///
    /// A transport request that was already in progress can have an unknown
    /// external outcome if the platform cancels it concurrently. A.P.C. handles
    /// that case through its durable outbox/reconciliation protocol; this method
    /// deliberately does not invent a success/failure result for in-flight I/O.
    pub fn enter_background(&self) {
        self.foreground.store(false, Ordering::Release);
    }

    pub fn is_foreground(&self) -> bool {
        self.foreground.load(Ordering::Acquire)
    }

    /// Wrap a concrete opaque transport with this lifecycle gate.
    pub fn guard<T>(&self, transport: T) -> ForegroundTransport<T> {
        ForegroundTransport {
            inner: transport,
            foreground: Arc::clone(&self.foreground),
        }
    }
}

/// Error produced by a foreground-gated transport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForegroundTransportError<E> {
    Backgrounded,
    Inner(E),
}

impl<E: core::fmt::Display> core::fmt::Display for ForegroundTransportError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Backgrounded => write!(f, "synchronization transport is disabled in background"),
            Self::Inner(error) => write!(f, "synchronization transport error: {error}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for ForegroundTransportError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Backgrounded => None,
            Self::Inner(error) => Some(error),
        }
    }
}

/// Opaque transport wrapper that refuses to start new I/O while backgrounded.
pub struct ForegroundTransport<T> {
    inner: T,
    foreground: Arc<AtomicBool>,
}

impl<T> ForegroundTransport<T> {
    pub fn inner(&self) -> &T {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    pub fn into_inner(self) -> T {
        self.inner
    }

    fn ensure_foreground<E>(&self) -> Result<(), ForegroundTransportError<E>> {
        if self.foreground.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(ForegroundTransportError::Backgrounded)
        }
    }
}

impl<T> OpaqueTransport for ForegroundTransport<T>
where
    T: OpaqueTransport,
{
    type Revision = T::Revision;
    type Error = ForegroundTransportError<T::Error>;

    fn head(&mut self) -> Result<Option<Self::Revision>, Self::Error> {
        self.ensure_foreground()?;
        self.inner.head().map_err(ForegroundTransportError::Inner)
    }

    fn fetch_since(
        &mut self,
        known_head: Option<&Self::Revision>,
    ) -> Result<FetchOutcome<Self::Revision>, Self::Error> {
        self.ensure_foreground()?;
        self.inner
            .fetch_since(known_head)
            .map_err(ForegroundTransportError::Inner)
    }

    fn publish(
        &mut self,
        expected_head: Option<&Self::Revision>,
        objects: &[Vec<u8>],
    ) -> Result<PublishOutcome<Self::Revision>, Self::Error> {
        self.ensure_foreground()?;
        self.inner
            .publish(expected_head, objects)
            .map_err(ForegroundTransportError::Inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct CountingTransport {
        calls: usize,
    }

    impl OpaqueTransport for CountingTransport {
        type Revision = u64;
        type Error = core::convert::Infallible;

        fn head(&mut self) -> Result<Option<Self::Revision>, Self::Error> {
            self.calls += 1;
            Ok(Some(7))
        }

        fn fetch_since(
            &mut self,
            _known_head: Option<&Self::Revision>,
        ) -> Result<FetchOutcome<Self::Revision>, Self::Error> {
            self.calls += 1;
            Ok(FetchOutcome::UpToDate { head: Some(7) })
        }

        fn publish(
            &mut self,
            _expected_head: Option<&Self::Revision>,
            _objects: &[Vec<u8>],
        ) -> Result<PublishOutcome<Self::Revision>, Self::Error> {
            self.calls += 1;
            Ok(PublishOutcome::Published { head: 8 })
        }
    }

    #[test]
    fn lifecycle_starts_backgrounded_and_blocks_all_transport_calls() {
        let lifecycle = ForegroundSyncLifecycle::new();
        let mut transport = lifecycle.guard(CountingTransport::default());

        assert!(!lifecycle.is_foreground());
        assert_eq!(
            transport.head(),
            Err(ForegroundTransportError::Backgrounded)
        );
        assert_eq!(
            transport.fetch_since(None),
            Err(ForegroundTransportError::Backgrounded)
        );
        assert_eq!(
            transport.publish(None, &[b"opaque".to_vec()]),
            Err(ForegroundTransportError::Backgrounded)
        );
        assert_eq!(transport.inner().calls, 0);
    }

    #[test]
    fn background_transition_blocks_future_calls_without_touching_inner_transport() {
        let lifecycle = ForegroundSyncLifecycle::new();
        let mut transport = lifecycle.guard(CountingTransport::default());

        lifecycle.enter_foreground();
        assert_eq!(transport.head().unwrap(), Some(7));
        assert_eq!(transport.inner().calls, 1);

        lifecycle.enter_background();
        assert_eq!(
            transport.publish(Some(&7), &[b"opaque".to_vec()]),
            Err(ForegroundTransportError::Backgrounded)
        );
        assert_eq!(transport.inner().calls, 1);

        lifecycle.enter_foreground();
        assert_eq!(
            transport.publish(Some(&7), &[b"opaque".to_vec()]).unwrap(),
            PublishOutcome::Published { head: 8 }
        );
        assert_eq!(transport.inner().calls, 2);
    }
}
