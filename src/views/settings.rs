// SPDX-License-Identifier: MIT

//! Settings view for Maré Player.
//!
//! This module contains the settings interface for configuring
//! audio quality, managing cache, and account settings.

use cosmic::Element;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::config::AudioQuality;
use crate::fl;
use crate::messages::Message;
use crate::state::AppModel;
use crate::tidal::auth::AuthState;

/// Available audio cache size presets in megabytes
static CACHE_SIZE_OPTIONS: &[(u32, &str)] = &[
    (500, "500 MB"),
    (1000, "1 GB"),
    (2000, "2 GB"),
    (5000, "5 GB"),
    (10000, "10 GB"),
    (20000, "20 GB"),
];

/// Labels for the cache size dropdown (must be static for lifetime)
static CACHE_SIZE_LABELS: &[&str] = &["500 MB", "1 GB", "2 GB", "5 GB", "10 GB", "20 GB"];

/// Available audio quality options
static QUALITY_OPTIONS: &[AudioQuality] = &[
    AudioQuality::Low,
    AudioQuality::High,
    AudioQuality::Lossless,
    AudioQuality::HiRes,
];

impl AppModel {
    /// Render the settings view.
    pub fn view_settings(&self) -> Element<'_, Message> {
        let header = widget::row()
            .push(
                button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .on_press(Message::ShowMain)
                    .padding(4),
            )
            .push(text(fl!("settings")).size(18))
            .spacing(8)
            .align_y(Alignment::Center);

        let (_is_authenticated, user_profile) = {
            let client = self.tidal_client.blocking_lock();
            match client.auth_state() {
                AuthState::Authenticated { profile } => (true, Some(profile.clone())),
                _ => (false, None),
            }
        };

        // Audio quality section
        let current_quality = self.config.audio_quality;
        let selected_idx = QUALITY_OPTIONS
            .iter()
            .position(|q| *q == current_quality)
            .unwrap_or(1);

        let quality_section = widget::column()
            .push(text(fl!("audio-quality")).size(14))
            .push(
                widget::dropdown(QUALITY_OPTIONS, Some(selected_idx), |idx| {
                    Message::SetAudioQuality(
                        QUALITY_OPTIONS
                            .get(idx)
                            .copied()
                            .unwrap_or(AudioQuality::High),
                    )
                })
                .width(Length::Fill),
            )
            .push(
                text(match current_quality {
                    AudioQuality::Low => fl!("quality-description-low"),
                    AudioQuality::High => fl!("quality-description-high"),
                    AudioQuality::Lossless => fl!("quality-description-lossless"),
                    AudioQuality::HiRes => fl!("quality-description-hires"),
                })
                .size(11),
            )
            .spacing(8);

        // Cache section
        // Audio cache info (songs cached on disk)
        let audio_cache_size_mb = {
            let client = self.tidal_client.blocking_lock();
            client.audio_cache_size() / (1024 * 1024)
        };

        let audio_cache_max_mb = self.config.audio_cache_max_mb as u64;

        // Find the currently-selected cache size preset index
        let cache_size_idx = CACHE_SIZE_OPTIONS
            .iter()
            .position(|(mb, _)| *mb as u64 == audio_cache_max_mb)
            .unwrap_or_else(|| {
                // Pick the closest option
                CACHE_SIZE_OPTIONS
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, (mb, _))| {
                        (*mb as i64 - audio_cache_max_mb as i64).unsigned_abs()
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(2)
            });

        let cache_section = widget::column()
            .push(text(fl!("cache")).size(14))
            .push(
                widget::row()
                    .push(text(fl!("song-cache")).size(12))
                    .push(widget::space::horizontal())
                    .push(
                        text(fl!(
                            "song-cache-size",
                            used = audio_cache_size_mb.to_string(),
                            max = audio_cache_max_mb.to_string()
                        ))
                        .size(12),
                    ),
            )
            .push(
                widget::row()
                    .push(text(fl!("song-cache-limit")).size(12))
                    .push(widget::space::horizontal())
                    .push(
                        button::destructive(fl!("clear-cache")).on_press(Message::ClearAudioCache),
                    )
                    .push(
                        widget::dropdown(CACHE_SIZE_LABELS, Some(cache_size_idx), |idx| {
                            let mb = CACHE_SIZE_OPTIONS
                                .get(idx)
                                .map(|(mb, _)| *mb)
                                .unwrap_or(2000);
                            Message::SetAudioCacheMaxMb(mb)
                        })
                        .width(Length::Fixed(120.0)),
                    )
                    .spacing(8)
                    .align_y(Alignment::Center),
            )
            .spacing(8);

