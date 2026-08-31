// SPDX-License-Identifier: GPL-3.0-only

//! Image cache for album art and other images.
//!
//! This module provides async image loading with both memory and disk caching
//! to avoid repeated network requests for the same images. The disk layer is
//! backed by the embedded cache database ([`crate::cache::Db`]), which handles
//! byte-budgeted LRU eviction. Until the database finishes opening at startup
//! the disk tier is skipped and only the in-memory tier is used.

use image::GenericImageView;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{OnceCell, RwLock};
use tracing::{debug, error};

/// Decoded RGBA pixel data ready for direct use with `Handle::from_rgba`.
/// Avoids the cost of re-encoding to PNG just to have iced decode it again.
#[derive(Debug)]
pub struct RgbaPixels {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Cached image data
#[derive(Clone)]
pub struct CachedImage {
    /// Raw image bytes
    pub data: Arc<Vec<u8>>,
}

/// Image cache for storing downloaded images (memory + disk)
#[derive(Clone)]
pub struct ImageCache {
    /// In-memory cache storage: URL -> image data
    memory_cache: Arc<RwLock<HashMap<String, CachedImage>>>,
    /// HTTP client for downloading images
    client: reqwest::Client,
    /// Maximum total bytes to keep in the in-memory cache
    max_memory_bytes: u64,
    /// Byte budget for the on-database image tier (LRU-evicted).
    max_disk_bytes: i64,
    /// Cache database, populated once it finishes opening at startup. While
    /// empty, the disk tier is skipped and only the memory tier is consulted.
    db: Arc<OnceCell<crate::cache::Db>>,
}

/// Maximum pixel dimension for decoded image handles.
///
/// Source artwork is 320×320 from the TIDAL CDN.  The largest on-screen
/// usage is 96 px (album/artist detail); on a 2× HiDPI display that
/// requires 192 real pixels.  160 px is a good middle ground — sharp
/// enough for detail views while cutting per-image RGBA memory by 4×
/// compared to the full 320 px source.
pub const IMAGE_RENDER_MAX_PX: u32 = 160;

impl Default for ImageCache {
    fn default() -> Self {
        Self::new(200) // 200 MB on disk
    }
}

impl ImageCache {
    /// Create a new image cache.
    ///
    /// - `max_disk_size_mb`: maximum disk cache size in megabytes (LRU-evicted)
    ///
    /// The in-memory tier is automatically sized to 10% of the disk limit
    /// (e.g. 200 MB on disk → 20 MB in RAM).  This means lower-resolution
    /// images let more entries fit, while high-res artwork naturally evicts
    /// sooner.
    pub fn new(max_disk_size_mb: u32) -> Self {
        let max_memory_bytes = (max_disk_size_mb as u64) * 1024 * 1024 / 10;
        let max_disk_bytes = (max_disk_size_mb as i64) * 1024 * 1024;

        Self {
            memory_cache: Arc::new(RwLock::new(HashMap::new())),
            client: reqwest::Client::new(),
            max_memory_bytes,
            max_disk_bytes,
            db: Arc::new(OnceCell::new()),
        }
    }

    /// Attach the cache database once it has finished opening at startup.
    ///
    /// Idempotent: a second call is a no-op. Until this is called the disk
    /// tier is skipped (only the in-memory tier and network are used).
    pub fn set_db(&self, db: crate::cache::Db) {
        let _ = self.db.set(db);
    }

    /// Try to load an image from the cache database.
    async fn load_from_disk(&self, url: &str) -> Option<CachedImage> {
        let db = self.db.get()?;
        db.get_image(url).await.map(|data| {
            debug!("DB image cache hit: {}", url);
            CachedImage { data: Arc::new(data) }
        })
    }

    /// Save an image to the cache database (LRU-evicted by byte budget).
    async fn save_to_disk(&self, url: &str, data: &[u8]) {
        if let Some(db) = self.db.get() {
            db.put_image(url, data, self.max_disk_bytes).await;
            debug!("Saved image to DB cache: {} ({} bytes)", url, data.len());
        }
    }

