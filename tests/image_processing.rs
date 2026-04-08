// SPDX-License-Identifier: MIT

//! Integration tests for image processing functions.
//!
//! Covers `make_circular` and `make_grid_thumbnail` with synthetic images,
//! including edge cases like single-pixel images, non-square images,
//! various input counts for grid thumbnails, and error handling for
//! invalid/corrupt image data.

// Relax production safety lints for test code — clarity over strictness.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::manual_assert,
    clippy::wildcard_imports
)]

use cosmic_applet_mare::image_cache::{RgbaPixels, make_circular, make_grid_thumbnail};
use image::{ImageFormat, RgbaImage};
use std::io::Cursor;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a solid-colour PNG image of the given dimensions.
fn make_solid_png(width: u32, height: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
    let img = RgbaImage::from_pixel(width, height, image::Rgba([r, g, b, 255]));
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
        .expect("encode png");
    buf
}

/// Create a solid-colour JPEG image of the given dimensions.
fn make_solid_jpeg(width: u32, height: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(width, height, image::Rgb([r, g, b]));
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Jpeg)
        .expect("encode jpeg");
    buf
}

/// Convert an `RgbaPixels` result into an `RgbaImage` for inspection.
fn rgba_image(rgba: &RgbaPixels) -> RgbaImage {
    RgbaImage::from_raw(rgba.width, rgba.height, rgba.pixels.clone())
        .expect("RgbaPixels should have correct dimensions")
}

// ===========================================================================
// make_circular — basic behaviour
// ===========================================================================

mod circular_basic {
    use super::*;

    #[test]
    fn produces_rgba_output() {
        let input = make_solid_png(100, 100, 255, 0, 0);
        let result = make_circular(&input).unwrap();
        assert_eq!(result.width, 100);
        assert_eq!(result.height, 100);
        assert_eq!(
            result.pixels.len(),
            (100 * 100 * 4) as usize,
            "should have width*height*4 RGBA bytes"
        );
    }

    #[test]
    fn output_is_square() {
        let input = make_solid_png(100, 100, 0, 255, 0);
        let result = make_circular(&input).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), img.height(), "output should be square");
    }

    #[test]
    fn square_input_preserves_size() {
        let input = make_solid_png(64, 64, 0, 0, 255);
        let result = make_circular(&input).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 64);
        assert_eq!(img.height(), 64);
    }

    #[test]
    fn rectangular_input_crops_to_square() {
        // Landscape: 200x100 → should crop to 100x100
        let input = make_solid_png(200, 100, 128, 128, 128);
        let result = make_circular(&input).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 100);
        assert_eq!(img.height(), 100);
    }

    #[test]
    fn portrait_input_crops_to_square() {
        // Portrait: 100x200 → should crop to 100x100
        let input = make_solid_png(100, 200, 64, 64, 64);
        let result = make_circular(&input).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 100);
        assert_eq!(img.height(), 100);
    }

    #[test]
    fn corners_are_transparent() {
        let input = make_solid_png(100, 100, 255, 0, 0);
        let result = make_circular(&input).unwrap();
        let img = rgba_image(&result);

        // The four corners should be fully transparent (alpha = 0)
        let corners = [(0, 0), (99, 0), (0, 99), (99, 99)];
        for (x, y) in corners {
            let pixel = img.get_pixel(x, y);
            assert_eq!(
                pixel[3], 0,
                "corner ({},{}) should be transparent, alpha={}",
                x, y, pixel[3]
            );
        }
    }

    #[test]
    fn center_is_opaque() {
        let input = make_solid_png(100, 100, 255, 0, 0);
        let result = make_circular(&input).unwrap();
        let img = rgba_image(&result);

        // The center should be fully opaque
        let pixel = img.get_pixel(50, 50);
        assert_eq!(pixel[3], 255, "center should be opaque, alpha={}", pixel[3]);
        // And red
        assert_eq!(pixel[0], 255, "center should be red");
    }

    #[test]
    fn center_preserves_colour() {
        let input = make_solid_png(100, 100, 42, 128, 200);
        let result = make_circular(&input).unwrap();
        let img = rgba_image(&result);
        let pixel = img.get_pixel(50, 50);
        assert_eq!(pixel[0], 42);
        assert_eq!(pixel[1], 128);
        assert_eq!(pixel[2], 200);
        assert_eq!(pixel[3], 255);
    }
}

