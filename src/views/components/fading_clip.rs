// SPDX-License-Identifier: MIT

//! A clipping widget with a gradient fade-out overlay.
//!
//! [`FadingClip`] GPU-clips its child content and draws a horizontal gradient
//! strip on the right edge that fades from transparent to the enclosing
//! container's background colour.  This gives long text (track titles, artist
//! names) a smooth fade instead of a harsh clip edge.
//!
//! The gradient colour is resolved per-frame from the current cosmic theme so
//! it adapts automatically to dark/light mode and hover/pressed button states.

use cosmic::iced::{Radians, Rectangle};
use std::f32::consts::FRAC_PI_2;

/// Extra pixels added to the right edge of the gradient strip so the
/// fully-opaque tail covers any sub-pixel text artifacts that leak past the
/// GPU scissor boundary.
const FADE_BLEED: f32 = 2.0;

// =============================================================================
// Public API
// =============================================================================

/// A widget that clips its child and draws a gradient fade on the right edge.
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
/// extra space of its own.  The gradient is drawn as an overlay inside the
/// child bounds.
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
    /// gradient is drawn, no padding is wasted.  When callers want the
    /// column to absorb leftover space they `.width(Length::Fill)` it,
    /// which still triggers the gradient only when the child content
    /// overflows.
    ///
    /// In theory `Length::Fill` would also work as a default but it would
    /// unnecessarily stretch every single text element even when there
    /// are no siblings competing for space (e.g. a standalone label in a
    /// `Column`), producing a wider-than-necessary layout and a phantom
    /// gradient at the far right that serves no visual purpose.
    width: cosmic::iced::Length,

    /// Explicit height override (default: `Length::Shrink`).
    height: cosmic::iced::Length,

    /// Width (in pixels) of the gradient fade strip.
    ///
    /// This is measured inward from the right edge of the widget.  A
    /// larger value gives a gentler fade at the cost of hiding more text.
    /// The caller passes this in at construction time — the default in
    /// [`super::list_helpers`] is 32 px.
    ///
    /// The gradient always starts fully transparent on the left and ends
    /// fully opaque (background colour) on the right.  An additional
    /// [`FADE_BLEED`] strip of the opaque colour extends past the nominal
    /// width to cover sub-pixel text rendering artifacts.
    fade_width: f32,

    /// Which background colour family the gradient should fade *to*.
    /// See [`FadeTarget`] for the available options.
    fade_target: FadeTarget,
}

/// Determines which theme colour the gradient fades into.
///
/// Each variant corresponds to a different visual context — the right
/// one must be chosen so the gradient's opaque end blends seamlessly
/// with whatever sits behind the text.
#[derive(Default)]
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
    /// composited over the surface).  For content inside a
    /// `Container::Card`.
    Card,
    /// Fade for the panel button label.  Uses `text_button.hover`
    /// composited over the surface on hover, raw surface otherwise.
    Panel,
    /// Fade for text inside a `Button::Suggested` (accent) button.
    Suggested,
    /// Fade for text inside a `Button::Standard` button.
    Standard,
}

/// Tracks layout state for the fade overlay.
#[derive(Debug, Clone, Default)]
struct FadingClipState {
    /// `true` when the child's natural (unconstrained) width exceeds the
    /// available layout width — i.e. the text is being clipped and needs
    /// the gradient fade overlay.
    content_overflows: bool,
}

impl<'a, Msg> FadingClip<'a, Msg> {
    pub(crate) fn new(child: impl Into<cosmic::Element<'a, Msg>>, fade_width: f32) -> Self {
        Self {
            child: child.into(),
            width: cosmic::iced::Length::Shrink,
            height: cosmic::iced::Length::Shrink,
            fade_width,
            fade_target: FadeTarget::default(),
        }
    }

    /// Fade to the popup surface colour (no button background).
    pub(crate) fn surface_only(mut self) -> Self {
        self.fade_target = FadeTarget::Surface;
        self
    }

    /// Fade to the card/component background.
    pub(crate) fn card(mut self) -> Self {
        self.fade_target = FadeTarget::Card;
        self
    }

    /// Fade to the panel text-button background.
    pub(crate) fn panel(mut self) -> Self {
        self.fade_target = FadeTarget::Panel;
        self
    }

    /// Fade to the `Button::Suggested` (accent) background.
    pub(crate) fn suggested(mut self) -> Self {
        self.fade_target = FadeTarget::Suggested;
        self
    }

    /// Fade to the `Button::Standard` background.
    pub(crate) fn standard(mut self) -> Self {
        self.fade_target = FadeTarget::Standard;
        self
    }

