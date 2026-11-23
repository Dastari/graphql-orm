//! Process-local retention of trusted Changed witnesses.

use super::{SubscriptionEvent, subscription_end};
use crate::db::Database;
use crate::graphql::auth::AuthSubject;
use crate::graphql::loaders::RelationKey;
use crate::graphql::orm::{ChangeAction, OrmBackend, SqlValue};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    hash::{Hash, Hasher},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tokio::{sync::broadcast, time::Instant};

/// Wire format for cursors produced by opted-in generated Changed streams.
pub(crate) const CURSOR_FORMAT: u32 = 1;
pub(crate) const MAX_EVENTS: usize = 128;
pub(crate) const MAX_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_AGE: Duration = Duration::from_secs(120);
const NONCE_BYTES: usize = 24;
const POSITION_BYTES: usize = 16;
const TOKEN_BYTES: usize = NONCE_BYTES + POSITION_BYTES + 16;

/// The cursor could not be resumed. Subscribe without it, then refetch.
pub const SUBSCRIPTION_REPLAY_RESET: &str = "SUBSCRIPTION_REPLAY_RESET";

pub(super) fn reset() -> async_graphql::Error {
    subscription_end(
        SUBSCRIPTION_REPLAY_RESET,
        "subscription replay is unavailable; subscribe again without a cursor, then refetch",
    )
}

#[derive(Clone, Copy)]
pub(super) struct Position {
    pub(super) sequence: u64,
    milliseconds: u64,
}

type Witness<E, B> = SubscriptionEvent<E, B>;

struct Entry<T> {
    position: Position,
    bytes: usize,
    event: T,
}

struct Retention<T> {
    retired: bool,
    entries: VecDeque<Entry<T>>,
    bytes: usize,
    sequence: u64,
}

/// Owns the publication lock and cursor key for one typed channel generation.
pub(crate) struct ChangedReplay<T> {
    channel: broadcast::WeakSender<T>,
    cipher: XChaCha20Poly1305,
    start: Instant,
    retained: Mutex<Retention<T>>,
}