// ===========================================================================
// make_circular — input formats
// ===========================================================================

mod circular_formats {
    use super::*;

    #[test]
    fn accepts_jpeg_input() {
        let input = make_solid_jpeg(80, 80, 255, 128, 0);
        let result = make_circular(&input).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 80);
        assert_eq!(img.height(), 80);
    }

    #[test]
    fn accepts_png_input() {
        let input = make_solid_png(60, 60, 0, 255, 128);
        let result = make_circular(&input).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 60);
        assert_eq!(img.height(), 60);
    }
}

// ===========================================================================
// make_circular — edge cases
// ===========================================================================

mod circular_edge_cases {
    use super::*;

    #[test]
    fn single_pixel_image() {
        let input = make_solid_png(1, 1, 255, 0, 0);
        let result = make_circular(&input).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 1);
        assert_eq!(img.height(), 1);
    }

    #[test]
    fn two_by_two_image() {
        let input = make_solid_png(2, 2, 0, 255, 0);
        let result = make_circular(&input).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 2);
        assert_eq!(img.height(), 2);
    }

    #[test]
    fn large_image() {
        let input = make_solid_png(1000, 1000, 100, 100, 100);
        let result = make_circular(&input).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 1000);
        assert_eq!(img.height(), 1000);

        // Corner should be transparent
        let corner = img.get_pixel(0, 0);
        assert_eq!(corner[3], 0);

        // Center should be opaque
        let center = img.get_pixel(500, 500);
        assert_eq!(center[3], 255);
    }

    #[test]
    fn very_wide_rectangle() {
        let input = make_solid_png(500, 10, 200, 50, 50);
        let result = make_circular(&input).unwrap();
        let img = rgba_image(&result);
        // Should be cropped to the smaller dimension
        assert_eq!(img.width(), 10);
        assert_eq!(img.height(), 10);
    }

    #[test]
    fn very_tall_rectangle() {
        let input = make_solid_png(10, 500, 50, 200, 50);
        let result = make_circular(&input).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 10);
        assert_eq!(img.height(), 10);
    }

    #[test]
    fn odd_dimensions() {
        let input = make_solid_png(77, 77, 128, 128, 128);
        let result = make_circular(&input).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 77);
        assert_eq!(img.height(), 77);
    }
}

// ===========================================================================
// make_circular — error handling
// ===========================================================================

mod circular_errors {
    use super::*;

    #[test]
    fn empty_data_returns_error() {
        let result = make_circular(b"");
        assert!(result.is_err());
    }

    #[test]
    fn garbage_data_returns_error() {
        let result = make_circular(b"this is not an image at all");
        assert!(result.is_err());
    }

    #[test]
    fn truncated_png_returns_error() {
        let full = make_solid_png(50, 50, 255, 0, 0);
        let truncated = &full[..full.len() / 2];
        let result = make_circular(truncated);
        assert!(result.is_err());
    }

    #[test]
    fn error_message_is_descriptive() {
        let result = make_circular(b"not an image");
        let err = result.unwrap_err();
        assert!(
            err.contains("decode") || err.contains("Failed"),
            "error message should mention decoding: {}",
            err
        );
    }
}

// ===========================================================================
// make_circular — transparency / mask verification
// ===========================================================================

mod circular_mask {
    use super::*;

