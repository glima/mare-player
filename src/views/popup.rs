// SPDX-License-Identifier: MIT

//! Popup window view for Maré Player.
//!
//! This module renders the main popup window including the content area
//! (dispatching to the appropriate view based on ViewState) and the
//! now-playing bar when music is playing.

use std::rc::Rc;

use cosmic::Element;
use cosmic::iced::gradient;
use cosmic::iced::widget::text::Wrapping;
#[cfg(feature = "panel-applet")]
use cosmic::iced::window::Id;
use cosmic::iced::{Alignment, Background, Border, Color, Length, Radians};
#[cfg(not(feature = "panel-applet"))]
use cosmic::widget::popover::{Position, popover};
#[cfg(not(feature = "panel-applet"))]
use cosmic::widget::vertical_slider;
use cosmic::widget::{self, button, container, icon, slider, text};

#[cfg(not(feature = "panel-applet"))]
use crate::views::components::scroll_to_volume_delta;

use crate::fl;
use crate::helpers::format_seconds;
use crate::messages::Message;
use crate::state::{AppModel, ViewState};
use crate::tidal::player::PlaybackState;
use crate::views::components::{LYRICS_SVG, NOW_PLAYING_ART_SIZE, RADIO_SVG, favorite_icon_handle};
#[cfg(feature = "panel-applet")]
use crate::views::components::{POPIN_SVG, POPOUT_SVG};

/// Placeholder height (px) for the embedded video area before the first frame
/// arrives. Once frames flow, the area shrinks to the video's true aspect ratio
/// so there are no letterbox bars (the pop-out window shows full pixels).
const VIDEO_LOADING_HEIGHT: f32 = 180.0;

/// How long the video-mode overlay controls stay visible after the last
/// pointer interaction before fading out.
const VIDEO_CONTROLS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Build a custom [`cosmic::theme::style::iced::Slider`] class whose handle
/// fills from the bottom up according to `progress` (0.0 → 1.0), showing
/// download/buffering progress inside the slider thumb itself.
///
/// Everything else (rail colours, sizes, radii) is copied verbatim from the
/// cosmic `Slider::Standard` implementation so the two states look identical
/// apart from the fill effect on the handle.
fn buffering_slider_class(progress: f32) -> cosmic::theme::style::iced::Slider {
    let style_fn = Rc::new(move |theme: &cosmic::Theme| {
        let cosmic = theme.cosmic();

        let active_track = cosmic.accent.base;
        let inactive_track = cosmic.palette.neutral_6;

        let accent: Color = cosmic.accent.base.into();
        let dim = Color { a: 0.25, ..accent };

        // Gradient fills the handle from bottom (accent) to top (dim).
        // The transition point moves upward as `progress` increases.
        // offset 0.0 = top of handle, 1.0 = bottom (angle points upward).
        let cutoff = (1.0 - progress).clamp(0.0, 1.0);
        let handle_bg = Background::Gradient(
            gradient::Linear::new(Radians(std::f32::consts::PI)) // top → bottom
                .add_stop(0.0, dim)
                .add_stop(cutoff, dim)
                .add_stop((cutoff + 0.01).min(1.0), accent)
                .add_stop(1.0, accent)
                .into(),
        );

        slider::Style {
            rail: slider::Rail {
                backgrounds: (
                    Background::Color(active_track.into()),
                    Background::Color(inactive_track.into()),
                ),
                border: Border {
                    radius: cosmic.corner_radii.radius_xs.into(),
                    color: Color::TRANSPARENT,
                    width: 0.0,
                },
                width: 4.0,
            },
            handle: slider::Handle {
                shape: slider::HandleShape::Rectangle {
                    height: 20,
                    width: 20,
                    border_radius: cosmic.corner_radii.radius_m.into(),
                },
                border_color: Color::TRANSPARENT,
                border_width: 0.0,
                background: handle_bg,
            },
            breakpoint: slider::Breakpoint {
                color: cosmic.on_bg_color().into(),
            },
        }
    });

    cosmic::theme::style::iced::Slider::Custom {
        active: style_fn.clone(),
        hovered: style_fn.clone(),
        dragging: style_fn,
    }
}

