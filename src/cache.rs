//! Keeping what a provider reported, so repeat runs are fast and offline.
//!
//! The cache is a reproducible copy of someone else's data, never a source of
//! truth. That is what licenses the two decisions here: a file whose schema
//! does not match is discarded rather than migrated, and a file that cannot be
//! read is treated as absent rather than as a failure. The worst outcome is
//! asking the provider again.

use std::fs;
use std::path::{Path, PathBuf};

use jiff::{SignedDuration, Timestamp};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The shape of what is written. A file written by any other version is
/// discarded on sight.
pub const SCHEMA_VERSION: u32 = 1;

/// How long cached data is considered fresh.
pub const DEFAULT_TTL: SignedDuration = SignedDuration::from_mins(15);

/// Something cached, with enough context to judge whether it is still useful.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cached<T> {
    /// The schema this file was written against.
    pub version: u32,

    /// When it was written.
    pub fetched_at: Timestamp,

    /// What was cached.
    pub data: T,
}

impl<T> Cached<T> {
    /// How long ago this was written, as of `now`.
    #[must_use]
    pub fn age(&self, now: Timestamp) -> SignedDuration {
        now.duration_since(self.fetched_at)
    }

    /// Whether this is old enough to be worth refetching.
    #[must_use]
    pub fn is_stale(&self, now: Timestamp, ttl: SignedDuration) -> bool {
        self.age(now) >= ttl
    }
}

/// Writing to the cache failed.
///
/// Reading never fails: an unreadable file is simply absent.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// The cache directory could not be created.
    #[error("cannot create the cache directory {}: {source}", path.display())]
    Directory {
        /// The directory that could not be created.
        path: PathBuf,
        /// What the operating system reported.
        #[source]
        source: std::io::Error,
    },

    /// The file could not be written.
    #[error("cannot write the cache file {}: {source}", path.display())]
    Write {
        /// The file that could not be written.
        path: PathBuf,
        /// What the operating system reported.
        #[source]
        source: std::io::Error,
    },

    /// The value could not be turned into JSON.
    #[error("cannot serialise cache data for {key}: {source}")]
    Serialise {
        /// Which entry was being written.
        key: String,
        /// What went wrong.
        #[source]
        source: serde_json::Error,
    },
}

/// A directory holding cached provider responses and scan results.
#[derive(Debug, Clone)]
pub struct Cache {
    root: PathBuf,
}

impl Cache {
    /// Uses `root` as the cache directory.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Where the cache lives, for reporting.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Reads an entry, or `None` when there is nothing usable to read.
    ///
    /// Absent, unreadable, malformed, and superseded all collapse to `None`,
    /// because the remedy for every one of them is to ask the provider again.
    #[must_use]
    pub fn load<T: DeserializeOwned>(&self, key: &str) -> Option<Cached<T>> {
        let text = fs::read_to_string(self.path_for(key)).ok()?;
        let cached: Cached<T> = serde_json::from_str(&text).ok()?;

        (cached.version == SCHEMA_VERSION).then_some(cached)
    }

    /// Writes an entry, stamped with the current schema and time.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory cannot be created, the value cannot
    /// be serialised, or the file cannot be written.
    pub fn store<T: Serialize>(
        &self,
        key: &str,
        data: &T,
        now: Timestamp,
    ) -> Result<(), CacheError> {
        fs::create_dir_all(&self.root).map_err(|source| CacheError::Directory {
            path: self.root.clone(),
            source,
        })?;

        let cached = Cached {
            version: SCHEMA_VERSION,
            fetched_at: now,
            data,
        };

        let text =
            serde_json::to_string_pretty(&cached).map_err(|source| CacheError::Serialise {
                key: key.to_owned(),
                source,
            })?;

        let path = self.path_for(key);

        // Write beside the target and rename, so that an interrupted write
        // leaves the previous entry intact rather than a truncated one. The
        // temporary name carries the process id, so two `minato` runs writing
        // the same key do not share one temporary and publish a torn file.
        let temporary = self.temp_path_for(key);

        fs::write(&temporary, text).map_err(|source| CacheError::Write {
            path: temporary.clone(),
            source,
        })?;

        fs::rename(&temporary, &path).map_err(|source| CacheError::Write { path, source })
    }

    /// Removes every entry, so the next run asks the provider again.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory exists but cannot be removed.
    pub fn clear(&self) -> Result<(), CacheError> {
        match fs::remove_dir_all(&self.root) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(CacheError::Directory {
                path: self.root.clone(),
                source,
            }),
        }
    }

    fn path_for(&self, key: &str) -> PathBuf {
        self.root.join(format!("{}.json", sanitise(key)))
    }

    /// The scratch path a write renames from, unique to this process so
    /// concurrent runs do not share one temporary.
    fn temp_path_for(&self, key: &str) -> PathBuf {
        self.root
            .join(format!("{}.json.{}.tmp", sanitise(key), std::process::id()))
    }
}

