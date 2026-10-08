// SPDX-License-Identifier: GPL-3.0-only

//! Explore view for Maré Player.
//!
//! Renders TIDAL's browse pages (`/v1/pages/{path}`): a Featured carousel,
//! plus clouds of links (Genres, Moods & Activities, Decades, More) and
//! content lists (albums/playlists/artists).  Link clouds drill down into
//! sub-pages recursively; an in-view back button pops the stack.
//!
//! Rows are rendered through the virtual `List` widget — only the rows
//! visible in the viewport are materialised — so long browse pages (the
//! root Explore page alone has ~70 entries) scroll smoothly.

use cosmic::Element;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, text};

use crate::fl;
use crate::messages::Message;
use crate::state::{AppModel, HandleCache};
use crate::tidal::models::{ExploreRow, ExploreTarget};
use crate::views::components::rows::build_thumbnail;
use crate::views::components::{back_button, fading_text_column, list_item, scrollable_element, virtual_list_row};

impl AppModel {
    /// Render the Explore view for the currently-loaded browse page.
    pub fn view_explore(&self) -> Element<'_, Message> {
        // Back goes up the explore stack if we drilled into a sub-page,
        // otherwise out to the main collection menu.
        let back_msg = if self.explore_stack.len() > 1 { Message::ExploreBack } else { Message::ShowMain };

        let title = self.explore_page.as_ref().map(|p| p.title.clone()).unwrap_or_else(|| fl!("explore"));

        let header = widget::Row::new()
            .push(back_button(back_msg))
            .push(text(title).size(18))
            .push(widget::space::horizontal())
            .spacing(8)
            .align_y(Alignment::Center);

        let content: Element<'_, Message> = if self.explore_rows.is_empty() {
            if self.explore_loading {
                text(fl!("loading")).size(14).into()
            } else {
                widget::Column::new()
                    .push(text(fl!("no-explore")).size(14))
                    .push(button::text(fl!("refresh")).on_press(Message::LoadExplorePage(
                        self.explore_stack.last().cloned().unwrap_or_else(|| "explore".to_string()),
                    )))
                    .spacing(8)
                    .into()
            }
        } else {
            let loaded_images = &self.loaded_images;
            let list = cosmic::iced::widget::list::List::new(&self.explore_rows, move |_index, row| {
                build_explore_row(loaded_images, row)
            });
            scrollable_element(list)
        };

        widget::Column::new()
            .push(header)
            .push(explore_page_content(self.explore_rows_revision, content))
            .spacing(12)
            .padding(12)
            .width(Length::Fill)
            .into()
    }
}

/// Keep list geometry and scroll position within one row-set revision. A new
/// identity discards the whole subtree even if loading and completion happen
/// before the next rendered frame; redraws of the same page preserve it.
fn explore_page_content<'a>(revision: u64, content: Element<'a, Message>) -> Element<'a, Message> {
    let id = cosmic::iced::core::widget::Id::new(format!("mare-explore-page-{revision}"));
    widget::id_container(content, id).into()
}