    pub(crate) fn width(mut self, width: cosmic::iced::Length) -> Self {
        self.width = width;
        self
    }
}

// =============================================================================
// Colour helpers
// =============================================================================

/// sRGB component → linear component.
///
/// Uses the standard IEC 61966-2-1 transfer function, the same one that
/// `iced::Color::into_linear` uses and that GPU hardware applies when
/// reading/writing `*Srgb` texture formats.
fn srgb_to_linear(u: f32) -> f32 {
    if u < 0.04045 {
        u / 12.92
    } else {
        ((u + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear component → sRGB component.
fn linear_to_srgb(u: f32) -> f32 {
    if u <= 0.0031308 {
        u * 12.92
    } else {
        1.055 * u.powf(1.0 / 2.4) - 0.055
    }
}

/// Alpha-over composite of `fg` on top of a **fully-opaque** `bg`,
/// blended in **linear RGB** space to match the GPU pipeline.
///
/// iced packs colours via `Color::into_linear()` and uses an sRGB
/// framebuffer with `PREMULTIPLIED_ALPHA_BLENDING`.  The hardware
/// therefore blends in linear space.  Doing the same here ensures
/// the gradient's opaque end is pixel-identical to the button
/// background rendered by the GPU.
///
/// Use [`composite_srgb`] instead for elements composited by the
/// desktop compositor (e.g. the panel bar) which may blend in sRGB.
fn composite(fg: cosmic::iced::Color, bg: cosmic::iced::Color) -> cosmic::iced::Color {
    let a = fg.a;

    // Convert to linear
    let fg_r = srgb_to_linear(fg.r);
    let fg_g = srgb_to_linear(fg.g);
    let fg_b = srgb_to_linear(fg.b);
    let bg_r = srgb_to_linear(bg.r);
    let bg_g = srgb_to_linear(bg.g);
    let bg_b = srgb_to_linear(bg.b);

    // Blend in linear space
    cosmic::iced::Color::from_rgba(
        linear_to_srgb(fg_r * a + bg_r * (1.0 - a)),
        linear_to_srgb(fg_g * a + bg_g * (1.0 - a)),
        linear_to_srgb(fg_b * a + bg_b * (1.0 - a)),
        1.0,
    )
}

/// Alpha-over composite in **sRGB** space (no gamma conversion).
///
/// The COSMIC panel bar is rendered by the Wayland compositor, which
/// may blend in sRGB rather than linear.  Using sRGB here matches the
/// compositor's pipeline so the gradient's opaque end is invisible
/// against the panel surface.
fn composite_srgb(fg: cosmic::iced::Color, bg: cosmic::iced::Color) -> cosmic::iced::Color {
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
        // First pass: measure the child with unbounded width to learn its
        // natural (unconstrained) width.
        let unbounded = cosmic::iced::core::layout::Limits::NONE.max_height(limits.max().height);
        let natural =
            self.child
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, &unbounded);
        let natural_width = natural.bounds().width;

        // Second pass: real layout with the actual (constrained) limits.
        let node =
            cosmic::iced::core::layout::contained(limits, self.width, self.height, |limits| {
                self.child
                    .as_widget_mut()
                    .layout(&mut tree.children[0], renderer, limits)
            });

        // Record whether the child overflows so draw() can skip the
        // gradient when it would be invisible.
        tree.state
            .downcast_mut::<FadingClipState>()
            .content_overflows = natural_width > node.bounds().width + 1.0;

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

                let btn_bg: Color = if is_mouse_over {
                    cosmic_theme.background.component.hover.into()
                } else {
                    cosmic_theme.background.component.base.into()
                };
                composite(btn_bg, surface)
            }
            FadeTarget::Panel => {
                let is_mouse_over = cursor.position().is_some_and(|pos| viewport.contains(pos));

                if is_mouse_over {
                    let text_btn = &cosmic_theme.text_button;
                    composite_srgb(text_btn.hover.into(), surface)
                } else {
                    surface
                }
            }
            FadeTarget::Suggested => {
                let is_mouse_over = cursor.position().is_some_and(|pos| viewport.contains(pos));

                let comp = &cosmic_theme.accent_button;
                let bg: Color = if is_mouse_over {
                    comp.hover.into()
                } else {
                    comp.base.into()
                };
                composite(bg, surface)
            }
            FadeTarget::Standard => {
                let is_mouse_over = cursor.position().is_some_and(|pos| viewport.contains(pos));

                let comp = &cosmic_theme.button;
                let bg: Color = if is_mouse_over {
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
        translation: cosmic::iced::Vector,
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
