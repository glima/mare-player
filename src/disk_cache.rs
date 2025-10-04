// SPDX-License-Identifier: MIT

//! General-purpose size-limited disk cache with LRU eviction.
//!
//! Callers can mark paths as *protected* ([`DiskCache::protect_path`]) so that
//! the eviction loop never deletes a file that is still in active use (e.g. the
//! currently-playing audio track).
//!
//! Provides a reusable cache directory manager that enforces a maximum disk
//! usage and evicts the oldest files (by mtime) when the limit is exceeded.
//! Used for image thumbnails, DASH manifests, logs, and any other transient
//! files the applet produces.

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tracing::{debug, info, warn};

use crate::views::components::constants::CACHE_DIR_NAME;

/// A size-limited disk cache backed by a single directory.
///
/// Files are evicted in LRU order (oldest `mtime` first) when the total
/// directory size would exceed `max_bytes`.
#[derive(Clone)]
pub struct DiskCache {
    /// Root directory for this cache partition
    dir: PathBuf,
    /// Maximum allowed size in bytes
    max_bytes: u64,
    /// Approximate current size (updated atomically)
    current_bytes: Arc<AtomicU64>,
    /// Paths that the eviction loop must never delete (e.g. currently-playing
    /// audio file).  Protected behind an `Arc<Mutex>` so `&self` methods can
    /// update the set without requiring `&mut self`, and the struct stays
    /// `Clone`.
    protected_paths: Arc<Mutex<HashSet<PathBuf>>>,
}

impl DiskCache {
    // ── constructors ────────────────────────────────────────────────────

    /// Create (or open) a cache in `dir` with a cap of `max_mb` megabytes.
    ///
    /// The directory and all parent directories are created if they don't
    /// exist.  On open the current size is calculated from the directory
    /// listing.
    pub fn new(dir: PathBuf, max_mb: u32) -> Self {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            warn!("Failed to create cache directory {:?}: {}", dir, e);
        }

        let current = Self::scan_size(&dir);
        let max_bytes = (max_mb as u64) * 1024 * 1024;

        info!(
            "DiskCache {:?}: {:.1} MB used / {} MB max ({:.1}%)",
            dir,
            current as f64 / (1024.0 * 1024.0),
            max_mb,
            if max_bytes > 0 {
                current as f64 / max_bytes as f64 * 100.0
            } else {
                0.0
            },
        );