    /// Verify the circular mask by checking that all pixels outside the
    /// inscribed circle are transparent and all pixels well inside are opaque.
    #[test]
    fn mask_inside_outside() {
        let size = 100u32;
        let input = make_solid_png(size, size, 255, 128, 0);
        let result = make_circular(&input).unwrap();
        let img = rgba_image(&result);

        let center = size as f32 / 2.0;
        let radius = center;

        for y in 0..size {
            for x in 0..size {
                let dx = x as f32 - center + 0.5;
                let dy = y as f32 - center + 0.5;
                let dist = (dx * dx + dy * dy).sqrt();
                let pixel = img.get_pixel(x, y);

                if dist > radius + 1.0 {
                    // Well outside — should be transparent
                    assert_eq!(
                        pixel[3], 0,
                        "pixel ({},{}) at dist={:.1} should be transparent, alpha={}",
                        x, y, dist, pixel[3]
                    );
                } else if dist < radius - 2.0 {
                    // Well inside — should be opaque
                    assert_eq!(
                        pixel[3], 255,
                        "pixel ({},{}) at dist={:.1} should be opaque, alpha={}",
                        x, y, dist, pixel[3]
                    );
                }
                // Near the edge (anti-aliasing zone) — any alpha is acceptable
            }
        }
    }

    /// Verify that the anti-aliased edge has pixels with intermediate alpha values.
    #[test]
    fn has_antialiased_edge() {
        let size = 200u32;
        let input = make_solid_png(size, size, 200, 200, 200);
        let result = make_circular(&input).unwrap();
        let img = rgba_image(&result);

        let center = size as f32 / 2.0;
        let radius = center;

        let mut found_intermediate = false;
        for y in 0..size {
            for x in 0..size {
                let dx = x as f32 - center + 0.5;
                let dy = y as f32 - center + 0.5;
                let dist = (dx * dx + dy * dy).sqrt();
                let pixel = img.get_pixel(x, y);

                if dist > radius - 1.5 && dist < radius + 0.5 && pixel[3] > 0 && pixel[3] < 255 {
                    found_intermediate = true;
                    break;
                }
            }
            if found_intermediate {
                break;
            }
        }

        assert!(
            found_intermediate,
            "should have anti-aliased edge pixels with intermediate alpha"
        );
    }
}

// ===========================================================================
// make_grid_thumbnail — basic behaviour
// ===========================================================================

mod grid_basic {
    use super::*;

    #[test]
    fn produces_rgba_output() {
        let input = make_solid_png(100, 100, 255, 0, 0);
        let result = make_grid_thumbnail(&[input.as_slice()], 160).unwrap();
        assert_eq!(result.width, 160);
        assert_eq!(result.height, 160);
        assert_eq!(
            result.pixels.len(),
            (160 * 160 * 4) as usize,
            "should have width*height*4 RGBA bytes"
        );
    }

    #[test]
    fn output_is_requested_size() {
        let img1 = make_solid_png(50, 50, 255, 0, 0);
        let img2 = make_solid_png(50, 50, 0, 255, 0);
        let img3 = make_solid_png(50, 50, 0, 0, 255);
        let img4 = make_solid_png(50, 50, 255, 255, 0);
        let images: Vec<&[u8]> = vec![&img1, &img2, &img3, &img4];

        let result = make_grid_thumbnail(&images, 200).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 200);
        assert_eq!(img.height(), 200);
    }

    #[test]
    fn output_size_64() {
        let img1 = make_solid_png(50, 50, 128, 128, 128);
        let images: Vec<&[u8]> = vec![&img1];
        let result = make_grid_thumbnail(&images, 64).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 64);
        assert_eq!(img.height(), 64);
    }

    #[test]
    fn output_size_320() {
        let img1 = make_solid_png(100, 100, 255, 0, 0);
        let img2 = make_solid_png(100, 100, 0, 255, 0);
        let images: Vec<&[u8]> = vec![&img1, &img2];
        let result = make_grid_thumbnail(&images, 320).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 320);
        assert_eq!(img.height(), 320);
    }

    #[test]
    fn corners_are_transparent_circle_mask() {
        let img1 = make_solid_png(50, 50, 255, 0, 0);
        let img2 = make_solid_png(50, 50, 0, 255, 0);
        let img3 = make_solid_png(50, 50, 0, 0, 255);
        let img4 = make_solid_png(50, 50, 255, 255, 0);
        let images: Vec<&[u8]> = vec![&img1, &img2, &img3, &img4];

        let result = make_grid_thumbnail(&images, 100).unwrap();
        let img = rgba_image(&result);

        let corners = [(0, 0), (99, 0), (0, 99), (99, 99)];
        for (x, y) in corners {
            let pixel = img.get_pixel(x, y);
            assert_eq!(
                pixel[3], 0,
                "corner ({},{}) should be transparent, alpha={}",
                x, y, pixel[3]
            );
        }
    }
}

