// SPDX-License-Identifier: MIT

//! Self-animating audio visualizer widget for the now-playing bar.
//!
//! This module provides a real-time stereo spectrum visualizer that displays
//! actual frequency data from the audio engine's FFT analysis.
//! Left channel bars grow upward from center, right channel bars grow downward.
//!
//! ## Performance
//!
//! The visualizer is implemented as a **self-animating custom `Widget`** that
//! drives its own redraws via `shell.request_redraw()`.  It reads spectrum
//! data directly from a `SharedSpectrumAnalyzer` (an `Arc`-wrapped handle
//! shared with the audio engine's playback thread) and stores all animation
//! state (bar heights, smoothing, frame counter) inside iced's widget tree
//! via `Tree::state`.
//!
//! This means **no `Message` is emitted and no `update()` → `view()` cycle
//! is triggered** by the visualizer's animation loop.  The iced runtime
//! simply calls `draw()` on the existing widget tree each frame, confining
//! the damage rectangle to the visualizer's tiny bounding box (~54 × 40 px).

use crate::audio::SpectrumData;
use crate::audio::spectrum::SharedSpectrumAnalyzer;
use cosmic::Element;
use cosmic::iced::core::widget::tree;
use cosmic::iced::core::{Clipboard, Event, Shell};
use cosmic::iced::{Color, Length, Rectangle, Size};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tracing::{debug, trace};

// =============================================================================
// Constants
// =============================================================================

/// Number of bars in the visualizer (per channel).
const NUM_BARS: usize = 12;

/// Maximum height of bars in pixels (per channel — half of total height).
const MAX_BAR_HEIGHT: f32 = 18.0;

/// Width of each bar in pixels.
const BAR_WIDTH: f32 = 3.0;

/// Gap between bars in pixels.
const BAR_GAP: f32 = 1.5;

/// Total visualizer width: bars + gaps only (no extra padding).
const VISUALIZER_WIDTH: f32 = (NUM_BARS as f32 * BAR_WIDTH) + ((NUM_BARS - 1) as f32 * BAR_GAP);

/// Total visualizer height (both channels, no centre gap).
const VISUALIZER_HEIGHT: f32 = MAX_BAR_HEIGHT * 2.0;

/// Vertical padding applied by the outer container.
const VERTICAL_PAD: f32 = 2.0;

/// Horizontal padding so the rightmost bars aren't clipped against the
/// parent container edge.
const HORIZONTAL_PAD: f32 = 4.0;

/// Smoothing factor for **falling** (decay) edges.  A higher value means
/// bars fall more slowly, giving a pleasant "gravity" feel.
const SMOOTH_DECAY: f32 = 0.55;

/// Smoothing factor for **rising** (attack) edges.  Kept very low so
/// transients punch through almost immediately.
const SMOOTH_ATTACK: f32 = 0.15;

/// Target frame interval for the self-animating loop (~30 fps).
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// Minimum per-bar change to count as visually different.
/// Below this threshold the pixel-level output is identical.
const DIRTY_EPSILON: f32 = 0.005;

// =============================================================================
// VisualizerState — lightweight handle stored on the app model
// =============================================================================

/// Lightweight handle that the app model stores and passes into the widget.
///
/// This does **not** contain bar heights or animation state — those live
/// inside the widget's tree state ([`AnimState`]).  `VisualizerState` only
/// holds the shared data handles that the widget needs to read spectrum data
/// and know whether it should be animating.
#[derive(Debug, Clone)]
pub struct VisualizerState {
    /// Shared spectrum analyzer handle (cloned from the audio engine).
    /// `None` when no player has been created yet.
    analyzer: Option<SharedSpectrumAnalyzer>,

    /// Shared flag: `true` when music is playing and the widget should
    /// animate.  Written by the app model, read by the widget.
    active: Arc<AtomicBool>,