        Self {
            dir,
            max_bytes,
            current_bytes: Arc::new(AtomicU64::new(current)),
            protected_paths: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Convenience: create a cache under the standard XDG cache directory.
    ///
    /// Resolves to `$XDG_CACHE_HOME/cosmic-applet-mare/<partition>`.
    /// Falls back to `~/.cache/cosmic-applet-mare/<partition>` when
    /// `XDG_CACHE_HOME` is unset.
    pub fn xdg(partition: &str, max_mb: u32) -> Self {
        let base = dirs::cache_dir()
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("/tmp"))
                    .join(".cache")
            })
            .join(CACHE_DIR_NAME)
            .join(partition);
        Self::new(base, max_mb)
    }

    // ── public API ──────────────────────────────────────────────────────

    /// The root directory of this cache.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Current approximate disk usage in bytes.
    pub fn current_bytes(&self) -> u64 {
        self.current_bytes.load(Ordering::Relaxed)
    }

    /// Maximum allowed bytes.
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// Update the maximum cache size at runtime (e.g. from a settings change).
    ///
    /// This takes `&mut self` so callers that hold the cache behind a
    /// `Mutex` (like `TidalAppClient`) can update the limit in place.
    pub fn set_max_mb(&mut self, max_mb: u32) {
        let new_max = (max_mb as u64) * 1024 * 1024;
        info!(
            "DiskCache {:?}: max changed from {} MB to {} MB",
            self.dir,
            self.max_bytes / (1024 * 1024),
            max_mb,
        );
        self.max_bytes = new_max;

        // Evict excess files that no longer fit under the new limit.
        self.ensure_room(0);
    }

    // ── protection ──────────────────────────────────────────────────────

    /// Mark `path` as protected so the eviction loop will never delete it.
    ///
    /// Use this for files that are currently open / being decoded (e.g. the
    /// audio file that is playing right now).
    pub fn protect_path(&self, path: &Path) {
        if let Ok(mut set) = self.protected_paths.lock() {
            set.insert(path.to_path_buf());
        }
    }

    /// Remove eviction protection for `path`.
    ///
    /// Call this when the file is no longer in active use (e.g. playback of
    /// the track has finished or a new track has started).
    pub fn unprotect_path(&self, path: &Path) {
        if let Ok(mut set) = self.protected_paths.lock() {
            set.remove(path);
        }
    }

    /// Replace the entire set of protected paths (convenience for callers
    /// that know exactly which files must be kept).
    pub fn set_protected_paths(&self, paths: impl IntoIterator<Item = PathBuf>) {
        if let Ok(mut set) = self.protected_paths.lock() {
            set.clear();
            set.extend(paths);
        }
    }

    // ── external-write helpers ──────────────────────────────────────────

    /// Inform the cache that `bytes` were written directly to the cache
    /// directory (bypassing [`Self::put`]).
    ///
    /// This keeps the in-memory byte counter accurate so that the next
    /// [`Self::ensure_room`] / [`Self::reserve_room`] call uses a realistic
    /// value instead of a stale one.
    pub fn notify_written(&self, bytes: u64) {
        self.current_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Re-scan the cache directory and update the in-memory size counter.
    ///
    /// This is useful when files are written directly to the cache
    /// directory (bypassing [`Self::put`]), since those writes don't
    /// update the internal counter.  The scan is cheap enough to call
    /// from the settings view on demand.
    pub fn rescan(&self) {
        let fresh = Self::scan_size(&self.dir);
        self.current_bytes.store(fresh, Ordering::Relaxed);
    }

    /// Build the full path for a cache entry from a plain filename.
    ///
    /// This does **not** hash the name — use [`Self::hashed_path`] when
    /// you want a collision-resistant key derived from an arbitrary string.
    pub fn path(&self, filename: &str) -> PathBuf {
        self.dir.join(filename)
    }

    /// Build a cache path by hashing an arbitrary key string.
    ///
    /// The resulting filename is `{hash:016x}.{ext}` where `ext` is the
    /// provided extension (e.g. `"jpg"`, `"mpd"`).
    pub fn hashed_path(&self, key: &str, ext: &str) -> PathBuf {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let hash = hasher.finish();
        self.dir.join(format!("{:016x}.{}", hash, ext))
    }

    /// Write `data` to `filename`, evicting old entries if necessary.
    ///
    /// Returns the full path on success.
    pub fn put(&self, filename: &str, data: &[u8]) -> std::io::Result<PathBuf> {
        let path = self.path(filename);
        self.write_sync(&path, data)
    }

    /// Write `data` to a hashed filename, evicting old entries if necessary.
    ///
    /// Returns the full path on success.
    pub fn put_hashed(&self, key: &str, ext: &str, data: &[u8]) -> std::io::Result<PathBuf> {
        let path = self.hashed_path(key, ext);
        self.write_sync(&path, data)
    }

    /// Async version of [`Self::put_hashed`].
    pub async fn put_hashed_async(
        &self,
        key: &str,
        ext: &str,
        data: &[u8],
    ) -> std::io::Result<PathBuf> {
        let path = self.hashed_path(key, ext);
        self.write_async(&path, data).await
    }

    /// Read a file by hashed key.
    pub fn get_hashed(&self, key: &str, ext: &str) -> Option<Vec<u8>> {
        self.read_sync(&self.hashed_path(key, ext))
    }

    /// Async read by hashed key.
    pub async fn get_hashed_async(&self, key: &str, ext: &str) -> Option<Vec<u8>> {
        self.read_async(&self.hashed_path(key, ext)).await
    }

    /// Remove all files from this cache partition.
    pub fn clear(&self) {
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        self.current_bytes.store(0, Ordering::Relaxed);
        info!("DiskCache {:?}: cleared", self.dir);
    }

    // ── shared write / read helpers ─────────────────────────────────────

    /// Synchronous write: ensure room → write → update counter → log.
    fn write_sync(&self, path: &Path, data: &[u8]) -> std::io::Result<PathBuf> {
        self.ensure_room(data.len() as u64);
        std::fs::write(path, data)?;
        self.after_write(path, data.len() as u64);
        Ok(path.to_path_buf())
    }

    /// Async write: ensure room → write → update counter → log.
    async fn write_async(&self, path: &Path, data: &[u8]) -> std::io::Result<PathBuf> {
        self.ensure_room(data.len() as u64);
        tokio::fs::write(path, data).await?;
        self.after_write(path, data.len() as u64);
        Ok(path.to_path_buf())
    }

    /// Post-write bookkeeping shared by sync and async paths.
    fn after_write(&self, path: &Path, len: u64) {
        self.current_bytes.fetch_add(len, Ordering::Relaxed);
        debug!(
            "DiskCache write {:?} ({} bytes)",
            path.file_name().unwrap_or_default(),
            len,
        );
    }

    /// Synchronous read: read → touch mtime → log.
    fn read_sync(&self, path: &Path) -> Option<Vec<u8>> {
        match std::fs::read(path) {
            Ok(data) => {
                Self::touch(path);
                debug!("DiskCache hit: {:?}", path.file_name().unwrap_or_default());
                Some(data)
            }
            Err(_) => None,
        }
    }

    /// Async read: read → touch mtime → log.
    async fn read_async(&self, path: &Path) -> Option<Vec<u8>> {
        match tokio::fs::read(path).await {
            Ok(data) => {
                Self::touch(path);
                debug!("DiskCache hit: {:?}", path.file_name().unwrap_or_default());
                Some(data)
            }
            Err(_) => None,
        }
    }

    // ── eviction ────────────────────────────────────────────────────────

    /// Ensure there's room for `needed` additional bytes by evicting the
    /// oldest files until `current + needed <= max_bytes`.
    ///
    /// A fresh directory scan is performed first so that decisions are
    /// based on *actual* disk usage rather than a potentially stale
    /// in-memory counter (the audio decoder writes files directly,
    /// bypassing [`Self::put`]).
    fn ensure_room(&self, needed: u64) {
        // Rescan to make sure the counter reflects reality before we
        // decide whether to evict.  This prevents phantom evictions
        // caused by an over-counted `current_bytes`.
        self.rescan();
        let current = self.current_bytes.load(Ordering::Relaxed);
        if current + needed <= self.max_bytes {
            return;
        }
        self.evict(needed);
    }

    /// Public version of [`Self::ensure_room`] for callers that write files
    /// directly to the cache directory (e.g. the audio download thread).
    ///
    /// Call this **before** writing the file.
    pub fn reserve_room(&self, needed: u64) {
        self.ensure_room(needed);
    }

    /// Evict oldest files until we can fit `needed` more bytes.
    ///
    /// Files whose paths are in the `protected_paths` set are **never**
    /// deleted — they are silently skipped.  This prevents the eviction
    /// loop from pulling the rug out from under the audio decoder while
    /// a track is still playing.
    fn evict(&self, needed: u64) {
        let target = self.max_bytes.saturating_sub(needed);

        let protected = self
            .protected_paths
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();

        // Collect (path, size, mtime)
        let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata()
                    && meta.is_file()
                {
                    let path = entry.path();
                    if protected.contains(&path) {
                        continue; // never evict a protected file
                    }
                    let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    files.push((path, meta.len(), mtime));
                }
            }
        }

        // Oldest first
        files.sort_by_key(|(_, _, t)| *t);

        let mut current = self.current_bytes.load(Ordering::Relaxed);
        let mut evicted_count = 0u64;
        let mut evicted_bytes = 0u64;

        for (path, size, _) in &files {
            if current <= target {
                break;
            }
            if std::fs::remove_file(path).is_ok() {
                current = current.saturating_sub(*size);
                evicted_count += 1;
                evicted_bytes += size;
            }
        }

        self.current_bytes.store(current, Ordering::Relaxed);

        if evicted_count > 0 {
            info!(
                "DiskCache {:?}: evicted {} files ({:.1} MB), {} protected files skipped",
                self.dir,
                evicted_count,
                evicted_bytes as f64 / (1024.0 * 1024.0),
                protected.len(),
            );
        }
    }

    // ── helpers ─────────────────────────────────────────────────────────

    /// Calculate the total size of all files in `dir`.
    fn scan_size(dir: &Path) -> u64 {
        let mut total = 0u64;
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata()
                    && meta.is_file()
                {
                    total += meta.len();
                }
            }
        }
        total
    }

    /// Update a file's mtime to now (best-effort).
    ///
    /// Public variant for callers that already have a `PathBuf` (e.g. the
    /// audio cache hit path in `TidalAppClient`).
    pub fn touch_path(path: &Path) {
        Self::touch(path);
    }

    /// Update a file's mtime to now (best-effort).
    fn touch(path: &Path) {
        // filetime crate would be cleaner, but we avoid the extra dep.
        // A zero-length append-open updates mtime on Linux.
        let _ = std::fs::OpenOptions::new().append(true).open(path);
    }
}

