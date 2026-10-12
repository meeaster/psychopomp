//! Exact CPU glyph rasterization. Font provisioning, text identity, theme mapping and layout belong to callers.
use cosmic_text::{Attrs, Buffer, Color, FontSystem, Metrics, Shaping, SwashCache, Wrap};

#[derive(Clone)]
pub(crate) struct TextSprite {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) advance: f32,
    pub(crate) pixels: Vec<u8>,
}

pub(crate) fn make_sprite<'a>(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    spans: Vec<(&'a str, Attrs<'a>)>,
    base: Attrs<'a>,
    metrics: Metrics,
    width: u32,
    height: u32,
) -> TextSprite {
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_size(Some(width as f32), Some(height as f32));
    buffer.set_wrap(Wrap::None);
    buffer.set_rich_text(spans, &base, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);
    let advance = buffer.layout_runs().next().map_or(0.0, |run| run.line_w);
    let mut pixels = vec![0_u8; width as usize * height as usize * 4];
    buffer.draw(
        font_system,
        swash_cache,
        Color::rgb(228, 228, 231),
        |x, y, rect_width, rect_height, color| {
            paint_rect(
                &mut pixels,
                width,
                height,
                x,
                y,
                rect_width,
                rect_height,
                [color.r(), color.g(), color.b(), color.a()],
            );
        },
    );
    TextSprite {
        width,
        height,
        advance,
        pixels,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_rect(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    rect_width: u32,
    rect_height: u32,
    color: [u8; 4],
) {
    for row in 0..rect_height as i32 {
        for column in 0..rect_width as i32 {
            blend_pixel_at(pixels, width, height, x + column, y + row, color, 1.0);
        }
    }
}

/// Blends one pixel of a `width × height` RGBA canvas, ignoring points outside it.
pub(crate) fn blend_pixel_at(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    color: [u8; 4],
    opacity: f32,
) {
    if !(0..width as i32).contains(&x) || !(0..height as i32).contains(&y) {
        return;
    }
    let index = (y as usize * width as usize + x as usize) * 4;
    blend_pixel(&mut pixels[index..index + 4], color, opacity);
}

pub(crate) fn blend_pixel(destination: &mut [u8], source: [u8; 4], opacity: f32) {
    let source_alpha = f32::from(source[3]) / 255.0 * opacity.clamp(0.0, 1.0);
    if source_alpha <= 0.0 {
        return;
    }
    // A fresh transparent layer needs only the source's straight RGB and
    // weighted alpha. Opaque sources similarly replace the destination. The
    // 1e-6 floor keeps tiny alpha on the original path, where subnormal
    // multiply/divide rounding can change the RGB bytes.
    if (destination[3] == 0 && (1e-6..=1.0).contains(&source_alpha)) || source_alpha == 1.0 {
        destination[..3].copy_from_slice(&source[..3]);
        destination[3] = (source_alpha * 255.0).round() as u8;
        return;
    }
    let destination_alpha = f32::from(destination[3]) / 255.0;
    let output_alpha = source_alpha + destination_alpha * (1.0 - source_alpha);
    for channel in 0..3 {
        let source_channel = f32::from(source[channel]) / 255.0;
        let destination_channel = f32::from(destination[channel]) / 255.0;
        let output = (source_channel * source_alpha
            + destination_channel * destination_alpha * (1.0 - source_alpha))
            / output_alpha;
        destination[channel] = (output * 255.0).round() as u8;
    }
    destination[3] = (output_alpha * 255.0).round() as u8;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transparent_and_opaque_blends_preserve_straight_color_and_weighted_alpha() {
        for channel in 0..=255 {
            for alpha in 1..=255 {
                let source = [channel, 255 - channel, channel / 2, alpha];
                for opacity in [0.001, 0.125, 0.5, 0.97, 1.0] {
                    let mut pixel = [73, 29, 183, 0];
                    blend_pixel(&mut pixel, source, opacity);
                    assert_eq!(&pixel[..3], &source[..3]);
                    assert_eq!(
                        pixel[3],
                        (f32::from(alpha) / 255.0 * opacity * 255.0).round() as u8
                    );
                }
            }
            let source = [channel, 255 - channel, channel / 2, 255];
            for alpha in 0..=255 {
                let mut pixel = [73, 29, 183, alpha];
                blend_pixel(&mut pixel, source, 1.0);
                assert_eq!(pixel, source);
            }
        }
    }

    #[test]
    fn transparent_blend_matches_expected_bytes_around_alpha_cutoff() {
        // Opaque source alpha makes opacity the effective alpha exactly.
        // Adjacent f32 values straddle 1e-6; the weighted alpha still rounds to zero.
        let cutoff = 1e-6_f32;
        let source = [1, 128, 254, 255];
        for opacity in [
            f32::from_bits(cutoff.to_bits() - 1),
            cutoff,
            f32::from_bits(cutoff.to_bits() + 1),
        ] {
            let mut pixel = [73, 29, 183, 0];
            blend_pixel(&mut pixel, source, opacity);
            assert_eq!(pixel, [1, 128, 254, 0], "opacity={opacity:?}");
        }
    }

    #[test]
    fn transparent_blend_preserves_original_rounding_for_subnormal_alpha() {
        // With the least subnormal alpha, 1/255 * alpha underflows to zero,
        // while 128/255 * alpha rounds back to alpha. The original division
        // therefore yields RGB [0, 255, 255]; weighted alpha rounds to zero.
        let mut pixel = [73, 29, 183, 0];
        blend_pixel(&mut pixel, [1, 128, 255, 255], f32::from_bits(1));
        assert_eq!(pixel, [0, 255, 255, 0]);
    }

    #[test]
    fn overlapping_rectangles_keep_straight_alpha_bytes_and_clip_at_the_original_origin() {
        let mut pixel = [0; 4];
        blend_pixel(&mut pixel, [255, 0, 0, 128], 1.);
        blend_pixel(&mut pixel, [0, 0, 255, 128], 1.);
        assert_eq!(pixel, [85, 0, 170, 192]);
        let mut pixels = vec![0; 3 * 2 * 4];
        paint_rect(&mut pixels, 3, 2, -1, -1, 3, 3, [12, 34, 56, 128]);
        for (i, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
            assert_eq!(
                pixel,
                if [0, 1, 3, 4].contains(&i) {
                    &[12, 34, 56, 128]
                } else {
                    &[0, 0, 0, 0]
                }
            );
        }
    }
}
