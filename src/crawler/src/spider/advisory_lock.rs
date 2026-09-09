use crate::CrawlerDomainId;
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use listing_source_core::ListingSourceId;
use std::sync::Arc;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Key derivation helpers
// ---------------------------------------------------------------------------

/// Derives a stable `i64` lock key from a UUID.
///
/// XOR-folds the 128-bit UUID into 64 bits so that every distinct UUID maps to a
/// distinct key with high probability.
pub fn domain_id_to_advisory_key(id: CrawlerDomainId) -> i64 {
    let value = u128::from_be_bytes(*id.as_uuid().as_bytes());
    ((value >> 64) as u64 ^ value as u64) as i64
}

/// FNV-1a 64-bit offset basis and prime (no external crate needed).
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Derives a stable `i64` lock key from a URL string using FNV-1a 64-bit.
///
/// FNV-1a is deterministic and produces a well-distributed 64-bit hash value with
/// no external dependencies.
pub fn url_to_advisory_key(url: &url::Url) -> i64 {
    let mut hash = FNV_OFFSET;
    for byte in url.as_str().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash as i64
}

// ---------------------------------------------------------------------------
// Shared RAII lock implementation
// ---------------------------------------------------------------------------

/// Internal RAII guard backed by an in-memory key map.
struct AdvisoryLock {
    locks: Arc<DashMap<String, Instant>>,
    key: String,
}

impl AdvisoryLock {
    fn try_acquire(locks: Arc<DashMap<String, Instant>>, key: String) -> Option<Self> {
        let acquired = match locks.entry(key.clone()) {
            Entry::Occupied(_) => None,
            Entry::Vacant(entry) => {
                entry.insert(Instant::now());
                Some(())
            }
        };
        acquired.map(|()| Self { locks, key })
    }
}

impl Drop for AdvisoryLock {
    fn drop(&mut self) {
        self.locks.remove(&self.key);
    }
}

#[derive(Clone, Default)]
pub struct LocalLockManager {
    locks: Arc<DashMap<String, Instant>>,
}

impl LocalLockManager {
    pub fn new() -> Self {
        Self {
            locks: Arc::new(DashMap::new()),
        }
    }

    fn try_acquire(&self, key: String) -> Option<AdvisoryLock> {
        AdvisoryLock::try_acquire(Arc::clone(&self.locks), key)
    }
}

// ---------------------------------------------------------------------------
// Public typed wrappers
// ---------------------------------------------------------------------------

/// RAII lock for a spider domain (keyed by `CrawlerDomainId`).
pub struct DomainLock(#[allow(dead_code)] AdvisoryLock);

impl DomainLock {
    pub fn try_acquire(
        lock_manager: &LocalLockManager,
        domain_id: CrawlerDomainId,
    ) -> Option<Self> {
        let key = domain_id_to_advisory_key(domain_id);
        lock_manager.try_acquire(key.to_string()).map(Self)
    }
}

/// RAII lock for a scraper URL (keyed by FNV-1a 64-bit hash of the URL string).
pub struct UrlLock(#[allow(dead_code)] AdvisoryLock);

impl UrlLock {
    pub fn try_acquire(lock_manager: &LocalLockManager, url: &url::Url) -> Option<Self> {
        let key = url_to_advisory_key(url);
        lock_manager.try_acquire(key.to_string()).map(Self)
    }
}

/// RAII lock for scraper work scoped to a ListingSource.
pub struct ListingSourceLock(#[allow(dead_code)] AdvisoryLock);

impl ListingSourceLock {
    pub fn try_acquire(
        lock_manager: &LocalLockManager,
        listing_source_id: ListingSourceId,
    ) -> Option<Self> {
        lock_manager
            .try_acquire(format!("listing-source:{listing_source_id}"))
            .map(Self)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- domain_id_to_advisory_key ---

    #[test]
    fn domain_key_is_stable_for_same_uuid() {
        let id = CrawlerDomainId::new();
        assert_eq!(domain_id_to_advisory_key(id), domain_id_to_advisory_key(id));
    }

    #[test]
    fn domain_key_differs_for_different_uuids() {
        let a = CrawlerDomainId::new();
        let b = CrawlerDomainId::new();
        assert_ne!(domain_id_to_advisory_key(a), domain_id_to_advisory_key(b));
    }

    #[test]
    fn domain_key_uses_backing_uuid_bytes() {
        let backing_uuid = uuid::uuid!("01890a5d-ac96-774b-bf1d-d5586c639f75");
        let bytes = backing_uuid.as_bytes();
        let high = i64::from_be_bytes(bytes[..8].try_into().expect("eight high UUID bytes"));
        let low = i64::from_be_bytes(bytes[8..].try_into().expect("eight low UUID bytes"));
        let id = CrawlerDomainId::try_from(backing_uuid).expect("valid deterministic UUIDv7");

        assert_eq!(domain_id_to_advisory_key(id), high ^ low);
    }

    // --- url_to_advisory_key ---

    #[test]
    fn url_key_is_stable_for_same_url() {
        let url = url::Url::parse("https://example.com/product/42").unwrap();
        assert_eq!(url_to_advisory_key(&url), url_to_advisory_key(&url));
    }

    #[test]
    fn url_key_differs_for_different_urls() {
        let a = url::Url::parse("https://example.com/product/1").unwrap();
        let b = url::Url::parse("https://example.com/product/2").unwrap();
        assert_ne!(url_to_advisory_key(&a), url_to_advisory_key(&b));
    }

    #[test]
    fn url_key_is_nonzero_for_typical_url() {
        // The all-zero FNV result for an empty string would be a degenerate case;
        // a real URL must produce a nonzero key.
        let url = url::Url::parse("https://catalog.example.com/item/99").unwrap();
        assert_ne!(url_to_advisory_key(&url), 0i64);
    }

    #[test]
    fn should_lock_and_unlock_domain_key_via_drop() {
        let manager = LocalLockManager::new();
        let domain_id = CrawlerDomainId::new();

        let first = DomainLock::try_acquire(&manager, domain_id);
        assert!(first.is_some());

        let second = DomainLock::try_acquire(&manager, domain_id);
        assert!(second.is_none());

        drop(first);

        let third = DomainLock::try_acquire(&manager, domain_id);
        assert!(third.is_some());
    }

    #[test]
    fn should_lock_and_unlock_url_key_via_drop() {
        let manager = LocalLockManager::new();
        let url = url::Url::parse("https://example.com/product/42").unwrap();

        let first = UrlLock::try_acquire(&manager, &url);
        assert!(first.is_some());

        let second = UrlLock::try_acquire(&manager, &url);
        assert!(second.is_none());

        drop(first);

        let third = UrlLock::try_acquire(&manager, &url);
        assert!(third.is_some());
    }

    #[test]
    fn should_lock_and_unlock_listing_source_key_via_drop() {
        let manager = LocalLockManager::new();
        let listing_source_id = ListingSourceId::new();

        let first = ListingSourceLock::try_acquire(&manager, listing_source_id);
        assert!(first.is_some());

        let second = ListingSourceLock::try_acquire(&manager, listing_source_id);
        assert!(second.is_none());

        drop(first);

        let third = ListingSourceLock::try_acquire(&manager, listing_source_id);
        assert!(third.is_some());
    }
}