// ===========================================================================
// make_grid_thumbnail — image count variations
// ===========================================================================

mod grid_image_counts {
    use super::*;

    #[test]
    fn one_image_fills_grid() {
        let img1 = make_solid_png(50, 50, 255, 0, 0);
        let images: Vec<&[u8]> = vec![&img1];
        let result = make_grid_thumbnail(&images, 100).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 100);
        assert_eq!(img.height(), 100);
    }

    #[test]
    fn two_images_fill_grid() {
        let img1 = make_solid_png(50, 50, 255, 0, 0);
        let img2 = make_solid_png(50, 50, 0, 255, 0);
        let images: Vec<&[u8]> = vec![&img1, &img2];
        let result = make_grid_thumbnail(&images, 100).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 100);
    }

    #[test]
    fn three_images_fill_grid() {
        let img1 = make_solid_png(50, 50, 255, 0, 0);
        let img2 = make_solid_png(50, 50, 0, 255, 0);
        let img3 = make_solid_png(50, 50, 0, 0, 255);
        let images: Vec<&[u8]> = vec![&img1, &img2, &img3];
        let result = make_grid_thumbnail(&images, 100).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 100);
    }

    #[test]
    fn four_images_exactly() {
        let img1 = make_solid_png(50, 50, 255, 0, 0);
        let img2 = make_solid_png(50, 50, 0, 255, 0);
        let img3 = make_solid_png(50, 50, 0, 0, 255);
        let img4 = make_solid_png(50, 50, 255, 255, 0);
        let images: Vec<&[u8]> = vec![&img1, &img2, &img3, &img4];
        let result = make_grid_thumbnail(&images, 100).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 100);
    }

    #[test]
    fn more_than_four_uses_first_four() {
        let img1 = make_solid_png(50, 50, 255, 0, 0);
        let img2 = make_solid_png(50, 50, 0, 255, 0);
        let img3 = make_solid_png(50, 50, 0, 0, 255);
        let img4 = make_solid_png(50, 50, 255, 255, 0);
        let img5 = make_solid_png(50, 50, 128, 128, 128);
        let images: Vec<&[u8]> = vec![&img1, &img2, &img3, &img4, &img5];
        let result = make_grid_thumbnail(&images, 100).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 100);
    }

    #[test]
    fn ten_images_still_works() {
        let imgs: Vec<Vec<u8>> = (0..10)
            .map(|i| make_solid_png(50, 50, (i * 25) as u8, 100, 200))
            .collect();
        let refs: Vec<&[u8]> = imgs.iter().map(|v| v.as_slice()).collect();
        let result = make_grid_thumbnail(&refs, 100).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 100);
    }
}

// ===========================================================================
// make_grid_thumbnail — error handling
// ===========================================================================

mod grid_errors {
    use super::*;