    /// Get an image from cache (memory or disk), or download and cache it
    pub async fn get_or_load(&self, url: &str) -> Option<CachedImage> {
        // Check memory cache first
        {
            let cache = self.memory_cache.read().await;
            if let Some(img) = cache.get(url) {
                debug!("Memory cache hit: {}", url);
                return Some(img.clone());
            }
        }

        // Check disk cache
        if let Some(cached) = self.load_from_disk(url).await {
            // Add to memory cache
            self.add_to_memory_cache(url, cached.clone()).await;
            return Some(cached);
        }

        // Download the image
        debug!("Cache miss, downloading: {}", url);
        match self.download_image(url).await {
            Ok(data) => {
                let cached = CachedImage { data: Arc::new(data) };

                // Save to disk cache
                self.save_to_disk(url, &cached.data).await;

                // Add to memory cache
                self.add_to_memory_cache(url, cached.clone()).await;

                Some(cached)
            }
            Err(e) => {
                error!("Failed to download image {}: {}", url, e);
                None
            }
        }
    }

    /// Add an image to the memory cache, evicting oldest entries if the
    /// total byte size would exceed the limit.
    async fn add_to_memory_cache(&self, url: &str, cached: CachedImage) {
        let mut cache = self.memory_cache.write().await;

        // Evict oldest entries until we have room for the new image
        let new_size = cached.data.len() as u64;
        let mut total: u64 = cache.values().map(|v| v.data.len() as u64).sum();
        while total + new_size > self.max_memory_bytes {
            if let Some(key) = cache.keys().next().cloned() {
                if let Some(removed) = cache.remove(&key) {
                    total -= removed.data.len() as u64;
                }
            } else {
                break;
            }
        }

        cache.insert(url.to_string(), cached);
    }

    /// Try to load a cached grid thumbnail from the database.
    ///
    /// `cache_key` should be a stable identifier (e.g. playlist UUID or a
    /// hash of the cover URLs).  Returns `None` on cache miss.
    pub async fn get_cached_grid(&self, cache_key: &str) -> Option<Vec<u8>> {
        let db = self.db.get()?;
        db.get_image(&format!("grid:{cache_key}")).await
    }

    /// Save a generated grid thumbnail PNG to the database cache.
    pub async fn save_grid(&self, cache_key: &str, png_data: &[u8]) {
        if let Some(db) = self.db.get() {
            db.put_image(&format!("grid:{cache_key}"), png_data, self.max_disk_bytes).await;
        }
    }

