// SPDX-License-Identifier: MIT

//! Playlist thumbnail generation handlers for Maré Player.
//!
//! Generates 2×2 album-art grid thumbnails for playlists by compositing
//! up to 4 distinct cover images into a single circular thumbnail.

use cosmic::prelude::*;

use crate::messages::Message;
use crate::state::AppModel;

impl AppModel {
    /// Generate 2×2 album-art grid thumbnails for every loaded playlist.
    ///
    /// For each playlist we try the API cache first (instant, no network), then
    /// fall back to fetching the first 4 tracks from the TIDAL API. The cover
    /// art of up to 4 distinct albums is composited into a circular grid image.
    pub fn handle_generate_playlist_thumbnails(&mut self) -> Task<cosmic::Action<Message>> {
        let playlists: Vec<(String, u32)> = self
            .user_playlists
            .iter()
            .filter(|p| !self.playlist_thumbnails.contains_key(&p.uuid))
            .map(|p| (p.uuid.clone(), p.num_tracks))
            .collect();

        if playlists.is_empty() {
            return Task::none();
        }

        let client = self.tidal_client.clone();
        let image_cache = self.image_cache.clone();

        let tasks: Vec<Task<cosmic::Action<Message>>> = playlists
            .into_iter()
            .filter(|(_, num_tracks)| *num_tracks > 0)
            .map(|(uuid, _)| {
                let client = client.clone();
                let image_cache = image_cache.clone();
                let uuid_clone = uuid.clone();
                Task::perform(
                    async move {
                        // Try cached tracks first, otherwise fetch a small page
                        let tracks = {
                            let c = client.lock().await;
                            c.get_cached_playlist_tracks(&uuid_clone)
                        };
                        let tracks = match tracks {
                            Some(t) if !t.is_empty() => t,
                            _ => {
                                let c = client.lock().await;
                                match c.get_playlist_tracks(&uuid_clone, Some(10), None).await {
                                    Ok(t) => t,
                                    Err(e) => {
                                        tracing::debug!(
                                            "Skipping thumbnail for {}: {}",
                                            uuid_clone,
                                            e
                                        );
                                        return None;
                                    }
                                }
                            }
                        };

                        // Collect up to 4 unique cover URLs
                        let mut seen = std::collections::HashSet::new();
                        let cover_urls: Vec<String> = tracks
                            .iter()
                            .filter_map(|t| t.cover_url.as_ref())
                            .filter(|url| seen.insert((*url).clone()))
                            .take(4)
                            .cloned()
                            .collect();

                        if cover_urls.is_empty() {
                            return None;
                        }

                        // Build a stable cache key from the sorted cover URLs
                        // so the composite is reused as long as the same art is
                        // present, regardless of track order.
                        let grid_cache_key = {
                            let mut sorted = cover_urls.clone();
                            sorted.sort();
                            format!("grid:{}", sorted.join("|"))
                        };

                        // Check the disk cache for a previously composited grid
                        if let Some(cached_png) = image_cache.get_cached_grid(&grid_cache_key).await
                        {
                            tracing::debug!("Grid thumbnail cache hit for playlist {}", uuid_clone);
                            // Decode the cached PNG to raw RGBA so the UI can
                            // use Handle::from_rgba without re-decoding.
                            if let Ok(img) = image::load_from_memory(&cached_png) {
                                let rgba = img.into_rgba8();
                                let (w, h) = rgba.dimensions();
                                return Some((uuid_clone, w, h, rgba.into_raw()));
                            }
                        }

                        // Download the raw images (via cache)
                        let mut raw_images: Vec<Vec<u8>> = Vec::new();
                        for url in &cover_urls {
                            if let Some(cached) = image_cache.get_or_load(url).await {
                                raw_images.push(cached.data.to_vec());
                            }
                        }

                        if raw_images.is_empty() {
                            return None;
                        }

                        // Composite into a 2×2 circular grid (render at 160 px for quality)
                        let refs: Vec<&[u8]> = raw_images.iter().map(|v| v.as_slice()).collect();
                        match crate::image_cache::make_grid_thumbnail(&refs, 160) {
                            Ok(rgba) => {
                                // Persist as PNG so subsequent startups skip
                                // the download + composite entirely.
                                if let Some(img) = image::RgbaImage::from_raw(
                                    rgba.width,
                                    rgba.height,
                                    rgba.pixels.clone(),
                                ) {
                                    let mut png_buf = Vec::new();
                                    if img
                                        .write_to(
                                            &mut std::io::Cursor::new(&mut png_buf),
                                            image::ImageFormat::Png,
                                        )
                                        .is_ok()
                                    {
                                        image_cache.save_grid(&grid_cache_key, &png_buf).await;
                                    }
                                }
                                Some((uuid_clone, rgba.width, rgba.height, rgba.pixels))
                            }
                            Err(e) => {
                                tracing::warn!("Grid thumbnail failed for {}: {}", uuid_clone, e);
                                None
                            }
                        }
                    },
                    |result| {
                        if let Some((uuid, w, h, pixels)) = result {
                            cosmic::Action::App(Message::PlaylistThumbnailGenerated(
                                uuid, w, h, pixels,
                            ))
                        } else {
                            cosmic::Action::App(Message::ClearError) // no-op
                        }
                    },
                )
            })
            .collect();

        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }

    /// Handle a completed playlist grid thumbnail (store as image handle).
    pub fn handle_playlist_thumbnail_generated(
        &mut self,
        uuid: String,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    ) {
        let handle = cosmic::widget::image::Handle::from_rgba(width, height, pixels);
        self.playlist_thumbnails.insert(uuid, handle);
    }
}