    #[test]
    fn empty_image_list_returns_error() {
        let images: Vec<&[u8]> = vec![];
        let result = make_grid_thumbnail(&images, 100);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("No images"),
            "error should mention no images: {}",
            err
        );
    }

    #[test]
    fn all_corrupt_images_returns_error() {
        let garbage = b"not an image";
        let images: Vec<&[u8]> = vec![garbage];
        let result = make_grid_thumbnail(&images, 100);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("decoded") || err.contains("None"),
            "error should mention decode failure: {}",
            err
        );
    }

    #[test]
    fn mix_of_valid_and_corrupt_uses_valid() {
        let valid = make_solid_png(50, 50, 255, 0, 0);
        let garbage = b"not an image at all";
        let images: Vec<&[u8]> = vec![&valid, garbage];
        // Should succeed using the one valid image
        let result = make_grid_thumbnail(&images, 100).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 100);
    }

    #[test]
    fn corrupt_first_valid_second() {
        let garbage = b"corrupted data";
        let valid = make_solid_png(50, 50, 0, 255, 0);
        let images: Vec<&[u8]> = vec![garbage, &valid];
        let result = make_grid_thumbnail(&images, 100).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 100);
    }
}

// ===========================================================================
// make_grid_thumbnail — input image dimensions
// ===========================================================================

mod grid_input_sizes {
    use super::*;

    #[test]
    fn mixed_sizes() {
        let img1 = make_solid_png(100, 100, 255, 0, 0);
        let img2 = make_solid_png(50, 50, 0, 255, 0);
        let img3 = make_solid_png(200, 200, 0, 0, 255);
        let img4 = make_solid_png(30, 30, 255, 255, 0);
        let images: Vec<&[u8]> = vec![&img1, &img2, &img3, &img4];
        let result = make_grid_thumbnail(&images, 120).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 120);
        assert_eq!(img.height(), 120);
    }

    #[test]
    fn rectangular_input_images() {
        let img1 = make_solid_png(200, 100, 255, 0, 0); // landscape
        let img2 = make_solid_png(100, 200, 0, 255, 0); // portrait
        let img3 = make_solid_png(150, 75, 0, 0, 255); // landscape
        let img4 = make_solid_png(75, 150, 255, 255, 0); // portrait
        let images: Vec<&[u8]> = vec![&img1, &img2, &img3, &img4];
        let result = make_grid_thumbnail(&images, 100).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 100);
        assert_eq!(img.height(), 100);
    }

    #[test]
    fn tiny_input_images() {
        let img1 = make_solid_png(2, 2, 255, 0, 0);
        let img2 = make_solid_png(2, 2, 0, 255, 0);
        let img3 = make_solid_png(2, 2, 0, 0, 255);
        let img4 = make_solid_png(2, 2, 255, 255, 0);
        let images: Vec<&[u8]> = vec![&img1, &img2, &img3, &img4];
        let result = make_grid_thumbnail(&images, 100).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 100);
    }

    #[test]
    fn large_input_small_output() {
        let img1 = make_solid_png(1000, 1000, 255, 0, 0);
        let images: Vec<&[u8]> = vec![&img1];
        let result = make_grid_thumbnail(&images, 32).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 32);
        assert_eq!(img.height(), 32);
    }

    #[test]
    fn jpeg_inputs() {
        let img1 = make_solid_jpeg(80, 80, 255, 0, 0);
        let img2 = make_solid_jpeg(80, 80, 0, 255, 0);
        let img3 = make_solid_jpeg(80, 80, 0, 0, 255);
        let img4 = make_solid_jpeg(80, 80, 255, 255, 0);
        let images: Vec<&[u8]> = vec![&img1, &img2, &img3, &img4];
        let result = make_grid_thumbnail(&images, 100).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 100);
    }

    #[test]
    fn mixed_png_and_jpeg() {
        let img1 = make_solid_png(80, 80, 255, 0, 0);
        let img2 = make_solid_jpeg(80, 80, 0, 255, 0);
        let img3 = make_solid_png(80, 80, 0, 0, 255);
        let img4 = make_solid_jpeg(80, 80, 255, 255, 0);
        let images: Vec<&[u8]> = vec![&img1, &img2, &img3, &img4];
        let result = make_grid_thumbnail(&images, 100).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 100);
    }
}