/// Turns a key into something safe to use as a file name.
///
/// Keys carry provider and owner names, which may hold characters a filesystem
/// treats specially, and which differ only by case on a case-insensitive
/// filesystem.
fn sanitise(key: &str) -> String {
    key.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Timestamp {
        "2026-07-20T12:00:00Z".parse().expect("a timestamp")
    }

    fn cache() -> (tempfile::TempDir, Cache) {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::new(directory.path().join("minato"));

        (directory, cache)
    }

    #[test]
    fn the_temporary_write_path_is_unique_to_the_process() {
        let (_directory, cache) = cache();

        let temporary = cache.temp_path_for("github-mcanouil");

        assert_ne!(
            temporary,
            cache.path_for("github-mcanouil"),
            "the scratch path must differ from the published one"
        );
        assert!(
            temporary
                .to_string_lossy()
                .contains(&std::process::id().to_string()),
            "the scratch path must carry the process id, got: {}",
            temporary.display()
        );
    }

    #[test]
    fn what_is_written_can_be_read_back() {
        let (_directory, cache) = cache();

        cache
            .store("github:mcanouil", &vec!["one", "two"], now())
            .expect("the write to succeed");

        let loaded: Cached<Vec<String>> = cache.load("github:mcanouil").expect("an entry");

        assert_eq!(loaded.data, ["one", "two"]);
        assert_eq!(loaded.fetched_at, now());
        assert_eq!(loaded.version, SCHEMA_VERSION);
    }

    #[test]
    fn an_absent_entry_is_simply_absent() {
        let (_directory, cache) = cache();

        assert!(cache.load::<Vec<String>>("never-written").is_none());
    }

    #[test]
    fn an_entry_from_another_schema_is_discarded_rather_than_migrated() {
        let (_directory, cache) = cache();

        cache.store("key", &"data", now()).expect("the write");

        let path = cache.path_for("key");
        let text = fs::read_to_string(&path).expect("the file");
        fs::write(
            &path,
            text.replace(
                &format!("\"version\": {SCHEMA_VERSION}"),
                "\"version\": 9999",
            ),
        )
        .expect("the rewrite");

        assert!(
            cache.load::<String>("key").is_none(),
            "a file from a future schema must not be trusted"
        );
    }

    #[test]
    fn a_corrupt_entry_is_treated_as_absent_rather_than_as_a_failure() {
        let (_directory, cache) = cache();

        cache.store("key", &"data", now()).expect("the write");
        fs::write(cache.path_for("key"), "{ this is not json").expect("the corruption");

        assert!(
            cache.load::<String>("key").is_none(),
            "the remedy for a corrupt cache is to refetch, not to fail"
        );
    }

    #[test]
    fn an_entry_holding_the_wrong_shape_is_treated_as_absent() {
        let (_directory, cache) = cache();

        cache.store("key", &"a string", now()).expect("the write");

        assert!(
            cache.load::<Vec<u32>>("key").is_none(),
            "a changed data shape must not panic or half-load"
        );
    }

    #[test]
    fn age_is_measured_from_when_the_entry_was_written() {
        let (_directory, cache) = cache();

        cache.store("key", &"data", now()).expect("the write");

        let loaded: Cached<String> = cache.load("key").expect("an entry");
        let later = now() + SignedDuration::from_mins(5);

        assert_eq!(loaded.age(later), SignedDuration::from_mins(5));
    }

    #[test]
    fn freshness_turns_over_exactly_at_the_time_to_live() {
        let entry = Cached {
            version: SCHEMA_VERSION,
            fetched_at: now(),
            data: (),
        };

        let ttl = SignedDuration::from_mins(15);

        assert!(!entry.is_stale(now() + SignedDuration::from_mins(14), ttl));
        assert!(entry.is_stale(now() + ttl, ttl));
        assert!(entry.is_stale(now() + SignedDuration::from_mins(16), ttl));
    }

    #[test]
    fn keys_that_differ_only_by_case_share_one_entry() {
        let (_directory, cache) = cache();

        cache
            .store("github:McAnouil", &"first", now())
            .expect("the write");
        cache
            .store("github:mcanouil", &"second", now())
            .expect("the write");

        let loaded: Cached<String> = cache.load("github:MCANOUIL").expect("an entry");

        assert_eq!(
            loaded.data, "second",
            "one owner must not occupy two entries on a case-insensitive filesystem"
        );
    }

    #[test]
    fn a_key_cannot_escape_the_cache_directory() {
        let (_directory, cache) = cache();

        let path = cache.path_for("../../etc/passwd");

        assert_eq!(
            path.parent(),
            Some(cache.root()),
            "a key must not be able to name a file outside the cache"
        );
    }

    #[test]
    fn clearing_removes_every_entry_and_tolerates_there_being_none() {
        let (_directory, cache) = cache();

        cache.store("key", &"data", now()).expect("the write");
        cache.clear().expect("the clear");

        assert!(cache.load::<String>("key").is_none());
        cache
            .clear()
            .expect("clearing an absent cache is not a failure");
    }

    #[test]
    fn writing_leaves_no_temporary_file_behind() {
        let (_directory, cache) = cache();

        cache.store("key", &"data", now()).expect("the write");

        let leftovers: Vec<_> = fs::read_dir(cache.root())
            .expect("the directory")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| {
                std::path::Path::new(name)
                    .extension()
                    .is_some_and(|ext| ext == "tmp")
            })
            .collect();

        assert!(leftovers.is_empty(), "found {leftovers:?}");
    }
}
