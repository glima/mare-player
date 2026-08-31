// SPDX-License-Identifier: GPL-3.0-only

//! A clipping widget that fades overflowing content to transparency.
//!
//! [`FadingClip`] GPU-clips its child and, when the child overflows the
//! available width, fades the text out along the right edge instead of ending
//! it with a hard clip.
//!
//! The fade is **alpha-native**: rather than painting a background colour over
//! the text (which can never look right on translucent "frosted glass" panels,
//! where no opaque colour matches what shows through), it re-draws the text
//! across the fade band with its alpha ramped down to zero. The glyphs
//! themselves dissolve into whatever is actually behind the widget — an opaque
//! surface, or the desktop showing through a translucent panel.
//!
//! ## Why not a gradient overlay?
//!
//! iced's renderer only offers premultiplied **src-over** blending: for any
//! quad we draw, `dst.a = src.a + dst.a·(1 − src.a)`, which can only *keep or
//! increase* destination alpha — never erase it. So a coloured gradient can
//! tint the text but can't make it transparent. Ramping the text's own alpha
//! across disjoint scissor strips is the only transparency-correct option
//! within those primitives.

use cosmic::iced::Rectangle;

/// Number of vertical strips the fade band is split into. Each strip re-draws
/// the child at a lower text alpha, producing a stepped ramp — more strips is
/// smoother. It's also the main cost dial: the fade does `1 + FADE_STRIPS`
/// child draws per *overflowing* label, every frame. 4 was needed while the
/// applet was stuck on software GL (llvmpipe); now that it renders on
/// tiny_skia (a light CPU rasterizer — see the `wgpu` feature gating), 12 is
/// cheap enough and gives a smooth ramp over the ~32 px band (~2.7 px/strip).
const FADE_STRIPS: usize = 12;

// =============================================================================
// Public API
// =============================================================================

/// A widget that clips its child and fades overflowing text on the right edge.
///
/// # Type parameters
///
/// * `'a` — lifetime of the child element.
/// * `Msg` — application message type.
///
/// # Layout
///
/// `FadingClip` behaves like a transparent wrapper: it measures its child
/// normally, caps the child width to the available space, and reserves no
/// extra space of its own.  The fade is drawn inside the child bounds.
pub(crate) struct FadingClip<'a, Msg> {
    /// The wrapped child element.
    child: cosmic::Element<'a, Msg>,

    /// Explicit width override (default: `Length::Shrink`).
    ///
    /// Callers typically set this to `Length::Fill` so the fade column
    /// expands to fill the remaining space after fixed-width siblings
    /// (thumbnails, duration labels, etc.).
    ///
    /// # Why `Length::Shrink` is the default
    ///
    /// A `Shrink` default means that short strings that fit entirely
    /// inside their parent receive exactly the width they need — no
    /// fade is drawn, no padding is wasted.  When callers want the
    /// column to absorb leftover space they `.width(Length::Fill)` it,
    /// which still triggers the fade only when the child content
    /// overflows.
    ///
    /// In theory `Length::Fill` would also work as a default but it would
    /// unnecessarily stretch every single text element even when there
    /// are no siblings competing for space (e.g. a standalone label in a
    /// `Column`), producing a wider-than-necessary layout and a phantom
    /// fade at the far right that serves no visual purpose.
    width: cosmic::iced::Length,

    /// Explicit height override (default: `Length::Shrink`).
    height: cosmic::iced::Length,

    /// Width (in pixels) of the fade band, measured inward from the right
    /// edge of the widget.  A larger value gives a gentler fade at the cost
    /// of hiding more text.  The caller passes this in at construction time —
    /// the default in [`super::list_helpers`] is 32 px.
    fade_width: f32,
}

/// Tracks layout state for the fade.
#[derive(Debug, Clone, Default)]
struct FadingClipState {
    /// `true` when the child's natural (unconstrained) width exceeds the
    /// available layout width — i.e. the text is being clipped and needs
    /// the fade.
    content_overflows: bool,
}

impl<'a, Msg> FadingClip<'a, Msg> {
    pub(crate) fn new(child: impl Into<cosmic::Element<'a, Msg>>, fade_width: f32) -> Self {
        Self { child: child.into(), width: cosmic::iced::Length::Shrink, height: cosmic::iced::Length::Shrink, fade_width }
    }

    pub(crate) fn width(mut self, width: cosmic::iced::Length) -> Self {
        self.width = width;
        self
    }
}

// iced's Widget trait methods require `tree.children[0]` and
// `layout.children().next().unwrap()` — there is no fallible API for
// accessing the single mandatory child node.
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
impl<Msg: 'static> cosmic::iced::core::Widget<Msg, cosmic::Theme, cosmic::Renderer> for FadingClip<'_, Msg> {
    fn size(&self) -> cosmic::iced::Size<cosmic::iced::Length> {
        cosmic::iced::Size::new(self.width, self.height)
    }

    fn tag(&self) -> cosmic::iced::core::widget::tree::Tag {
        cosmic::iced::core::widget::tree::Tag::of::<FadingClipState>()
    }

    fn state(&self) -> cosmic::iced::core::widget::tree::State {
        cosmic::iced::core::widget::tree::State::new(FadingClipState::default())
    }

    fn children(&self) -> Vec<cosmic::iced::core::widget::Tree> {
        vec![cosmic::iced::core::widget::Tree::new(&self.child)]
    }