/// Build a single Explore row for the virtual `List` closure.
fn build_explore_row<'a>(loaded_images: &HandleCache, row: &ExploreRow) -> Element<'a, Message> {
    // The virtual `List` keeps spacing at 0 (its `spacing()` is buggy — see
    // `virtual_list_row`); the old `spacing(4)` gap is baked in below instead.
    let inner: Element<'a, Message> = match row {
        ExploreRow::SectionHeader(title) => widget::container(text(title.clone()).size(15)).padding([8, 0, 2, 0]).into(),

        ExploreRow::Featured(card) => {
            let thumb = build_thumbnail(loaded_images, card.image_url.as_deref(), "view-list-symbolic");
            let mut texts: Vec<Element<'_, Message>> = vec![text(card.title.clone()).size(13).wrapping(Wrapping::None).into()];
            if let Some(sub) = card.subtitle.as_ref().filter(|s| !s.trim().is_empty()) {
                texts.push(text(sub.clone()).size(11).wrapping(Wrapping::None).into());
            }
            let info = fading_text_column(texts);
            let r = widget::Row::new().push(thumb).push(info).spacing(8).align_y(Alignment::Center).width(Length::Fill);
            list_item(r, Message::OpenExploreTarget(card.target.clone()), 6)
        }

        ExploreRow::Link(link) => {
            let r = widget::Row::new()
                .push(text(link.text.clone()).size(13))
                .push(widget::space::horizontal())
                .push(widget::icon::from_name("go-next-symbolic").size(14))
                .align_y(Alignment::Center)
                .width(Length::Fill);
            list_item(r, Message::OpenExploreTarget(ExploreTarget::Page(link.path.clone())), 10)
        }

        ExploreRow::Album(album) => {
            let thumb = build_thumbnail(loaded_images, album.cover_url.as_deref(), "media-optical-symbolic");
            let info = fading_text_column(vec![
                text(album.title.clone()).size(13).wrapping(Wrapping::None).into(),
                text(album.artist_name.clone()).size(11).wrapping(Wrapping::None).into(),
            ]);
            let r = widget::Row::new().push(thumb).push(info).spacing(8).align_y(Alignment::Center).width(Length::Fill);
            list_item(r, Message::ShowAlbumDetail(album.clone()), 6)
        }

        ExploreRow::Playlist(playlist) => {
            let thumb = build_thumbnail(loaded_images, playlist.image_url.as_deref(), "folder-music-symbolic");
            let info = fading_text_column(vec![text(playlist.title.clone()).size(13).wrapping(Wrapping::None).into()]);
            let r = widget::Row::new().push(thumb).push(info).spacing(8).align_y(Alignment::Center).width(Length::Fill);
            list_item(r, Message::ShowPlaylistDetail(playlist.uuid.clone(), playlist.title.clone()), 6)
        }

        ExploreRow::Artist(artist) => {
            let thumb = build_thumbnail(loaded_images, artist.picture_url.as_deref(), "system-users-symbolic");
            let info = fading_text_column(vec![text(artist.name.clone()).size(13).wrapping(Wrapping::None).into()]);
            let r = widget::Row::new().push(thumb).push(info).spacing(8).align_y(Alignment::Center).width(Length::Fill);
            list_item(r, Message::ShowArtistDetail(artist.id.clone()), 6)
        }
    };

    virtual_list_row(inner, 4)
}

#[cfg(test)]
mod tests {
    use super::explore_page_content;
    use crate::messages::Message;
    use crate::state::{AppModel, ViewState};
    use crate::tidal::models::{ExplorePage, ExploreSection, ExploreTarget, PageLink, Playlist};
    use cosmic::Application;
    use cosmic::iced::core::{Layout, Length, Rectangle, Size, Widget, layout, mouse, renderer, widget};
    use cosmic::iced::widget::list::{Content, List};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A row with a known height and observable persistent state. The null
    /// renderer lets the List regression run without a display server or GPU.
    struct Probe {
        height: f32,
        nonce: u64,
        draws: Rc<RefCell<Vec<Rectangle>>>,
    }

    impl<Msg, Theme, Renderer: renderer::Renderer> Widget<Msg, Theme, Renderer> for Probe {
        fn size(&self) -> Size<Length> {
            Size::new(Length::Fixed(200.0), Length::Fixed(self.height))
        }

        fn tag(&self) -> widget::tree::Tag {
            widget::tree::Tag::of::<u64>()
        }

        fn state(&self) -> widget::tree::State {
            widget::tree::State::new(self.nonce)
        }

        fn layout(&mut self, _tree: &mut widget::Tree, _renderer: &Renderer, _limits: &layout::Limits) -> layout::Node {
            layout::Node::new(Size::new(200.0, self.height))
        }

        fn draw(
            &self,
            _tree: &widget::Tree,
            _renderer: &mut Renderer,
            _theme: &Theme,
            _style: &renderer::Style,
            layout: Layout<'_>,
            _cursor: mouse::Cursor,
            _viewport: &Rectangle,
        ) {
            self.draws.borrow_mut().push(layout.bounds());
        }
    }