    /// Whether the fade-out after deactivation has fully settled.
    /// When `true` **and** not active, the subscription tick can be
    /// dropped entirely.
    settled: Arc<AtomicBool>,
}

impl Default for VisualizerState {
    fn default() -> Self {
        Self::new()
    }
}

impl VisualizerState {
    /// Create a new (inactive, no analyzer) state.
    pub fn new() -> Self {
        Self {
            analyzer: None,
            active: Arc::new(AtomicBool::new(false)),
            settled: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Attach the spectrum analyzer from the audio engine / player.
    ///
    /// Call this once after the player is created so the widget can read
    /// FFT data directly.
    pub fn set_analyzer(&mut self, analyzer: SharedSpectrumAnalyzer) {
        self.analyzer = Some(analyzer);
    }

    /// Set whether the visualizer should be animating.
    pub fn set_active(&mut self, active: bool) {
        let was = self.active.load(Ordering::Relaxed);
        if active != was {
            debug!("Visualizer set_active: {} -> {}", was, active);
            self.active.store(active, Ordering::Relaxed);
            if active {
                // When activating, mark as not settled so the widget
                // starts its animation loop.
                self.settled.store(false, Ordering::Relaxed);
            }
        }
    }

    /// Whether the visualizer needs the subscription tick to keep running.
    ///
    /// Returns `false` when inactive **and** the bars have fully settled
    /// (fade-out complete).  The caller can skip the tick subscription.
    pub fn needs_tick(&self) -> bool {
        self.active.load(Ordering::Relaxed) || !self.settled.load(Ordering::Relaxed)
    }

    /// Create the visualizer widget element.
    ///
    /// Returns a self-animating [`VisualizerWidget`] that reads spectrum
    /// data from the shared analyzer and drives its own redraws — no
    /// `Message` is emitted, so `update()` / `view()` are never called
    /// by the visualizer's animation loop.
    pub fn view<Msg: 'static>(&self) -> Element<'_, Msg> {
        VisualizerWidget {
            analyzer: self.analyzer.clone(),
            active: Arc::clone(&self.active),
            settled: Arc::clone(&self.settled),
        }
        .into()
    }
}

// =============================================================================
// Widget tree state — all mutable animation data lives here
// =============================================================================

/// Per-widget animation state stored inside iced's widget `Tree`.
///
/// This is created once (in `Widget::state()`) and then mutated in-place
/// on each `RedrawRequested` event.  Because it lives in the tree, it
/// persists across `view()` rebuilds.
struct AnimState {
    left_heights: Vec<f32>,
    right_heights: Vec<f32>,
    frame: u64,
    colors: Vec<Color>,
    last_update: Instant,
    /// Track previous heights for dirty detection.
    prev_left: Vec<f32>,
    prev_right: Vec<f32>,
}

impl AnimState {
    fn new() -> Self {
        let colors = generate_gradient_colors(NUM_BARS);
        Self {
            left_heights: vec![0.1; NUM_BARS],
            right_heights: vec![0.1; NUM_BARS],
            frame: 0,
            colors,
            last_update: Instant::now(),
            prev_left: vec![0.1; NUM_BARS],
            prev_right: vec![0.1; NUM_BARS],
        }
    }

    /// Snapshot current heights, then compare after mutation to decide
    /// whether the widget needs another redraw.
    fn snapshot(&mut self) {
        self.prev_left.copy_from_slice(&self.left_heights);
        self.prev_right.copy_from_slice(&self.right_heights);
    }

    /// Returns `true` if any bar moved enough to be visually different.
    fn is_dirty(&self) -> bool {
        self.left_heights
            .iter()
            .zip(&self.prev_left)
            .chain(self.right_heights.iter().zip(&self.prev_right))
            .any(|(cur, prev)| (cur - prev).abs() > DIRTY_EPSILON)
    }

    /// Update heights from real spectrum data.
    fn update_from_spectrum(&mut self, spectrum: &SpectrumData) {
        self.snapshot();

        let left_bands = &spectrum.left_bands;
        let right_bands = &spectrum.right_bands;

        for i in 0..NUM_BARS {
            let left_target = compute_target_height(i, left_bands);
            let left_current = self.left_heights.get(i).copied().unwrap_or(0.05);
            if let Some(h) = self.left_heights.get_mut(i) {
                *h = smooth_height(left_current, left_target);
            }

            let right_target = compute_target_height(i, right_bands);
            let right_current = self.right_heights.get(i).copied().unwrap_or(0.05);
            if let Some(h) = self.right_heights.get_mut(i) {
                *h = smooth_height(right_current, right_target);
            }
        }

        self.frame = self.frame.wrapping_add(1);
    }

    /// Fade all bars toward the resting height (0.05).
    fn fade_out(&mut self) {
        self.snapshot();

        for h in &mut self.left_heights {
            *h = (*h * 0.85).max(0.05);
        }
        for h in &mut self.right_heights {
            *h = (*h * 0.85).max(0.05);
        }
    }
}

// =============================================================================
// VisualizerWidget — self-animating, zero-message custom widget
// =============================================================================

/// A leaf widget that reads spectrum data from a [`SharedSpectrumAnalyzer`],
/// updates bar heights in its tree state, and requests its own redraws via
/// `shell.request_redraw()`.
///
/// Because no `Message` is emitted, `update()` and `view()` are **never**
/// called by the visualizer's animation loop — only `draw()` runs each
/// frame.
pub struct VisualizerWidget {
    analyzer: Option<SharedSpectrumAnalyzer>,
    active: Arc<AtomicBool>,
    settled: Arc<AtomicBool>,
}

impl<Msg: 'static> cosmic::iced::core::Widget<Msg, cosmic::Theme, cosmic::Renderer>
    for VisualizerWidget
{
    // -- tree state -----------------------------------------------------------

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<AnimState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(AnimState::new())
    }

    // -- layout (fixed, never changes) ----------------------------------------

    fn size(&self) -> Size<Length> {
        Size::new(
            Length::Fixed(VISUALIZER_WIDTH + HORIZONTAL_PAD),
            Length::Fixed(VISUALIZER_HEIGHT + VERTICAL_PAD * 2.0),
        )
    }

    fn layout(
        &mut self,
        _tree: &mut cosmic::iced::core::widget::Tree,
        _renderer: &cosmic::Renderer,
        _limits: &cosmic::iced::core::layout::Limits,
    ) -> cosmic::iced::core::layout::Node {
        cosmic::iced::core::layout::Node::new(Size::new(
            VISUALIZER_WIDTH + HORIZONTAL_PAD,
            VISUALIZER_HEIGHT + VERTICAL_PAD * 2.0,
        ))
    }

    // -- self-animation via on_event ------------------------------------------

    fn update(
        &mut self,
        tree: &mut cosmic::iced::core::widget::Tree,
        event: &Event,
        _layout: cosmic::iced::core::Layout<'_>,
        _cursor: cosmic::iced::core::mouse::Cursor,
        _renderer: &cosmic::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Msg>,
        _viewport: &Rectangle,
    ) {
        // For non-redraw events, check whether we need to kick-start the
        // animation loop.  This handles the case where the widget went
        // dormant (settled) and was later re-activated by `set_active(true)`
        // from the app model — `update` won't receive `RedrawRequested`
        // until *someone* calls `request_redraw` first.
        let Event::Window(cosmic::iced::window::Event::RedrawRequested(now)) = event else {
            if !self.settled.load(Ordering::Relaxed) {
                shell.request_redraw();
            }
            return;
        };

        let state = tree.state.downcast_mut::<AnimState>();
        let is_active = self.active.load(Ordering::Relaxed);

        // Throttle: skip if not enough time has passed since last update.
        if now.duration_since(state.last_update) < FRAME_INTERVAL {
            // Still schedule the next frame so we don't miss it.
            if is_active || !self.settled.load(Ordering::Relaxed) {
                shell.request_redraw_at(state.last_update + FRAME_INTERVAL);
            }
            return;
        }
        state.last_update = *now;

        if is_active {
            if let Some(ref analyzer) = self.analyzer {
                let spectrum = analyzer.compute();
                state.update_from_spectrum(&spectrum);
            } else {
                // No analyzer means no audio output — nothing to visualize.
                state.fade_out();
            }

            // Log every ~1 second.
            if state.frame.is_multiple_of(30) {
                trace!(
                    "Visualizer frame={}, L[0]={:.2}, R[0]={:.2}",
                    state.frame,
                    state.left_heights.first().copied().unwrap_or(0.0),
                    state.right_heights.first().copied().unwrap_or(0.0),
                );
            }
        } else {
            state.fade_out();

            if !state.is_dirty() {
                // Bars have fully settled — signal the app model.
                self.settled.store(true, Ordering::Relaxed);
                // Don't request another redraw; we're done.
                return;
            }
        }

        // Schedule the next frame.
        shell.request_redraw_at(*now + FRAME_INTERVAL);
    }

    // -- draw -----------------------------------------------------------------

    fn draw(
        &self,
        tree: &cosmic::iced::core::widget::Tree,
        renderer: &mut cosmic::Renderer,
        _theme: &cosmic::Theme,
        _style: &cosmic::iced::core::renderer::Style,
        layout: cosmic::iced::core::Layout<'_>,
        _cursor: cosmic::iced::core::mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use cosmic::iced::core::Renderer as _;

        let bounds = layout.bounds();
        let Some(clip) = bounds.intersection(viewport) else {
            return;
        };

        let state = tree.state.downcast_ref::<AnimState>();

        renderer.with_layer(clip, |renderer| {
            let colors = &state.colors;
            let center_y = bounds.y + VERTICAL_PAD + MAX_BAR_HEIGHT;

            // ── Top row: left channel (bars grow upward from centre) ──
            for i in 0..NUM_BARS {
                let height_frac = state.left_heights.get(i).copied().unwrap_or(0.1);
                let bar_h = (height_frac * MAX_BAR_HEIGHT).max(3.0);
                let color = colors.get(i).copied().unwrap_or(Color::WHITE);

                let x = bounds.x + HORIZONTAL_PAD / 2.0 + i as f32 * (BAR_WIDTH + BAR_GAP);
                let y = center_y - bar_h;

                renderer.fill_quad(
                    cosmic::iced::core::renderer::Quad {
                        bounds: Rectangle {
                            x,
                            y,
                            width: BAR_WIDTH,
                            height: bar_h,
                        },
                        border: cosmic::iced::core::Border {
                            radius: (BAR_WIDTH / 2.0).into(),
                            ..Default::default()
                        },
                        shadow: cosmic::iced::core::Shadow::default(),
                        snap: false,
                    },
                    cosmic::iced::Background::Color(color),
                );
            }

            // ── Bottom row: right channel (bars grow downward from centre) ──
            for i in 0..NUM_BARS {
                let height_frac = state.right_heights.get(i).copied().unwrap_or(0.1);
                let bar_h = (height_frac * MAX_BAR_HEIGHT).max(3.0);
                let color = colors.get(i).copied().unwrap_or(Color::WHITE);

                let x = bounds.x + HORIZONTAL_PAD / 2.0 + i as f32 * (BAR_WIDTH + BAR_GAP);
                let y = center_y;

                renderer.fill_quad(
                    cosmic::iced::core::renderer::Quad {
                        bounds: Rectangle {
                            x,
                            y,
                            width: BAR_WIDTH,
                            height: bar_h,
                        },
                        border: cosmic::iced::core::Border {
                            radius: (BAR_WIDTH / 2.0).into(),
                            ..Default::default()
                        },
                        shadow: cosmic::iced::core::Shadow::default(),
                        snap: false,
                    },
                    cosmic::iced::Background::Color(color),
                );
            }
        });
    }
}

impl<'a, Msg: 'static> From<VisualizerWidget> for cosmic::Element<'a, Msg> {
    fn from(widget: VisualizerWidget) -> Self {
        cosmic::Element::new(widget)
    }
}