    fn diff(&mut self, tree: &mut cosmic::iced::core::widget::Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn layout(
        &mut self,
        tree: &mut cosmic::iced::core::widget::Tree,
        renderer: &cosmic::Renderer,
        limits: &cosmic::iced::core::layout::Limits,
    ) -> cosmic::iced::core::layout::Node {
        // Measure the child's natural (unconstrained) width on a
        // *throwaway* tree. Probing on a separate tree keeps the real
        // child laid out exactly once.
        let unbounded = cosmic::iced::core::layout::Limits::NONE.max_height(limits.max().height);
        let mut probe = cosmic::iced::core::widget::Tree::new(self.child.as_widget());
        let natural_width = self.child.as_widget_mut().layout(&mut probe, renderer, &unbounded).bounds().width;

        // Real layout with the actual (constrained) limits.
        let node = cosmic::iced::core::layout::contained(limits, self.width, self.height, |limits| {
            self.child.as_widget_mut().layout(&mut tree.children[0], renderer, limits)
        });

        // Record whether the child overflows so draw() can skip the
        // fade when it would be invisible.
        tree.state.downcast_mut::<FadingClipState>().content_overflows = natural_width > node.bounds().width + 1.0;

        node
    }

    fn draw(
        &self,
        tree: &cosmic::iced::core::widget::Tree,
        renderer: &mut cosmic::Renderer,
        theme: &cosmic::Theme,
        style: &cosmic::iced::core::renderer::Style,
        layout: cosmic::iced::core::Layout<'_>,
        cursor: cosmic::iced::core::mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use cosmic::iced::Color;
        use cosmic::iced::core::Renderer as _;

        let bounds = layout.bounds();
        let Some(clipped) = bounds.intersection(viewport) else {
            return;
        };

        let child = self.child.as_widget();
        let child_tree = &tree.children[0];
        let child_layout = layout.children().next().unwrap().with_virtual_offset(layout.virtual_offset());

        let state = tree.state.downcast_ref::<FadingClipState>();

        // Content fits — no fade needed; draw the child clipped to bounds.
        if !state.content_overflows {
            renderer.with_layer(clipped, |renderer| {
                child.draw(child_tree, renderer, theme, style, child_layout, cursor, &clipped);
            });
            return;
        }

        // Content overflows: fade the *text itself* to transparent across the
        // right-edge band. See the module docs for why a coloured gradient
        // can't work here. We re-draw the child in `FADE_STRIPS` disjoint
        // vertical scissor strips, each with the inherited text colour's alpha
        // ramped 1 -> 0. Disjoint clips mean the alphas don't compound, so the
        // glyphs dissolve cleanly into whatever is behind the panel.
        let band_x = bounds.x + bounds.width - self.fade_width;

        // 1. Solid part: the child clipped to everything left of the band.
        let solid = Rectangle { width: (band_x - bounds.x).max(0.0), ..bounds };
        if let Some(solid_clip) = solid.intersection(viewport) {
            renderer.with_layer(solid_clip, |renderer| {
                child.draw(child_tree, renderer, theme, style, child_layout, cursor, &solid_clip);
            });
        }

        // 2. Fade band: `FADE_STRIPS` strips of decreasing text alpha.
        let strip_w = self.fade_width / FADE_STRIPS as f32;
        for i in 0..FADE_STRIPS {
            let strip = Rectangle { x: band_x + i as f32 * strip_w, width: strip_w, ..bounds };
            let Some(strip_clip) = strip.intersection(viewport) else {
                continue;
            };
            // Midpoint sampling: ~1.0 alpha at the inner edge of the band,
            // ~0.0 at the outer edge.
            let factor = 1.0 - (i as f32 + 0.5) / FADE_STRIPS as f32;
            let mut faded = *style;
            faded.text_color = Color { a: style.text_color.a * factor, ..style.text_color };
            renderer.with_layer(strip_clip, |renderer| {
                child.draw(child_tree, renderer, theme, &faded, child_layout, cursor, &strip_clip);
            });
        }
    }

    fn update(
        &mut self,
        tree: &mut cosmic::iced::core::widget::Tree,
        event: &cosmic::iced::core::Event,
        layout: cosmic::iced::core::Layout<'_>,
        cursor: cosmic::iced::core::mouse::Cursor,
        renderer: &cosmic::Renderer,
        clipboard: &mut dyn cosmic::iced::core::Clipboard,
        shell: &mut cosmic::iced::core::Shell<'_, Msg>,
        viewport: &Rectangle,
    ) {
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout.children().next().unwrap(),
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &cosmic::iced::core::widget::Tree,
        layout: cosmic::iced::core::Layout<'_>,
        cursor: cosmic::iced::core::mouse::Cursor,
        viewport: &Rectangle,
        renderer: &cosmic::Renderer,
    ) -> cosmic::iced::core::mouse::Interaction {
        self.child.as_widget().mouse_interaction(&tree.children[0], layout.children().next().unwrap(), cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut cosmic::iced::core::widget::Tree,
        layout: cosmic::iced::core::Layout<'b>,
        renderer: &cosmic::Renderer,
        viewport: &Rectangle,
        translation: cosmic::iced::Vector,
    ) -> Option<cosmic::iced::core::overlay::Element<'b, Msg, cosmic::Theme, cosmic::Renderer>> {
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            layout.children().next().unwrap(),
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Msg: 'static> From<FadingClip<'a, Msg>> for cosmic::Element<'a, Msg> {
    fn from(clip: FadingClip<'a, Msg>) -> Self {
        cosmic::Element::new(clip)
    }
}
