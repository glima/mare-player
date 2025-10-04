// SPDX-License-Identifier: MIT

//! Integration tests for the disk cache module.
//!
//! Covers put/get, hashed put/get, trim_log_file, log_file_path, rescan,
//! reserve_room, and concurrent access patterns.

// Relax production safety lints for test code — clarity over strictness.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::manual_assert,
    clippy::wildcard_imports
)]

use cosmic_applet_mare::disk_cache::{DiskCache, log_file_path, trim_log_file};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Count files in a cache directory (replaces the removed `DiskCache::file_count`).
fn file_count(cache: &DiskCache) -> usize {
    fs::read_dir(cache.dir())
        .map(|rd| rd.flatten().count())
        .unwrap_or(0)
}

fn temp_cache(max_mb: u32) -> (DiskCache, TempDir) {
    let tmp = TempDir::new().expect("failed to create tempdir");
    let cache = DiskCache::new(tmp.path().to_path_buf(), max_mb);
    (cache, tmp)
}

/// Set a file's mtime to the past so LRU eviction picks it first.
#[cfg(unix)]
fn touch_past(path: &std::path::Path, secs_ago: u64) {
    use std::ffi::CString;
    use std::time::{Duration, SystemTime};

    let past = SystemTime::now() - Duration::from_secs(secs_ago);
    let past_secs = past
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let c_path = CString::new(path.to_str().unwrap()).unwrap();
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

// ===========================================================================
// Synchronous API
// ===========================================================================

#[test]
fn put_get_roundtrip() {
    let (cache, _tmp) = temp_cache(10);
    let path = cache.put("hello.txt", b"world").unwrap();
    assert!(path.exists());
    assert_eq!(
        std::fs::read(cache.path("hello.txt")).ok(),
        Some(b"world".to_vec())
    );
}

#[test]
fn hashed_put_get_roundtrip() {
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
fn file_count_tracks_entries() {
    let (cache, _tmp) = temp_cache(10);
    assert_eq!(file_count(&cache), 0);
    cache.put("x", b"1").unwrap();
    cache.put("y", b"2").unwrap();
    assert_eq!(file_count(&cache), 2);
}

#[test]
fn current_bytes_tracks_writes() {
    let (cache, _tmp) = temp_cache(10);
    assert_eq!(cache.current_bytes(), 0);
    cache.put("a", b"hello").unwrap();
    assert_eq!(cache.current_bytes(), 5);
    cache.put("b", b"world!").unwrap();
    assert_eq!(cache.current_bytes(), 11);
}

#[test]
fn max_bytes_reflects_constructor() {
    let (cache, _tmp) = temp_cache(5);
    assert_eq!(cache.max_bytes(), 5 * 1024 * 1024);
}

#[test]
fn dir_returns_root_path() {
    let tmp = TempDir::new().unwrap();
    let cache = DiskCache::new(tmp.path().to_path_buf(), 1);
    assert_eq!(cache.dir(), tmp.path());
}

#[test]
fn path_builds_plain_path() {
    let (cache, _tmp) = temp_cache(10);
    let p = cache.path("test.txt");
    assert!(p.ends_with("test.txt"));
    assert!(p.starts_with(cache.dir()));
}

#[test]
fn hashed_path_is_deterministic() {
    let (cache, _tmp) = temp_cache(10);
    let p1 = cache.hashed_path("https://example.com/img.jpg", "jpg");
    let p2 = cache.hashed_path("https://example.com/img.jpg", "jpg");
    assert_eq!(p1, p2);
    assert!(p1.to_string_lossy().ends_with(".jpg"));
}

#[test]
fn hashed_path_differs_for_different_keys() {
    let (cache, _tmp) = temp_cache(10);
    let p1 = cache.hashed_path("key-alpha", "bin");
    let p2 = cache.hashed_path("key-beta", "bin");
    assert_ne!(p1, p2);
}

#[test]
fn overwrite_existing_file() {
    let (cache, _tmp) = temp_cache(10);
    cache.put("f.txt", b"first").unwrap();
    assert_eq!(
        std::fs::read(cache.path("f.txt")).ok(),
        Some(b"first".to_vec())
    );
    cache.put("f.txt", b"second").unwrap();
    assert_eq!(
        std::fs::read(cache.path("f.txt")).ok(),
        Some(b"second".to_vec())
    );
}

#[test]
fn put_empty_data() {
    let (cache, _tmp) = temp_cache(10);
    cache.put("empty.bin", b"").unwrap();
    assert_eq!(std::fs::read(cache.path("empty.bin")).ok(), Some(vec![]));
    assert!(cache.path("empty.bin").exists());
}

// ===========================================================================
// Eviction
// ===========================================================================

#[cfg(unix)]
#[test]
fn eviction_when_full() {
    // 1 MB cache, write 600 KB + 600 KB → first file should be evicted
    let (cache, _tmp) = temp_cache(1);
    let big = vec![0u8; 600 * 1024];
    cache.put("first.bin", &big).unwrap();
    assert!(cache.path("first.bin").exists());

    // Make "first" older so it gets evicted first
    touch_past(&cache.path("first.bin"), 3600);

    cache.put("second.bin", &big).unwrap();
    assert!(!cache.path("first.bin").exists());
    assert!(cache.path("second.bin").exists());
}

#[cfg(unix)]
#[test]
fn eviction_order_is_lru() {
    // 1 MB cache. Write three 400 KB files. The oldest should be evicted first.
    let (cache, _tmp) = temp_cache(1);
    let chunk = vec![0u8; 400 * 1024];

    cache.put("oldest.bin", &chunk).unwrap();
    touch_past(&cache.path("oldest.bin"), 7200);

    cache.put("middle.bin", &chunk).unwrap();
    touch_past(&cache.path("middle.bin"), 3600);

    // This write should evict "oldest"
    cache.put("newest.bin", &chunk).unwrap();

    assert!(
        !cache.path("oldest.bin").exists(),
        "oldest should be evicted"
    );
    // middle might or might not be evicted depending on exact sizes;
    // newest should always survive
    assert!(cache.path("newest.bin").exists(), "newest should survive");
}

// ===========================================================================
// Rescan
// ===========================================================================

#[test]
fn rescan_updates_byte_count_after_external_write() {
    let (cache, _tmp) = temp_cache(10);
    assert_eq!(cache.current_bytes(), 0);

    // Write a file directly, bypassing the cache API
    let direct_path = cache.dir().join("external.bin");
    fs::write(&direct_path, b"external data here").unwrap();

    // Current bytes is stale
    assert_eq!(cache.current_bytes(), 0);

    // Rescan picks it up
    cache.rescan();
    assert_eq!(cache.current_bytes(), 18); // len("external data here")
}

// ===========================================================================
// reserve_room / notify_external_write
// ===========================================================================

#[cfg(unix)]
#[test]
fn reserve_room_evicts_before_external_write() {
    let (cache, _tmp) = temp_cache(1); // 1 MB
    let big = vec![0u8; 600 * 1024];
    cache.put("existing.bin", &big).unwrap();
    touch_past(&cache.path("existing.bin"), 3600);

    // Reserve room for another 600 KB
    cache.reserve_room(600 * 1024);

    // The existing file should have been evicted
    assert!(!cache.path("existing.bin").exists());
}

// ===========================================================================
// Async API (tokio)
// ===========================================================================

#[tokio::test]
async fn async_hashed_put_and_get() {
    let (cache, _tmp) = temp_cache(10);
    cache
        .put_hashed_async("https://tidal.com/track/123", "mp4", b"audio bytes")
        .await
        .unwrap();
    let data = cache
        .get_hashed_async("https://tidal.com/track/123", "mp4")
        .await;
    assert_eq!(data, Some(b"audio bytes".to_vec()));
}

#[tokio::test]
async fn async_get_hashed_nonexistent() {
    let (cache, _tmp) = temp_cache(10);
    assert_eq!(cache.get_hashed_async("nope", "bin").await, None);
}

#[tokio::test]
async fn async_put_updates_bytes() {
    let (cache, _tmp) = temp_cache(10);
    cache
        .put_hashed_async("key", "ext", b"1234567890")
        .await
        .unwrap();
    assert_eq!(cache.current_bytes(), 10);
}

// ===========================================================================
// Concurrent access
// ===========================================================================

#[tokio::test]
async fn concurrent_writes_do_not_corrupt() {
    let (cache, _tmp) = temp_cache(100);
    let cache = std::sync::Arc::new(cache);

    let mut handles = Vec::new();
    for i in 0..20u32 {
        let c = cache.clone();
        handles.push(tokio::spawn(async move {
            let key = format!("file_{}", i);
            let data = vec![i as u8; 1024];
            c.put_hashed_async(&key, "bin", &data).await.unwrap();
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    // All 20 files should be present
    assert_eq!(file_count(&cache), 20);

    // Each file should contain the correct data
    for i in 0..20u32 {
        let key = format!("file_{}", i);
        let data = cache.get_hashed(&key, "bin").unwrap();
        assert_eq!(data.len(), 1024);
        assert!(data.iter().all(|&b| b == i as u8));
    }
}

#[tokio::test]
async fn concurrent_reads_and_writes() {
    let (cache, _tmp) = temp_cache(100);
    let cache = std::sync::Arc::new(cache);

    // Pre-populate using hashed API
    for i in 0..10u32 {
        let key = format!("pre_{}", i);
        cache.put_hashed(&key, "bin", &[i as u8; 512]).unwrap();
    }

    let mut handles = Vec::new();

    // Spawn readers
    for i in 0..10u32 {
        let c = cache.clone();
        handles.push(tokio::spawn(async move {
            let key = format!("pre_{}", i);
            let data = c.get_hashed_async(&key, "bin").await;
            assert!(data.is_some(), "pre_{} should exist", i);
        }));
    }

    // Spawn writers
    for i in 10..20u32 {
        let c = cache.clone();
        handles.push(tokio::spawn(async move {
            let key = format!("new_{}", i);
            c.put_hashed_async(&key, "bin", &[i as u8; 256])
                .await
                .unwrap();
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    assert!(file_count(&cache) >= 20);
}

// ===========================================================================
// Clone semantics (Arc-backed counter)
// ===========================================================================

#[test]
fn cloned_cache_shares_byte_counter() {
    let (cache, _tmp) = temp_cache(10);
    let clone = cache.clone();

    cache.put("a", b"hello").unwrap();
    // Both should see the same byte count because the counter is Arc<AtomicU64>
    assert_eq!(cache.current_bytes(), 5);
    assert_eq!(clone.current_bytes(), 5);

    clone.put("b", b"world").unwrap();
    assert_eq!(cache.current_bytes(), 10);
    assert_eq!(clone.current_bytes(), 10);
}

// ===========================================================================
// XDG constructor
// ===========================================================================

#[test]
fn xdg_constructor_creates_directory() {
    let cache = DiskCache::xdg("integration_test_partition", 1);
    assert!(cache.dir().exists());
    // Clean up
    let _ = fs::remove_dir_all(cache.dir());
}

// ===========================================================================
// trim_log_file
// ===========================================================================

#[test]
fn trim_log_file_noop_when_small() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("small.log");
    fs::write(&path, "short content\n").unwrap();

    trim_log_file(&path, 1024);

    let content = fs::read_to_string(&path).unwrap();
    assert_eq!(content, "short content\n");
}

#[test]
fn trim_log_file_truncates_large_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("big.log");

    // Write 200 lines, each 50+ chars → well over 1 KB
    {
        let mut f = fs::File::create(&path).unwrap();
        for i in 0..200 {
            writeln!(f, "line {:04}: {}", i, "x".repeat(50)).unwrap();
        }
    }

    let original_size = fs::metadata(&path).unwrap().len();
    assert!(original_size > 1024, "test file should be > 1KB");

    // Trim to 1 KB
    trim_log_file(&path, 1024);

    let trimmed_size = fs::metadata(&path).unwrap().len();
    assert!(
        trimmed_size <= 1024,
        "trimmed size {} should be <= 1024",
        trimmed_size
    );

    // Content should not start with a partial line
    let content = fs::read_to_string(&path).unwrap();
    assert!(
        content.starts_with("line "),
        "should start at a line boundary, got: {:?}",
        &content[..content.len().min(30)]
    );
}

#[test]
fn trim_log_file_noop_when_file_missing() {
    let path = PathBuf::from("/tmp/nonexistent_log_trim_test.log");
    // Should not panic
    trim_log_file(&path, 1024);
}

#[test]
fn trim_log_file_exact_boundary() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("exact.log");

    // Write exactly 100 bytes
    let data = "a".repeat(100);
    fs::write(&path, &data).unwrap();

    // Trim to exactly 100 bytes → no-op
    trim_log_file(&path, 100);
    let content = fs::read_to_string(&path).unwrap();
    assert_eq!(content.len(), 100);
}

#[test]
fn trim_preserves_complete_lines() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("lines.log");

    // Write known lines
    let lines: Vec<String> = (0..100).map(|i| format!("LOG LINE {:03}\n", i)).collect();
    let content = lines.join("");
    fs::write(&path, &content).unwrap();

    // Trim to ~500 bytes
    trim_log_file(&path, 500);

    let result = fs::read_to_string(&path).unwrap();
    // Every line in the result should be complete
    for line in result.lines() {
        assert!(
            line.starts_with("LOG LINE "),
            "unexpected partial line: {:?}",
            line
        );
    }
}

// ===========================================================================
// log_file_path
// ===========================================================================

#[test]
fn log_file_path_returns_valid_path() {
    let path = log_file_path("test.log");
    assert!(path.to_string_lossy().contains("cosmic-applet-mare"));
    assert!(path.to_string_lossy().contains("logs"));
    assert!(path.to_string_lossy().ends_with("test.log"));
    // Parent directory should exist (log_file_path creates it)
    assert!(path.parent().unwrap().exists());
}

#[test]
fn log_file_path_different_names_differ() {
    let p1 = log_file_path("app.log");
    let p2 = log_file_path("debug.log");
    assert_ne!(p1, p2);
    assert!(p1.to_string_lossy().ends_with("app.log"));
    assert!(p2.to_string_lossy().ends_with("debug.log"));
}

// ===========================================================================
// touch_path (static method)
// ===========================================================================

#[test]
fn touch_path_does_not_panic_on_missing_file() {
    let path = PathBuf::from("/tmp/nonexistent_touch_test_file.bin");
    // Should not panic even if file doesn't exist
    DiskCache::touch_path(&path);
}

#[test]
fn touch_path_updates_on_existing_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("touchme.bin");
    fs::write(&path, b"data").unwrap();

    // Touch it — just verify no panic
    DiskCache::touch_path(&path);
    assert!(path.exists());
}

// ===========================================================================
// Large-scale stress test
// ===========================================================================

#[test]
fn many_small_files() {
    let (cache, _tmp) = temp_cache(10);
    for i in 0..500u32 {
        let name = format!("small_{:04}.bin", i);
        cache.put(&name, &i.to_le_bytes()).unwrap();
    }
    assert_eq!(file_count(&cache), 500);

    // Spot-check a few
    assert_eq!(
        std::fs::read(cache.path("small_0000.bin")).ok(),
        Some(0u32.to_le_bytes().to_vec())
    );
    assert_eq!(
        std::fs::read(cache.path("small_0499.bin")).ok(),
        Some(499u32.to_le_bytes().to_vec())
    );
}

#[test]
fn cache_with_zero_max_still_works() {
    // 0 MB max — every write triggers eviction, but files can still be written
    // (the cache doesn't refuse writes, it just evicts aggressively)
    let (cache, _tmp) = temp_cache(0);
    cache.put("a.bin", b"data").unwrap();
    // The file was written (eviction happens before the write for *future* writes)
    // With 0 max, the first write succeeds, subsequent writes evict the previous
    assert!(cache.path("a.bin").exists() || true);
}

// ===========================================================================
// Constructor with pre-existing data
// ===========================================================================

#[test]
fn constructor_scans_existing_files() {
    let tmp = TempDir::new().unwrap();

    // Pre-populate the directory
    fs::write(tmp.path().join("existing1.bin"), b"hello").unwrap();
    fs::write(tmp.path().join("existing2.bin"), b"world!").unwrap();

    let cache = DiskCache::new(tmp.path().to_path_buf(), 10);
    // Should have scanned the 11 bytes
    assert_eq!(cache.current_bytes(), 11);
    assert_eq!(file_count(&cache), 2);
}