// ===========================================================================
// make_grid_thumbnail — quadrant colour verification
// ===========================================================================

mod grid_quadrants {
    use super::*;

    /// With four distinct-colour images, verify that each quadrant has the
    /// expected dominant colour (checking well inside each quadrant to avoid
    /// the gap and the circular mask).
    #[test]
    fn four_colours_in_quadrants() {
        let size = 200u32;
        let gap = 1u32;
        let half = (size - gap) / 2;

        let red = make_solid_png(100, 100, 255, 0, 0);
        let green = make_solid_png(100, 100, 0, 255, 0);
        let blue = make_solid_png(100, 100, 0, 0, 255);
        let yellow = make_solid_png(100, 100, 255, 255, 0);

        let images: Vec<&[u8]> = vec![&red, &green, &blue, &yellow];
        let result = make_grid_thumbnail(&images, size).unwrap();
        let img = rgba_image(&result);

        let center = size as f32 / 2.0;
        let radius = center;

        // Check a point well inside each quadrant, also inside the circle
        // TL quadrant: around (quarter, quarter)
        let q = half / 4;
        let check_points = [
            (q, q, 255, 0, 0, "top-left red"),
            (half + gap + q, q, 0, 255, 0, "top-right green"),
            (q, half + gap + q, 0, 0, 255, "bottom-left blue"),
            (
                half + gap + q,
                half + gap + q,
                255,
                255,
                0,
                "bottom-right yellow",
            ),
        ];

        for (x, y, er, eg, eb, label) in check_points {
            let dx = x as f32 - center + 0.5;
            let dy = y as f32 - center + 0.5;
            let dist = (dx * dx + dy * dy).sqrt();

            // Skip if this point is outside the circle (masked)
            if dist > radius - 2.0 {
                continue;
            }

            let pixel = img.get_pixel(x, y);
            assert!(
                pixel[3] > 200,
                "{} pixel ({},{}) should be opaque, alpha={}",
                label,
                x,
                y,
                pixel[3]
            );
            // Allow some tolerance for JPEG compression / resampling
            let tol = 30;
            assert!(
                (pixel[0] as i32 - er as i32).unsigned_abs() < tol
                    && (pixel[1] as i32 - eg as i32).unsigned_abs() < tol
                    && (pixel[2] as i32 - eb as i32).unsigned_abs() < tol,
                "{}: expected ~({},{},{}) got ({},{},{}) at ({},{})",
                label,
                er,
                eg,
                eb,
                pixel[0],
                pixel[1],
                pixel[2],
                x,
                y,
            );
        }
    }
}

// ===========================================================================
// make_grid_thumbnail — circular mask
// ===========================================================================

mod grid_circle_mask {
    use super::*;

    #[test]
    fn outside_circle_is_transparent() {
        let size = 100u32;
        let img1 = make_solid_png(50, 50, 255, 255, 255);
        let images: Vec<&[u8]> = vec![&img1];
        let result = make_grid_thumbnail(&images, size).unwrap();
        let img = rgba_image(&result);

        let center = size as f32 / 2.0;
        let radius = center;

        for y in 0..size {
            for x in 0..size {
                let dx = x as f32 - center + 0.5;
                let dy = y as f32 - center + 0.5;
                let dist = (dx * dx + dy * dy).sqrt();
                let pixel = img.get_pixel(x, y);

                if dist > radius + 1.0 {
                    assert_eq!(
                        pixel[3], 0,
                        "pixel ({},{}) at dist={:.1} should be transparent",
                        x, y, dist
                    );
                }
            }
        }
    }

