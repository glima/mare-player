// SPDX-License-Identifier: MIT

//! Audio decoder using symphonia for various formats.
//!
//! This module provides audio decoding for FLAC, AAC, and MP3
//! using the pure Rust symphonia library.

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex as StdMutex};
use std::time::Duration;

use symphonia::core::audio::AudioSpec;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::{AudioDecoder as AudioDecoderTrait, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, ReadOnlySource};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;
use tracing::{debug, error, info, warn};

/// Decoded audio format information
#[derive(Debug, Clone)]
pub struct AudioFormat {
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Number of audio channels
    pub channels: usize,
    /// Total duration in seconds (if known)
    pub duration: Option<f64>,
}

/// Result type for decoder operations
pub type DecoderResult<T> = Result<T, DecoderError>;

/// Errors that can occur during decoding
#[derive(Debug)]
pub enum DecoderError {
    /// Failed to open or probe the media
    OpenError(String),
    /// No supported audio track found
    NoAudioTrack,
    /// Decoder initialization failed
    DecoderInit(String),
    /// Decoding error
    DecodeError(String),
    /// Seek error
    SeekError(String),
    /// IO error
    IoError(String),
    /// Unsupported format
    UnsupportedFormat(String),
}

impl std::fmt::Display for DecoderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpenError(msg) => write!(f, "Failed to open media: {}", msg),
            Self::NoAudioTrack => write!(f, "No audio track found"),
            Self::DecoderInit(msg) => write!(f, "Decoder initialization failed: {}", msg),
            Self::DecodeError(msg) => write!(f, "Decoding error: {}", msg),
            Self::SeekError(msg) => write!(f, "Seek error: {}", msg),
            Self::IoError(msg) => write!(f, "IO error: {}", msg),
            Self::UnsupportedFormat(msg) => write!(f, "Unsupported format: {}", msg),
        }
    }
}

impl std::error::Error for DecoderError {}

impl From<SymphoniaError> for DecoderError {
    fn from(err: SymphoniaError) -> Self {
        match err {
            SymphoniaError::IoError(e) => DecoderError::IoError(e.to_string()),
            SymphoniaError::DecodeError(msg) => DecoderError::DecodeError(msg.to_string()),
            SymphoniaError::SeekError(kind) => DecoderError::SeekError(format!("{:?}", kind)),
            SymphoniaError::Unsupported(msg) => DecoderError::UnsupportedFormat(msg.to_string()),
            _ => DecoderError::DecodeError(format!("{:?}", err)),
        }
    }
}

/// A wrapper for bytes that implements MediaSource
struct BytesMediaSource {
    cursor: Cursor<Vec<u8>>,
    len: u64,
}

impl BytesMediaSource {
    fn new(data: Vec<u8>) -> Self {
        let len = data.len() as u64;
        Self {
            cursor: Cursor::new(data),
            len,
        }
    }
}

impl Read for BytesMediaSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.cursor.read(buf)
    }
}

impl Seek for BytesMediaSource {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.cursor.seek(pos)
    }
}

impl MediaSource for BytesMediaSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

// =============================================================================
// Streaming HTTP media source
// =============================================================================

/// Minimum bytes to buffer before attempting to probe/decode the stream.
const MIN_PROBE_BYTES: u64 = 65_536;

/// How long `Read::read` waits for new data before re-checking the abort flag.
const READ_WAIT_TIMEOUT: Duration = Duration::from_millis(50);

/// Shared state between the download thread and the [`HttpStreamingSource`]
/// reader.  All fields are safe to access from multiple threads.
struct StreamingBuffer {
    /// Accumulated bytes downloaded so far.  Append-only (chunks are pushed
    /// by the download thread; the reader only ever reads).
    data: StdMutex<Vec<u8>>,
    /// Signalled whenever new bytes are appended or the download finishes.
    notify: Condvar,
    /// Total bytes downloaded so far (also readable via `data.len()` but
    /// kept as an atomic to avoid locking the mutex on the hot path).
    bytes_downloaded: AtomicU64,
    /// `true` once the entire response body has been consumed (or an error
    /// terminated the download).
    download_complete: AtomicBool,
    /// Set by the consumer (engine) to tell the download thread to stop
    /// early (e.g. when the user presses Stop or starts a new track).
    abort: AtomicBool,
}

impl StreamingBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            data: StdMutex::new(Vec::with_capacity(capacity)),
            notify: Condvar::new(),
            bytes_downloaded: AtomicU64::new(0),
            download_complete: AtomicBool::new(false),
            abort: AtomicBool::new(false),
        }
    }
}

/// A [`MediaSource`] whose backing bytes arrive incrementally from an HTTP
/// download running on a background thread.
///
/// *  `Read::read` blocks (with a short timeout) when the read position is
///    past the end of the data downloaded so far, then resumes as soon as more
///    data arrives.
/// *  `Seek` works freely within the already-downloaded region and blocks
///    briefly when seeking beyond it (the data will arrive in order).
/// *  An *abort* flag lets the owning engine cancel a stalled download.
struct HttpStreamingSource {
    /// Shared buffer + signalling.
    shared: Arc<StreamingBuffer>,
    /// Current byte-offset for the next `read()` call.
    position: u64,
    /// Total size of the resource (`Content-Length`), if the server provided
    /// it.
    content_length: Option<u64>,
}

impl Read for HttpStreamingSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            // Fast path – data is already available.
            {
                let data = self
                    .shared
                    .data
                    .lock()
                    .map_err(|e| std::io::Error::other(format!("lock poisoned: {e}")))?;
                let pos = self.position as usize;
                if pos < data.len() {
                    let available = data.len() - pos;
                    let to_read = available.min(buf.len());
                    if let (Some(dst), Some(src)) =
                        (buf.get_mut(..to_read), data.get(pos..pos + to_read))
                    {
                        dst.copy_from_slice(src);
                    }
                    self.position += to_read as u64;
                    return Ok(to_read);
                }
                // EOF – download finished and we've read everything.
                if self.shared.download_complete.load(Ordering::Acquire) {
                    return Ok(0);
                }
            }

            // Check abort.
            if self.shared.abort.load(Ordering::Acquire) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "download aborted",
                ));
            }

            // Wait for the download thread to push more data.
            let data = self
                .shared
                .data
                .lock()
                .map_err(|e| std::io::Error::other(format!("lock poisoned: {e}")))?;
            let _guard = self
                .shared
                .notify
                .wait_timeout(data, READ_WAIT_TIMEOUT)
                .map_err(|e| std::io::Error::other(format!("lock poisoned: {e}")))?;
            // Loop back to re-check availability.
        }
    }
}

impl Seek for HttpStreamingSource {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        let new_pos = match pos {
            std::io::SeekFrom::Start(offset) => offset,
            std::io::SeekFrom::Current(offset) => {
                if offset >= 0 {
                    self.position.saturating_add(offset as u64)
                } else {
                    self.position.saturating_sub(offset.unsigned_abs())
                }
            }
            std::io::SeekFrom::End(offset) => {
                let len = self
                    .content_length
                    .unwrap_or_else(|| self.shared.bytes_downloaded.load(Ordering::Acquire));
                if offset >= 0 {
                    len.saturating_add(offset as u64)
                } else {
                    len.saturating_sub(offset.unsigned_abs())
                }
            }
        };
        self.position = new_pos;
        Ok(new_pos)
    }
}

impl MediaSource for HttpStreamingSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        self.content_length
    }
}

/// Handle returned from [`AudioDecoder::from_url_streaming`] that lets the
/// caller abort the background download when playback is stopped or a new
/// track is started.
pub struct DownloadHandle {
    shared: Arc<StreamingBuffer>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl DownloadHandle {
    /// Signal the download thread to stop and wait for it to finish.
    pub fn abort(&mut self) {
        self.shared.abort.store(true, Ordering::Release);
        self.shared.notify.notify_all();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }

    /// Returns `true` when the entire file has been downloaded.
    pub fn is_complete(&self) -> bool {
        self.shared.download_complete.load(Ordering::Acquire)
    }
}

impl Drop for DownloadHandle {
    fn drop(&mut self) {
        self.abort();
    }
}

/// Audio decoder that wraps symphonia
pub struct AudioDecoder {
    /// The format reader
    format: Box<dyn FormatReader>,
    /// The audio decoder
    decoder: Box<dyn AudioDecoderTrait>,
    /// Track ID of the audio track
    track_id: u32,
    /// Audio format information
    pub format_info: AudioFormat,
    /// Audio spec of the decoded audio
    spec: AudioSpec,
}

impl AudioDecoder {
    /// Create a decoder from a file path
    pub fn from_file<P: AsRef<Path>>(path: P) -> DecoderResult<Self> {
        let path = path.as_ref();
        info!("Opening audio file: {:?}", path);

        let file = File::open(path)
            .map_err(|e| DecoderError::IoError(format!("Failed to open: {}", e)))?;

        let reader = BufReader::new(file);
        let source: Box<dyn MediaSource> = Box::new(ReadOnlySource::new(reader));
        let mss = MediaSourceStream::new(source, Default::default());

        // Create a hint based on file extension
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }

        Self::from_media_source_stream(mss, hint)
    }

    /// Create a decoder from raw bytes (e.g., downloaded stream)
    pub fn from_bytes(data: Vec<u8>, format_hint: Option<&str>) -> DecoderResult<Self> {
        info!("Creating decoder from {} bytes", data.len());

        let source: Box<dyn MediaSource> = Box::new(BytesMediaSource::new(data));
        let mss = MediaSourceStream::new(source, Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = format_hint {
            hint.with_extension(ext);
        }

        Self::from_media_source_stream(mss, hint)
    }

    /// Create a decoder from a URL by downloading the content
    /// Start an HTTP download in the background and return a decoder as soon
    /// as enough data has been buffered to probe the format (typically
    /// <100ms).  The rest of the file streams in while playback proceeds.
    ///
    /// The returned [`DownloadHandle`] **must** be kept alive for the duration
    /// of playback — dropping it aborts the download.
    pub fn from_url_streaming(
        url: &str,
        cache_path: Option<String>,
    ) -> DecoderResult<(Self, DownloadHandle)> {
        let url_display = url.get(..url.len().min(80)).unwrap_or(url);
        info!("Streaming audio from URL: {url_display}...");

        // -- shared state --------------------------------------------------
        // Pre-allocate 4 MiB; the vec will grow as chunks arrive.
        let shared = Arc::new(StreamingBuffer::new(4 * 1024 * 1024));

        let shared_dl = Arc::clone(&shared);
        let url_owned = url.to_string();

        // Channel to receive headers (content-length, format hint) from the
        // download thread.
        let (header_tx, header_rx) =
            std::sync::mpsc::sync_channel::<Result<(Option<u64>, Option<String>), String>>(1);

        // -- download thread -----------------------------------------------
        let thread = std::thread::Builder::new()
            .name("audio-download".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = header_tx.send(Err(format!("runtime init failed: {e}")));
                        return;
                    }
                };

                rt.block_on(async move {
                    // Start the HTTP request.
                    let mut response = match reqwest::get(&url_owned).await {
                        Ok(r) => r,
                        Err(e) => {
                            let _ = header_tx.send(Err(format!("HTTP request failed: {e}")));
                            shared_dl.download_complete.store(true, Ordering::Release);
                            shared_dl.notify.notify_all();
                            return;
                        }
                    };

                    if !response.status().is_success() {
                        let _ = header_tx.send(Err(format!("HTTP error: {}", response.status())));
                        shared_dl.download_complete.store(true, Ordering::Release);
                        shared_dl.notify.notify_all();
                        return;
                    }

                    // Extract headers.
                    let content_length = response.content_length();
                    let content_type = response
                        .headers()
                        .get("content-type")
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.to_string());

                    let format_hint = content_type.as_ref().and_then(|ct| {
                        if ct.contains("flac") {
                            Some("flac".to_string())
                        } else if ct.contains("mp4") || ct.contains("aac") || ct.contains("m4a") {
                            Some("m4a".to_string())
                        } else if ct.contains("mpeg") || ct.contains("mp3") {
                            Some("mp3".to_string())
                        } else {
                            None
                        }
                    });

                    info!(
                        "Stream headers: content-length={:?}, type={:?}",
                        content_length, content_type,
                    );

                    // Tell the caller about the headers.
                    let _ = header_tx.send(Ok((content_length, format_hint)));

                    // Pre-allocate buffer when content-length is known.
                    if let Some(cl) = content_length
                        && let Ok(mut data) = shared_dl.data.lock()
                    {
                        data.reserve(cl.min(256 * 1024 * 1024) as usize);
                    }

                    // Stream the body chunk by chunk.
                    loop {
                        if shared_dl.abort.load(Ordering::Acquire) {
                            info!("Download aborted by consumer");
                            break;
                        }

                        match response.chunk().await {
                            Ok(Some(chunk)) => {
                                let len = chunk.len() as u64;
                                if let Ok(mut data) = shared_dl.data.lock() {
                                    data.extend_from_slice(&chunk);
                                }
                                shared_dl.bytes_downloaded.fetch_add(len, Ordering::Release);
                                shared_dl.notify.notify_all();
                            }
                            Ok(None) => break, // EOF
                            Err(e) => {
                                error!("Download chunk error: {e}");
                                break;
                            }
                        }
                    }

                    let aborted = shared_dl.abort.load(Ordering::Acquire);
                    let total = shared_dl.bytes_downloaded.load(Ordering::Relaxed);
                    info!("Download finished: {total} bytes (aborted: {aborted})");

                    // Save to audio cache if a cache path was provided AND the
                    // download completed normally.  When the user skips tracks
                    // quickly the download is aborted mid-stream; saving the
                    // partial data would poison the cache and cause
                    // "unexpected end of file" errors on the next cache hit.
                    if let Some(ref save_path) = cache_path {
                        if aborted {
                            // Remove any stale/partial file that a previous
                            // (also-aborted) run may have left behind.
                            if std::path::Path::new(save_path).exists() {
                                let _ = std::fs::remove_file(save_path);
                                info!("Removed partial cache file after abort: {}", save_path,);
                            }
                        } else if let Ok(data) = shared_dl.data.lock() {
                            if let Some(parent) = std::path::Path::new(save_path).parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            match std::fs::write(save_path, &*data) {
                                Ok(()) => {
                                    info!(
                                        "Saved downloaded audio to cache: {} ({:.1} MB)",
                                        save_path,
                                        data.len() as f64 / (1024.0 * 1024.0),
                                    );
                                }
                                Err(e) => {
                                    warn!("Failed to save audio to cache {}: {}", save_path, e);
                                }
                            }
                        }
                    }

                    shared_dl.download_complete.store(true, Ordering::Release);
                    shared_dl.notify.notify_all();
                });
            })
            .map_err(|e| DecoderError::IoError(format!("spawn download thread: {e}")))?;

        // -- wait for headers ---------------------------------------------
        let (content_length, format_hint) = header_rx
            .recv()
            .map_err(|_| {
                DecoderError::IoError("download thread died before sending headers".into())
            })?
            .map_err(DecoderError::IoError)?;

        // -- wait for enough data to probe --------------------------------
        let min_bytes = content_length
            .map(|cl| cl.min(MIN_PROBE_BYTES))
            .unwrap_or(MIN_PROBE_BYTES);

        loop {
            let downloaded = shared.bytes_downloaded.load(Ordering::Acquire);
            if downloaded >= min_bytes || shared.download_complete.load(Ordering::Acquire) {
                info!(
                    "Initial buffer ready: {downloaded} bytes (needed {min_bytes}), starting decode"
                );
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        // -- create the streaming source & decoder ------------------------
        let source: Box<dyn MediaSource> = Box::new(HttpStreamingSource {
            shared: Arc::clone(&shared),
            position: 0,
            content_length,
        });

        let mss = MediaSourceStream::new(source, Default::default());

        let mut hint = Hint::new();
        if let Some(ref ext) = format_hint {
            hint.with_extension(ext);
        }

        let decoder = Self::from_media_source_stream(mss, hint)?;

        let handle = DownloadHandle {
            shared,
            thread: Some(thread),
        };

        Ok((decoder, handle))
    }

    /// Download and decode DASH/HiRes content (fMP4 + FLAC).
    ///
    /// # Engineering compromise: full download before playback
    ///
    /// This function downloads **all** DASH segments into memory before
    /// creating the decoder.  Segments are fetched in parallel batches
    /// (sized to `nproc`), so a typical HiRes track (60–200 MB) loads
    /// in roughly 1–2 seconds on a decent connection.
    ///
    /// Progressive/streaming playback was attempted extensively but is
    /// blocked by a hard constraint in symphonia's `MediaSourceStream`:
    /// it **caches `byte_len()`** at construction time, and the isomp4
    /// format reader uses that cached value as the authoritative file
    /// size.  Every partial-buffer strategy we tried hits one of these
    /// walls:
    ///
    /// | `byte_len` value        | Probe phase           | Decode phase            |
    /// |-------------------------|-----------------------|-------------------------|
    /// | `None`                  | ❌ "not seekable"     | ✅ Read-based EOF       |
    /// | `Some(initial_buffer)`  | ✅ fast               | ❌ premature EOF (~4 s) |
    /// | `Some(large_estimate)`  | ❌ hangs (seeks far)  | ✅ streaming works      |
    /// | dynamic (phase toggle)  | — cached, no effect — | — cached, no effect —   |
    ///
    /// Truly fixing this would require either a custom fMP4 demuxer that
    /// feeds raw FLAC frames to symphonia's codec decoder, or an upstream
    /// change to symphonia to support growable streams.  Until then, the
    /// full-download path is the only reliable option.
    ///
    /// Returns `(decoder, download_handle, duration, sample_rate)`.
    /// Saves the fully-downloaded audio data to `cache_path` (when `Some`)
    /// so subsequent plays can use [`Self::from_file`] instead.
    pub fn from_dash_streaming_cached<P, F>(
        manifest_path: P,
        on_progress: &mut F,
        cache_path: Option<String>,
    ) -> DecoderResult<(Self, DownloadHandle, f64, u32)>
    where
        P: AsRef<std::path::Path>,
        F: FnMut(f64),
    {
        use super::dash::DashManifest;

        let manifest_path = manifest_path.as_ref();
        info!("Loading DASH from: {}", manifest_path.display());

        // -- parse manifest (fast, local file) ----------------------------
        let manifest = DashManifest::parse(
            &std::fs::read_to_string(manifest_path)
                .map_err(|e| DecoderError::IoError(format!("read manifest: {e}")))?,
        )
        .map_err(|e| DecoderError::IoError(format!("parse manifest: {e}")))?;

        let audio_info = manifest.audio_info.clone();
        let duration = audio_info.duration;
        let sample_rate = audio_info.sample_rate;
        let init_url = manifest.init_url.clone();
        let segment_urls: Vec<String> = manifest.segment_urls.clone();
        let segment_count = segment_urls.len();

        info!(
            "DASH stream: {}Hz, {} segments, {:.2}s, codec={}",
            sample_rate, segment_count, duration, audio_info.codec,
        );

        // -- download ALL segments ----------------------------------------
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| DecoderError::IoError(format!("runtime init: {e}")))?;

        let http = reqwest::Client::new();
        let start = std::time::Instant::now();

        // Init segment
        let init_data = rt
            .block_on(Self::download_one(&http, &init_url))
            .map_err(|e| DecoderError::IoError(format!("init segment: {e}")))?;

        // Media segments — downloaded in parallel batches, reassembled in order.
        // fMP4 concatenation order matters, but download order does not.
        let parallel_batch = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(8);
        let mut segment_data: Vec<Vec<u8>> = vec![Vec::new(); segment_count];
        let mut completed = 0usize;

        for chunk_start in (0..segment_count).step_by(parallel_batch) {
            let chunk_end = (chunk_start + parallel_batch).min(segment_count);
            let batch = segment_urls.get(chunk_start..chunk_end).unwrap_or_default();

            let futures: Vec<_> = batch
                .iter()
                .enumerate()
                .map(|(rel, url)| {
                    let abs = chunk_start + rel;
                    let url = url.clone();
                    let client = http.clone();
                    async move {
                        let data = Self::download_one(&client, &url).await.map_err(|e| {
                            DecoderError::IoError(format!(
                                "segment {}/{segment_count}: {e}",
                                abs + 1
                            ))
                        })?;
                        Ok::<(usize, Vec<u8>), DecoderError>((abs, data))
                    }
                })
                .collect();

            let results = rt.block_on(futures_util::future::join_all(futures));

            for result in results {
                let (idx, data) = result?;
                if let Some(slot) = segment_data.get_mut(idx) {
                    *slot = data;
                }
                completed += 1;
                on_progress(completed as f64 / segment_count as f64);
            }

            if segment_count >= 4 && chunk_end % (segment_count / 4).max(1) < parallel_batch {
                info!(
                    "DASH download progress: {}/{} segments",
                    completed, segment_count,
                );
            }
        }

        // Concatenate: init + all media segments in order
        let total_size: usize =
            init_data.len() + segment_data.iter().map(|s| s.len()).sum::<usize>();
        let mut all_data = Vec::with_capacity(total_size);
        all_data.extend_from_slice(&init_data);
        for seg in segment_data {
            all_data.extend(seg);
        }

        let elapsed = start.elapsed();
        info!(
            "DASH download complete: {:.1} MB, {} segments in {:.2}s ({:.1} MB/s)",
            all_data.len() as f64 / (1024.0 * 1024.0),
            segment_count,
            elapsed.as_secs_f64(),
            all_data.len() as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64(),
        );

        // -- patch fMP4 duration for Symphonia 0.6 compatibility ----------
        // Symphonia 0.6's IsoMp4Reader has a bug where MoovSegment::all_tracks_ended()
        // returns true for fragmented MP4 with zero-duration tracks (common in DASH).
        // This causes next_packet() to return Ok(None) immediately without reading
        // any moof fragments. Workaround: patch the mdhd atom's duration field to
        // a non-zero value so the reader proceeds to read fragments.
        Self::patch_fmp4_duration(&mut all_data, duration, sample_rate);

        // -- save to audio cache if requested (DASH is fully downloaded) --
        if let Some(ref save_path) = cache_path {
            if let Some(parent) = std::path::Path::new(save_path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(save_path, &all_data) {
                Ok(()) => {
                    info!(
                        "Saved DASH audio to cache: {} ({:.1} MB)",
                        save_path,
                        all_data.len() as f64 / (1024.0 * 1024.0),
                    );
                }
                Err(e) => {
                    warn!("Failed to save DASH audio to cache {}: {}", save_path, e);
                }
            }
        }

        // -- create decoder from complete data ----------------------------
        let decoder = Self::from_bytes(all_data, Some("mp4"))?;

        // Dummy download handle (nothing to abort — download is done)
        let shared = Arc::new(StreamingBuffer::new(0));
        shared.download_complete.store(true, Ordering::Release);
        let handle = DownloadHandle {
            shared,
            thread: None,
        };

        Ok((decoder, handle, duration, sample_rate))
    }

    /// Download a single URL and return its bytes.
    async fn download_one(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| format!("read body: {e}"))
    }

    /// Internal: create decoder from MediaSourceStream
    fn from_media_source_stream(
        mss: MediaSourceStream<'static>,
        hint: Hint,
    ) -> DecoderResult<Self> {
        let meta_opts: MetadataOptions = Default::default();
        let fmt_opts: FormatOptions = Default::default();

        // Probe the format
        let format = symphonia::default::get_probe()
            .probe(&hint, mss, fmt_opts, meta_opts)
            .map_err(|e| DecoderError::OpenError(format!("Failed to probe format: {}", e)))?;

        // Find the default audio track
        let track = format
            .default_track(TrackType::Audio)
            .ok_or(DecoderError::NoAudioTrack)?;

        let track_id = track.id;

        // Extract audio codec parameters from the track
        let audio_params = match track.codec_params.as_ref() {
            Some(CodecParameters::Audio(audio)) => audio,
            _ => return Err(DecoderError::NoAudioTrack),
        };

        info!(
            "Found audio track: id={}, codec={:?}",
            track_id, audio_params.codec
        );

        // Get audio format info
        let sample_rate = audio_params
            .sample_rate
            .ok_or_else(|| DecoderError::DecoderInit("Unknown sample rate".to_string()))?;

        let channels = audio_params
            .channels
            .as_ref()
            .map(|c| c.count())
            .unwrap_or(2);

        // Duration comes from the track in 0.6, not from codec params
        let duration = track
            .num_frames
            .map(|frames| frames as f64 / sample_rate as f64);

        let format_info = AudioFormat {
            sample_rate,
            channels,
            duration,
        };

        info!(
            "Audio format: {}Hz, {} channels, duration: {:?}s",
            sample_rate, channels, duration
        );

        // Create the decoder
        let dec_opts: AudioDecoderOptions = Default::default();
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(audio_params, &dec_opts)
            .map_err(|e| DecoderError::DecoderInit(format!("Failed to create decoder: {}", e)))?;

        let spec = AudioSpec::new(
            sample_rate,
            audio_params.channels.clone().unwrap_or_default(),
        );

        Ok(Self {
            format,
            decoder,
            track_id,
            format_info,
            spec,
        })
    }

    /// Decode the next packet and return f32 samples
    /// Returns None when end of stream is reached
    pub fn decode_next(&mut self) -> DecoderResult<Option<Vec<f32>>> {
        loop {
            // Get the next packet (0.6: returns Ok(None) at EOS)
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => {
                    debug!("End of audio stream");
                    return Ok(None);
                }
                Err(SymphoniaError::ResetRequired) => {
                    // Reset the decoder and try again
                    self.decoder.reset();
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            // Skip packets from other tracks
            if packet.track_id() != self.track_id {
                continue;
            }

            // Decode the packet
            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(SymphoniaError::DecodeError(msg)) => {
                    warn!("Decode error (continuing): {}", msg);
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            // Update spec if it changed
            let new_spec = decoded.spec().clone();
            if self.spec != new_spec {
                self.spec = new_spec;
            }

            // Copy decoded audio to f32 interleaved samples (no more SampleBuffer)
            let mut samples: Vec<f32> = Vec::new();
            decoded.copy_to_vec_interleaved(&mut samples);

            return Ok(Some(samples));
        }
    }

    /// Patch the mdhd (media header) duration in fMP4 binary data.
    ///
    /// Symphonia 0.6's ISOBMFF reader has a bug: for fragmented MP4 files where the
    /// moov atom has zero-duration tracks (standard for DASH), `MoovSegment::all_tracks_ended()`
    /// incorrectly returns `true` because `stts.total_duration (0) < trak.duration (0)` is false.
    /// This prevents the reader from ever reading moof fragments, causing instant "end of stream".
    ///
    /// Workaround: scan the binary data for `mdhd` atoms and set their duration to a non-zero
    /// value computed from the DASH manifest's known duration and sample rate.
    fn patch_fmp4_duration(data: &mut [u8], duration_secs: f64, sample_rate: u32) {
        // mdhd atom type bytes: 'm' 'd' 'h' 'd'
        let mdhd_marker: [u8; 4] = [0x6D, 0x64, 0x68, 0x64];

        // Duration in media timescale units (timescale = sample_rate for audio)
        let duration_in_timescale = (duration_secs * sample_rate as f64) as u64;

        if duration_in_timescale == 0 {
            return;
        }

        // Scan for mdhd atoms (typically only one in a DASH init segment)
        let mut i = 0;
        while i + 8 <= data.len() {
            // Check for mdhd type marker at position i+4 (after 4-byte size field)
            if i + 4 + 4 <= data.len() && data.get(i + 4..i + 8) == Some(&mdhd_marker) {
                // Read the atom size to validate
                let atom_size = u32::from_be_bytes([
                    data.get(i).copied().unwrap_or(0),
                    data.get(i + 1).copied().unwrap_or(0),
                    data.get(i + 2).copied().unwrap_or(0),
                    data.get(i + 3).copied().unwrap_or(0),
                ]) as usize;

                // Minimum valid mdhd: version 0 = 32 bytes, version 1 = 44 bytes
                if atom_size < 32 || i + atom_size > data.len() {
                    i += 1;
                    continue;
                }

                // Byte at offset 8 is the version field (after size[4] + type[4])
                let version = data.get(i + 8).copied().unwrap_or(0);

                match version {
                    0 => {
                        // Version 0 layout after size+type:
                        //   version(1) + flags(3) + creation(4) + modification(4) + timescale(4) + duration(4)
                        // Duration is at offset 8+1+3+4+4+4 = 24 from atom start
                        let dur_offset = i + 24;
                        if dur_offset + 4 <= data.len() {
                            let current_dur = u32::from_be_bytes([
                                data.get(dur_offset).copied().unwrap_or(0),
                                data.get(dur_offset + 1).copied().unwrap_or(0),
                                data.get(dur_offset + 2).copied().unwrap_or(0),
                                data.get(dur_offset + 3).copied().unwrap_or(0),
                            ]);
                            if current_dur == 0 {
                                let dur_bytes = (duration_in_timescale as u32).to_be_bytes();
                                if let Some(slot) = data.get_mut(dur_offset..dur_offset + 4) {
                                    slot.copy_from_slice(&dur_bytes);
                                    info!(
                                        "Patched mdhd v0 duration: 0 -> {} (timescale units)",
                                        duration_in_timescale as u32
                                    );
                                }
                            }
                        }
                    }
                    1 => {
                        // Version 1 layout after size+type:
                        //   version(1) + flags(3) + creation(8) + modification(8) + timescale(4) + duration(8)
                        // Duration is at offset 8+1+3+8+8+4 = 32 from atom start
                        let dur_offset = i + 32;
                        if dur_offset + 8 <= data.len() {
                            let current_dur = u64::from_be_bytes([
                                data.get(dur_offset).copied().unwrap_or(0),
                                data.get(dur_offset + 1).copied().unwrap_or(0),
                                data.get(dur_offset + 2).copied().unwrap_or(0),
                                data.get(dur_offset + 3).copied().unwrap_or(0),
                                data.get(dur_offset + 4).copied().unwrap_or(0),
                                data.get(dur_offset + 5).copied().unwrap_or(0),
                                data.get(dur_offset + 6).copied().unwrap_or(0),
                                data.get(dur_offset + 7).copied().unwrap_or(0),
                            ]);
                            if current_dur == 0 {
                                let dur_bytes = duration_in_timescale.to_be_bytes();
                                if let Some(slot) = data.get_mut(dur_offset..dur_offset + 8) {
                                    slot.copy_from_slice(&dur_bytes);
                                    info!(
                                        "Patched mdhd v1 duration: 0 -> {} (timescale units)",
                                        duration_in_timescale
                                    );
                                }
                            }
                        }
                    }
                    _ => {
                        warn!("Unknown mdhd version {}, skipping patch", version);
                    }
                }

                // Move past this atom
                i += atom_size;
            } else {
                i += 1;
            }
        }
    }

    /// Seek to a position in seconds
    pub fn seek(&mut self, seconds: f64) -> DecoderResult<()> {
        use tracing::info;

        info!("Decoder: Starting seek to {:.2}s", seconds);
        let start = std::time::Instant::now();

        let time = Time::try_from_secs_f64(seconds)
            .ok_or_else(|| DecoderError::SeekError(format!("Invalid seek time: {seconds}")))?;

        self.format
            .seek(
                SeekMode::Coarse,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|e| DecoderError::SeekError(format!("{:?}", e)))?;

        info!("Decoder: format.seek() completed in {:?}", start.elapsed());

        // Reset decoder state after seek
        self.decoder.reset();

        info!("Decoder: Seek fully completed in {:?}", start.elapsed());

        Ok(())
    }
}

/// Streaming decoder that provides samples on demand
pub struct StreamingDecoder {
    decoder: AudioDecoder,
    /// Buffer of decoded samples not yet consumed
    buffer: Vec<f32>,
    /// Current position in the buffer
    buffer_pos: usize,
}

impl StreamingDecoder {
    /// Create a new streaming decoder from an existing [`AudioDecoder`].
    pub fn from_decoder(decoder: AudioDecoder) -> Self {
        Self {
            decoder,
            buffer: Vec::new(),
            buffer_pos: 0,
        }
    }

    /// Fill a buffer with decoded audio samples, returning how many were written.
    /// Returns 0 at end of stream.
    pub fn fill_buffer(&mut self, output: &mut [f32]) -> DecoderResult<usize> {
        let mut written = 0;

        while written < output.len() {
            // Consume from existing buffer first
            if self.buffer_pos < self.buffer.len() {
                let remaining_in_buffer = self.buffer.len() - self.buffer_pos;
                let to_copy = remaining_in_buffer.min(output.len() - written);

                if let (Some(output_slice), Some(buffer_slice)) = (
                    output.get_mut(written..written + to_copy),
                    self.buffer.get(self.buffer_pos..self.buffer_pos + to_copy),
                ) {
                    output_slice.copy_from_slice(buffer_slice);
                }

                self.buffer_pos += to_copy;
                written += to_copy;
                continue;
            }

            // Need to decode more
            match self.decoder.decode_next()? {
                Some(samples) => {
                    self.buffer = samples;
                    self.buffer_pos = 0;
                }
                None => {
                    // End of stream
                    break;
                }
            }
        }

        Ok(written)
    }

    /// Seek to a position in seconds
    pub fn seek(&mut self, seconds: f64) -> DecoderResult<()> {
        self.decoder.seek(seconds)?;
        self.buffer.clear();
        self.buffer_pos = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // Test code prioritises clarity and conciseness over production safety
    // lints — `.unwrap()`, indexing, `panic!`, and wildcard imports are all
    // acceptable here.
    #![allow(
        clippy::expect_used,
        clippy::unwrap_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::manual_assert,
        clippy::wildcard_imports
    )]

    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    // ── Tiny valid MP3 (440 Hz sine, 50 ms, mono, 32 kbps) ──────────
    // Generated with:
    //   ffmpeg -f lavfi -i "sine=frequency=440:duration=0.05" \
    //          -ar 44100 -ac 1 -c:a libmp3lame -b:a 32k tiny.mp3
    const TINY_MP3: &[u8] = &[
        0x49, 0x44, 0x33, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x22, 0x54, 0x53, 0x53, 0x45, 0x00,
        0x00, 0x00, 0x0e, 0x00, 0x00, 0x03, 0x4c, 0x61, 0x76, 0x66, 0x36, 0x31, 0x2e, 0x37, 0x2e,
        0x31, 0x30, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff,
        0xfb, 0x40, 0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x49, 0x6e, 0x66, 0x6f, 0x00, 0x00, 0x00, 0x0f, 0x00, 0x00,
        0x00, 0x03, 0x00, 0x00, 0x01, 0xef, 0x00, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93,
        0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93,
        0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0xca, 0xca, 0xca, 0xca, 0xca,
        0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca,
        0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xca, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0x00, 0x00, 0x00, 0x00, 0x4c, 0x61, 0x76, 0x63, 0x36, 0x31, 0x2e, 0x31, 0x39, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x24, 0x02, 0xa3, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xef, 0x28, 0xb6, 0x68, 0xcc, 0x00, 0x00, 0x00, 0x00,
        0x00, 0xff, 0xfb, 0x10, 0xc4, 0x00, 0x00, 0x04, 0x74, 0x13, 0x55, 0x54, 0x90, 0x80, 0x30,
        0xa6, 0x09, 0xaf, 0x37, 0x1a, 0x20, 0x02, 0x00, 0x01, 0xad, 0x39, 0x40, 0x00, 0x01, 0x59,
        0x3a, 0x3d, 0x50, 0x50, 0x08, 0x06, 0x09, 0x01, 0xf0, 0x7c, 0x1f, 0x07, 0xca, 0x02, 0x00,
        0x80, 0x61, 0x10, 0x7c, 0x1f, 0xd4, 0x08, 0x3b, 0x13, 0x87, 0xf8, 0x83, 0x70, 0x04, 0x93,
        0xf6, 0xc0, 0x60, 0x38, 0x1c, 0x0e, 0x00, 0x00, 0x00, 0x00, 0x00, 0x28, 0x89, 0x2a, 0x99,
        0x14, 0x64, 0x08, 0xe9, 0x02, 0x48, 0x16, 0xa3, 0xf7, 0x85, 0x01, 0xf0, 0x13, 0x1b, 0xf0,
        0x22, 0x94, 0x2f, 0xa8, 0x1a, 0x12, 0xfc, 0x24, 0x0d, 0x2a, 0x0a, 0x00, 0x18, 0x30, 0x00,
        0xff, 0xfb, 0x12, 0xc4, 0x02, 0x83, 0xc5, 0x58, 0x1d, 0x20, 0x1d, 0xe0, 0x00, 0x28, 0xa4,
        0x83, 0xa4, 0x82, 0xbc, 0x00, 0x05, 0xcc, 0x09, 0x00, 0xbc, 0x40, 0x04, 0x86, 0x00, 0xe0,
        0x78, 0x67, 0xee, 0xf6, 0xa6, 0x63, 0x03, 0x96, 0x61, 0xc4, 0x11, 0x26, 0x0c, 0x00, 0x7e,
        0x60, 0x42, 0x06, 0x06, 0x05, 0x20, 0x4c, 0x60, 0x5e, 0x03, 0xc5, 0x9a, 0xb4, 0x95, 0xf9,
        0x48, 0xf3, 0x01, 0x30, 0x11, 0x30, 0x00, 0x03, 0x63, 0x03, 0x60, 0x84, 0x33, 0x6e, 0x50,
        0xd3, 0x2e, 0xb1, 0x77, 0x30, 0xbf, 0x07, 0xd3, 0x05, 0x90, 0x1d, 0x30, 0x0b, 0x02, 0xd3,
        0x02, 0x50, 0x1f, 0x30, 0x23, 0x01, 0xb4, 0x4f, 0x9f, 0x49, 0x03, 0x92, 0x48, 0x00, 0x0a,
        0xff, 0xfb, 0x10, 0xc4, 0x02, 0x80, 0x04, 0xb4, 0x43, 0x52, 0xb9, 0x92, 0x80, 0x10, 0x97,
        0x06, 0xa6, 0xeb, 0x98, 0x30, 0x04, 0x61, 0x11, 0xd2, 0xa1, 0x4c, 0x16, 0xe9, 0x9a, 0xd1,
        0x5c, 0xfa, 0x22, 0xaa, 0xf9, 0x12, 0xcc, 0xbb, 0xbf, 0x7f, 0x37, 0x96, 0x4f, 0xe0, 0x61,
        0x5f, 0xc7, 0x8b, 0x17, 0xc0, 0xc7, 0x7e, 0x15, 0x50, 0x0c, 0x5d, 0x85, 0xc0, 0x00, 0x00,
        0x26, 0x12, 0x84, 0x62, 0x73, 0xcc, 0x92, 0x41, 0xa8, 0x1d, 0x5e, 0x49, 0x12, 0x42, 0x95,
        0x2d, 0x3c, 0x94, 0x49, 0x14, 0x14, 0x05, 0x63, 0x18, 0x53, 0xbc, 0x4b, 0x74, 0xa8, 0x2d,
        0xc4, 0xaa, 0x4c, 0x41, 0x4d, 0x45, 0x33, 0x2e, 0x31, 0x30, 0x30, 0xaa, 0xaa, 0xaa,
    ];

    // =====================================================================
    // AudioFormat
    // =====================================================================

    #[test]
    fn test_audio_format_default() {
        let format = AudioFormat {
            sample_rate: 44100,
            channels: 2,
            duration: Some(180.0),
        };
        assert_eq!(format.sample_rate, 44100);
        assert_eq!(format.channels, 2);
        assert_eq!(format.duration, Some(180.0));
    }

    #[test]
    fn audio_format_no_duration() {
        let f = AudioFormat {
            sample_rate: 96000,
            channels: 1,
            duration: None,
        };
        assert_eq!(f.sample_rate, 96000);
        assert_eq!(f.channels, 1);
        assert!(f.duration.is_none());
    }

    #[test]
    fn audio_format_clone() {
        let f = AudioFormat {
            sample_rate: 48000,
            channels: 6,
            duration: Some(300.5),
        };
        let f2 = f.clone();
        assert_eq!(f.sample_rate, f2.sample_rate);
        assert_eq!(f.channels, f2.channels);
        assert_eq!(f.duration, f2.duration);
    }

    #[test]
    fn audio_format_debug() {
        let f = AudioFormat {
            sample_rate: 44100,
            channels: 2,
            duration: None,
        };
        let dbg = format!("{:?}", f);
        assert!(dbg.contains("44100"));
        assert!(dbg.contains("None"));
    }

    // =====================================================================
    // DecoderError — Display
    // =====================================================================

    #[test]
    fn test_decoder_error_display() {
        let err = DecoderError::NoAudioTrack;
        assert_eq!(format!("{}", err), "No audio track found");
    }

    #[test]
    fn decoder_error_display_all_variants() {
        let cases: Vec<(DecoderError, &str)> = vec![
            (
                DecoderError::OpenError("bad file".into()),
                "Failed to open media: bad file",
            ),
            (DecoderError::NoAudioTrack, "No audio track found"),
            (
                DecoderError::DecoderInit("codec fail".into()),
                "Decoder initialization failed: codec fail",
            ),
            (
                DecoderError::DecodeError("corrupt".into()),
                "Decoding error: corrupt",
            ),
            (
                DecoderError::SeekError("past end".into()),
                "Seek error: past end",
            ),
            (
                DecoderError::IoError("disk full".into()),
                "IO error: disk full",
            ),
            (
                DecoderError::UnsupportedFormat("opus".into()),
                "Unsupported format: opus",
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(format!("{}", err), expected);
        }
    }

    #[test]
    fn decoder_error_is_std_error() {
        let err: Box<dyn std::error::Error> = Box::new(DecoderError::IoError("test".into()));
        assert!(err.to_string().contains("test"));
    }

    #[test]
    fn decoder_error_debug_format() {
        let err = DecoderError::OpenError("x".into());
        let dbg = format!("{:?}", err);
        assert!(dbg.contains("OpenError"));
    }

    // =====================================================================
    // DecoderError — From<SymphoniaError>
    // =====================================================================

    #[test]
    fn from_symphonia_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
        let sym = SymphoniaError::IoError(io_err);
        let de: DecoderError = sym.into();
        match de {
            DecoderError::IoError(msg) => assert!(msg.contains("not found")),
            other => panic!("expected IoError, got {:?}", other),
        }
    }

    #[test]
    fn from_symphonia_decode_error() {
        let sym = SymphoniaError::DecodeError("bad frame");
        let de: DecoderError = sym.into();
        match de {
            DecoderError::DecodeError(msg) => assert!(msg.contains("bad frame")),
            other => panic!("expected DecodeError, got {:?}", other),
        }
    }

    #[test]
    fn from_symphonia_unsupported() {
        let sym = SymphoniaError::Unsupported("vorbis");
        let de: DecoderError = sym.into();
        match de {
            DecoderError::UnsupportedFormat(msg) => assert!(msg.contains("vorbis")),
            other => panic!("expected UnsupportedFormat, got {:?}", other),
        }
    }

    #[test]
    fn from_symphonia_seek_error() {
        use symphonia::core::errors::SeekErrorKind;
        let sym = SymphoniaError::SeekError(SeekErrorKind::Unseekable);
        let de: DecoderError = sym.into();
        match de {
            DecoderError::SeekError(msg) => assert!(msg.contains("Unseekable")),
            other => panic!("expected SeekError, got {:?}", other),
        }
    }

    #[test]
    fn from_symphonia_reset_required() {
        let sym = SymphoniaError::ResetRequired;
        let de: DecoderError = sym.into();
        // ResetRequired falls into the catch-all arm → DecodeError
        match de {
            DecoderError::DecodeError(msg) => assert!(msg.contains("ResetRequired")),
            other => panic!("expected DecodeError for ResetRequired, got {:?}", other),
        }
    }

    // =====================================================================
    // BytesMediaSource
    // =====================================================================

    #[test]
    fn bytes_media_source_read() {
        let data = vec![10, 20, 30, 40, 50];
        let mut src = BytesMediaSource::new(data);
        let mut buf = [0u8; 3];
        let n = src.read(&mut buf).unwrap();
        assert_eq!(n, 3);
        assert_eq!(&buf, &[10, 20, 30]);

        let n2 = src.read(&mut buf).unwrap();
        assert_eq!(n2, 2);
        assert_eq!(&buf[..2], &[40, 50]);

        // EOF
        let n3 = src.read(&mut buf).unwrap();
        assert_eq!(n3, 0);
    }

    #[test]
    fn bytes_media_source_seek_start() {
        let data = vec![1, 2, 3, 4, 5];
        let mut src = BytesMediaSource::new(data);
        let mut buf = [0u8; 2];
        src.read(&mut buf).unwrap();

        let pos = src.seek(SeekFrom::Start(1)).unwrap();
        assert_eq!(pos, 1);
        src.read(&mut buf).unwrap();
        assert_eq!(&buf, &[2, 3]);
    }

    #[test]
    fn bytes_media_source_seek_current() {
        let data = vec![1, 2, 3, 4, 5];
        let mut src = BytesMediaSource::new(data);
        src.seek(SeekFrom::Start(2)).unwrap();
        let pos = src.seek(SeekFrom::Current(1)).unwrap();
        assert_eq!(pos, 3);
        let mut buf = [0u8; 1];
        src.read(&mut buf).unwrap();
        assert_eq!(buf[0], 4);
    }

    #[test]
    fn bytes_media_source_seek_end() {
        let data = vec![1, 2, 3, 4, 5];
        let mut src = BytesMediaSource::new(data);
        let pos = src.seek(SeekFrom::End(-2)).unwrap();
        assert_eq!(pos, 3);
        let mut buf = [0u8; 2];
        src.read(&mut buf).unwrap();
        assert_eq!(&buf, &[4, 5]);
    }

    #[test]
    fn bytes_media_source_is_seekable() {
        let src = BytesMediaSource::new(vec![0; 10]);
        assert!(src.is_seekable());
    }

    #[test]
    fn bytes_media_source_byte_len() {
        let src = BytesMediaSource::new(vec![0; 42]);
        assert_eq!(src.byte_len(), Some(42));
    }

    #[test]
    fn bytes_media_source_empty() {
        let mut src = BytesMediaSource::new(vec![]);
        assert_eq!(src.byte_len(), Some(0));
        let mut buf = [0u8; 1];
        assert_eq!(src.read(&mut buf).unwrap(), 0);
    }

    // =====================================================================
    // StreamingBuffer
    // =====================================================================

    #[test]
    fn streaming_buffer_new() {
        let buf = StreamingBuffer::new(1024);
        assert_eq!(buf.bytes_downloaded.load(Ordering::Relaxed), 0);
        assert!(!buf.download_complete.load(Ordering::Relaxed));
        assert!(!buf.abort.load(Ordering::Relaxed));
        assert_eq!(buf.data.lock().unwrap().len(), 0);
    }

    #[test]
    fn streaming_buffer_append_data() {
        let buf = StreamingBuffer::new(256);
        {
            let mut data = buf.data.lock().unwrap();
            data.extend_from_slice(&[1, 2, 3, 4]);
        }
        buf.bytes_downloaded.store(4, Ordering::Release);
        assert_eq!(buf.bytes_downloaded.load(Ordering::Acquire), 4);
        assert_eq!(buf.data.lock().unwrap().len(), 4);
    }

    #[test]
    fn streaming_buffer_abort_flag() {
        let buf = StreamingBuffer::new(0);
        assert!(!buf.abort.load(Ordering::Relaxed));
        buf.abort.store(true, Ordering::Release);
        assert!(buf.abort.load(Ordering::Acquire));
    }

    #[test]
    fn streaming_buffer_download_complete_flag() {
        let buf = StreamingBuffer::new(0);
        assert!(!buf.download_complete.load(Ordering::Relaxed));
        buf.download_complete.store(true, Ordering::Release);
        assert!(buf.download_complete.load(Ordering::Acquire));
    }

    // =====================================================================
    // HttpStreamingSource — Read
    // =====================================================================

    /// Build a ready-to-read `HttpStreamingSource` with the given data
    /// already in the buffer and marked as complete.
    fn make_complete_http_source(data: &[u8]) -> HttpStreamingSource {
        let shared = Arc::new(StreamingBuffer::new(data.len()));
        {
            let mut buf = shared.data.lock().unwrap();
            buf.extend_from_slice(data);
        }
        shared
            .bytes_downloaded
            .store(data.len() as u64, Ordering::Release);
        shared.download_complete.store(true, Ordering::Release);
        HttpStreamingSource {
            shared,
            position: 0,
            content_length: Some(data.len() as u64),
        }
    }

    #[test]
    fn http_source_read_all() {
        let mut src = make_complete_http_source(&[10, 20, 30, 40, 50]);
        let mut out = vec![0u8; 10];
        let n = src.read(&mut out).unwrap();
        assert_eq!(n, 5);
        assert_eq!(&out[..5], &[10, 20, 30, 40, 50]);
    }

    #[test]
    fn http_source_read_in_chunks() {
        let mut src = make_complete_http_source(&[1, 2, 3, 4, 5, 6]);
        let mut buf = [0u8; 3];

        let n1 = src.read(&mut buf).unwrap();
        assert_eq!(n1, 3);
        assert_eq!(&buf, &[1, 2, 3]);

        let n2 = src.read(&mut buf).unwrap();
        assert_eq!(n2, 3);
        assert_eq!(&buf, &[4, 5, 6]);

        // EOF
        let n3 = src.read(&mut buf).unwrap();
        assert_eq!(n3, 0);
    }

    #[test]
    fn http_source_read_empty_eof() {
        let mut src = make_complete_http_source(&[]);
        let mut buf = [0u8; 4];
        let n = src.read(&mut buf).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn http_source_read_abort_returns_error() {
        // Create a source where no data is available and download is NOT complete,
        // but the abort flag is set.
        let shared = Arc::new(StreamingBuffer::new(0));
        shared.abort.store(true, Ordering::Release);
        let mut src = HttpStreamingSource {
            shared,
            position: 0,
            content_length: None,
        };
        let mut buf = [0u8; 4];
        let result = src.read(&mut buf);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
        assert!(err.to_string().contains("aborted"));
    }

    // =====================================================================
    // HttpStreamingSource — Seek
    // =====================================================================

    #[test]
    fn http_source_seek_start() {
        let mut src = make_complete_http_source(&[10, 20, 30, 40, 50]);
        let pos = src.seek(SeekFrom::Start(3)).unwrap();
        assert_eq!(pos, 3);
        let mut buf = [0u8; 2];
        src.read(&mut buf).unwrap();
        assert_eq!(&buf, &[40, 50]);
    }

    #[test]
    fn http_source_seek_current_forward() {
        let mut src = make_complete_http_source(&[1, 2, 3, 4, 5]);
        src.seek(SeekFrom::Start(1)).unwrap();
        let pos = src.seek(SeekFrom::Current(2)).unwrap();
        assert_eq!(pos, 3);
        assert_eq!(src.position, 3);
    }

    #[test]
    fn http_source_seek_current_backward() {
        let mut src = make_complete_http_source(&[1, 2, 3, 4, 5]);
        src.seek(SeekFrom::Start(4)).unwrap();
        let pos = src.seek(SeekFrom::Current(-2)).unwrap();
        assert_eq!(pos, 2);
    }

    #[test]
    fn http_source_seek_end() {
        let mut src = make_complete_http_source(&[1, 2, 3, 4, 5]);
        let pos = src.seek(SeekFrom::End(-1)).unwrap();
        assert_eq!(pos, 4);
        let mut buf = [0u8; 1];
        src.read(&mut buf).unwrap();
        assert_eq!(buf[0], 5);
    }

    #[test]
    fn http_source_seek_end_positive() {
        let mut src = make_complete_http_source(&[1, 2, 3]);
        // Seeking past the end — the position moves but reads will EOF.
        let pos = src.seek(SeekFrom::End(5)).unwrap();
        assert_eq!(pos, 8);
    }

    #[test]
    fn http_source_seek_end_no_content_length() {
        // When content_length is None, SeekFrom::End uses bytes_downloaded.
        let shared = Arc::new(StreamingBuffer::new(0));
        shared.bytes_downloaded.store(100, Ordering::Release);
        shared.download_complete.store(true, Ordering::Release);
        let mut src = HttpStreamingSource {
            shared,
            position: 0,
            content_length: None,
        };
        let pos = src.seek(SeekFrom::End(-10)).unwrap();
        assert_eq!(pos, 90);
    }

    // =====================================================================
    // HttpStreamingSource — MediaSource trait
    // =====================================================================

    #[test]
    fn http_source_is_seekable() {
        let src = make_complete_http_source(&[]);
        assert!(src.is_seekable());
    }

    #[test]
    fn http_source_byte_len_some() {
        let src = make_complete_http_source(&[0; 100]);
        assert_eq!(src.byte_len(), Some(100));
    }

    #[test]
    fn http_source_byte_len_none() {
        let shared = Arc::new(StreamingBuffer::new(0));
        let src = HttpStreamingSource {
            shared,
            position: 0,
            content_length: None,
        };
        assert_eq!(src.byte_len(), None);
    }

    // =====================================================================
    // HttpStreamingSource — concurrent read / write
    // =====================================================================

    #[test]
    fn http_source_read_waits_for_data() {
        // Spawn a thread that writes data after a short delay, verifying
        // that `Read::read` blocks and then returns data once available.
        let shared = Arc::new(StreamingBuffer::new(64));
        let shared_writer = Arc::clone(&shared);

        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            {
                let mut data = shared_writer.data.lock().unwrap();
                data.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
            }
            shared_writer.bytes_downloaded.store(3, Ordering::Release);
            shared_writer.notify.notify_all();

            std::thread::sleep(Duration::from_millis(10));
            shared_writer
                .download_complete
                .store(true, Ordering::Release);
            shared_writer.notify.notify_all();
        });

        let mut src = HttpStreamingSource {
            shared,
            position: 0,
            content_length: None,
        };

        let mut buf = [0u8; 10];
        let n = src.read(&mut buf).unwrap();
        assert_eq!(n, 3);
        assert_eq!(&buf[..3], &[0xAA, 0xBB, 0xCC]);

        writer.join().unwrap();
    }

    // =====================================================================
    // DownloadHandle
    // =====================================================================

    #[test]
    fn download_handle_is_complete_false() {
        let shared = Arc::new(StreamingBuffer::new(0));
        let handle = DownloadHandle {
            shared,
            thread: None,
        };
        assert!(!handle.is_complete());
    }

    #[test]
    fn download_handle_is_complete_true() {
        let shared = Arc::new(StreamingBuffer::new(0));
        shared.download_complete.store(true, Ordering::Release);
        let handle = DownloadHandle {
            shared,
            thread: None,
        };
        assert!(handle.is_complete());
    }

    #[test]
    fn download_handle_abort_no_thread() {
        let shared = Arc::new(StreamingBuffer::new(0));
        let mut handle = DownloadHandle {
            shared: Arc::clone(&shared),
            thread: None,
        };
        handle.abort();
        assert!(shared.abort.load(Ordering::Acquire));
    }

    #[test]
    fn download_handle_abort_with_thread() {
        let shared = Arc::new(StreamingBuffer::new(0));
        let shared_t = Arc::clone(&shared);
        let thread = std::thread::spawn(move || {
            // Wait until abort is signalled.
            loop {
                if shared_t.abort.load(Ordering::Acquire) {
                    break;
                }
                let data = shared_t.data.lock().unwrap();
                let _ = shared_t
                    .notify
                    .wait_timeout(data, Duration::from_millis(10));
            }
        });
        let mut handle = DownloadHandle {
            shared: Arc::clone(&shared),
            thread: Some(thread),
        };
        handle.abort();
        assert!(shared.abort.load(Ordering::Acquire));
        // Thread should have been joined (thread field is now None).
        assert!(handle.thread.is_none());
    }

    #[test]
    fn download_handle_drop_aborts() {
        let shared = Arc::new(StreamingBuffer::new(0));
        let shared_ref = Arc::clone(&shared);
        {
            let _handle = DownloadHandle {
                shared: Arc::clone(&shared),
                thread: None,
            };
            // handle drops here
        }
        // After drop, abort should be set.
        assert!(shared_ref.abort.load(Ordering::Acquire));
    }

    // =====================================================================
    // patch_fmp4_duration — version 0
    // =====================================================================

    /// Build a minimal mdhd v0 atom.
    ///
    /// Layout (32 bytes):
    ///   size(4) + "mdhd"(4) + version(1) + flags(3) +
    ///   creation_time(4) + modification_time(4) +
    ///   timescale(4) + duration(4) + language(2) + quality(2)
    fn make_mdhd_v0(timescale: u32, duration: u32) -> Vec<u8> {
        let mut atom = Vec::new();
        atom.extend_from_slice(&32u32.to_be_bytes()); // atom size
        atom.extend_from_slice(b"mdhd"); // atom type
        atom.push(0); // version 0
        atom.extend_from_slice(&[0, 0, 0]); // flags
        atom.extend_from_slice(&0u32.to_be_bytes()); // creation_time
        atom.extend_from_slice(&0u32.to_be_bytes()); // modification_time
        atom.extend_from_slice(&timescale.to_be_bytes()); // timescale
        atom.extend_from_slice(&duration.to_be_bytes()); // duration
        atom.extend_from_slice(&[0x55, 0xC4]); // language
        atom.extend_from_slice(&[0, 0]); // quality
        assert_eq!(atom.len(), 32);
        atom
    }

    #[test]
    fn patch_fmp4_v0_zero_duration_gets_patched() {
        let mut data = make_mdhd_v0(44100, 0);
        AudioDecoder::patch_fmp4_duration(&mut data, 10.0, 44100);
        // Duration should now be 10.0 * 44100 = 441000
        let patched_dur = u32::from_be_bytes([data[24], data[25], data[26], data[27]]);
        assert_eq!(patched_dur, 441000);
    }

    #[test]
    fn patch_fmp4_v0_nonzero_duration_untouched() {
        let mut data = make_mdhd_v0(44100, 12345);
        AudioDecoder::patch_fmp4_duration(&mut data, 10.0, 44100);
        let dur = u32::from_be_bytes([data[24], data[25], data[26], data[27]]);
        assert_eq!(dur, 12345); // unchanged
    }

    #[test]
    fn patch_fmp4_zero_duration_secs_is_noop() {
        let mut data = make_mdhd_v0(44100, 0);
        let original = data.clone();
        AudioDecoder::patch_fmp4_duration(&mut data, 0.0, 44100);
        assert_eq!(data, original);
    }

    // =====================================================================
    // patch_fmp4_duration — version 1
    // =====================================================================

    /// Build a minimal mdhd v1 atom.
    ///
    /// Layout (44 bytes):
    ///   size(4) + "mdhd"(4) + version(1) + flags(3) +
    ///   creation_time(8) + modification_time(8) +
    ///   timescale(4) + duration(8) + language(2) + quality(2)
    fn make_mdhd_v1(timescale: u32, duration: u64) -> Vec<u8> {
        let mut atom = Vec::new();
        atom.extend_from_slice(&44u32.to_be_bytes()); // atom size
        atom.extend_from_slice(b"mdhd"); // atom type
        atom.push(1); // version 1
        atom.extend_from_slice(&[0, 0, 0]); // flags
        atom.extend_from_slice(&0u64.to_be_bytes()); // creation_time
        atom.extend_from_slice(&0u64.to_be_bytes()); // modification_time
        atom.extend_from_slice(&timescale.to_be_bytes()); // timescale
        atom.extend_from_slice(&duration.to_be_bytes()); // duration
        atom.extend_from_slice(&[0x55, 0xC4]); // language
        atom.extend_from_slice(&[0, 0]); // quality
        assert_eq!(atom.len(), 44);
        atom
    }

    #[test]
    fn patch_fmp4_v1_zero_duration_gets_patched() {
        let mut data = make_mdhd_v1(96000, 0);
        AudioDecoder::patch_fmp4_duration(&mut data, 5.0, 96000);
        let patched_dur = u64::from_be_bytes([
            data[32], data[33], data[34], data[35], data[36], data[37], data[38], data[39],
        ]);
        assert_eq!(patched_dur, 480000); // 5.0 * 96000
    }

    #[test]
    fn patch_fmp4_v1_nonzero_duration_untouched() {
        let mut data = make_mdhd_v1(96000, 99999);
        AudioDecoder::patch_fmp4_duration(&mut data, 5.0, 96000);
        let dur = u64::from_be_bytes([
            data[32], data[33], data[34], data[35], data[36], data[37], data[38], data[39],
        ]);
        assert_eq!(dur, 99999);
    }

    #[test]
    fn patch_fmp4_unknown_version_skipped() {
        // Craft a mdhd with version = 99 — should be skipped without panic.
        let mut atom = Vec::new();
        atom.extend_from_slice(&32u32.to_be_bytes());
        atom.extend_from_slice(b"mdhd");
        atom.push(99); // unknown version
        atom.extend_from_slice(&[0u8; 23]); // pad to 32 bytes
        let original = atom.clone();
        AudioDecoder::patch_fmp4_duration(&mut atom, 10.0, 44100);
        assert_eq!(atom, original); // untouched
    }

    #[test]
    fn patch_fmp4_with_prefix_data() {
        // Put some random bytes before the mdhd atom to ensure scanning works.
        let mut data = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x00, 0x00, 0x00];
        data.extend(make_mdhd_v0(44100, 0));
        data.extend_from_slice(&[0xFF; 16]); // trailing junk
        AudioDecoder::patch_fmp4_duration(&mut data, 2.0, 44100);
        let dur = u32::from_be_bytes([data[8 + 24], data[8 + 25], data[8 + 26], data[8 + 27]]);
        assert_eq!(dur, 88200); // 2.0 * 44100
    }

    #[test]
    fn patch_fmp4_empty_data_no_panic() {
        let mut data: Vec<u8> = vec![];
        AudioDecoder::patch_fmp4_duration(&mut data, 10.0, 44100);
        assert!(data.is_empty());
    }

    #[test]
    fn patch_fmp4_truncated_atom_no_panic() {
        // Just "mdhd" marker with no room for the full atom.
        let mut data = vec![0, 0, 0, 32];
        data.extend_from_slice(b"mdhd");
        // Only 8 bytes total — way too short for a 32-byte atom.
        AudioDecoder::patch_fmp4_duration(&mut data, 10.0, 44100);
        // Should not panic; data unchanged because atom_size > data.len().
    }

    #[test]
    fn patch_fmp4_multiple_mdhd_atoms() {
        let mut data = Vec::new();
        data.extend(make_mdhd_v0(44100, 0));
        data.extend(make_mdhd_v0(48000, 0));
        AudioDecoder::patch_fmp4_duration(&mut data, 3.0, 44100);
        let dur1 = u32::from_be_bytes([data[24], data[25], data[26], data[27]]);
        assert_eq!(dur1, 132300); // 3.0 * 44100
        // Second atom starts at offset 32.
        let dur2 = u32::from_be_bytes([data[32 + 24], data[32 + 25], data[32 + 26], data[32 + 27]]);
        // The second atom also gets patched because the function uses
        // the same duration_in_timescale for all mdhd atoms it finds.
        assert_eq!(dur2, 132300);
    }

    // =====================================================================
    // AudioDecoder::from_bytes — success path (valid MP3)
    // =====================================================================

    #[test]
    fn from_bytes_valid_mp3() {
        let decoder = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3"));
        assert!(decoder.is_ok(), "from_bytes failed: {:?}", decoder.err());
        let dec = decoder.unwrap();
        assert_eq!(dec.format_info.sample_rate, 44100);
        assert!(dec.format_info.channels >= 1);
    }

    #[test]
    fn from_bytes_valid_mp3_no_hint() {
        // Without a format hint, symphonia should still probe and identify MP3.
        let decoder = AudioDecoder::from_bytes(TINY_MP3.to_vec(), None);
        assert!(
            decoder.is_ok(),
            "from_bytes no-hint failed: {:?}",
            decoder.err()
        );
    }

    // =====================================================================
    // AudioDecoder::from_bytes — error paths
    // =====================================================================

    #[test]
    fn from_bytes_empty_data() {
        let result = AudioDecoder::from_bytes(vec![], Some("mp3"));
        assert!(result.is_err());
    }

    #[test]
    fn from_bytes_garbage_data() {
        let result = AudioDecoder::from_bytes(vec![0xDE, 0xAD, 0xBE, 0xEF], Some("mp3"));
        assert!(result.is_err());
    }

    #[test]
    fn from_bytes_wrong_hint() {
        // Give FLAC hint for MP3 data — should still either succeed (probing
        // overrides hint) or fail gracefully without panic.
        let result = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("flac"));
        // Either outcome is fine; we're testing no-panic.
        let _ = result;
    }

    // =====================================================================
    // AudioDecoder::from_file — error path
    // =====================================================================

    #[test]
    fn from_file_nonexistent() {
        let result = AudioDecoder::from_file("/tmp/__nonexistent_audio_file_12345.mp3");
        assert!(result.is_err());
        match result {
            Err(DecoderError::IoError(msg)) => assert!(msg.contains("open")),
            Err(other) => panic!("expected IoError, got {:?}", other),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn from_file_empty_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let result = AudioDecoder::from_file(tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn from_file_valid_mp3() {
        let mut tmp = tempfile::NamedTempFile::with_suffix(".mp3").unwrap();
        std::io::Write::write_all(&mut tmp, TINY_MP3).unwrap();
        tmp.flush().unwrap();
        let result = AudioDecoder::from_file(tmp.path());
        assert!(result.is_ok(), "from_file failed: {:?}", result.err());
        let dec = result.unwrap();
        assert_eq!(dec.format_info.sample_rate, 44100);
    }

    // =====================================================================
    // AudioDecoder::decode_next
    // =====================================================================

    #[test]
    fn decode_next_returns_samples() {
        let mut dec = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3")).unwrap();
        let first = dec.decode_next().unwrap();
        assert!(first.is_some(), "expected at least one decoded packet");
        let samples = first.unwrap();
        assert!(!samples.is_empty());
    }

    #[test]
    fn decode_next_eventually_returns_none() {
        let mut dec = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3")).unwrap();
        let mut packets = 0;
        loop {
            match dec.decode_next().unwrap() {
                Some(_) => packets += 1,
                None => break,
            }
            if packets > 10000 {
                panic!("too many packets — likely infinite loop");
            }
        }
        assert!(packets >= 1);
    }

    // =====================================================================
    // StreamingDecoder
    // =====================================================================

    #[test]
    fn streaming_decoder_fill_buffer() {
        let dec = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3")).unwrap();
        let mut sd = StreamingDecoder::from_decoder(dec);
        let mut buf = vec![0.0f32; 4096];
        let n = sd.fill_buffer(&mut buf).unwrap();
        assert!(n > 0, "expected some samples");
    }

    #[test]
    fn streaming_decoder_fill_buffer_to_eof() {
        let dec = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3")).unwrap();
        let mut sd = StreamingDecoder::from_decoder(dec);
        let mut total = 0usize;
        loop {
            let mut buf = vec![0.0f32; 1024];
            let n = sd.fill_buffer(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            total += n;
            if total > 10_000_000 {
                panic!("too many samples — likely infinite loop");
            }
        }
        assert!(total > 0);
    }

    #[test]
    fn streaming_decoder_from_decoder() {
        let dec = AudioDecoder::from_bytes(TINY_MP3.to_vec(), Some("mp3")).unwrap();
        let sd = StreamingDecoder::from_decoder(dec);
        assert_eq!(sd.decoder.format_info.sample_rate, 44100);
    }

    // =====================================================================
    // AudioDecoder::from_url_streaming — mock HTTP server
    // =====================================================================

    /// Spawn a tiny HTTP server on localhost that serves `body` with the
    /// given `content_type`. Returns the URL.
    fn spawn_http_server(body: &'static [u8], content_type: &str) -> (String, u16) {
        use std::io::Write;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{}/audio", port);

        let ct = content_type.to_string();

        std::thread::spawn(move || {
            // Accept exactly one connection.
            if let Ok((mut stream, _)) = listener.accept() {
                // Read the request (we don't care about its contents).
                let mut req_buf = [0u8; 4096];
                let _ = stream.read(&mut req_buf);

                let response = format!(
                    "HTTP/1.1 200 OK\r\n\
                     Content-Type: {}\r\n\
                     Content-Length: {}\r\n\
                     Connection: close\r\n\
                     \r\n",
                    ct,
                    body.len(),
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(body);
                let _ = stream.flush();
            }
        });

        // Give the listener thread a moment to start.
        std::thread::sleep(Duration::from_millis(20));
        (url, port)
    }

    /// Spawn a tiny HTTP server that returns the given status code with no body.
    fn spawn_http_server_error(status: u16) -> (String, u16) {
        use std::io::Write;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{}/audio", port);

        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut req_buf = [0u8; 4096];
                let _ = stream.read(&mut req_buf);
                let response = format!(
                    "HTTP/1.1 {} Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    status,
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });

        std::thread::sleep(Duration::from_millis(20));
        (url, port)
    }

    #[test]
    fn from_url_streaming_valid_mp3() {
        let (url, _port) = spawn_http_server(TINY_MP3, "audio/mpeg");
        let result = AudioDecoder::from_url_streaming(&url, None);
        assert!(
            result.is_ok(),
            "from_url_streaming failed: {:?}",
            result.err()
        );
        let (dec, mut handle) = result.unwrap();
        assert_eq!(dec.format_info.sample_rate, 44100);
        // Clean up.
        handle.abort();
    }

    #[test]
    fn from_url_streaming_http_error() {
        let (url, _port) = spawn_http_server_error(404);
        let result = AudioDecoder::from_url_streaming(&url, None);
        assert!(result.is_err());
    }

    #[test]
    fn from_url_streaming_garbage_audio() {
        let (url, _port) = spawn_http_server(
            b"this is not audio data at all, just garbage bytes for testing",
            "audio/mpeg",
        );
        let result = AudioDecoder::from_url_streaming(&url, None);
        // Should fail at probe/decode stage, not panic.
        assert!(result.is_err());
    }

    #[test]
    fn from_url_streaming_with_cache() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let cache_path = tmp_dir
            .path()
            .join("cached.mp3")
            .to_string_lossy()
            .to_string();
        let (url, _port) = spawn_http_server(TINY_MP3, "audio/mpeg");
        let result = AudioDecoder::from_url_streaming(&url, Some(cache_path.clone()));
        assert!(result.is_ok());
        let (_dec, mut handle) = result.unwrap();
        // Wait for download to complete so the cache file gets written.
        for _ in 0..50 {
            if handle.is_complete() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        handle.abort();
        // The file should have been cached.
        assert!(
            std::path::Path::new(&cache_path).exists(),
            "cache file was not written"
        );
    }

    #[test]
    fn from_url_streaming_content_type_flac() {
        // Serve MP3 data with a flac content-type — the probe will still
        // succeed because symphonia probes the actual bytes.
        let (url, _port) = spawn_http_server(TINY_MP3, "audio/flac");
        let result = AudioDecoder::from_url_streaming(&url, None);
        // Might succeed or fail depending on probe vs hint priority.
        // Either way, no panic.
        let _ = result;
    }
}