    /// Whether a URL may be fetched.
    ///
    /// Artwork URLs arrive from TIDAL's API responses, so this is the last
    /// point where a hostile one can be turned away: `https://` only, in
    /// anything that ships. The loopback exemption exists for the tests
    /// below, which serve PNGs from a local server, and is compiled out of
    /// release builds — otherwise an API response naming `http://127.0.0.1:…`
    /// would have the player fetch from a service on the user's own machine.
    fn is_fetchable(url: &str) -> bool {
        if url.starts_with("https://") {
            return true;
        }
        cfg!(test) && (url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost"))
    }

    /// Download an image from a URL
    async fn download_image(&self, url: &str) -> Result<Vec<u8>, String> {
        if !Self::is_fetchable(url) {
            return Err(format!("Refusing non-HTTPS image URL: {url}"));
        }

        let response = self.client.get(url).send().await.map_err(|e| format!("Request failed: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        let bytes = response.bytes().await.map_err(|e| format!("Failed to read response: {}", e))?;

        Ok(bytes.to_vec())
    }
}

/// Composite up to 4 images into a 2×2 grid, then apply a circular mask.
///
/// Takes a slice of raw image byte arrays (JPEG/PNG). The first 4 unique images
/// are placed top-left, top-right, bottom-left, bottom-right. If fewer than 4
/// are provided the available images are repeated to fill the grid. A 1 px gap
/// separates the quadrants, filled with a dark background (`#1a1a1a`).
///
/// The result is a circular PNG at `output_size × output_size` pixels, suitable
/// for use as a playlist thumbnail that mirrors the TIDAL 2×2 album art style.
pub fn make_grid_thumbnail(images: &[&[u8]], output_size: u32) -> Result<RgbaPixels, String> {
    if images.is_empty() {
        return Err("No images provided for grid thumbnail".to_string());
    }

    let gap = 1u32; // 1 px gap between quadrants
    let half = (output_size - gap) / 2;

    // Dark background colour for the gap
    let bg = image::Rgba([26u8, 26, 26, 255]);

    // Start with a solid background
    let mut canvas = image::RgbaImage::from_pixel(output_size, output_size, bg);

    // Decode available images, falling back to repeating earlier ones
    let mut decoded: Vec<image::DynamicImage> = Vec::with_capacity(4);
    for raw in images.iter().take(4) {
        match image::load_from_memory(raw) {
            Ok(img) => decoded.push(img),
            Err(e) => tracing::warn!("Skipping grid image: {}", e),
        }
    }
    if decoded.is_empty() {
        return Err("None of the provided images could be decoded".to_string());
    }
    // Pad to 4 by cycling through available images (e.g. [A,B] → [A,B,A,B])
    let base_count = decoded.len();
    while decoded.len() < 4 {
        let idx = decoded.len() % base_count;
        let Some(img) = decoded.get(idx).cloned() else {
            break;
        };
        decoded.push(img);
    }

    // Positions: TL, TR, BL, BR
    let positions = [(0u32, 0u32), (half + gap, 0), (0, half + gap), (half + gap, half + gap)];

    for (i, (ox, oy)) in positions.iter().enumerate() {
        let Some(img) = decoded.get(i) else {
            continue;
        };
        let (w, h) = img.dimensions();
        let side = w.min(h);
        let cx = (w - side) / 2;
        let cy = (h - side) / 2;
        let cropped = img.crop_imm(cx, cy, side, side);
        let resized = cropped.resize_exact(half, half, image::imageops::FilterType::Lanczos3);
        image::imageops::overlay(&mut canvas, &resized.to_rgba8(), *ox as i64, *oy as i64);
    }

    // Apply circular mask (same logic as make_circular)
    let center = output_size as f32 / 2.0;
    let radius = center;
    for y in 0..output_size {
        for x in 0..output_size {
            let dx = x as f32 - center + 0.5;
            let dy = y as f32 - center + 0.5;
            let distance = (dx * dx + dy * dy).sqrt();
            if distance > radius {
                canvas.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
            } else if distance > radius - 1.0 {
                let alpha = (radius - distance).clamp(0.0, 1.0);
                let pixel = canvas.get_pixel(x, y);
                let new_alpha = (pixel[3] as f32 * alpha) as u8;
                canvas.put_pixel(x, y, image::Rgba([pixel[0], pixel[1], pixel[2], new_alpha]));
            }
        }
    }

    Ok(RgbaPixels { width: output_size, height: output_size, pixels: canvas.into_raw() })
}

/// Make an image circular by applying an alpha mask.
/// Takes raw image bytes (JPEG/PNG) and returns raw RGBA pixels with circular transparency.
pub fn make_circular(image_data: &[u8], max_size: u32) -> Result<RgbaPixels, String> {
    // Decode the image
    let img = image::load_from_memory(image_data).map_err(|e| format!("Failed to decode image: {}", e))?;

    let (width, height) = img.dimensions();
    let size = width.min(height);

    // Crop to square (center crop)
    let x_offset = (width - size) / 2;
    let y_offset = (height - size) / 2;
    let cropped = img.crop_imm(x_offset, y_offset, size, size);

    // Downscale to save memory (320×320 RGBA = 400 KB → 160×160 = 100 KB)
    let cropped =
        if size > max_size { cropped.resize_exact(max_size, max_size, image::imageops::FilterType::Lanczos3) } else { cropped };
    let size = cropped.width();

    // Create RGBA image with circular mask
    let mut rgba = cropped.to_rgba8();
    let center = size as f32 / 2.0;
    let radius = center;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center + 0.5;
            let dy = y as f32 - center + 0.5;
            let distance = (dx * dx + dy * dy).sqrt();

            if distance > radius {
                // Outside circle - make transparent
                rgba.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
            } else if distance > radius - 1.0 {
                // Anti-aliasing at edge
                let alpha = (radius - distance).clamp(0.0, 1.0);
                let pixel = rgba.get_pixel(x, y);
                let new_alpha = (pixel[3] as f32 * alpha) as u8;
                rgba.put_pixel(x, y, image::Rgba([pixel[0], pixel[1], pixel[2], new_alpha]));
            }
        }
    }

    Ok(RgbaPixels { width: size, height: size, pixels: rgba.into_raw() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    // ── helpers ──────────────────────────────────────────────────────────

    /// Build an `ImageCache` whose disk tier is an in-memory cache database,
    /// so tests are isolated and never touch the real XDG cache or filesystem.
    /// `max_memory_bytes` sizes the in-RAM tier for fine-grained eviction tests.
    async fn temp_cache(max_memory_bytes: u64) -> ImageCache {
        let cache = ImageCache {
            memory_cache: Arc::new(RwLock::new(HashMap::new())),
            client: reqwest::Client::new(),
            max_memory_bytes,
            max_disk_bytes: 50 * 1024 * 1024,
            db: Arc::new(OnceCell::new()),
        };
        let db = crate::cache::Db::open(std::path::Path::new(":memory:")).await.expect("open in-memory cache db");
        cache.set_db(db);
        cache
    }

    /// Create a minimal valid 1×1 red PNG in memory (~67-70 bytes).
    fn tiny_png() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        img.write_to(&mut cursor, image::ImageFormat::Png).expect("encode tiny png");
        buf
    }

    /// Spawn a one-shot HTTP server that responds with the given body
    /// and content type. Returns `(url, port)`.
    fn spawn_http_server(body: Vec<u8>, content_type: &str) -> (String, u16) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{}/image.png", port);
        let ct = content_type.to_string();

        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut req = [0u8; 4096];
                let _ = stream.read(&mut req);
                let header = format!(
                    "HTTP/1.1 200 OK\r\n\
                     Content-Type: {}\r\n\
                     Content-Length: {}\r\n\
                     Connection: close\r\n\r\n",
                    ct,
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            }
        });

