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
use crate::views::components::{NOW_PLAYING_ART_SIZE, RADIO_SVG, favorite_icon_handle};

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
            ViewState::TrackDetail => self.view_track_detail(),
            ViewState::FavoriteTracks => self.view_favorite_tracks(),
            ViewState::History => self.view_history(),
            ViewState::Profiles => self.view_profiles(),
            ViewState::Settings => self.view_settings(),
            ViewState::SharePrompt(track_id, track_title, album_id, album_title) => self
                .view_share_prompt(
                    track_id.clone(),
                    track_title.clone(),
                    album_id.clone(),
                    album_title.clone(),
                ),
        }
    }

    /// Build an error banner element for display at the top of the content.
    fn view_error_banner<'a>(&'a self, error: &'a str) -> Element<'a, Message> {
        let error_row = widget::Row::new()
            .push(text(error).size(12))
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

        // Add now playing bar if something is playing
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

        // Now-playing bar pinned at the bottom (shrink to intrinsic height)
        if let Some(np) = &self.now_playing {
            col = col.push(self.view_now_playing_bar(np));
        }

        col.into()
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

        // Visualizer widget
        let visualizer = self.visualizer_state.view();

        // Info row with album art on the left, track info, then visualizer on the right
        let info_row = widget::Row::new()
            .push(now_playing_art)
            .push(track_info)
            .push(visualizer)
            .spacing(8)
            .align_y(Alignment::Center);

        // Buttons row below - centered
        let track_for_radio = self.playback_queue.get(self.playback_queue_index).cloned();

        let buttons_row = widget::Row::new()
            .push({
                let tip = if is_favorite {
                    fl!("tooltip-remove-from-favorites")
                } else {
                    fl!("tooltip-add-to-favorites")
                };
                let btn = button::icon(favorite_icon_handle(is_favorite))
                    .tooltip(tip)
                    .padding(4);
                if let Some(track) = current_track {
                    btn.on_press(Message::ToggleFavorite(track))
                } else {
                    btn
                }
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
            .push({
                let mut ri = icon::from_svg_bytes(RADIO_SVG);
                ri.symbolic = true;
                let btn = button::icon(ri)
                    .tooltip(fl!("tooltip-go-to-track-radio"))
                    .padding(4);
                if let Some(track) = track_for_radio {
                    btn.on_press(Message::ShowTrackRadio(track))
                } else {
                    btn
                }
            })
            .push({
                let btn = button::icon(widget::icon::from_name("emblem-shared-symbolic"))
                    .tooltip(fl!("tooltip-share"))
                    .padding(4);
                if let Some(track) = track_for_share_prompt {
                    btn.on_press(Message::ShowSharePrompt(track))
                } else {
                    btn
                }
            });

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
