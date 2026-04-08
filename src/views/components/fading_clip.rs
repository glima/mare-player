// SPDX-License-Identifier: MIT

//! Custom [`FadingClip`] widget that GPU-clips its child and draws a
//! hover-aware gradient fade overlay on the right edge.
//!
//! This is a self-contained iced widget used by the various `fading_*`
//! helpers in [`super::list_helpers`].

use std::f32::consts::FRAC_PI_2;

use cosmic::iced::{Length, Radians, Rectangle, Size, Vector};

/// Extra pixels added to the right of the gradient fade strip.
///
/// Sub-pixel text rendering (e.g. ClearType / FreeType LCD filtering) can
/// produce colour fringes that extend a pixel or two past the nominal glyph
/// bounds.  Because the GPU scissor rectangle in step 1 is axis-aligned to
/// the widget bounds, these fringes sometimes survive the clip and appear as
/// tiny coloured dots at the right edge.  Extending the fully-opaque tail of
/// the gradient by this many pixels ensures they are painted over.
const FADE_BLEED: f32 = 2.0;

// =============================================================================
// FadingClip Widget
// =============================================================================

/// A widget that GPU-clips its child and draws a hover-aware gradient fade
/// overlay on the right edge.
///
/// 1. Clips content with `renderer.with_layer()` (real GPU scissor — the
///    current cosmic/iced fork's `Container.clip` only narrows a viewport
///    *hint* that text rendering ignores).
/// 2. Draws a gradient strip whose opaque colour matches the **current**
///    button background (normal vs hovered), computed on every frame from
///    cursor position and the cosmic theme.
pub(crate) struct FadingClip<'a, Msg> {
    /// The inner element to render with clipping and a fade overlay.
    /// Typically a row of text labels that may overflow horizontally.
    child: cosmic::Element<'a, Msg>,
    /// Horizontal sizing hint forwarded to the iced layout engine via
    /// [`Widget::size`] and [`iced_core::layout::contained`].
    ///
    /// Defaults to [`Length::Shrink`], which means:
    ///
    /// - The widget asks for *only as much width as its child naturally
    ///   needs* (i.e. the full unconstrained text width), clamped to the
    ///   parent's maximum.  If the child fits, the widget is exactly that
    ///   wide and no clipping or fade occurs.  If the child's natural
    ///   width exceeds the parent's maximum, `contained` caps the node at
    ///   that maximum, the child overflows, and the gradient fade kicks
    ///   in.
    ///
    /// - Because `Shrink` never *expands* to fill remaining space, the
    ///   widget leaves room for siblings in a `Row`.  This is the correct
    ///   behaviour for the panel button ([`fading_panel_text`]), where the
    ///   text should hug its content and sit beside an icon without
    ///   pushing it away or consuming the full button width.
    ///
    /// Most popup-side callers override this to [`Length::Fill`] via the
    /// [`width`](Self::width) builder method, which makes the widget
    /// greedily consume all remaining horizontal space in a `Row`.  That
    /// is the correct behaviour for list-item rows where the text column
    /// should stretch to fill whatever space the thumbnail and trailing
    /// icons leave behind — the constrained `Fill` width becomes the
    /// clip boundary, and any text beyond it fades out.
    width: Length,
    /// Vertical sizing hint forwarded to the iced layout engine.
    /// Defaults to [`Length::Shrink`] so the widget is only as tall as
    /// its child content.
    height: Length,
    /// Width in logical pixels of the gradient fade strip drawn along the
    /// right edge of the clipped area.  A larger value gives a more
    /// gradual fade-out; a smaller value makes the cut-off more abrupt.
    ///
    /// The actual rendered strip is [`FADE_BLEED`] pixels wider than this
    /// value — the extra fully-opaque margin covers sub-pixel text
    /// rendering artifacts that can otherwise escape the GPU scissor
    /// boundary at the right edge of the clip region.
    fade_width: f32,
    /// Controls which background colour the gradient fades into.
    /// Different parent surfaces (button, card, popup, panel) have
    /// different background colours, so the fade must match whichever
    /// surface this widget sits on.  See [`FadeTarget`] for details.
    fade_target: FadeTarget,
}

/// Which background the [`FadingClip`] gradient should match.
#[derive(Debug, Clone, Copy, Default)]
enum FadeTarget {
    /// Fade matches the enclosing `list_item` button background, reacting
    /// to hover and pressed states.  This is the default for track/album/
    /// playlist rows.
    #[default]
    Button,
    /// Fade to the bare popup surface colour (`background.base`).
    /// Use for elements sitting directly on the popup background, such as
    /// header titles.
    Surface,
    /// Fade to the card/component colour (`background.component.base`
    /// composited over `background.base`).  Use for elements inside a
    /// `Container::Card`, such as the now-playing bar.
    Card,
    /// Fade for the system panel applet button (`Button::AppletIcon`).
    /// Normal state uses raw `background.base` (the button is transparent),
    /// hover/pressed composites the component highlight over the surface,
    /// matching how `AppletIcon` draws its background.
    Panel,
    /// Fade matches a `Button::Suggested` (accent) button background.
    /// Uses `cosmic.accent_button.{base,hover,pressed}`.
    Suggested,
    /// Fade matches a `Button::Standard` button background.
    /// Uses `cosmic.button.{base,hover,pressed}`.
    Standard,
}

