// SPDX-License-Identifier: MIT

//! Track radio view for Maré Player.
//!
//! Shows a list of recommended/similar tracks generated from a seed track,
//! mirroring TIDAL's "Go to track radio" feature.
//!
//! Track rows are rendered through a virtual `List` — only the rows
//! visible in the scroll viewport are materialised.

use std::sync::Arc;

use cosmic::Element;
use cosmic::iced::Length;
use cosmic::widget::{self, button, text};

use crate::fl;
use crate::messages::Message;
use crate::state::AppModel;
use crate::views::components::rows::build_track_row;
use crate::views::components::{TrackRowOptions, back_button, fading_header_title, scrollable_element, virtual_list_row};

impl AppModel {
    /// Render the track radio view showing similar tracks based on a seed track.
    pub fn view_track_radio(&self) -> Element<'_, Message> {
        let title = self
            .selected_radio_source_track
            .as_ref()
            .map(|t| fl!("track-radio", title = t.title.clone()))
            .unwrap_or_else(|| fl!("track-radio-fallback"));

        let tracks: Arc<[_]> = self.selected_radio_tracks.clone().into();

        // Attribute plays as MIX:<mix_id> once the track-seeded mix has
        // loaded -- this is the only sourceType that surfaces track-radio
        // listening in TIDAL's Recently Played (rendered as a "Track
        // Radio" tile via the mix's mixType=TRACK_MIX).  Before the mix
        // resolves no playback can start (the list is empty), so the
        // seed-track TRACK_RADIO fallback is purely defensive.
        let radio_source = match (&self.selected_radio_mix_id, &self.selected_radio_source_track) {
            (Some(mix_id), _) => crate::tidal::models::PlaybackSource::mix(mix_id.clone(), title.clone()),
            (None, Some(seed)) => crate::tidal::models::PlaybackSource::track_radio(seed.id.clone(), title.clone()),
            (None, None) => crate::tidal::models::PlaybackSource::ad_hoc(title.clone()),
        };

        let header = widget::Row::new()
            .push(back_button(Message::NavigateBack))
            .push(fading_header_title(&title))
            .push(
                button::icon(widget::icon::from_name("media-playlist-shuffle-symbolic"))
                    .tooltip(fl!("tooltip-shuffle-play"))
                    .on_press_maybe(if tracks.is_empty() {
                        None
                    } else {
                        Some(Message::ShufflePlay(Arc::clone(&tracks), Some(radio_source.clone())))
                    })
                    .padding(4),
            )
            .spacing(8)
            .align_y(cosmic::iced::Alignment::Center);

        let tracks_content: Element<'_, Message> = if self.is_loading {
            text(fl!("loading-radio-tracks")).size(14).into()
        } else if self.selected_radio_tracks.is_empty() {
            text(fl!("no-radio-tracks")).size(14).into()
        } else {
            let loaded_images = &self.loaded_images;
            let opts = TrackRowOptions {
                tracks: Arc::clone(&self.track_list_arc),
                source: Some(radio_source),
                show_radio_button: false,
                ..Default::default()
            };

            let track_list = cosmic::iced::widget::list::List::new(&self.track_list_content, move |index, track| {
                virtual_list_row(build_track_row(loaded_images, track, index, &opts), 2)
            });

            scrollable_element(track_list)
        };

        widget::Column::new().push(header).push(tracks_content).spacing(12).padding(12).width(Length::Fill).into()
    }
}
