// SPDX-License-Identifier: GPL-3.0-only

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
            .push(button::standard(fl!("sign-in")).on_press(Message::StartLogin).width(Length::Fill))
            .spacing(12)
            .align_x(Alignment::Center)
            .padding(20)
            .width(Length::Fill);

        container(content).width(Length::Fill).align_x(Alignment::Center).align_y(Alignment::Center).into()
    }

    /// Render the login view shown while the user completes the TIDAL sign-in
    /// in their browser.
    ///
    /// Two shapes, depending on what TIDAL was asked to redirect to (see
    /// [`login_uri`](crate::tidal::login_uri)):
    ///
    /// * `tidal://` — the browser hands the code back to us, so there is
    ///   nothing to do here but wait.
    /// * https — the browser lands on a page that fails to load, and its
    ///   address, which carries the code, has to be brought over by hand.
    pub fn view_awaiting_oauth(&self) -> Element<'_, Message> {
        let content = if self.is_loading {
            // Exchanging the code for tokens
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
        } else if let Some(request) = &self.login_request {
            let mut col = widget::Column::new()
                .push(text(fl!("sign-in-title")).size(20))
                .push(widget::space::vertical().height(10))
                .push(text(fl!("login-step-browser")).size(12))
                .push(button::standard(fl!("open-browser")).on_press(Message::OpenLoginUrl))
                .push(widget::space::vertical().height(15));

            col = if request.delivers_itself {
                col.push(text(fl!("login-returns-here")).size(12))
            } else {
                // No handler for `tidal://` on this desktop: the code stops at
                // the browser and has to be carried over.
                col.push(text(fl!("login-step-paste")).size(12))
                    .push(
                        widget::text_input(fl!("login-redirect-placeholder"), &self.login_redirect_url)
                            .on_input(Message::LoginRedirectUrlChanged)
                            .on_submit(|_| Message::SubmitLoginRedirectUrl)
                            .width(Length::Fill),
                    )
                    .push(button::suggested(fl!("login-finish")).on_press(Message::SubmitLoginRedirectUrl).width(Length::Fill))
            };

            col.push(widget::space::vertical().height(10))
                .push(button::text(fl!("cancel")).on_press(Message::ShowMain))
                .spacing(8)
                .align_x(Alignment::Center)
        } else {
            widget::Column::new().push(text(fl!("preparing-login")).size(16)).align_x(Alignment::Center)
        };

        container(content.padding(20)).width(Length::Fill).align_x(Alignment::Center).align_y(Alignment::Center).into()
    }

    /// Render a simple loading view.
    pub fn view_loading(&self) -> Element<'_, Message> {
        let content = widget::Column::new().push(text(fl!("loading")).size(16)).spacing(8).align_x(Alignment::Center);

        container(content).width(Length::Fill).align_x(Alignment::Center).align_y(Alignment::Center).padding(20).into()
    }
}
