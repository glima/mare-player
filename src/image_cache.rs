// SPDX-License-Identifier: MIT

//! Image cache for album art and other images.
//!
//! This module provides async image loading with both memory and disk caching
//! to avoid repeated network requests for the same images. The disk layer is
//! backed by the general-purpose [`DiskCache`],
//! which handles size enforcement and LRU eviction.

use image::GenericImageView;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error};

use crate::disk_cache::DiskCache;

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
    /// Disk cache (size-limited, LRU-evicted)
    disk: DiskCache,
}

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
        let disk = DiskCache::xdg("images", max_disk_size_mb);
        let max_memory_bytes = (max_disk_size_mb as u64) * 1024 * 1024 / 10;

        Self {
            memory_cache: Arc::new(RwLock::new(HashMap::new())),
            client: reqwest::Client::new(),
            max_memory_bytes,
            disk,
        }
    }

    /// Get the disk cache path for a URL (hashed filename with extension).
    fn disk_cache_ext(url: &str) -> &str {
        url.rsplit('/')
            .next()
            .and_then(|s| s.rsplit('.').next())
            .filter(|e| e.len() <= 4 && e.chars().all(|c| c.is_alphanumeric()))
            .unwrap_or("jpg")
    }

    /// Try to load an image from disk cache
    async fn load_from_disk(&self, url: &str) -> Option<CachedImage> {
        let ext = Self::disk_cache_ext(url);
        self.disk.get_hashed_async(url, ext).await.map(|data| {
            debug!("Disk cache hit: {}", url);
            CachedImage {
                data: Arc::new(data),
            }
        })
    }

    /// Save an image to disk cache, enforcing max size via DiskCache eviction
    async fn save_to_disk(&self, url: &str, data: &[u8]) {
        let ext = Self::disk_cache_ext(url);
        if let Err(e) = self.disk.put_hashed_async(url, ext, data).await {
            tracing::warn!("Failed to write image to disk cache: {}", e);
        } else {
            debug!("Saved to disk cache: {} ({} bytes)", url, data.len());
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
                let cached = CachedImage {
                    data: Arc::new(data),
                };

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

    /// Try to load a cached grid thumbnail from disk.
    ///
    /// `cache_key` should be a stable identifier (e.g. playlist UUID or a
    /// hash of the cover URLs).  Returns `None` on cache miss.
    pub async fn get_cached_grid(&self, cache_key: &str) -> Option<Vec<u8>> {
        self.disk.get_hashed_async(cache_key, "png").await
    }

    /// Save a generated grid thumbnail PNG to the disk cache.
    pub async fn save_grid(&self, cache_key: &str, png_data: &[u8]) {
        if let Err(e) = self.disk.put_hashed_async(cache_key, "png", png_data).await {
            tracing::warn!("Failed to cache grid thumbnail: {}", e);
        }
    }

    /// Download an image from a URL
    async fn download_image(&self, url: &str) -> Result<Vec<u8>, String> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read response: {}", e))?;

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
    let positions = [
        (0u32, 0u32),
        (half + gap, 0),
        (0, half + gap),
        (half + gap, half + gap),
    ];

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

    Ok(RgbaPixels {
        width: output_size,
        height: output_size,
        pixels: canvas.into_raw(),
    })
}

/// Make an image circular by applying an alpha mask.
/// Takes raw image bytes (JPEG/PNG) and returns raw RGBA pixels with circular transparency.
pub fn make_circular(image_data: &[u8]) -> Result<RgbaPixels, String> {
    // Decode the image
    let img = image::load_from_memory(image_data)
        .map_err(|e| format!("Failed to decode image: {}", e))?;

    let (width, height) = img.dimensions();
    let size = width.min(height);

    // Crop to square (center crop)
    let x_offset = (width - size) / 2;
    let y_offset = (height - size) / 2;
    let cropped = img.crop_imm(x_offset, y_offset, size, size);

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

    Ok(RgbaPixels {
        width: size,
        height: size,
        pixels: rgba.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    // ── helpers ──────────────────────────────────────────────────────────

    /// Build an `ImageCache` backed by a temporary directory so tests
    /// never touch the real XDG cache.  `max_memory_bytes` is set to
    /// the supplied value for fine-grained eviction testing.
    fn temp_cache(dir: &std::path::Path, max_memory_bytes: u64) -> ImageCache {
        let disk = DiskCache::new(dir.to_path_buf(), 50); // 50 MB disk limit
        ImageCache {
            memory_cache: Arc::new(RwLock::new(HashMap::new())),
            client: reqwest::Client::new(),
            max_memory_bytes,
            disk,
        }
    }

    /// Create a minimal valid 1×1 red PNG in memory (~67-70 bytes).
    fn tiny_png() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        img.write_to(&mut cursor, image::ImageFormat::Png)
            .expect("encode tiny png");
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
                let resp = format!(
                    "HTTP/1.1 {} Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    status
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });

        (url, port)
    }

    // ── disk_cache_ext ──────────────────────────────────────────────────

    #[test]
    fn test_disk_cache_ext() {
        assert_eq!(
            ImageCache::disk_cache_ext("https://example.com/image.jpg"),
            "jpg"
        );
        assert_eq!(
            ImageCache::disk_cache_ext("https://example.com/image.png"),
            "png"
        );
        assert_eq!(
            ImageCache::disk_cache_ext("https://example.com/image"),
            "jpg"
        ); // fallback
    }

    #[test]
    fn test_disk_cache_ext_webp() {
        assert_eq!(
            ImageCache::disk_cache_ext("https://cdn.tidal.com/artwork.webp"),
            "webp"
        );
    }

    #[test]
    fn test_disk_cache_ext_query_string() {
        // The last path segment is "photo.png?size=large", the extension
        // after the last '.' is "png?size=large" which is >4 chars and
        // contains '?' → should fall back to "jpg".
        assert_eq!(
            ImageCache::disk_cache_ext("https://example.com/photo.png?size=large"),
            "jpg"
        );
    }

    #[test]
    fn test_disk_cache_ext_no_path() {
        // Trailing slash → last segment is "", and "" passes the filter
        // (len 0 ≤ 4, vacuously alphanumeric), so we get "".
        assert_eq!(ImageCache::disk_cache_ext("https://example.com/"), "");
    }

    #[test]
    fn test_disk_cache_ext_very_long_extension() {
        // Extension > 4 chars → fallback
        assert_eq!(
            ImageCache::disk_cache_ext("https://example.com/data.jsondata"),
            "jpg"
        );
    }

    #[test]
    fn test_disk_cache_ext_empty_url() {
        // Empty string → rsplit('/') yields [""], rsplit('.') yields [""],
        // "" passes the filter (vacuously alphanumeric, len ≤ 4).
        assert_eq!(ImageCache::disk_cache_ext(""), "");
    }

    #[test]
    fn test_disk_cache_ext_gif() {
        assert_eq!(
            ImageCache::disk_cache_ext("https://example.com/anim.gif"),
            "gif"
        );
    }

    // ── constructors ────────────────────────────────────────────────────

    #[test]
    fn test_disk_cache_path() {
        let cache = ImageCache::new(100);
        // Just verify it doesn't panic and produces a path ending with the right extension
        let ext = ImageCache::disk_cache_ext("https://example.com/image.jpg");
        let path = cache.disk.hashed_path("https://example.com/image.jpg", ext);
        assert!(path.to_string_lossy().ends_with(".jpg"));
    }

    #[test]
    fn test_new_sets_memory_limit() {
        // 200 MB disk → 20 MB RAM (10%)
        let cache = ImageCache::new(200);
        assert_eq!(cache.max_memory_bytes, 200 * 1024 * 1024 / 10);
    }

    #[test]
    fn test_default_uses_200mb_disk() {
        let cache = ImageCache::default();
        // default is 200 MB disk
        assert_eq!(cache.max_memory_bytes, 200 * 1024 * 1024 / 10);
    }

    #[test]
    fn test_new_zero_mb() {
        let cache = ImageCache::new(0);
        assert_eq!(cache.max_memory_bytes, 0);
    }

    #[test]
    fn test_clone_shares_memory_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = temp_cache(tmp.path(), 1024 * 1024);
        let clone = cache.clone();
        // Both refer to the same Arc
        assert!(Arc::ptr_eq(&cache.memory_cache, &clone.memory_cache));
    }

    // ── add_to_memory_cache ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_add_to_memory_cache_basic() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = temp_cache(tmp.path(), 1024 * 1024); // 1 MB limit

        let img = CachedImage {
            data: Arc::new(vec![1, 2, 3, 4]),
        };
        cache.add_to_memory_cache("http://a.test/1.png", img).await;

        let mem = cache.memory_cache.read().await;
        assert!(mem.contains_key("http://a.test/1.png"));
        assert_eq!(mem.get("http://a.test/1.png").unwrap().data.len(), 4);
    }

    #[tokio::test]
    async fn test_add_to_memory_cache_multiple() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = temp_cache(tmp.path(), 1024 * 1024);

        for i in 0..5 {
            let img = CachedImage {
                data: Arc::new(vec![i; 10]),
            };
            cache
                .add_to_memory_cache(&format!("http://a.test/{}.png", i), img)
                .await;
        }

        let mem = cache.memory_cache.read().await;
        assert_eq!(mem.len(), 5);
    }

    #[tokio::test]
    async fn test_add_to_memory_cache_eviction() {
        let tmp = tempfile::tempdir().unwrap();
        // Set limit to 20 bytes — inserting items larger than that forces eviction
        let cache = temp_cache(tmp.path(), 20);

        // Insert 3 items of 8 bytes each (24 total > 20 limit)
        for i in 0..3u8 {
            let img = CachedImage {
                data: Arc::new(vec![i; 8]),
            };
            cache
                .add_to_memory_cache(&format!("http://a.test/{}.png", i), img)
                .await;
        }

        let mem = cache.memory_cache.read().await;
        // At least one old entry should have been evicted
        assert!(mem.len() <= 3);
        // The newest entry should always be present
        assert!(mem.contains_key("http://a.test/2.png"));
        // Total size should be <= 20
        let total: u64 = mem.values().map(|v| v.data.len() as u64).sum();
        assert!(total <= 20);
    }

    #[tokio::test]
    async fn test_add_to_memory_cache_evicts_all_when_single_item_exceeds_limit() {
        let tmp = tempfile::tempdir().unwrap();
        // Limit of 10 bytes
        let cache = temp_cache(tmp.path(), 10);

        // Pre-fill with small items
        for i in 0..3u8 {
            let img = CachedImage {
                data: Arc::new(vec![i; 3]),
            };
            cache
                .add_to_memory_cache(&format!("http://a.test/{}.png", i), img)
                .await;
        }

        // Now insert one item that is larger than the limit.  The eviction
        // loop removes everything it can; the item is still inserted (the
        // code doesn't reject oversized items).
        let big = CachedImage {
            data: Arc::new(vec![99; 15]),
        };
        cache
            .add_to_memory_cache("http://a.test/big.png", big)
            .await;

        let mem = cache.memory_cache.read().await;
        // Only the big item remains (old ones evicted, but it still inserts)
        assert!(mem.contains_key("http://a.test/big.png"));
        assert_eq!(mem.len(), 1);
    }

    #[tokio::test]
    async fn test_add_to_memory_cache_replaces_same_key() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = temp_cache(tmp.path(), 1024 * 1024);

        let img1 = CachedImage {
            data: Arc::new(vec![1; 10]),
        };
        cache
            .add_to_memory_cache("http://a.test/same.png", img1)
            .await;

        let img2 = CachedImage {
            data: Arc::new(vec![2; 20]),
        };
        cache
            .add_to_memory_cache("http://a.test/same.png", img2)
            .await;

        let mem = cache.memory_cache.read().await;
        assert_eq!(mem.len(), 1);
        assert_eq!(mem.get("http://a.test/same.png").unwrap().data.len(), 20);
    }

    // ── save_to_disk / load_from_disk ───────────────────────────────────

    #[tokio::test]
    async fn test_disk_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = temp_cache(tmp.path(), 1024 * 1024);
        let data = tiny_png();

        cache
            .save_to_disk("https://example.com/art.png", &data)
            .await;

        let loaded = cache.load_from_disk("https://example.com/art.png").await;
        assert!(loaded.is_some());
        assert_eq!(&*loaded.unwrap().data, &data);
    }

    #[tokio::test]
    async fn test_load_from_disk_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = temp_cache(tmp.path(), 1024 * 1024);

        let loaded = cache
            .load_from_disk("https://example.com/nonexistent.png")
            .await;
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_save_to_disk_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = temp_cache(tmp.path(), 1024 * 1024);
        let data = b"fake image bytes";

        cache
            .save_to_disk("https://example.com/photo.jpg", data)
            .await;

        let ext = ImageCache::disk_cache_ext("https://example.com/photo.jpg");
        let path = cache.disk.hashed_path("https://example.com/photo.jpg", ext);
        assert!(path.exists());
    }

    #[tokio::test]
    async fn test_disk_round_trip_multiple_urls() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = temp_cache(tmp.path(), 1024 * 1024);

        for i in 0..5u8 {
            let url = format!("https://example.com/img{}.png", i);
            let data = vec![i; 100];
            cache.save_to_disk(&url, &data).await;
        }

        for i in 0..5u8 {
            let url = format!("https://example.com/img{}.png", i);
            let loaded = cache.load_from_disk(&url).await;
            assert!(loaded.is_some(), "expected disk hit for {}", url);
            assert_eq!(loaded.unwrap().data.len(), 100);
        }
    }

    // ── download_image ──────────────────────────────────────────────────

    #[tokio::test]
    async fn test_download_image_success() {
        let png = tiny_png();
        let (url, _port) = spawn_http_server(png.clone(), "image/png");
        // Give the server thread a moment to start listening
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let tmp = tempfile::tempdir().unwrap();
        let cache = temp_cache(tmp.path(), 1024 * 1024);

        let result = cache.download_image(&url).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), png);
    }

    #[tokio::test]
    async fn test_download_image_http_error() {
        let (url, _port) = spawn_http_error(404);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let tmp = tempfile::tempdir().unwrap();
        let cache = temp_cache(tmp.path(), 1024 * 1024);

        let result = cache.download_image(&url).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("HTTP error"));
    }

    #[tokio::test]
    async fn test_download_image_500_error() {
        let (url, _port) = spawn_http_error(500);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let tmp = tempfile::tempdir().unwrap();
        let cache = temp_cache(tmp.path(), 1024 * 1024);

        let result = cache.download_image(&url).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("HTTP error"));
    }

    #[tokio::test]
    async fn test_download_image_connection_refused() {
        // Use a port that is almost certainly not listening
        let tmp = tempfile::tempdir().unwrap();
        let cache = temp_cache(tmp.path(), 1024 * 1024);

        let result = cache.download_image("http://127.0.0.1:1/image.png").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Request failed"));
    }

    // ── get_or_load (full integration) ──────────────────────────────────

    #[tokio::test]
    async fn test_get_or_load_downloads_and_caches() {
        let png = tiny_png();
        let (url, _port) = spawn_http_server(png.clone(), "image/png");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let tmp = tempfile::tempdir().unwrap();
        let cache = temp_cache(tmp.path(), 1024 * 1024);

        // First call — downloads from network
        let result = cache.get_or_load(&url).await;
        assert!(result.is_some());
        assert_eq!(&*result.unwrap().data, &png);

        // Verify it's now in memory cache
        {
            let mem = cache.memory_cache.read().await;
            assert!(mem.contains_key(&url as &str));
        }

        // Verify it's on disk
        let disk_hit = cache.load_from_disk(&url).await;
        assert!(disk_hit.is_some());
    }

    #[tokio::test]
    async fn test_get_or_load_memory_hit() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = temp_cache(tmp.path(), 1024 * 1024);

        // Pre-populate memory cache directly
        let data = vec![42u8; 50];
        let img = CachedImage {
            data: Arc::new(data.clone()),
        };
        {
            let mut mem = cache.memory_cache.write().await;
            mem.insert("http://a.test/cached.png".to_string(), img);
        }

        // Should return from memory without any network call
        let result = cache.get_or_load("http://a.test/cached.png").await;
        assert!(result.is_some());
        assert_eq!(&*result.unwrap().data, &data);
    }

    #[tokio::test]
    async fn test_get_or_load_disk_hit() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = temp_cache(tmp.path(), 1024 * 1024);
        let data = tiny_png();
        let url = "https://example.com/disk-only.png";

        // Write directly to disk (not in memory)
        cache.save_to_disk(url, &data).await;

        // Verify memory is empty
        {
            let mem = cache.memory_cache.read().await;
            assert!(!mem.contains_key(url));
        }

        // get_or_load should find it on disk and promote to memory
        let result = cache.get_or_load(url).await;
        assert!(result.is_some());
        assert_eq!(&*result.unwrap().data, &data);

        // Should now be in memory too
        {
            let mem = cache.memory_cache.read().await;
            assert!(mem.contains_key(url));
        }
    }

    #[tokio::test]
    async fn test_get_or_load_download_failure() {
        let (url, _port) = spawn_http_error(503);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let tmp = tempfile::tempdir().unwrap();
        let cache = temp_cache(tmp.path(), 1024 * 1024);

        let result = cache.get_or_load(&url).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_or_load_connection_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = temp_cache(tmp.path(), 1024 * 1024);

        let result = cache.get_or_load("http://127.0.0.1:1/no-server.png").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_or_load_second_call_uses_memory() {
        let png = tiny_png();
        let (url, _port) = spawn_http_server(png.clone(), "image/png");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let tmp = tempfile::tempdir().unwrap();
        let cache = temp_cache(tmp.path(), 1024 * 1024);

        // First call downloads
        let r1 = cache.get_or_load(&url).await;
        assert!(r1.is_some());

        // Second call — the mock server already closed, so if this
        // tried to download again it would fail.  Success means memory
        // cache is being used.
        let r2 = cache.get_or_load(&url).await;
        assert!(r2.is_some());
        assert_eq!(&*r2.unwrap().data, &png);
    }

    // ── CachedImage ─────────────────────────────────────────────────────

    #[test]
    fn test_cached_image_clone() {
        let img = CachedImage {
            data: Arc::new(vec![1, 2, 3]),
        };
        let cloned = img.clone();
        assert_eq!(&*img.data, &*cloned.data);
        // Clone shares the same Arc allocation
        assert!(Arc::ptr_eq(&img.data, &cloned.data));
    }

    #[test]
    fn test_cached_image_empty_data() {
        let img = CachedImage {
            data: Arc::new(Vec::new()),
        };
        assert!(img.data.is_empty());
    }

    // ── save_to_disk error path ─────────────────────────────────────────

    #[tokio::test]
    async fn test_save_to_disk_readonly_dir_does_not_panic() {
        // On Linux we can make the dir read-only to trigger the warn! path
        // inside save_to_disk.  This must not panic.
        let tmp = tempfile::tempdir().unwrap();
        let cache = temp_cache(tmp.path(), 1024 * 1024);

        // Make the cache directory read-only
        let mut perms = std::fs::metadata(tmp.path()).unwrap().permissions();
        // Deliberately toggling permissions to exercise the error path
        // in save_to_disk; `set_readonly` is the simplest portable way.
        #[allow(clippy::permissions_set_readonly_false)]
        {
            perms.set_readonly(true);
        }
        std::fs::set_permissions(tmp.path(), perms.clone()).unwrap();

        // This should hit the Err branch and warn, not panic
        cache
            .save_to_disk("https://example.com/fail.png", b"data")
            .await;

        // Restore write permission so tempdir cleanup can remove the dir.
        #[allow(clippy::permissions_set_readonly_false)]
        {
            perms.set_readonly(false);
        }
        std::fs::set_permissions(tmp.path(), perms).unwrap();
    }
}
