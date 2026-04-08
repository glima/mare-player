// SPDX-License-Identifier: MIT

//! Authentication views for Maré Player.
//!
//! This module contains the login and OAuth waiting views.

use cosmic::Element;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, container, text};

use crate::fl;
use crate::messages::Message;
use crate::state::AppModel;
use crate::views::components::branded_title;

impl AppModel {
    /// Render the login view prompting user to sign in.
    pub fn view_login(&self) -> Element<'_, Message> {
        let content = widget::Column::new()
            .push(branded_title(24))
            .push(text(fl!("sign-in-prompt")).size(14))
            .push(widget::space::vertical().height(20))
            .push(
                button::standard(fl!("sign-in"))
                    .on_press(Message::StartLogin)
                    .width(Length::Fill),
            )
            .spacing(12)
            .align_x(Alignment::Center)
            .padding(20)
            .width(Length::Fill);

        container(content)
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .into()
    }

    /// Render the OAuth waiting view shown during login flow.
    pub fn view_awaiting_oauth(&self) -> Element<'_, Message> {
        let content = if self.is_loading {
            // Show loading state while polling for OAuth completion
            widget::Column::new()
                .push(text(fl!("sign-in-title")).size(20))
                .push(widget::space::vertical().height(20))
                .push(text("⏳").size(32))
                .push(widget::space::vertical().height(10))
                .push(text(fl!("verifying-auth")).size(14))
                .push(text(fl!("verifying-auth-wait")).size(12))
                .push(widget::space::vertical().height(20))
                .push(button::text(fl!("cancel")).on_press(Message::ShowMain))
                .spacing(8)
                .align_x(Alignment::Center)
        } else if let Some(info) = &self.device_code_info {
            let url_container = container(text(&info.verification_uri_complete).size(11))
                .padding(8)
                .class(cosmic::theme::Container::Card);

            widget::Column::new()
                .push(text(fl!("sign-in-title")).size(20))
                .push(widget::space::vertical().height(10))
                .push(text(fl!("oauth-open-url")).size(12))
                .push(url_container)
                .push(widget::space::vertical().height(10))
                .push(text(fl!("oauth-enter-code", code = info.user_code.clone())).size(14))
                .push(widget::space::vertical().height(20))
                .push(button::standard(fl!("open-browser")).on_press(Message::OpenOAuthUrl))
                .push(widget::space::vertical().height(15))
                .push(text("⏳").size(24))
                .push(text(fl!("waiting-for-login")).size(12))
                .push(text(fl!("complete-login-in-browser")).size(11))
                .push(widget::space::vertical().height(10))
                .push(button::text(fl!("cancel")).on_press(Message::ShowMain))
                .spacing(8)
                .align_x(Alignment::Center)
        } else {
            widget::Column::new()
                .push(text(fl!("preparing-login")).size(16))
                .align_x(Alignment::Center)
        };

        container(content.padding(20))
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .into()
    }

    /// Render a simple loading view.
    pub fn view_loading(&self) -> Element<'_, Message> {
        let content = widget::Column::new()
            .push(text(fl!("loading")).size(16))
            .spacing(8)
            .align_x(Alignment::Center);

        container(content)
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .padding(20)
            .into()
    }
}