/// Shorten an error string for display in the banner.
///
/// Error messages can be huge — a serde parse failure, for instance, appends
/// the entire raw JSON response body on a following line. Rendering that
/// verbatim fills the whole popup. We keep only the first line (dropping any
/// appended body/dumps) and cap its length; the full error is still recorded
/// in the log via `tracing::error!` at the point it's produced.
fn concise_error(error: &str) -> String {
    /// Maximum number of characters shown in the banner.
    const MAX_LEN: usize = 120;

    // Error strings often append a raw payload we never want in the banner: an
    // HTTP error's JSON body (starts with `{`), or a serde parse dump on a
    // following line. Cut at whichever comes first.
    let cut = [error.find('\n'), error.find('{')]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(error.len());

    let head = error[..cut].trim().trim_end_matches([':', '-', ' ']).trim();

    if head.chars().count() > MAX_LEN {
        let truncated: String = head.chars().take(MAX_LEN).collect();
        format!("{}\u{2026}", truncated.trim_end())
    } else {
        head.to_string()
    }
}

impl AppModel {
    /// Dispatch to the appropriate view based on the current [`ViewState`].
    ///
    /// This is the pure routing logic — it returns the page content element
    /// without any chrome (no now-playing bar, no error banner).  Both
    /// [`Self::view_content`] and [`Self::view_standalone`] call this and
    /// then compose the result with the surrounding UI elements.
    fn view_page_content(&self) -> Element<'_, Message> {
        match &self.view_state {
            ViewState::Loading => self.view_loading(),
            ViewState::Login => self.view_login(),
            ViewState::AwaitingOAuth => self.view_awaiting_oauth(),
            ViewState::Main => self.view_main(),
            ViewState::Search => self.view_search(),
            ViewState::Mixes => self.view_mixes(),
            ViewState::MixDetail => self.view_mix_detail(),
            ViewState::Playlists => self.view_playlists(),
            ViewState::PlaylistDetail => self.view_playlist_detail(),
            ViewState::Albums => self.view_albums(),
            ViewState::AlbumDetail => self.view_album_detail(),
            ViewState::ArtistDetail => self.view_artist_detail(),
            ViewState::TrackRadio => self.view_track_radio(),
            ViewState::Lyrics => self.view_lyrics(),
            ViewState::TrackDetail => self.view_track_detail(),
            ViewState::FavoriteTracks => self.view_favorite_tracks(),
            ViewState::Feed => self.view_feed(),
            ViewState::Explore => self.view_explore(),
            ViewState::History => self.view_history(),
            ViewState::Profiles => self.view_profiles(),
            ViewState::Settings => self.view_settings(),
            ViewState::SharePrompt(track_id, track_title, album_id, album_title, is_video) => self
                .view_share_prompt(
                    track_id.clone(),
                    track_title.clone(),
                    album_id.clone(),
                    album_title.clone(),
                    *is_video,
                ),
        }
    }

    /// Build an error banner element for display at the top of the content.
    fn view_error_banner<'a>(&'a self, error: &'a str) -> Element<'a, Message> {
        let error_row = widget::Row::new()
            .push(text(concise_error(error)).size(12))
            .push(
                button::icon(widget::icon::from_name("window-close-symbolic"))
                    .on_press(Message::ClearError)
                    .padding(2),
            )
            .spacing(8)
            .align_y(Alignment::Center)
            .width(Length::Fill);

        container(error_row)
            .padding(8)
            .width(Length::Fill)
            .class(cosmic::theme::Container::custom(|_theme| {
                cosmic::widget::container::Style {
                    background: Some(cosmic::iced::Background::Color(
                        cosmic::iced::Color::from_rgb(0.8, 0.2, 0.2),
                    )),
                    text_color: Some(cosmic::iced::Color::WHITE),
                    border: cosmic::iced::Border {
                        radius: 4.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            }))
            .into()
    }

    /// Build the full content tree for the **applet-popup** layout.
    ///
    /// This dispatches to the appropriate view based on [`ViewState`], appends
    /// the now-playing bar when music is active, and overlays any pending error
    /// message.  The caller is responsible for placing this content into the
    /// right shell (popup container vs. normal window).
    pub fn view_content(&self) -> Element<'_, Message> {
        let main_content = self.view_page_content();

        // Add now playing bar (or video theater) if something is playing.
        let content: Element<'_, Message> = if let Some(np) = &self.now_playing {
            let now_playing_bar = self.view_now_playing_bar(np);

            widget::Column::new()
                .push(main_content)
                .push(now_playing_bar)
                .into()
        } else {
            main_content
        };

        // Wrap with error display if needed
        if let Some(error) = &self.error_message {
            widget::Column::new()
                .push(self.view_error_banner(error))
                .push(content)
                .spacing(8)
                .into()
        } else {
            content
        }
    }

    /// Render the main popup window (panel-applet mode).
    ///
    /// Builds the shared content tree via [`Self::view_content`] and wraps it
    /// in the applet popup container.
    #[cfg(feature = "panel-applet")]
    pub fn view_popup(&self, _id: Id) -> Element<'_, Message> {
        let content = self.view_content();
        self.core.applet.popup_container(content).into()
    }

    /// Render the standalone application window.
    ///
    /// Unlike the applet popup, this uses a flex-column layout that **pins
    /// the now-playing bar to the bottom** of the window.  Iced's flex
    /// algorithm sizes `Shrink` children first (error banner, now-playing
    /// bar) and then gives all remaining space to the `Fill` content area,
    /// so the bar is always fully visible regardless of window height — as
    /// long as the window respects the minimum size we set in
    /// [`Settings::size_limits`].
    #[cfg(not(feature = "panel-applet"))]
    pub fn view_standalone(&self) -> Element<'_, Message> {
        let page = self.view_page_content();

        let mut col = widget::Column::new().height(Length::Fill);

        // Error banner at the top (shrink — only present when needed)
        if let Some(error) = &self.error_message {
            col = col.push(self.view_error_banner(error));
        }

        // Main page content fills all remaining vertical space
        col = col.push(container(page).height(Length::Fill));

        // Now-playing bar (or video theater) pinned at the bottom
        if let Some(np) = &self.now_playing {
            col = col.push(self.view_now_playing_bar(np));
        }

        col.into()
    }

    /// Build the live video-frame element shown in the now-playing pane while a
    /// music video is playing (replacing album art + track info + spectrum).
    ///
    /// The frame fills the available width and the height follows its aspect
    /// ratio (`ContentFit::Contain` with a `Shrink` height), so the embedded
    /// area hugs the picture rather than padding it with black bars.
    fn video_frame_element<'a>(
        &self,
        video: &crate::playback::MediaPlayer,
        radius: [f32; 4],
    ) -> Element<'a, Message> {
        let frame = video
            .frame_buffer()
            .lock()
            .ok()
            .and_then(|g| g.as_ref().cloned());
        if let Some(f) = frame {
            let handle =
                cosmic::widget::image::Handle::from_rgba(f.width, f.height, (*f.rgba).clone());
            cosmic::widget::image(handle)
                .width(Length::Fill)
                .height(Length::Shrink)
                .content_fit(cosmic::iced::ContentFit::Contain)
                .border_radius(radius)
                .into()
        } else {
            container(text(fl!("loading")).size(12))
                .width(Length::Fill)
                .height(Length::Fixed(VIDEO_LOADING_HEIGHT))
                .align_x(Alignment::Center)
                .align_y(Alignment::Center)
                .into()
        }
    }

    /// Whether the video-mode overlay controls are currently shown.
    fn video_controls_visible(&self) -> bool {
        self.video_controls_shown_at
            .is_some_and(|t| t.elapsed() < VIDEO_CONTROLS_TIMEOUT)
    }

    /// Build the video "theater": the live frame filling the content area, with
    /// playback controls overlaid at the bottom that auto-hide when idle.
    ///
    /// Pointer movement over the surface re-reveals the controls; they fade out
    /// again [`VIDEO_CONTROLS_TIMEOUT`] after the last interaction.
    fn video_theater<'a>(
        &'a self,
        video: &crate::playback::MediaPlayer,
        info: Element<'a, Message>,
        controls: Element<'a, Message>,
    ) -> Element<'a, Message> {
        // Round the video's corners to match the surrounding pane / popup.
        let radius = cosmic::theme::active().cosmic().corner_radii.radius_m;
        let corner = radius[0];

        // Base layer: the video, filling the width with its height following
        // the picture's aspect ratio (no letterboxing).
        let surface = self.video_frame_element(video, radius);

        // Overlay layer: controls pinned to the bottom on a translucent strip,
        // shown only while recently interacted with.  Its bottom corners are
        // rounded too, so it doesn't square off the video's rounded bottom.
        let overlay: Element<'_, Message> = if self.video_controls_visible() {
            let strip = container(
                widget::Column::new()
                    .push(info)
                    .push(controls)
                    .spacing(6)
                    .width(Length::Fill),
            )
            .padding(8)
            .width(Length::Fill)
            .class(cosmic::theme::Container::custom(move |theme| {
                // Use the card colour so the clickable buttons' base backdrop
                // and the track-info fade blend in (as they do on the audio
                // bar); hover still highlights. Bottom corners match the video.
                cosmic::widget::container::Style {
                    background: Some(cosmic::iced::Background::Color(
                        theme.cosmic().background.component.base.into(),
                    )),
                    border: cosmic::iced::Border {
                        radius: [0.0, 0.0, corner, corner].into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            }));
            container(strip)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_y(Alignment::End)
                .into()
        } else {
            container(widget::Column::new())
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        };

        let stack = cosmic::iced::widget::Stack::new()
            .push(surface)
            .push(overlay);

        // Any pointer movement over the theater (video or controls) keeps the
        // controls visible. Only `on_move` is wired — not `on_press` — so clicks
        // still pass through to the control buttons.
        let interactive = widget::mouse_area(stack).on_move(|_| Message::VideoInteraction);

        // Sized to the video's aspect ratio (the track list stays above); the
        // backdrop stays transparent so the rounded corners reveal the popup.
        container(interactive)
            .width(Length::Fill)
            .height(Length::Shrink)
            .into()
    }

    /// The now-playing-bar video pop-out toggle button — shown only while a
    /// video is playing (panel-applet only). Opens the video in a separate
    /// child window, or (when already popped out) returns it inline.
    fn pop_out_video_button(&self) -> Option<Element<'_, Message>> {
        #[cfg(not(feature = "panel-applet"))]
        {
            None
        }
        #[cfg(feature = "panel-applet")]
        {
            // Show the toggle whenever a video is the current playback, whether
            // it's inline (`video_player`) or popped out (`video_window`).
            if self.video_player.is_none() && self.video_window.is_none() {
                return None;
            }
            let popped = self.video_window.is_some();
            // Popped out → show the "bring back inline" arrow (pointing into the
            // panel); inline → show the "pop out" arrow (leaving the panel).
            let mut pi = icon::from_svg_bytes(if popped { POPIN_SVG } else { POPOUT_SVG });
            pi.symbolic = true;
            Some(
                button::icon(pi)
                    .tooltip(if popped {
                        fl!("tooltip-video-inline")
                    } else {
                        fl!("tooltip-video-popout")
                    })
                    .padding(4)
                    .on_press(Message::ToggleVideoWindow)
                    .into(),
            )
        }
    }

    /// The now-playing-bar "go to track radio" button. Hidden for videos,
    /// which have no TIDAL track radio (the /tracks/{id}/mix endpoint 404s).
    fn now_playing_radio_button(&self) -> Option<Element<'_, Message>> {
        let track = self.playback_queue.get(self.playback_queue_index)?;
        if track.is_video {
            return None;
        }
        let mut ri = icon::from_svg_bytes(RADIO_SVG);
        ri.symbolic = true;
        Some(
            button::icon(ri)
                .tooltip(fl!("tooltip-go-to-track-radio"))
                .padding(4)
                .on_press(Message::ShowTrackRadio(track.clone()))
                .into(),
        )
    }

    /// The now-playing-bar lyrics button, shown only once we've confirmed the
    /// current track actually has lyrics (otherwise the icon is hidden).
    fn now_playing_lyrics_button(&self) -> Option<Element<'_, Message>> {
        let track = self.playback_queue.get(self.playback_queue_index)?;
        match &self.now_playing_lyrics {
            Some((id, true)) if *id == track.id => {}
            _ => return None,
        }
        let mut li = icon::from_svg_bytes(LYRICS_SVG);
        li.symbolic = true;
        Some(
            button::icon(li)
                .tooltip(fl!("tooltip-show-lyrics"))
                .padding(4)
                .on_press(Message::ShowLyrics(track.clone()))
                .into(),
        )
    }

    /// Render the now-playing bar shown at the bottom of the popup.
    fn view_now_playing_bar(&self, np: &crate::tidal::player::NowPlaying) -> Element<'_, Message> {
        let play_pause_icon = if self.playback_state == PlaybackState::Playing {
            "media-playback-pause-symbolic"
        } else {
            "media-playback-start-symbolic"
        };

        let progress = if np.duration > 0.0 {
            (self.playback_position / np.duration * 100.0) as u8
        } else {
            0
        };

        // Check if current track is favorited
        let is_favorite = self.favorite_track_ids.contains(&np.track_id);

        // Get the current track from the queue for the favorite toggle
        let current_track = self.playback_queue.get(self.playback_queue_index).cloned();
        let track_for_share_prompt = current_track.clone();

        // Build context line: "Album • Playlist" or just one if the other is missing
        // Avoid duplication when album name equals playlist/context name
        let context_line = {
            let album = np.album.as_deref().unwrap_or("");
            let playlist = np.playlist_name.as_deref().unwrap_or("");
            match (album.is_empty(), playlist.is_empty(), album == playlist) {
                // Both present but same (e.g., playing from album context)
                (false, false, true) => album.to_string(),
                // Both present and different (e.g., track from album in a playlist)
                (false, false, false) => format!("{} • {}", album, playlist),
                (false, true, _) => album.to_string(),
                (true, false, _) => playlist.to_string(),
                (true, true, _) => String::new(),
            }
        };

        // Album art for now playing bar
        let now_playing_art: Element<'_, Message> = if let Some(url) = &np.cover_url {
            if let Some(handle) = self.loaded_images.get(url) {
                cosmic::widget::image(handle.clone())
                    .width(NOW_PLAYING_ART_SIZE)
                    .height(NOW_PLAYING_ART_SIZE)
                    .into()
            } else {
                widget::icon::from_name("media-optical-symbolic")
                    .size(NOW_PLAYING_ART_SIZE)
                    .into()
            }
        } else {
            widget::icon::from_name("media-optical-symbolic")
                .size(NOW_PLAYING_ART_SIZE)
                .into()
        };

        // Artist name — clickable if we have an artist_id from the current track
        let artist_name = np.artist.clone();
        let artist_element: Element<'_, Message> =
            if let Some(artist_id) = current_track.as_ref().and_then(|t| t.artist_id.clone()) {
                button::custom(text(artist_name).size(14).wrapping(Wrapping::None))
                    .on_press(Message::ShowArtistDetail(artist_id))
                    .width(Length::Shrink)
                    .padding(0)
                    .class(cosmic::theme::Button::MenuItem)
                    .into()
            } else {
                text(artist_name).size(14).wrapping(Wrapping::None).into()
            };

        // Context line (album • playlist) — album part is clickable if we have album_id
        let context_element: Option<Element<'_, Message>> = if context_line.is_empty() {
            None
        } else if let Some(album_id) = current_track.as_ref().and_then(|t| t.album_id.clone()) {
            // Make the whole context line clickable, navigating to the album
            Some(
                button::custom(text(context_line.clone()).size(12).wrapping(Wrapping::None))
                    .on_press(Message::ShowAlbumDetailById(album_id))
                    .width(Length::Shrink)
                    .padding(0)
                    .class(cosmic::theme::Button::MenuItem)
                    .into(),
            )
        } else {
            Some(text(context_line).size(12).wrapping(Wrapping::None).into())
        };

        // Track title — clickable to navigate to the track detail (recommendations) view
        let track_for_detail = self.playback_queue.get(self.playback_queue_index).cloned();
        let title_element: Element<'_, Message> = if let Some(track) = track_for_detail {
            button::custom(text(np.title.clone()).size(16).wrapping(Wrapping::None))
                .on_press(Message::ShowTrackDetail(track))
                .width(Length::Shrink)
                .padding(0)
                .class(cosmic::theme::Button::MenuItem)
                .into()
        } else {
            text(np.title.clone())
                .size(16)
                .wrapping(Wrapping::None)
                .into()
        };

        // Track info text — GPU-clipped with gradient fade to surface colour
        let track_info_col = widget::Column::new()
            .push(title_element)
            .push(artist_element)
            .push_maybe(context_element)
            .spacing(3)
            .align_x(Alignment::Start)
            .width(Length::Fill);

        let track_info = crate::views::components::fading_card_column(track_info_col);

        // Info row: in video mode, album thumbnail + track info + spectrum
        // visualizer (the video pipeline taps its audio into the same analyzer)
        // — this same row, with its clickable title/artist/context, is overlaid
        // as the auto-hiding HUD. The audio bar uses the identical layout.
        let info_row: Element<'_, Message> = if self.video_player.is_some() {
            let visualizer = self.visualizer_state.view();
            widget::Row::new()
                .push(now_playing_art)
                .push(track_info)
                .push(visualizer)
                .spacing(8)
                .align_y(Alignment::Center)
                .width(Length::Fill)
                .into()
        } else {
            let visualizer = self.visualizer_state.view();
            widget::Row::new()
                .push(now_playing_art)
                .push(track_info)
                .push(visualizer)
                .spacing(8)
                .align_y(Alignment::Center)
                .into()
        };

        // Buttons row below - centered
        let buttons_row = widget::Row::new()
            .push_maybe({
                // Videos can't be favorited (TIDAL's track-favorites endpoint
                // 404s on a video id), so omit the heart for them.
                let is_video = current_track.as_ref().is_some_and(|t| t.is_video);
                (!is_video).then(|| {
                    let tip = if is_favorite {
                        fl!("tooltip-remove-from-favorites")
                    } else {
                        fl!("tooltip-add-to-favorites")
                    };
                    let btn = button::icon(favorite_icon_handle(is_favorite))
                        .tooltip(tip)
                        .padding(4);
                    let btn: Element<'_, Message> = match current_track.clone() {
                        Some(track) => btn.on_press(Message::ToggleFavorite(track)).into(),
                        None => btn.into(),
                    };
                    btn
                })
            })
            .push(
                button::icon(widget::icon::from_name("media-skip-backward-symbolic"))
                    .tooltip(fl!("tooltip-previous-track"))
                    .on_press(Message::PreviousTrack)
                    .padding(4),
            )
            .push(
                button::icon(widget::icon::from_name(play_pause_icon))
                    .tooltip(if self.playback_state == PlaybackState::Playing {
                        fl!("tooltip-pause")
                    } else {
                        fl!("tooltip-play")
                    })
                    .on_press(Message::TogglePlayPause)
                    .padding(4),
            )
            .push(
                button::icon(widget::icon::from_name("media-skip-forward-symbolic"))
                    .tooltip(fl!("tooltip-next-track"))
                    .on_press(Message::NextTrack)
                    .padding(4),
            )
            .push({
                let (mode_icon, tip) = if self.shuffle_enabled {
                    (
                        "media-playlist-shuffle-symbolic",
                        fl!("tooltip-mode-shuffle"),
                    )
                } else {
                    match self.loop_status {
                        crate::tidal::mpris::LoopStatus::None => (
                            "media-playlist-consecutive-symbolic",
                            fl!("tooltip-mode-normal"),
                        ),
                        crate::tidal::mpris::LoopStatus::Playlist => (
                            "media-playlist-repeat-symbolic",
                            fl!("tooltip-mode-repeat-all"),
                        ),
                        crate::tidal::mpris::LoopStatus::Track => (
                            "media-playlist-repeat-song-symbolic",
                            fl!("tooltip-mode-repeat-track"),
                        ),
                    }
                };
                button::icon(widget::icon::from_name(mode_icon))
                    .tooltip(tip)
                    .on_press(Message::CyclePlaybackMode)
                    .padding(4)
            })
            .push(
                button::icon(widget::icon::from_name("media-playback-stop-symbolic"))
                    .tooltip(fl!("tooltip-stop"))
                    .on_press(Message::StopPlayback)
                    .padding(4),
            )
            .push_maybe(self.now_playing_radio_button())
            .push_maybe(self.now_playing_lyrics_button())
            .push({
                let btn = button::icon(widget::icon::from_name("emblem-shared-symbolic"))
                    .tooltip(fl!("tooltip-share"))
                    .padding(4);
                if let Some(track) = track_for_share_prompt {
                    btn.on_press(Message::ShowSharePrompt(track))
                } else {
                    btn
                }
            })
            .push_maybe(self.pop_out_video_button());

        // In standalone mode, append a volume button with a popover slider.
        // Panel-applet mode uses scroll wheel on the panel icon instead.
        #[cfg(not(feature = "panel-applet"))]
        let buttons_row = {
            let volume_icon_name = if self.volume_level <= 0.0 {
                "audio-volume-muted-symbolic"
            } else if self.volume_level < 0.34 {
                "audio-volume-low-symbolic"
            } else if self.volume_level < 0.67 {
                "audio-volume-medium-symbolic"
            } else {
                "audio-volume-high-symbolic"
            };

            let vol_btn = button::icon(widget::icon::from_name(volume_icon_name))
                .tooltip(fl!(
                    "tooltip-volume",
                    percent = format!("{}", (self.volume_level * 100.0).round() as u8)
                ))
                .on_press(Message::ToggleVolumePopup)
                .padding(4);

            let vol_element: Element<'_, Message> = if self.show_volume_popup {
                // Build a true vertical slider inside a styled card container.
                // `vertical_slider` renders bottom-to-top (min at bottom, max
                // at top) which is the natural orientation for a volume knob.
                let vol_pct_label =
                    text(format!("{}%", (self.volume_level * 100.0).round() as u8)).size(11);

                let vol_slider = vertical_slider(0.0..=1.0, self.volume_level, Message::SetVolume)
                    .step(0.01)
                    .width(20)
                    .height(Length::Fixed(120.0));

                let popup_content = container(
                    widget::Column::new()
                        .push(vol_pct_label)
                        .push(vol_slider)
                        .push(widget::icon::from_name(volume_icon_name).size(16))
                        .spacing(6)
                        .align_x(Alignment::Center),
                )
                .padding(8)
                .class(cosmic::theme::Container::Card);

                popover(vol_btn)
                    .popup(popup_content)
                    .position(Position::Point(cosmic::iced::Point::new(0.0, -180.0)))
                    .on_close(Message::CloseVolumePopup)
                    .into()
            } else {
                vol_btn.into()
            };

            // Wrap the volume icon in a mouse_area so the user can scroll
            // to adjust volume without needing to open the popover first.
            let vol_element: Element<'_, Message> = widget::mouse_area(vol_element)
                .on_scroll(|delta| Message::AdjustVolume(scroll_to_volume_delta(delta)))
                .into();

            buttons_row.push(vol_element)
        };

        let buttons_row = buttons_row.spacing(8).align_y(Alignment::Center);

        // Center the buttons row
        let centered_buttons = container(buttons_row).center_x(Length::Fill);

        // Elapsed and remaining time labels flanking the seek slider
        let elapsed = format_seconds(self.playback_position);
        let remaining = if np.duration > 0.0 {
            format!("-{}", format_seconds(np.duration - self.playback_position))
        } else {
            String::from("-0:00")
        };

        let is_buffering = self.playback_state == PlaybackState::Loading;

        // Use the same slider widget for both states.  During buffering the
        // handle smoothly pulses between full and dim accent colour so the
        // user can see that something is happening, while the slider keeps its
        // exact native size and rail styling.
        let seek_slider = {
            let mut s = widget::slider(0.0..=100.0, progress as f32, |val| {
                Message::SeekTo(val as f64)
            })
            .height(4)
            .width(Length::Fill);

            if is_buffering {
                s = s.class(buffering_slider_class(self.loading_progress));
            }

            s
        };

        let progress_row = widget::Row::new()
            .push(text(elapsed).size(10))
            .push(seek_slider)
            .push(text(remaining).size(10))
            .spacing(6)
            .align_y(Alignment::Center);

        // Video mode: replace the compact bar with a full-area theater whose
        // overlay (track info + controls) auto-hides. When the video is popped
        // out into its own window, `video_player` is None so this falls through
        // to the audio-style bar instead.
        if let Some(video) = &self.video_player
            && self.video_window.is_none()
        {
            let controls = widget::Column::new()
                .push(centered_buttons)
                .push(progress_row)
                .spacing(6)
                .width(Length::Fill)
                .into();
            return self.video_theater(video, info_row, controls);
        }

        let bar_col = widget::Column::new()
            .push(info_row)
            .push(centered_buttons)
            .push(progress_row)
            .spacing(6)
            .width(Length::Fill);

        container(bar_col)
            .padding(8)
            .class(cosmic::theme::Container::Card)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::concise_error;

    #[test]
    fn short_single_line_is_unchanged() {
        assert_eq!(concise_error("Network unavailable"), "Network unavailable");
    }

    #[test]
    fn drops_everything_after_the_first_line() {
        let err = "failed to parse JSON (status 200 OK): invalid type: null\nresponse body: {\"items\":[...huge...]}";
        let out = concise_error(err);
        assert!(!out.contains("response body"));
        assert!(!out.contains('\n'));
        assert!(out.starts_with("failed to parse JSON"));
    }

    #[test]
    fn drops_a_json_body_on_the_same_line() {
        let err = "Failed to get playback URL: Request failed: HTTP 500 Internal Server Error: {\"status\":500,\"userMessage\":\"oops\"}";
        let out = concise_error(err);
        assert!(!out.contains('{'));
        assert!(!out.contains("status"));
        assert_eq!(
            out,
            "Failed to get playback URL: Request failed: HTTP 500 Internal Server Error"
        );
    }

    #[test]
    fn caps_a_long_first_line_with_an_ellipsis() {
        let err = "e".repeat(500);
        let out = concise_error(&err);
        // 120 chars + the ellipsis.
        assert_eq!(out.chars().count(), 121);
        assert!(out.ends_with('\u{2026}'));
    }
}
