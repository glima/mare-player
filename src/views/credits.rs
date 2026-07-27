// SPDX-License-Identifier: MIT

//! Credits view for Maré Player.
//!
//! Renders the "who made this record" panel for a single track, mirroring the
//! credits tab in the TIDAL desktop client: a stack of labelled fields, first
//! the catalog basics (title, artists, album, release date, label) and then
//! one block per credit role (Producer, Composer, Mixing Engineer, …).
//!
//! Everything comes from [`TrackCredits`], fetched by
//! [`get_track_credits`](crate::tidal::client::TidalAppClient::get_track_credits).
//! Contributors carry real TIDAL artist ids, so each credited person is a
//! link straight to their artist page — the whole point of showing credits is
//! being able to follow the session players around the catalog.
//!
//! Three render states, same contract as the lyrics view:
//!
//! 1. **Loading** — nothing stored yet; the fetch is in flight.
//! 2. **Empty** — TIDAL has no credits for this track (a `200` with `[]`,
//!    which is common for older or independent releases).
//! 3. **Loaded** — the field stack.

use cosmic::Element;
use cosmic::iced::alignment::Horizontal;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, container, text};

use crate::fl;
use crate::messages::Message;
use crate::state::AppModel;
use crate::tidal::models::{CreditContributor, TrackCredits};
use crate::views::components::{
    fading_header_title, fading_text, fading_text_column, scrollable_list,
};

/// Font size for a field's caption ("PRODUCER", "LABEL", …).
const FIELD_LABEL_SIZE: u16 = 10;
/// Font size for a field's value.
const FIELD_VALUE_SIZE: u16 = 14;

impl AppModel {
    /// Render the credits view for the currently-selected credits track.
    pub fn view_credits(&self) -> Element<'_, Message> {
        let track = self.selected_credits_track.as_ref();
        let track_title = track
            .map(|t| t.title.clone())
            .unwrap_or_else(|| fl!("credits-title-fallback"));
        let header_title = fl!("credits-title", title = track_title.clone());

        let header = widget::Row::new()
            .push(
                button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .on_press(Message::NavigateBack)
                    .padding(4),
            )
            .push(fading_header_title(&header_title))
            .spacing(8)
            .align_y(Alignment::Center);

        let body: Element<'_, Message> = match &self.selected_track_credits {
            // Still fetching (or the fetch failed — the error banner covers
            // that case, and a retry is one back-and-forward away).
            None => container(text(fl!("loading-credits")).size(14))
                .width(Length::Fill)
                .align_x(Horizontal::Center)
                .padding([24, 0])
                .into(),
            Some(credits) if credits.is_empty() => {
                container(text(fl!("no-credits-available", title = track_title)).size(14))
                    .width(Length::Fill)
                    .align_x(Horizontal::Center)
                    .padding([24, 0])
                    .into()
            }
            Some(credits) => self.render_credits(credits),
        };

        widget::Column::new()
            .push(header)
            .push(body)
            .spacing(12)
            .padding(12)
            .width(Length::Fill)
            .into()
    }

    /// Render the field stack: catalog basics first, then the credit roles.
    ///
    /// This is a plain (non-virtual) column inside a scrollable: credit lists
    /// are short — a couple of dozen rows for even the most heavily-credited
    /// record — so the bookkeeping of a virtual `List` would cost more than it
    /// saves here.
    fn render_credits(&self, credits: &TrackCredits) -> Element<'_, Message> {
        let track = self.selected_credits_track.as_ref();
        let mut column = widget::Column::new().spacing(12).width(Length::Fill);

        // ── Catalog basics ────────────────────────────────────────────────
        if let Some(track) = track {
            column = column.push(field_text(fl!("credits-field-title"), &track.title));

            // Artist links through to the artist page when we know the id.
            let artist = CreditContributor {
                name: track.artist_name.clone(),
                id: track.artist_id.clone(),
            };
            column = column.push(field_people(
                fl!("credits-field-artists"),
                std::slice::from_ref(&artist),
            ));

            if let Some(album_name) = &track.album_name {
                let value: Element<'_, Message> = match &track.album_id {
                    Some(album_id) => {
                        link(album_name, Message::ShowAlbumDetailById(album_id.clone()))
                    }
                    None => plain_value(album_name),
                };
                column = column.push(field(fl!("credits-field-album"), value));
            }
        }

        if let Some(released) = &credits.released {
            column = column.push(field_text(fl!("credits-field-released"), released));
        }
        if let Some(copyright) = &credits.copyright {
            column = column.push(field_text(fl!("credits-field-label"), copyright));
        }

        // ── Credit roles (TIDAL's own ordering) ───────────────────────────
        for role in &credits.roles {
            column = column.push(field_people(role.role.clone(), &role.contributors));
        }

        // ── Technical tail ────────────────────────────────────────────────
        if let Some(isrc) = &credits.isrc {
            column = column.push(field_text(fl!("credits-field-isrc"), isrc));
        }
        if let Some(bpm) = credits.bpm {
            column = column.push(field_text(fl!("credits-field-bpm"), &bpm.to_string()));
        }

        scrollable_list(column)
    }
}

/// One labelled field: small caption above an arbitrary value element.
fn field<'a>(label: String, value: Element<'a, Message>) -> Element<'a, Message> {
    widget::Column::new()
        .push(fading_text_column(vec![
            text(label)
                .size(FIELD_LABEL_SIZE)
                .wrapping(Wrapping::None)
                .into(),
        ]))
        .push(value)
        .spacing(2)
        .width(Length::Fill)
        .into()
}

/// A labelled field whose value is plain, non-clickable text.
fn field_text<'a>(label: String, value: &str) -> Element<'a, Message> {
    field(label, plain_value(value))
}

/// A labelled field listing people: each contributor with a known artist id
/// becomes a link to their artist page, the rest render as plain text.
fn field_people<'a>(label: String, people: &[CreditContributor]) -> Element<'a, Message> {
    let mut names = widget::Column::new().spacing(1).width(Length::Fill);
    for person in people {
        names = names.push(match &person.id {
            Some(id) => link(&person.name, Message::ShowArtistDetail(id.clone())),
            None => plain_value(&person.name),
        });
    }
    field(label, names.into())
}

/// Non-interactive field value, alpha-faded when it overflows the popup width.
fn plain_value<'a>(value: &str) -> Element<'a, Message> {
    fading_text_column(vec![
        text(value.to_string())
            .size(FIELD_VALUE_SIZE)
            .wrapping(Wrapping::None)
            .into(),
    ])
}

/// Clickable field value (artist / album link).
///
/// The fade lives *inside* the button so it ramps the button's own text
/// colour; a shrink-width button keeps the hover highlight hugging the name
/// instead of spanning the whole row.
fn link<'a>(value: &str, on_press: Message) -> Element<'a, Message> {
    button::custom(fading_text(
        text(value.to_string())
            .size(FIELD_VALUE_SIZE)
            .wrapping(Wrapping::None),
    ))
    .on_press(on_press)
    .width(Length::Shrink)
    .padding(0)
    .class(cosmic::theme::Button::MenuItem)
    .into()
}