        let account_section: Element<'_, Message> = if let Some(profile) = &user_profile {
            let display_name = profile.display_name();

            // Avatar: use profile picture if loaded, otherwise show initials
            let avatar: Element<'_, Message> = if let Some(pic_url) = &profile.picture_url
                && let Some(handle) = self.loaded_images.get(pic_url)
            {
                widget::image(handle.clone())
                    .width(Length::Fixed(40.0))
                    .height(Length::Fixed(40.0))
                    .into()
            } else {
                // Initials circle fallback
                widget::container(text(profile.initials()).size(16))
                    .width(Length::Fixed(40.0))
                    .height(Length::Fixed(40.0))
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center)
                    .class(cosmic::theme::Container::custom(|theme| {
                        let cosmic = theme.cosmic();
                        cosmic::widget::container::Style {
                            icon_color: Some(cosmic.accent.on.into()),
                            text_color: Some(cosmic.accent.on.into()),
                            background: Some(cosmic::iced::Background::Color(
                                cosmic.accent.base.into(),
                            )),
                            border: cosmic::iced::Border {
                                radius: 20.0.into(),
                                ..Default::default()
                            },
                            shadow: Default::default(),
                            snap: false,
                        }
                    }))
                    .into()
            };

            // Name + email column
            let mut info_col = widget::column().spacing(2);
            info_col = info_col.push(text(display_name.clone()).size(14));

            // Show email underneath if it's different from the display name
            if let Some(email) = &profile.email
                && email != &display_name
            {
                info_col = info_col.push(text(email.clone()).size(11).class(
                    cosmic::theme::Text::Custom(|theme| cosmic::iced::widget::text::Style {
                        color: Some(theme.cosmic().palette.neutral_7.into()),
                    }),
                ));
            }

            // Plan badge — shows subscription tier from /v1/users/{id}/subscription
            if let Some(plan) = &profile.subscription_plan {
                info_col = info_col.push(
                    widget::container(text(plan.clone()).size(10))
                        .padding([2, 8])
                        .class(cosmic::theme::Container::custom(|theme| {
                            let cosmic = theme.cosmic();
                            cosmic::widget::container::Style {
                                icon_color: Some(cosmic.accent.on.into()),
                                text_color: Some(cosmic.accent.on.into()),
                                background: Some(cosmic::iced::Background::Color(
                                    cosmic.accent.base.into(),
                                )),
                                border: cosmic::iced::Border {
                                    radius: 4.0.into(),
                                    ..Default::default()
                                },
                                shadow: Default::default(),
                                snap: false,
                            }
                        })),
                );
            }

            let user_row = widget::row()
                .push(avatar)
                .push(info_col)
                .push(widget::space::horizontal())
                .push(button::destructive(fl!("sign-out")).on_press(Message::Logout))
                .spacing(12)
                .align_y(Alignment::Center);

            widget::column()
                .push(text(fl!("account")).size(14))
                .push(user_row)
                .spacing(12)
                .into()
        } else {
            widget::column()
                .push(text(fl!("account")).size(14))
                .push(text(fl!("not-signed-in")).size(12))
                .push(
                    button::suggested(fl!("sign-in-button"))
                        .on_press(Message::StartLogin)
                        .width(Length::Fill),
                )
                .spacing(12)
                .into()
        };

        // About section
        let about_section = widget::column()
            .push(text(fl!("about")).size(14))
            .push(
                widget::row()
                    .push(text(fl!("version")).size(12))
                    .push(widget::space::horizontal())
                    .push(text(env!("CARGO_PKG_VERSION")).size(12))
                    .align_y(Alignment::Center),
            )
            .spacing(8);

        // App icon at bottom center
        static APP_ICON_SVG: &[u8] = include_bytes!("../../resources/icon.svg");
        let icon_handle = widget::icon::from_svg_bytes(APP_ICON_SVG);
        let app_icon = widget::container(widget::icon(icon_handle).size(64))
            .width(Length::Fill)
            .align_x(Alignment::Center);

        widget::column()
            .push(header)
            .push(app_icon)
            .push(widget::space::vertical().height(8))
            .push(account_section)
            .push(widget::space::vertical().height(8))
            .push(quality_section)
            .push(widget::space::vertical().height(8))
            .push(cache_section)
            .push(widget::space::vertical().height(8))
            .push(about_section)
            .spacing(8)
            .padding(12)
            .width(Length::Fill)
            .into()
    }
}