// ── convenience: XDG-based log directory ────────────────────────────────

/// Trim a log file to at most `max_bytes` by keeping only the newest content.
///
/// If the file is already within the limit, this is a no-op.  Otherwise it
/// reads the last `max_bytes` of the file, advances to the first newline
/// (so we don't start with a partial line), and rewrites the file in place.
pub fn trim_log_file(path: &Path, max_bytes: u64) {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return, // file doesn't exist yet — nothing to trim
    };

    if meta.len() <= max_bytes {
        return;
    }

    let Ok(mut file) = std::fs::File::open(path) else {
        return;
    };

    use std::io::{Read, Seek, SeekFrom, Write};

    // Seek to `max_bytes` before the end
    let offset = meta.len() - max_bytes;
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return;
    }

    let mut tail = Vec::with_capacity(max_bytes as usize);
    if file.read_to_end(&mut tail).is_err() {
        return;
    }
    drop(file);

    // Skip to the first newline so we don't start with a partial line
    if let Some(pos) = tail.iter().position(|&b| b == b'\n') {
        tail.drain(..=pos);
    }

    // Rewrite the file with only the tail content
    if let Ok(mut out) = std::fs::File::create(path) {
        let _ = out.write_all(&tail);
    }
}

/// Return the XDG-compliant log file path.
///
/// Resolves to `$XDG_CACHE_HOME/<CACHE_DIR_NAME>/logs/<filename>`,
/// creating parent directories as needed.
pub fn log_file_path(filename: &str) -> PathBuf {
    let dir = dirs::cache_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".cache")
        })
        .join(CACHE_DIR_NAME)
        .join("logs");

    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("Warning: failed to create log directory {:?}: {}", dir, e);
    }

    dir.join(filename)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_cache(max_mb: u32) -> (DiskCache, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cache = DiskCache::new(tmp.path().to_path_buf(), max_mb);
        (cache, tmp)
    }

    #[test]
    fn put_and_get() {
        let (cache, _tmp) = temp_cache(10);
        let path = cache.put("hello.txt", b"world").unwrap();
        assert!(path.exists());
        assert_eq!(std::fs::read(path).ok(), Some(b"world".to_vec()));
    }

    #[test]
    fn hashed_put_and_get() {
        let (cache, _tmp) = temp_cache(10);
        cache
            .put_hashed("https://example.com/img.jpg", "jpg", b"pixels")
            .unwrap();
        assert_eq!(
            cache.get_hashed("https://example.com/img.jpg", "jpg"),
            Some(b"pixels".to_vec()),
        );
    }

    #[test]
    fn eviction_when_full() {
        // 1 MB cache, write 600 KB + 600 KB → first file should be evicted
        let (cache, _tmp) = temp_cache(1);
        let big = vec![0u8; 600 * 1024];
        cache.put("first.bin", &big).unwrap();
        assert!(cache.path("first.bin").exists());

        // Need to make "first" older so it gets evicted first
        let first_path = cache.path("first.bin");
        filetime_touch_past(&first_path);

        cache.put("second.bin", &big).unwrap();
        // The first file should have been evicted to make room
        assert!(!cache.path("first.bin").exists());
        assert!(cache.path("second.bin").exists());
    }

    /// Count files in a cache directory (test helper).
    fn file_count(cache: &DiskCache) -> usize {
        fs::read_dir(cache.dir())
            .map(|rd| rd.flatten().count())
            .unwrap_or(0)
    }

    #[test]
    fn clear_removes_all() {
        let (cache, _tmp) = temp_cache(10);
        cache.put("a.bin", b"aaa").unwrap();
        cache.put("b.bin", b"bbb").unwrap();
        assert_eq!(file_count(&cache), 2);
        cache.clear();
        assert_eq!(file_count(&cache), 0);
        assert_eq!(cache.current_bytes(), 0);
    }

    #[test]
    fn file_count_is_accurate() {
        let (cache, _tmp) = temp_cache(10);
        assert_eq!(file_count(&cache), 0);
        cache.put("x", b"1").unwrap();
        cache.put("y", b"2").unwrap();
        assert_eq!(file_count(&cache), 2);
    }

    #[test]
    fn set_max_mb_evicts_excess() {
        // Start with a 10 MB cache and write two 600 KB files (well within limit).
        let (mut cache, _tmp) = temp_cache(10);
        let big = vec![0u8; 600 * 1024];

        cache.put("old.bin", &big).unwrap();
        let old_path = cache.path("old.bin");
        filetime_touch_past(&old_path);

        cache.put("new.bin", &big).unwrap();
        assert!(cache.path("old.bin").exists());
        assert!(cache.path("new.bin").exists());
        assert_eq!(file_count(&cache), 2);

        // Shrink the limit so only one file fits → oldest should be evicted.
        // 600 KB ≈ 0.586 MB, so a 1 MB limit keeps only the newest file.
        cache.set_max_mb(0);
        assert!(
            !cache.path("old.bin").exists(),
            "old file should have been evicted"
        );
        assert!(
            !cache.path("new.bin").exists(),
            "new file should have been evicted"
        );
        assert_eq!(file_count(&cache), 0);
    }

    #[test]
    fn xdg_creates_dir() {
        // Smoke test — just make sure it doesn't panic.
        // We can't easily test the exact path without mocking env.
        let cache = DiskCache::xdg("test_partition", 1);
        assert!(cache.dir().exists());
        // Clean up
        let _ = fs::remove_dir_all(cache.dir());
    }

    /// Helper: set a file's mtime to the past so LRU eviction picks it first.
    fn filetime_touch_past(path: &Path) {
        // Write-then-read trick doesn't help us go backwards, so we use
        // the utime approach via std.  On Linux we can use `filetime` or
        // just rewrite the file with an old mtime.  For the test we simply
        // re-create with explicit times.
        use std::time::{Duration, SystemTime};
        let past = SystemTime::now() - Duration::from_secs(3600);
        let data = fs::read(path).unwrap();
        fs::write(path, &data).unwrap();
        // Best-effort mtime manipulation using a tiny platform shim
        #[cfg(unix)]
        {
            // Use libc::utimensat for precision
            use std::ffi::CString;
            let c_path = CString::new(path.to_str().unwrap()).unwrap();
            let past_secs = past
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;
            let times = [
                libc::timespec {
                    tv_sec: past_secs,
                    tv_nsec: 0,
                },
                libc::timespec {
                    tv_sec: past_secs,
                    tv_nsec: 0,
                },
            ];
            unsafe {
                libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0);
            }
        }
    }
}