/// Tracks whether the mouse button is currently held down so we can
/// distinguish hovered vs pressed button state.
#[derive(Debug, Clone, Default)]
struct FadingClipState {
    mouse_pressed: bool,
    /// `true` when the child's natural (unconstrained) width exceeds the
    /// available layout width — i.e. the text is being clipped and needs
    /// the gradient fade overlay.
    content_overflows: bool,
}

impl<'a, Msg> FadingClip<'a, Msg> {
    pub(crate) fn new(child: impl Into<cosmic::Element<'a, Msg>>, fade_width: f32) -> Self {
        Self {
            child: child.into(),
            width: Length::Shrink,
            height: Length::Shrink,
            fade_width,
            fade_target: FadeTarget::Button,
        }
    }

    /// Use the popup surface colour for the gradient instead of the
    /// button background.  Skips hover/pressed detection entirely.
    pub(crate) fn surface_only(mut self) -> Self {
        self.fade_target = FadeTarget::Surface;
        self
    }

    /// Use the card/component background colour for the gradient.
    /// For elements inside a `Container::Card` (e.g. now-playing bar).
    pub(crate) fn card(mut self) -> Self {
        self.fade_target = FadeTarget::Card;
        self
    }

    /// Use the panel applet button background for the gradient.
    /// Transparent when idle, composited highlight on hover/press.
    pub(crate) fn panel(mut self) -> Self {
        self.fade_target = FadeTarget::Panel;
        self
    }

    /// Use the suggested (accent) button background for the gradient.
    /// For text inside a `Button::Suggested`.
    pub(crate) fn suggested(mut self) -> Self {
        self.fade_target = FadeTarget::Suggested;
        self
    }

    /// Use the standard button background for the gradient.
    /// For text inside a `Button::Standard`.
    pub(crate) fn standard(mut self) -> Self {
        self.fade_target = FadeTarget::Standard;
        self
    }

    pub(crate) fn width(mut self, w: Length) -> Self {
        self.width = w;
        self
    }
}

/// Alpha-composite `fg` over `bg` (both assumed straight-alpha) and return
/// a fully opaque colour.
fn composite(fg: cosmic::iced::Color, bg: cosmic::iced::Color) -> cosmic::iced::Color {
    let a = fg.a;
    cosmic::iced::Color::from_rgba(
        fg.r * a + bg.r * (1.0 - a),
        fg.g * a + bg.g * (1.0 - a),
        fg.b * a + bg.b * (1.0 - a),
        1.0,
    )
}