    fn keyed_probe(revision: u64, nonce: u64) -> cosmic::Element<'static, Message> {
        explore_page_content(revision, cosmic::Element::new(Probe { height: 56.0, nonce, draws: Rc::default() }))
    }

    fn probe_nonce(tree: &widget::Tree) -> u64 {
        *tree.children[0].state.downcast_ref::<u64>()
    }

    #[test]
    fn page_identity_replaces_the_subtree_without_an_empty_frame() {
        let old = keyed_probe(1, 100);
        let mut tree = widget::Tree::new(old.as_widget());
        assert_eq!(probe_nonce(&tree), 100);

        let mut redraw = keyed_probe(1, 200);
        tree.diff(redraw.as_widget_mut());
        assert_eq!(probe_nonce(&tree), 100, "artwork redraw must retain the existing subtree");

        let mut new_page = keyed_probe(2, 300);
        tree.diff(new_page.as_widget_mut());
        assert_eq!(probe_nonce(&tree), 300, "page replacement must discard the previous subtree");
    }

    #[test]
    fn page_identity_works_with_iceds_named_state_reconciliation() {
        let old = keyed_probe(1, 100);
        let mut tree = widget::Tree::new(old.as_widget());
        widget::tree::NAMED.with(|states| *states.borrow_mut() = tree.take_all_named());
        let mut redraw = keyed_probe(1, 200);
        tree.diff(redraw.as_widget_mut());
        assert_eq!(probe_nonce(&tree), 100);
        widget::tree::NAMED.with(|states| *states.borrow_mut() = tree.take_all_named());
        let mut new_page = keyed_probe(2, 300);
        tree.diff(new_page.as_widget_mut());
        assert_eq!(probe_nonce(&tree), 300);
        widget::tree::NAMED.with(|states| states.borrow_mut().clear());
    }

    type HeadlessElement<'a> = cosmic::iced::core::Element<'a, (), (), ()>;

    fn probe_list<'a>(content: &'a Content<f32>, draws: Rc<RefCell<Vec<Rectangle>>>) -> HeadlessElement<'a> {
        List::new(content, move |_index, height| HeadlessElement::new(Probe { height: *height, nonce: 0, draws: draws.clone() }))
            .into()
    }

    fn frame(element: &mut HeadlessElement<'_>, tree: &mut widget::Tree, draws: &RefCell<Vec<Rectangle>>) {
        use cosmic::iced::core::{Event, Shell, clipboard, window};
        let limits = layout::Limits::new(Size::ZERO, Size::new(300.0, f32::INFINITY));
        let node = element.as_widget_mut().layout(tree, &(), &limits);
        let viewport = Rectangle::new(cosmic::iced::Point::ORIGIN, Size::new(300.0, 300.0));
        let mut messages = Vec::new();
        let mut clipboard = clipboard::Null;
        element.as_widget_mut().update(
            tree,
            &Event::Window(window::Event::RedrawRequested(std::time::Instant::now())),
            Layout::new(&node),
            mouse::Cursor::Unavailable,
            &(),
            &mut clipboard,
            &mut Shell::new(&mut messages),
            &viewport,
        );
        draws.borrow_mut().clear();
        element.as_widget().draw(
            tree,
            &mut (),
            &(),
            &renderer::Style::default(),
            Layout::new(&node),
            mouse::Cursor::Unavailable,
            &viewport,
        );
    }

    #[test]
    fn a_fresh_list_tree_matches_fresh_row_heights_after_page_replacement() {
        let old_content: Content<f32> = std::iter::repeat_n(41.0, 88).collect();
        let new_content: Content<f32> = std::iter::repeat_n(56.0, 166).collect();
        let draws = Rc::new(RefCell::new(Vec::new()));
        let mut old = probe_list(&old_content, draws.clone());
        let mut reused_tree = widget::Tree::new(old.as_widget());
        for _ in 0..4 {
            frame(&mut old, &mut reused_tree, &draws);
        }
        let mut replacement = probe_list(&new_content, draws.clone());
        reused_tree.diff(replacement.as_widget_mut());
        for _ in 0..6 {
            frame(&mut replacement, &mut reused_tree, &draws);
        }
        let stale = draws.borrow().clone();
        assert!(stale.len() > 1);
        assert!(
            stale.windows(2).any(|rows| rows[0].y + rows[0].height > rows[1].y),
            "control: reusing List state must reproduce the pinned dependency's overlapping rows"
        );

        let mut fresh = probe_list(&new_content, draws.clone());
        let mut fresh_tree = widget::Tree::new(fresh.as_widget());
        for _ in 0..6 {
            frame(&mut fresh, &mut fresh_tree, &draws);
        }
        let correct = draws.borrow();
        assert!(correct.len() > 1);
        assert!(correct.windows(2).all(|rows| (rows[1].y - rows[0].y - 56.0).abs() < 0.01));
    }

    fn app() -> AppModel {
        let (app, startup) = AppModel::init(cosmic::Core::default(), ());
        drop(startup);
        app
    }

    /// Paging (search "load more") must edit `Content` in place rather than
    /// replace it, because it keeps the list's widget identity: the control
    /// shows a replaced `Content` in a reused tree drawing with a stale row
    /// height, while incremental remove/push lays out correctly.
    #[test]
    fn incremental_appends_keep_a_reused_list_tree_contiguous() {
        let rows_after_append = |incremental: bool| {
            // Two 56px rows, then a 40px "load more" row.
            let mut content: Content<f32> = std::iter::repeat_n(56.0, 2).chain([40.0]).collect();
            let draws = Rc::new(RefCell::new(Vec::new()));
            let mut old = probe_list(&content, draws.clone());
            let mut tree = widget::Tree::new(old.as_widget());
            for _ in 0..4 {
                frame(&mut old, &mut tree, &draws);
            }
            drop(old);
            if incremental {
                content.remove(2);
                (0..50).for_each(|_| content.push(56.0));
                content.push(40.0);
            } else {
                content = std::iter::repeat_n(56.0, 52).chain([40.0]).collect();
            }
            let mut appended = probe_list(&content, draws.clone());
            tree.diff(appended.as_widget_mut());
            for _ in 0..6 {
                frame(&mut appended, &mut tree, &draws);
            }
            draws.take()
        };
        let contiguous = |rows: &[Rectangle]| rows.windows(2).all(|r| (r[1].y - r[0].y - 56.0).abs() < 0.01);

        let replaced = rows_after_append(false);
        assert!(replaced.len() > 3);
        assert!(!contiguous(&replaced), "control: replacing Content in a reused List tree must reproduce the bug");

        let incremental = rows_after_append(true);
        assert!(incremental.len() > 3);
        assert!(contiguous(&incremental));
    }

    fn root_page() -> ExplorePage {
        ExplorePage {
            title: "Explore".into(),
            sections: vec![ExploreSection::Links {
                title: "Genres".into(),
                links: (0..88).map(|i| PageLink { text: format!("Genre {i}"), path: format!("pages/genre_{i}") }).collect(),
            }],
        }
    }

    fn genre_page() -> ExplorePage {
        ExplorePage {
            title: "R&B / Soul".into(),
            sections: vec![ExploreSection::Playlists {
                title: "Essentials".into(),
                playlists: (0..166)
                    .map(|i| Playlist { uuid: i.to_string(), title: format!("Playlist {i}"), ..Playlist::default() })
                    .collect(),
            }],
        }
    }

    fn loaded_root(app: &mut AppModel) {
        drop(app.handle_show_explore());
        drop(app.handle_explore_loaded(Ok(root_page())));
        assert!(!app.explore_rows.is_empty());
    }

    #[tokio::test]
    async fn forward_and_back_navigation_clear_rows_and_change_identity() {
        let mut app = app();
        loaded_root(&mut app);
        let root_revision = app.explore_rows_revision;
        drop(app.handle_open_explore_target(ExploreTarget::Page("pages/genre_rnb".into())));
        assert!(app.explore_rows.is_empty());
        assert!(app.explore_page.is_none());
        assert!(app.explore_loading);
        assert_eq!(app.explore_stack, ["explore", "pages/genre_rnb"]);
        assert_ne!(app.explore_rows_revision, root_revision);

        // No intervening view/render is required to reset the widget identity.
        drop(app.handle_explore_loaded(Ok(genre_page())));
        let genre_revision = app.explore_rows_revision;
        assert_ne!(genre_revision, root_revision);
        assert!(!app.explore_loading);
        assert_eq!(app.explore_rows.len(), 167);

        drop(app.handle_explore_back());
        assert_eq!(app.explore_stack, ["explore"]);
        assert!(app.explore_rows.is_empty());
        assert!(app.explore_loading);
        drop(app.handle_explore_loaded(Ok(root_page())));
        assert_ne!(app.explore_rows_revision, genre_revision);
        assert_eq!(app.explore_rows.len(), 89);
    }

    #[tokio::test]
    async fn returning_from_a_detail_view_rebuilds_with_a_fresh_identity() {
        let mut app = app();
        loaded_root(&mut app);
        drop(app.handle_open_explore_target(ExploreTarget::Page("pages/genre_rnb".into())));
        drop(app.handle_explore_loaded(Ok(genre_page())));
        let revision = app.explore_rows_revision;
        let count = app.explore_rows.len();
        app.nav_stack.push(ViewState::Explore);
        app.view_state = ViewState::PlaylistDetail;
        drop(app.handle_navigate_back());
        assert_eq!(app.view_state, ViewState::Explore);
        assert_eq!(app.explore_rows.len(), count);
        assert_ne!(app.explore_rows_revision, revision);
        assert_eq!(app.explore_stack, ["explore", "pages/genre_rnb"]);
        assert!(!app.explore_loading);
    }

    #[tokio::test]
    async fn reopening_root_or_replacing_a_page_resets_identity() {
        let mut app = app();
        loaded_root(&mut app);
        let revision = app.explore_rows_revision;
        drop(app.handle_explore_loaded(Ok(root_page())));
        assert_ne!(app.explore_rows_revision, revision);
        let revision = app.explore_rows_revision;
        app.handle_show_main();
        drop(app.handle_show_explore());
        assert_eq!(app.explore_stack, ["explore"]);
        assert!(app.explore_rows.is_empty());
        assert!(app.explore_loading);
        assert_ne!(app.explore_rows_revision, revision);
    }

    #[tokio::test]
    async fn empty_pages_and_load_failures_cannot_display_previous_rows() {
        let mut app = app();
        loaded_root(&mut app);
        drop(app.handle_load_explore_page("empty".into()));
        drop(app.handle_explore_loaded(Ok(ExplorePage::default())));
        assert!(app.explore_rows.is_empty());
        assert!(!app.explore_loading);

        loaded_root(&mut app);
        drop(app.handle_load_explore_page("unavailable".into()));
        drop(app.handle_explore_loaded(Err("offline".into())));
        assert!(app.explore_rows.is_empty());
        assert!(app.explore_page.is_none());
        assert!(!app.explore_loading);
        assert!(app.error_message.as_deref().is_some_and(|s| s.contains("offline")));
    }

    #[tokio::test]
    async fn retrying_a_failed_subpage_preserves_the_back_stack() {
        let mut app = app();
        loaded_root(&mut app);
        drop(app.handle_load_explore_page("pages/genre_rnb".into()));
        drop(app.handle_explore_loaded(Err("offline".into())));
        let stack = app.explore_stack.clone();
        let revision = app.explore_rows_revision;

        drop(app.handle_load_explore_page("pages/genre_rnb".into()));
        assert_eq!(app.explore_stack, stack);
        assert!(app.explore_loading);
        assert!(app.explore_rows.is_empty());
        assert!(app.error_message.is_none());
        assert_ne!(app.explore_rows_revision, revision);
        drop(app.handle_explore_loaded(Ok(genre_page())));
        assert_eq!(app.explore_page.as_ref().unwrap().title, "R&B / Soul");
        drop(app.handle_explore_back());
        assert_eq!(app.explore_stack, ["explore"]);
    }

    #[tokio::test]
    async fn artwork_updates_preserve_page_identity() {
        let mut app = app();
        loaded_root(&mut app);
        let revision = app.explore_rows_revision;
        app.handle_image_loaded("artwork".into(), 1, 1, vec![0, 0, 0, 255]);
        assert_eq!(app.explore_rows_revision, revision);
        assert_eq!(app.explore_rows.len(), 89);
    }
}