impl<T: Clone + Send + Sync + 'static> ChangedReplay<T> {
    pub(crate) fn new(channel: &broadcast::Sender<T>) -> Arc<Self> {
        let replay = Arc::new(Self {
            channel: channel.downgrade(),
            cipher: XChaCha20Poly1305::new(&XChaCha20Poly1305::generate_key(&mut OsRng)),
            start: Instant::now(),
            retained: Mutex::new(Retention {
                retired: false,
                entries: VecDeque::with_capacity(MAX_EVENTS),
                bytes: 0,
                sequence: 0,
            }),
        });
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let weak = Arc::downgrade(&replay);
            runtime.spawn(async move {
                // Reclaim expired preimages even on an idle entity. The weak
                // reference allows a dropped Database to release storage at once.
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let Some(replay) = weak.upgrade() else { break };
                    replay.retained().prune(replay.now());
                }
            });
        }
        replay
    }

    pub(crate) fn matches(&self, channel: &broadcast::Sender<T>) -> bool {
        self.channel
            .upgrade()
            .is_some_and(|old| old.same_channel(channel))
    }

    pub(crate) fn retire(&self) {
        let mut retained = self.retained();
        retained.retired = true;
        retained.clear();
    }

    fn now(&self) -> u64 {
        self.start
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    fn retained(&self) -> MutexGuard<'_, Retention<T>> {
        self.retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn expired(&self, position: Position) -> bool {
        expired(position, self.now())
    }

    pub(super) fn seal(&self, position: Position, binding: &[u8; 32]) -> Option<String> {
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let mut plain = [0; POSITION_BYTES];
        plain[..8].copy_from_slice(&position.sequence.to_be_bytes());
        plain[8..].copy_from_slice(&position.milliseconds.to_be_bytes());
        let ciphertext = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: &plain,
                    aad: binding,
                },
            )
            .ok()?;
        let mut token = [0; TOKEN_BYTES];
        token[..NONCE_BYTES].copy_from_slice(&nonce);
        token[NONCE_BYTES..].copy_from_slice(&ciphertext);
        Some(URL_SAFE_NO_PAD.encode(token))
    }

    pub(super) fn open(&self, token: &str, binding: &[u8; 32]) -> Option<Position> {
        const TOKEN_CHARS: usize = (TOKEN_BYTES * 4).div_ceil(3);
        if token.len() != TOKEN_CHARS {
            return None;
        }
        let mut buffer = [0; TOKEN_BYTES];
        if URL_SAFE_NO_PAD.decode_slice(token, &mut buffer).ok()? != TOKEN_BYTES {
            return None;
        }
        let (nonce, ciphertext) = buffer.split_at(NONCE_BYTES);
        let plain = self
            .cipher
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: binding,
                },
            )
            .ok()?;
        Some(Position {
            sequence: u64::from_be_bytes(plain[..8].try_into().ok()?),
            milliseconds: u64::from_be_bytes(plain[8..].try_into().ok()?),
        })
    }

    /// Subscribe and establish the final replay position under the publication lock.
    pub(super) fn subscribe(
        &self,
        channel: &broadcast::Sender<T>,
        after: Option<Position>,
    ) -> Option<(broadcast::Receiver<T>, u64)> {
        let mut retained = self.retained();
        if retained.retired {
            return None;
        }
        let now = self.now();
        retained.prune(now);
        if let Some(position) = after
            && (position.sequence > retained.sequence
                || position.sequence < retained.floor()
                || expired(position, now))
        {
            return None;
        }
        Some((channel.subscribe(), retained.sequence))
    }

    /// Clone one witness at a time so a disconnected or slow subscriber
    /// cannot pin an entire retired ring.
    pub(super) fn get(&self, sequence: u64) -> Option<T> {
        let mut retained = self.retained();
        retained.prune(self.now());
        let offset = sequence.checked_sub(retained.floor().checked_add(1)?)?;
        retained
            .entries
            .get(usize::try_from(offset).ok()?)
            .map(|entry| entry.event.clone())
    }
}

impl<E: Clone + Send + Sync + 'static, B: OrmBackend> ChangedReplay<Witness<E, B>> {
    fn publish(
        &self,
        sender: &broadcast::Sender<Witness<E, B>>,
        mut event: Witness<E, B>,
        bytes: usize,
    ) {
        let mut retained = self.retained();
        if retained.retired {
            return;
        }
        let now = self.now();
        retained.prune(now);
        // Exhausting a u64 would take centuries even at a billion writes/second.
        // Never wrap a position in a live epoch.
        let Some(sequence) = retained.sequence.checked_add(1) else {
            let _ = sender.send(event);
            return;
        };
        let position = Position {
            sequence,
            milliseconds: now,
        };
        retained.sequence = sequence;
        event.replay_position = Some(position);
        if bytes > MAX_BYTES {
            // A hole before any of these entries would make them unreachable.
            retained.clear();
        } else {
            while retained.entries.len() == MAX_EVENTS || retained.bytes + bytes > MAX_BYTES {
                retained.evict();
            }
            retained.entries.push_back(Entry {
                position,
                bytes,
                event: event.clone(),
            });
            retained.bytes += bytes;
        }
        let _ = sender.send(event);
    }
}

impl<T> Retention<T> {
    fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    fn floor(&self) -> u64 {
        self.entries
            .front()
            .map_or(self.sequence, |entry| entry.position.sequence - 1)
    }

    fn evict(&mut self) {
        if let Some(entry) = self.entries.pop_front() {
            self.bytes -= entry.bytes;
        }
    }

    fn prune(&mut self, now: u64) {
        while self
            .entries
            .front()
            .is_some_and(|entry| expired(entry.position, now))
        {
            self.evict();
        }
    }
}

fn expired(position: Position, now: u64) -> bool {
    now.saturating_sub(position.milliseconds) >= MAX_AGE.as_millis() as u64
}