// iced's Widget trait methods require `tree.children[0]` and
// `layout.children().next().unwrap()` — there is no fallible API for
// accessing the single mandatory child node.
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
impl<Msg: 'static> cosmic::iced::core::Widget<Msg, cosmic::Theme, cosmic::Renderer>
    for FadingClip<'_, Msg>
{
    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
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
        // First pass: measure the child with unbounded width to learn its
        // natural (unconstrained) width.  For single-line `Wrapping::None`
        // text this is cheap — just one paragraph layout.
        let unbounded = cosmic::iced::core::layout::Limits::NONE.max_height(limits.max().height);
        let natural =
            self.child
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, &unbounded);
        let natural_width = natural.bounds().width;

        // Second pass: real layout with the actual (constrained) limits.
        // This overwrites any child-tree state set by the first pass.
        let node =
            cosmic::iced::core::layout::contained(limits, self.width, self.height, |limits| {
                self.child
                    .as_widget_mut()
                    .layout(&mut tree.children[0], renderer, limits)
            });

        // If the natural width exceeds the constrained width the content
        // is being clipped and we need the gradient fade overlay.
        let state = tree.state.downcast_mut::<FadingClipState>();
        state.content_overflows = natural_width > node.bounds().width + 1.0;

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

        // --- 1. GPU-clip the child content ---
        renderer.with_layer(clipped, |renderer| {
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout
                    .children()
                    .next()
                    .unwrap()
                    .with_virtual_offset(layout.virtual_offset()),
                cursor,
                &clipped,
            );
        });

        // --- 2. Draw the gradient fade strip on the right edge ---
        //
        // Only when the child content actually overflows the available
        // width.  Short strings that fit entirely skip the gradient so
        // they render cleanly without a phantom shading mask.
        let state = tree.state.downcast_ref::<FadingClipState>();
        if !state.content_overflows {
            return;
        }

        // The parent `list_item` button passes
        // `popup_viewport ∩ button_bounds` as the `viewport` to its
        // children, so `viewport` here effectively *is* the button's
        // bounds.  We test the cursor against `viewport` to perfectly
        // match the button's own `is_mouse_over` logic, covering the
        // full row area including padding and non-text children.
        let cosmic_theme = theme.cosmic();
        let surface: Color = cosmic_theme.background.base.into();

        let opaque = match self.fade_target {
            FadeTarget::Surface => surface,
            FadeTarget::Card => {
                let card_bg: Color = cosmic_theme.background.component.base.into();
                composite(card_bg, surface)
            }
            FadeTarget::Button => {
                let is_mouse_over = cursor.position().is_some_and(|pos| viewport.contains(pos));
                let state = tree.state.downcast_ref::<FadingClipState>();

                let btn_bg: Color = if is_mouse_over && state.mouse_pressed {
                    cosmic_theme.background.component.pressed.into()
                } else if is_mouse_over {
                    cosmic_theme.background.component.hover.into()
                } else {
                    cosmic_theme.background.component.base.into()
                };
                composite(btn_bg, surface)
            }
            FadeTarget::Panel => {
                let is_mouse_over = cursor.position().is_some_and(|pos| viewport.contains(pos));
                let state = tree.state.downcast_ref::<FadingClipState>();

                // AppletIcon uses `cosmic.text_button` — not `background.component`.
                let text_btn = &cosmic_theme.text_button;
                if is_mouse_over && state.mouse_pressed {
                    composite(text_btn.pressed.into(), surface)
                } else if is_mouse_over {
                    composite(text_btn.hover.into(), surface)
                } else {
                    // AppletIcon base is transparent → raw surface
                    composite(text_btn.base.into(), surface)
                }
            }
            FadeTarget::Suggested => {
                let is_mouse_over = cursor.position().is_some_and(|pos| viewport.contains(pos));
                let state = tree.state.downcast_ref::<FadingClipState>();

                let comp = &cosmic_theme.accent_button;
                let bg: Color = if is_mouse_over && state.mouse_pressed {
                    comp.pressed.into()
                } else if is_mouse_over {
                    comp.hover.into()
                } else {
                    comp.base.into()
                };
                composite(bg, surface)
            }
            FadeTarget::Standard => {
                let is_mouse_over = cursor.position().is_some_and(|pos| viewport.contains(pos));
                let state = tree.state.downcast_ref::<FadingClipState>();

                let comp = &cosmic_theme.button;
                let bg: Color = if is_mouse_over && state.mouse_pressed {
                    comp.pressed.into()
                } else if is_mouse_over {
                    comp.hover.into()
                } else {
                    comp.base.into()
                };
                composite(bg, surface)
            }
        };
        let transparent = Color::from_rgba(opaque.r, opaque.g, opaque.b, 0.0);

        // Extend the gradient strip by FADE_BLEED pixels to the right so
        // the fully-opaque tail covers any sub-pixel text artifacts that
        // leak past the GPU scissor edge.
        let fade_bounds = Rectangle {
            x: bounds.x + bounds.width - self.fade_width,
            y: bounds.y,
            width: self.fade_width + FADE_BLEED,
            height: bounds.height,
        };

        if let Some(fade_clipped) = fade_bounds.intersection(viewport) {
            renderer.with_layer(fade_clipped, |renderer| {
                renderer.fill_quad(
                    cosmic::iced::core::renderer::Quad {
                        bounds: fade_bounds,
                        border: cosmic::iced::core::Border::default(),
                        shadow: cosmic::iced::core::Shadow::default(),
                        snap: false,
                    },
                    cosmic::iced::Background::Gradient(
                        cosmic::iced::gradient::Linear::new(Radians(FRAC_PI_2))
                            .add_stop(0.0, transparent)
                            .add_stop(1.0, opaque)
                            .into(),
                    ),
                );
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
        // Track mouse-button state so draw() can match the pressed
        // background colour of the parent button.
        if let cosmic::iced::core::Event::Mouse(mouse_ev) = event {
            use cosmic::iced::core::mouse::{Button, Event as ME};
            match mouse_ev {
                ME::ButtonPressed(Button::Left) => {
                    tree.state.downcast_mut::<FadingClipState>().mouse_pressed = true;
                }
                ME::ButtonReleased(Button::Left) => {
                    tree.state.downcast_mut::<FadingClipState>().mouse_pressed = false;
                }
                _ => {}
            }
        }

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
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout.children().next().unwrap(),
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut cosmic::iced::core::widget::Tree,
        layout: cosmic::iced::core::Layout<'b>,
        renderer: &cosmic::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<cosmic::iced::core::overlay::Element<'b, Msg, cosmic::Theme, cosmic::Renderer>>
    {
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