        (url, port)
    }

    /// Spawn a one-shot HTTP server that returns the given HTTP status.
    fn spawn_http_error(status: u16) -> (String, u16) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{}/image.png", port);

        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut req = [0u8; 4096];
                let _ = stream.read(&mut req);
                let resp = format!("HTTP/1.1 {} Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", status);
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });

        (url, port)
    }

    // ── constructors ────────────────────────────────────────────────────

    #[test]
    fn test_new_sets_memory_limit() {
        // 200 MB disk → 20 MB RAM (10%)
        let cache = ImageCache::new(200);
        assert_eq!(cache.max_memory_bytes, 200 * 1024 * 1024 / 10);
        assert_eq!(cache.max_disk_bytes, 200 * 1024 * 1024);
    }

    #[test]
    fn test_default_uses_200mb_disk() {
        let cache = ImageCache::default();
        assert_eq!(cache.max_memory_bytes, 200 * 1024 * 1024 / 10);
    }

    #[test]
    fn test_new_zero_mb() {
        let cache = ImageCache::new(0);
        assert_eq!(cache.max_memory_bytes, 0);
    }

    #[tokio::test]
    async fn test_clone_shares_memory_cache() {
        let cache = temp_cache(1024 * 1024).await;
        let clone = cache.clone();
        // Both refer to the same Arc-backed memory tier and DB handle.
        assert!(Arc::ptr_eq(&cache.memory_cache, &clone.memory_cache));
        assert!(Arc::ptr_eq(&cache.db, &clone.db));
    }

    // ── add_to_memory_cache ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_add_to_memory_cache_basic() {
        let cache = temp_cache(1024 * 1024).await;
        let img = CachedImage { data: Arc::new(vec![1, 2, 3, 4]) };
        cache.add_to_memory_cache("http://a.test/1.png", img).await;
        let mem = cache.memory_cache.read().await;
        assert_eq!(mem.get("http://a.test/1.png").unwrap().data.len(), 4);
    }

    #[tokio::test]
    async fn test_add_to_memory_cache_eviction() {
        // 20-byte limit; inserting 3×8 bytes forces eviction.
        let cache = temp_cache(20).await;
        for i in 0..3u8 {
            let img = CachedImage { data: Arc::new(vec![i; 8]) };
            cache.add_to_memory_cache(&format!("http://a.test/{}.png", i), img).await;
        }
        let mem = cache.memory_cache.read().await;
        assert!(mem.contains_key("http://a.test/2.png"));
        let total: u64 = mem.values().map(|v| v.data.len() as u64).sum();
        assert!(total <= 20);
    }

    #[tokio::test]
    async fn test_add_to_memory_cache_replaces_same_key() {
        let cache = temp_cache(1024 * 1024).await;
        for sz in [10usize, 20] {
            cache.add_to_memory_cache("http://a.test/same.png", CachedImage { data: Arc::new(vec![0u8; sz]) }).await;
        }
        let mem = cache.memory_cache.read().await;
        assert_eq!(mem.len(), 1);
        assert_eq!(mem.get("http://a.test/same.png").unwrap().data.len(), 20);
    }

    // ── db disk tier (save_to_disk / load_from_disk) ────────────────────

    #[tokio::test]
    async fn test_db_round_trip() {
        let cache = temp_cache(1024 * 1024).await;
        let data = tiny_png();
        cache.save_to_disk("https://example.com/art.png", &data).await;
        let loaded = cache.load_from_disk("https://example.com/art.png").await;
        assert_eq!(loaded.map(|c| (*c.data).clone()), Some(data));
    }

    #[tokio::test]
    async fn test_load_from_db_miss() {
        let cache = temp_cache(1024 * 1024).await;
        assert!(cache.load_from_disk("https://example.com/none.png").await.is_none());
    }

    #[tokio::test]
    async fn test_db_round_trip_multiple_urls() {
        let cache = temp_cache(1024 * 1024).await;
        for i in 0..5u8 {
            cache.save_to_disk(&format!("https://example.com/img{}.png", i), &vec![i; 100]).await;
        }
        for i in 0..5u8 {
            let loaded = cache.load_from_disk(&format!("https://example.com/img{}.png", i)).await;
            assert_eq!(loaded.map(|c| c.data.len()), Some(100));
        }
    }

    #[tokio::test]
    async fn test_grid_round_trip() {
        let cache = temp_cache(1024 * 1024).await;
        let png = tiny_png();
        assert!(cache.get_cached_grid("playlist-1").await.is_none());
        cache.save_grid("playlist-1", &png).await;
        assert_eq!(cache.get_cached_grid("playlist-1").await, Some(png));
    }

    #[tokio::test]
    async fn test_disk_tier_noop_without_db() {
        // No DB attached: disk-tier writes are silently skipped, reads miss,
        // and nothing panics.
        let cache = ImageCache::new(50);
        cache.save_to_disk("https://x/y.png", b"data").await;
        assert!(cache.load_from_disk("https://x/y.png").await.is_none());
        cache.save_grid("k", b"data").await;
        assert!(cache.get_cached_grid("k").await.is_none());
    }

    // ── is_fetchable ─────────────────────────────────────────────────────

    #[test]
    fn is_fetchable_accepts_https_only_outside_tests() {
        assert!(ImageCache::is_fetchable("https://resources.tidal.com/images/x/320x320.jpg"));
        // Artwork URLs come from API responses; these are the shapes a
        // hostile one would take.
        assert!(!ImageCache::is_fetchable("http://resources.tidal.com/images/x.jpg"));
        assert!(!ImageCache::is_fetchable("file:///etc/passwd"));
        assert!(!ImageCache::is_fetchable("ftp://example.test/x.png"));
        assert!(!ImageCache::is_fetchable("//example.test/x.png"));
    }

    /// The loopback exemption is `cfg(test)`-only, which is why the tests
    /// below can serve PNGs over plain HTTP while a release build cannot be
    /// talked into fetching from a service on the user's machine.
    #[test]
    fn loopback_http_is_a_test_only_allowance() {
        assert_eq!(ImageCache::is_fetchable("http://127.0.0.1:8080/x.png"), cfg!(test));
        assert_eq!(ImageCache::is_fetchable("http://localhost:8080/x.png"), cfg!(test));
    }

    // ── download_image ──────────────────────────────────────────────────

    #[tokio::test]
    async fn test_download_image_success() {
        let png = tiny_png();
        let (url, _port) = spawn_http_server(png.clone(), "image/png");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let cache = temp_cache(1024 * 1024).await;
        assert_eq!(cache.download_image(&url).await.ok(), Some(png));
    }

    #[tokio::test]
    async fn test_download_image_http_error() {
        let (url, _port) = spawn_http_error(404);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let cache = temp_cache(1024 * 1024).await;
        let err = cache.download_image(&url).await.unwrap_err();
        assert!(err.contains("HTTP error"));
    }

    #[tokio::test]
    async fn test_download_image_connection_refused() {
        let cache = temp_cache(1024 * 1024).await;
        let err = cache.download_image("http://127.0.0.1:1/image.png").await.unwrap_err();
        assert!(err.contains("Request failed"));
    }

    // ── get_or_load (full integration) ──────────────────────────────────

    #[tokio::test]
    async fn test_get_or_load_downloads_and_caches() {
        let png = tiny_png();
        let (url, _port) = spawn_http_server(png.clone(), "image/png");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let cache = temp_cache(1024 * 1024).await;

        assert_eq!(cache.get_or_load(&url).await.map(|c| (*c.data).clone()), Some(png));
        // Promoted into the memory tier and persisted to the db tier.
        assert!(cache.memory_cache.read().await.contains_key(&url as &str));
        assert!(cache.load_from_disk(&url).await.is_some());
    }

    #[tokio::test]
    async fn test_get_or_load_memory_hit() {
        let cache = temp_cache(1024 * 1024).await;
        let data = vec![42u8; 50];
        cache
            .memory_cache
            .write()
            .await
            .insert("http://a.test/cached.png".to_string(), CachedImage { data: Arc::new(data.clone()) });
        assert_eq!(cache.get_or_load("http://a.test/cached.png").await.map(|c| (*c.data).clone()), Some(data));
    }

    #[tokio::test]
    async fn test_get_or_load_db_hit_promotes_to_memory() {
        let cache = temp_cache(1024 * 1024).await;
        let data = tiny_png();
        let url = "https://example.com/disk-only.png";
        cache.save_to_disk(url, &data).await;
        assert!(!cache.memory_cache.read().await.contains_key(url));

        assert_eq!(cache.get_or_load(url).await.map(|c| (*c.data).clone()), Some(data));
        assert!(cache.memory_cache.read().await.contains_key(url));
    }

    #[tokio::test]
    async fn test_get_or_load_download_failure() {
        let (url, _port) = spawn_http_error(503);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let cache = temp_cache(1024 * 1024).await;
        assert!(cache.get_or_load(&url).await.is_none());
    }

    #[tokio::test]
    async fn test_get_or_load_second_call_uses_cache() {
        let png = tiny_png();
        let (url, _port) = spawn_http_server(png.clone(), "image/png");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let cache = temp_cache(1024 * 1024).await;

        assert!(cache.get_or_load(&url).await.is_some());
        // The one-shot server has closed; a second hit must come from cache.
        assert_eq!(cache.get_or_load(&url).await.map(|c| (*c.data).clone()), Some(png));
    }

    // ── CachedImage ─────────────────────────────────────────────────────

    #[test]
    fn test_cached_image_clone_shares_arc() {
        let img = CachedImage { data: Arc::new(vec![1, 2, 3]) };
        let cloned = img.clone();
        assert!(Arc::ptr_eq(&img.data, &cloned.data));
    }
}