/// Commit-side emission of an opted-in generated Changed witness.
#[doc(hidden)]
pub fn publish_changed_replay<E, B>(db: &Database<B>, event: Witness<E, B>, event_heap_bytes: usize)
where
    E: Clone + Send + Sync + 'static,
    B: OrmBackend,
{
    // Include fixed event/witness storage and all retained Vec/String capacities.
    let bytes = size_of::<Entry<Witness<E, B>>>()
        .saturating_add(size_of::<E>())
        .saturating_add(event_heap_bytes)
        .saturating_add(event.keys.replay_heap_bytes())
        .saturating_add(
            event
                .sources
                .capacity()
                .saturating_mul(size_of::<super::SubscriptionEventSource<B>>()),
        );
    let bytes = event.sources.iter().fold(bytes, |sum, source| {
        sum.saturating_add(source.id.capacity())
            .saturating_add(source.keys.replay_heap_bytes())
            .saturating_add(
                source
                    .relation_fields
                    .capacity()
                    .saturating_mul(size_of::<(&str, &str)>()),
            )
    });
    let (replay, sender) = db.changed_replay();
    replay.publish(&sender, event, bytes);
}

struct BindingDigest(Sha256);

impl Hasher for BindingDigest {
    fn finish(&self) -> u64 {
        unreachable!("replay bindings use the full digest")
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }
}

pub(super) fn binding(
    subject: Option<&AuthSubject>,
    root: &'static str,
    key: Option<RelationKey>,
    keys: Option<&std::collections::HashSet<RelationKey>>,
    actions: Option<&[ChangeAction]>,
) -> [u8; 32] {
    let mut binding = BindingDigest(Sha256::new());
    CURSOR_FORMAT.hash(&mut binding);
    root.hash(&mut binding);
    subject
        .map(|s| {
            (
                &s.id,
                &s.user_id,
                &s.tenant_id,
                &s.organization_id,
                &s.actor_id,
            )
        })
        .hash(&mut binding);
    key.hash(&mut binding);
    keys.map(|keys| {
        let mut digests = keys
            .iter()
            .map(|key| {
                let mut digest = BindingDigest(Sha256::new());
                key.hash(&mut digest);
                <[u8; 32]>::from(digest.0.finalize())
            })
            .collect::<Vec<_>>();
        digests.sort_unstable();
        digests
    })
    .hash(&mut binding);
    actions
        .map(|actions| {
            [
                ChangeAction::Created,
                ChangeAction::Updated,
                ChangeAction::Deleted,
            ]
            .map(|action| actions.contains(&action))
        })
        .hash(&mut binding);
    binding.0.finalize().into()
}

/// Heap allocation accounting for fields retained in a generated Changed
/// witness. Custom owned field types must include all their owned allocations.
#[doc(hidden)]
pub trait ChangedReplaySize {
    /// Upper bound on heap bytes reachable through this value, excluding its
    /// inline size. Return `usize::MAX` for a value that cannot be retained.
    fn replay_heap_bytes(&self) -> usize;
}

macro_rules! inline {
    ($($ty:ty),* $(,)?) => {
        $(
            impl ChangedReplaySize for $ty {
                fn replay_heap_bytes(&self) -> usize { 0 }
            }
        )*
    };
}

inline!(
    (),
    bool,
    i8,
    i16,
    i32,
    i64,
    i128,
    isize,
    u8,
    u16,
    u32,
    u64,
    u128,
    usize,
    f32,
    f64,
    char,
    uuid::Uuid,
    rust_decimal::Decimal,
    chrono::NaiveDate,
    chrono::NaiveTime,
    chrono::NaiveDateTime,
    chrono::DateTime<chrono::Utc>,
    chrono::DateTime<chrono::FixedOffset>,
    std::net::IpAddr,
    std::net::Ipv4Addr,
    std::net::Ipv6Addr,
);