    #[test]
    fn inside_circle_is_opaque() {
        let size = 100u32;
        let img1 = make_solid_png(50, 50, 200, 200, 200);
        let img2 = make_solid_png(50, 50, 100, 100, 100);
        let img3 = make_solid_png(50, 50, 150, 150, 150);
        let img4 = make_solid_png(50, 50, 50, 50, 50);
        let images: Vec<&[u8]> = vec![&img1, &img2, &img3, &img4];
        let result = make_grid_thumbnail(&images, size).unwrap();
        let img = rgba_image(&result);

        let center = size as f32 / 2.0;
        let radius = center;

        // Check several well-inside points (avoiding the 1px gap)
        let check_points = [
            (25, 25),
            (75, 25),
            (25, 75),
            (75, 75),
            (50, 50), // near center/gap
        ];

        for (x, y) in check_points {
            let dx = x as f32 - center + 0.5;
            let dy = y as f32 - center + 0.5;
            let dist = (dx * dx + dy * dy).sqrt();

            if dist < radius - 2.0 {
                let pixel = img.get_pixel(x, y);
                assert!(
                    pixel[3] > 200,
                    "pixel ({},{}) inside circle should be opaque, alpha={}",
                    x,
                    y,
                    pixel[3]
                );
            }
        }
    }
}

// ===========================================================================
// make_grid_thumbnail — determinism
// ===========================================================================

mod grid_determinism {
    use super::*;

    #[test]
    fn same_input_same_output() {
        let img1 = make_solid_png(50, 50, 255, 0, 0);
        let img2 = make_solid_png(50, 50, 0, 255, 0);
        let img3 = make_solid_png(50, 50, 0, 0, 255);
        let img4 = make_solid_png(50, 50, 255, 255, 0);
        let images: Vec<&[u8]> = vec![&img1, &img2, &img3, &img4];

        let result1 = make_grid_thumbnail(&images, 100).unwrap();
        let result2 = make_grid_thumbnail(&images, 100).unwrap();
        assert_eq!(
            result1.pixels, result2.pixels,
            "same inputs should produce same output"
        );
    }

    #[test]
    fn repeated_calls_stable() {
        let img1 = make_solid_png(80, 80, 128, 64, 32);
        let images: Vec<&[u8]> = vec![&img1];

        let mut prev: Option<Vec<u8>> = None;
        for _ in 0..10 {
            let result = make_grid_thumbnail(&images, 64).unwrap();
            if let Some(ref p) = prev {
                assert_eq!(&result.pixels, p);
            }
            prev = Some(result.pixels);
        }
    }
}

// ===========================================================================
// make_grid_thumbnail — output size edge cases
// ===========================================================================

mod grid_output_sizes {
    use super::*;

    #[test]
    fn small_output_3x3() {
        let img1 = make_solid_png(50, 50, 255, 0, 0);
        let images: Vec<&[u8]> = vec![&img1];
        let result = make_grid_thumbnail(&images, 3).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 3);
        assert_eq!(img.height(), 3);
    }

    #[test]
    fn odd_output_size() {
        let img1 = make_solid_png(50, 50, 255, 0, 0);
        let img2 = make_solid_png(50, 50, 0, 255, 0);
        let images: Vec<&[u8]> = vec![&img1, &img2];
        let result = make_grid_thumbnail(&images, 99).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 99);
        assert_eq!(img.height(), 99);
    }

    #[test]
    fn even_output_size() {
        let img1 = make_solid_png(50, 50, 255, 0, 0);
        let images: Vec<&[u8]> = vec![&img1];
        let result = make_grid_thumbnail(&images, 128).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 128);
        assert_eq!(img.height(), 128);
    }

    #[test]
    fn large_output_size() {
        let img1 = make_solid_png(50, 50, 100, 100, 100);
        let images: Vec<&[u8]> = vec![&img1];
        let result = make_grid_thumbnail(&images, 500).unwrap();
        let img = rgba_image(&result);
        assert_eq!(img.width(), 500);
        assert_eq!(img.height(), 500);
    }
}
