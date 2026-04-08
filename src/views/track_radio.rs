// SPDX-License-Identifier: MIT

//! Track radio view for Maré Player.
//!
//! Shows a list of recommended/similar tracks generated from a seed track,
//! mirroring TIDAL's "Go to track radio" feature.

use cosmic::Element;
use cosmic::iced::Length;
use cosmic::widget::{self, button, text};

use crate::fl;
use crate::messages::Message;
use crate::state::AppModel;
use crate::views::components::{TrackRowOptions, fading_header_title, scrollable_list};

impl AppModel {
    /// Render the track radio view showing similar tracks based on a seed track.
    pub fn view_track_radio(&self) -> Element<'_, Message> {
        let title = self
            .selected_radio_source_track
            .as_ref()
            .map(|t| fl!("track-radio", title = t.title.clone()))
            .unwrap_or_else(|| fl!("track-radio-fallback"));

        let tracks = self.selected_radio_tracks.clone();

        let header = widget::Row::new()
            .push(
                button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .on_press(Message::NavigateBack)
                    .padding(4),
            )
            .push(fading_header_title(&title))
            .push(
                button::icon(widget::icon::from_name("media-playlist-shuffle-symbolic"))
                    .tooltip(fl!("tooltip-shuffle-play"))
                    .on_press_maybe(if tracks.is_empty() {
                        None
                    } else {
                        Some(Message::ShufflePlay(tracks.clone(), Some(title.clone())))
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
            let all_tracks = self.selected_radio_tracks.clone();
            let context = Some(title);
            let track_items: Vec<Element<'_, Message>> = self
                .selected_radio_tracks
                .iter()
                .enumerate()
                .map(|(index, track)| {
                    self.track_row(
                        track,
                        index,
                        &TrackRowOptions {
                            tracks: &all_tracks,
                            context: context.clone(),
                            show_radio_button: false,
                            ..Default::default()
                        },
                    )
                })
                .collect();

            scrollable_list(widget::Column::with_children(track_items).spacing(2))
        };

        widget::Column::new()
            .push(header)
            .push(tracks_content)
            .spacing(12)
            .padding(12)
            .width(Length::Fill)
            .into()
    }
}