impl ChangedReplaySize for String {
    fn replay_heap_bytes(&self) -> usize {
        self.capacity()
    }
}

impl<T: ChangedReplaySize> ChangedReplaySize for Option<T> {
    fn replay_heap_bytes(&self) -> usize {
        self.as_ref()
            .map_or(0, ChangedReplaySize::replay_heap_bytes)
    }
}

impl<T: ChangedReplaySize> ChangedReplaySize for Vec<T> {
    fn replay_heap_bytes(&self) -> usize {
        self.iter().fold(
            self.capacity().saturating_mul(size_of::<T>()),
            |sum, value| sum.saturating_add(value.replay_heap_bytes()),
        )
    }
}

impl<T: ChangedReplaySize, const N: usize> ChangedReplaySize for [T; N] {
    fn replay_heap_bytes(&self) -> usize {
        self.iter().fold(0usize, |sum, value| {
            sum.saturating_add(value.replay_heap_bytes())
        })
    }
}

impl ChangedReplaySize for serde_json::Value {
    fn replay_heap_bytes(&self) -> usize {
        match self {
            Self::String(value) => value.capacity(),
            Self::Array(values) => values.replay_heap_bytes(),
            // Map storage and arbitrary precision number storage are private
            // to serde_json. Keep retention fail-closed for these values.
            Self::Object(_) | Self::Number(_) => usize::MAX,
            _ => 0,
        }
    }
}

impl ChangedReplaySize for SqlValue {
    fn replay_heap_bytes(&self) -> usize {
        match self {
            Self::String(value) => value.capacity(),
            Self::Bytes(value) => value.capacity(),
            Self::Json(value) => value.replay_heap_bytes(),
            Self::Temporal(value) => value.text_capacity(),
            _ => 0,
        }
    }
}

#[cfg(all(test, any(feature = "sqlite", feature = "postgres", feature = "mssql")))]
mod tests {
    use super::*;
    use crate::graphql::orm::DefaultBackend;

    #[tokio::test(start_paused = true)]
    async fn sequences_are_monotonic_and_resume_checks_age_epoch_and_bytes() {
        let (sender, _) = broadcast::channel(8);
        let ring = ChangedReplay::new(&sender);
        let (mut live, _) = ring.subscribe(&sender, None).unwrap();
        let event = || Witness::<(), DefaultBackend>::new((), vec![], false, vec![]).unwrap();

        ring.publish(&sender, event(), MAX_BYTES / 2);
        ring.publish(&sender, event(), MAX_BYTES / 2);
        let first = live.recv().await.unwrap().replay_position.unwrap();
        let second = live.recv().await.unwrap().replay_position.unwrap();
        assert_eq!(first.sequence + 1, second.sequence);
        assert!(ring.get(first.sequence).is_some());
        let binding = [0; 32];
        let token = ring.seal(first, &binding).unwrap();
        assert_eq!(
            ring.open(&token, &binding).unwrap().sequence,
            first.sequence
        );
        assert_ne!(ring.seal(first, &binding).unwrap(), token);
        assert!(ChangedReplay::new(&sender).open(&token, &binding).is_none());
        assert!(ring.open(&token, &[1; 32]).is_none());

        ring.publish(&sender, event(), MAX_BYTES);
        // A cursor at the last evicted event still covers everything after it.
        assert!(ring.subscribe(&sender, Some(first)).is_none());
        assert!(ring.subscribe(&sender, Some(second)).is_some());
        assert_eq!(ring.retained().bytes, MAX_BYTES);

        ring.publish(&sender, event(), MAX_BYTES + 1);
        assert!(ring.subscribe(&sender, Some(second)).is_none());
        assert_eq!(ring.retained().bytes, 0);
        ring.publish(&sender, event(), 1);
        tokio::task::yield_now().await;
        tokio::time::advance(MAX_AGE).await;
        tokio::task::yield_now().await;
        assert!(ring.expired(ring.open(&token, &binding).unwrap()));
        assert!(ring.retained().entries.is_empty());
    }
}
