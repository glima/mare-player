// SPDX-License-Identifier: MIT

//! Panel button view for Maré Player.
//!
//! This module renders the panel button that appears in the system panel,
//! showing either the app icon or the currently playing track info.
//! Also handles mouse wheel for volume control with a visual volume bar overlay.

use cosmic::Element;
use cosmic::iced::gradient;

use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Background, Border, Color, Length, Radians};
use cosmic::widget::button::Catalog;
use cosmic::widget::{self, autosize, button, container};

use crate::messages::Message;
use crate::state::AppModel;
use crate::tidal::player::PlaybackState;
use crate::views::components::{PANEL_ART_SIZE, VOLUME_BAR_WIDTH, scroll_to_volume_delta};

/// Static ID for the autosize widget
static AUTOSIZE_MAIN_ID: std::sync::LazyLock<widget::Id> =
    std::sync::LazyLock::new(widget::Id::unique);

impl AppModel {
    /// Render the panel button view.
    ///
    /// If something is playing, shows album art + "Title · Artist · Album".
    /// Otherwise shows just the app icon.
    /// Mouse wheel adjusts volume and shows a green volume bar on the right.
    pub fn view_panel(&self) -> Element<'_, Message> {
        if let Some(np) = &self.now_playing {
            // Build the album art thumbnail (small, fits panel)
            let base_art: Element<'_, Message> = if let Some(url) = &np.cover_url {
                if let Some(handle) = self.loaded_images.get(url) {
                    cosmic::widget::image(handle.clone())
                        .width(PANEL_ART_SIZE)
                        .height(PANEL_ART_SIZE)
                        .into()
                } else {
                    widget::icon::from_name("media-optical-symbolic")
                        .size(PANEL_ART_SIZE)
                        .into()
                }
            } else {
                widget::icon::from_name("media-optical-symbolic")
                    .size(PANEL_ART_SIZE)
                    .into()
            };

            // During loading, overlay an opaque mask that shrinks downward,
            // progressively revealing the album art pixels from the bottom up.
            let album_art: Element<'_, Message> = if self.playback_state == PlaybackState::Loading {
                let progress = self.loading_progress;
                let size = PANEL_ART_SIZE;
                let radius = f32::from(size) / 2.0;

                // Mask: opaque bg at top (hides art), transparent at bottom (reveals art).
                // As progress goes 0→1 the cutoff rises, uncovering more image.
                let mask = container(
                    widget::Space::new()
                        .width(Length::Fixed(size.into()))
                        .height(Length::Fixed(size.into())),
                )
                .class(cosmic::theme::Container::custom(move |theme| {
                    let cosmic = theme.cosmic();
                    let solid: Color = cosmic.bg_color().into();
                    let clear = Color { a: 0.0, ..solid };

                    // cutoff 1.0 = everything masked; 0.0 = fully revealed
                    let cutoff = (1.0 - progress).clamp(0.0, 1.0);
                    let bg = Background::Gradient(
                        gradient::Linear::new(Radians(std::f32::consts::PI)) // top→bottom
                            .add_stop(0.0, solid)
                            .add_stop(cutoff, solid)
                            .add_stop((cutoff + 0.01).min(1.0), clear)
                            .add_stop(1.0, clear)
                            .into(),
                    );

                    container::Style {
                        background: Some(bg),
                        border: Border {
                            radius: radius.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                }));

                cosmic::iced::widget::Stack::new()
                    .push(base_art)
                    .push(mask)
                    .into()
            } else {
                base_art
            };

            // Format: "Title · Artist · Album" (or "Title · Artist" if no album)
            let display_text = if let Some(album) = &np.album {
                if !album.is_empty() {
                    format!("{} · {} · {}", np.title, np.artist, album)
                } else {
                    format!("{} · {}", np.title, np.artist)
                }
            } else {
                format!("{} · {}", np.title, np.artist)
            };

            // Wrap the text in a FadingClip so it fades instead of hard-clipping
            let panel_text = widget::text::body(display_text).wrapping(Wrapping::None);
            let faded_text = crate::views::components::fading_panel_text(panel_text);

            // Build button content row
            let content = widget::Row::new().push(album_art).push(faded_text);

            let mut content = content.spacing(6).align_y(Alignment::Center);

            // Add volume bar if visible
            if self.show_volume_bar {
                content = content.push(self.build_volume_bar());
            }

            // Use a custom button class that delegates to AppletIcon for
            // every state except pressed, which reuses the hovered style.
            // This ensures the FadingClip gradient (which uses the hover
            // colour) matches the button background on press too.
            let class = cosmic::theme::Button::Custom {
                active: Box::new(|focused, theme| {
                    Catalog::active(theme, focused, false, &cosmic::theme::Button::AppletIcon)
                }),
                disabled: Box::new(|theme| {
                    Catalog::disabled(theme, &cosmic::theme::Button::AppletIcon)
                }),
                hovered: Box::new(|focused, theme| {
                    Catalog::hovered(theme, focused, false, &cosmic::theme::Button::AppletIcon)
                }),
                pressed: Box::new(|focused, theme| {
                    Catalog::hovered(theme, focused, false, &cosmic::theme::Button::AppletIcon)
                }),
            };

            let btn = button::custom(content)
                .on_press_down(Message::TogglePopup)
                .class(class);

            // Wrap in mouse_area for right-click and scroll
            let interactive = widget::mouse_area(btn)
                .on_right_release(Message::NextTrack)
                .on_scroll(|delta| Message::AdjustVolume(scroll_to_volume_delta(delta)));

            autosize::autosize(interactive, AUTOSIZE_MAIN_ID.clone()).into()
        } else {
            // Nothing playing - just show the icon button
            let icon_btn = self
                .core
                .applet
                .icon_button("io.github.cosmic-applet-mare-symbolic")
                .on_press(Message::TogglePopup);

            // Wrap for scroll support even when not playing
            let interactive = widget::mouse_area(icon_btn)
                .on_scroll(|delta| Message::AdjustVolume(scroll_to_volume_delta(delta)));

            if self.show_volume_bar {
                widget::Row::new()
                    .push(interactive)
                    .push(self.build_volume_bar())
                    .spacing(2)
                    .align_y(Alignment::Center)
                    .into()
            } else {
                interactive.into()
            }
        }
    }

    /// Build the volume bar indicator (green bar showing current volume level)
    fn build_volume_bar(&self) -> Element<'_, Message> {
        let total_height = PANEL_ART_SIZE as f32;
        let volume_percent = self.volume_level;
        let filled_height = (total_height * volume_percent).max(2.0);
        let empty_height = total_height - filled_height;

        // Bright green color
        let green_color = cosmic::iced::Color::from_rgb(0.2, 0.9, 0.2);

        // Filled portion
        let filled = container(
            widget::Space::new()
                .width(Length::Fixed(VOLUME_BAR_WIDTH))
                .height(Length::Fixed(filled_height)),
        )
        .class(cosmic::theme::Container::custom(move |_theme| {
            container::Style {
                background: Some(Background::Color(green_color)),
                border: Border {
                    radius: 2.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }));

        // Empty portion (spacer)
        let empty = widget::Space::new()
            .width(Length::Fixed(VOLUME_BAR_WIDTH))
            .height(Length::Fixed(empty_height));

        // Stack: empty on top, filled on bottom
        widget::Column::new().push(empty).push(filled).into()
    }
}