// =============================================================================
// Pure helper functions
// =============================================================================

/// Compute target height for a bar from spectrum bands.
fn compute_target_height(bar_index: usize, bands: &[f32]) -> f32 {
    let spectrum_bands = bands.len();
    if spectrum_bands == 0 {
        return 0.05;
    }

    let start_band = (bar_index * spectrum_bands) / NUM_BARS;
    let end_band = ((bar_index + 1) * spectrum_bands) / NUM_BARS;

    let mut sum = 0.0f32;
    let mut count = 0;

    for band_idx in start_band..end_band.max(start_band + 1) {
        if let Some(&band_val) = bands.get(band_idx) {
            sum += band_val;
            count += 1;
        }
    }

    if count > 0 {
        (sum / count as f32).clamp(0.05, 1.0)
    } else {
        0.05
    }
}

/// Apply asymmetric smoothing: near-instant attack, moderate decay.
fn smooth_height(current: f32, target: f32) -> f32 {
    let alpha = if target > current {
        SMOOTH_ATTACK // fast rise — transients punch through
    } else {
        SMOOTH_DECAY // slower fall — pleasant gravity
    };
    let new_height = alpha * current + (1.0 - alpha) * target;
    new_height.clamp(0.05, 1.0)
}

/// Generate gradient colours — bright cyan to magenta spectrum.
fn generate_gradient_colors(count: usize) -> Vec<Color> {
    let mut colors = Vec::with_capacity(count);

    for i in 0..count {
        let t = i as f32 / (count - 1).max(1) as f32;

        let (r, g, b) = if t < 0.33 {
            let local_t = (t * 3.0).clamp(0.0, 1.0);
            (0.0, 1.0, 1.0 - local_t * 0.5)
        } else if t < 0.66 {
            let local_t = ((t - 0.33) * 3.0).clamp(0.0, 1.0);
            (local_t * 0.5, 1.0, 0.5 - local_t * 0.3)
        } else {
            let local_t = ((t - 0.66) * 3.0).clamp(0.0, 1.0);
            (
                0.5 + local_t * 0.5,
                1.0 - local_t * 0.3,
                0.2 + local_t * 0.8,
            )
        };

        colors.push(Color::from_rgb(
            r.clamp(0.0, 1.0),
            g.clamp(0.0, 1.0),
            b.clamp(0.0, 1.0),
        ));
    }

    colors
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_visualizer_state_new() {
        let state = VisualizerState::new();
        assert!(!state.active.load(Ordering::Relaxed));
        assert!(state.settled.load(Ordering::Relaxed));
        assert!(state.analyzer.is_none());
    }

    #[test]
    fn test_set_active() {
        let mut state = VisualizerState::new();
        assert!(!state.active.load(Ordering::Relaxed));

        state.set_active(true);
        assert!(state.active.load(Ordering::Relaxed));
        // Activating should mark as not settled.
        assert!(!state.settled.load(Ordering::Relaxed));

        state.set_active(false);
        assert!(!state.active.load(Ordering::Relaxed));
    }

    #[test]
    fn test_needs_tick_when_active() {
        let mut state = VisualizerState::new();
        state.set_active(true);
        assert!(state.needs_tick());
    }

    #[test]
    fn test_needs_tick_when_not_settled() {
        let state = VisualizerState::new();
        state.settled.store(false, Ordering::Relaxed);
        assert!(state.needs_tick());
    }

    #[test]
    fn test_needs_tick_when_settled_and_inactive() {
        let state = VisualizerState::new();
        // Default: inactive + settled = no tick needed.
        assert!(!state.needs_tick());
    }

    #[test]
    fn test_anim_state_new() {
        let anim = AnimState::new();
        assert_eq!(anim.left_heights.len(), NUM_BARS);
        assert_eq!(anim.right_heights.len(), NUM_BARS);
        assert_eq!(anim.colors.len(), NUM_BARS);
        assert_eq!(anim.frame, 0);
    }

    #[test]
    fn test_update_from_spectrum() {
        let mut anim = AnimState::new();

        let spectrum = SpectrumData {
            left_bands: vec![0.8; 12],
            right_bands: vec![0.3; 12],
            bands: vec![0.55; 12],
        };

        anim.update_from_spectrum(&spectrum);

        // Left heights should be higher than right heights.
        let left_avg: f32 = anim.left_heights.iter().sum::<f32>() / NUM_BARS as f32;
        let right_avg: f32 = anim.right_heights.iter().sum::<f32>() / NUM_BARS as f32;

        assert!(
            left_avg > right_avg,
            "Left avg {} should be > right avg {}",
            left_avg,
            right_avg
        );
    }

    #[test]
    fn test_no_analyzer_fades_out() {
        let mut anim = AnimState::new();
        // Start with raised bars so fade_out has something to decay.
        anim.left_heights = vec![0.8; NUM_BARS];
        anim.right_heights = vec![0.8; NUM_BARS];

        for _ in 0..40 {
            anim.fade_out();
        }

        // All bars should have decayed toward the resting height (0.05).
        for h in &anim.left_heights {
            assert!(*h < 0.1, "left bar should have faded, got {}", h);
        }
        for h in &anim.right_heights {
            assert!(*h < 0.1, "right bar should have faded, got {}", h);
        }
    }

    #[test]
    fn test_fade_out() {
        let mut anim = AnimState::new();
        anim.left_heights = vec![0.8; NUM_BARS];
        anim.right_heights = vec![0.8; NUM_BARS];

        for _ in 0..40 {
            anim.fade_out();
        }

        assert!(anim.left_heights.iter().all(|&h| h < 0.15));
        assert!(anim.right_heights.iter().all(|&h| h < 0.15));
    }

    #[test]
    fn test_dirty_detection() {
        let mut anim = AnimState::new();

        // After snapshot without changes, should not be dirty.
        anim.snapshot();
        assert!(!anim.is_dirty());

        // After a significant change, should be dirty.
        anim.snapshot();
        if let Some(h) = anim.left_heights.get_mut(0) {
            *h += 0.1;
        }
        assert!(anim.is_dirty());
    }

    #[test]
    fn test_gradient_colors() {
        let colors = generate_gradient_colors(16);
        assert_eq!(colors.len(), 16);

        // First color should be cyan-ish.
        assert!(colors[0].r < 0.5);
        assert!(colors[0].g > 0.5);

        // Last color should be magenta-ish.
        assert!(colors[15].r > 0.5);
        assert!(colors[15].b > 0.5);
    }

    #[test]
    fn test_compute_target_height_empty_bands() {
        assert!((compute_target_height(0, &[]) - 0.05).abs() < f32::EPSILON);
    }

    #[test]
    fn test_compute_target_height_uniform() {
        let bands = vec![0.5; 12];
        let h = compute_target_height(6, &bands);
        assert!((h - 0.5).abs() < 0.01, "got {h}");
    }

    #[test]
    fn test_smooth_height_rising() {
        let h = smooth_height(0.1, 0.9);
        // Should move significantly toward target.
        assert!(h > 0.5, "got {h}");
    }

    #[test]
    fn test_smooth_height_falling() {
        let h = smooth_height(0.9, 0.1);
        // Should move toward target, but slower than rising.
        assert!(h < 0.9 && h > 0.1, "got {h}");
    }

    #[test]
    fn test_colors_cached_on_construction() {
        let anim = AnimState::new();
        assert_eq!(anim.colors.len(), NUM_BARS);
    }
}
