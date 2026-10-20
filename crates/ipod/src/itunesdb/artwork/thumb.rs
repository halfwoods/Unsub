//! Turning a cover image into the raw pixel block an iPod reads from an `.ithmb` file. Scaling and
//! padding follow libgpod's `ithumb_writer_scale_and_crop` and `pack_RGB_565` (ithumb-writer.c).

use image::imageops::{FilterType, resize};
use image::{ImageReader, RgbImage};
use std::io::Cursor;

use super::formats::{ArtworkFormat, PixelFormat};

/// One rendered thumbnail and where the picture sits inside it.
pub(super) struct Thumb {
    pub(super) pixels: Vec<u8>,
    /// Picture width plus the padding on its left, as the ArtworkDB stores it.
    pub(super) width: i16,
    pub(super) height: i16,
    pub(super) horizontal_padding: i16,
    pub(super) vertical_padding: i16,
}

/// JPEG or PNG. `None` for anything else or damaged data: a bad cover must not stop a sync, so the
/// track simply gets no artwork.
pub(super) fn decode(data: &[u8]) -> Option<RgbImage> {
    let image = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    Some(image.to_rgb8())
}

/// Fits the picture inside the format's rectangle without distorting it, centres it, and fills the
/// rest (and any row alignment padding) with the format's background colour.
pub(super) fn render(source: &RgbImage, format: &ArtworkFormat) -> Thumb {
    let (frame_width, frame_height) = (format.width, format.height);
    let (width_scale, height_scale) = (
        f64::from(frame_width) / f64::from(source.width()),
        f64::from(frame_height) / f64::from(source.height()),
    );
    let scaled = |length: u32, scale: f64, limit: u32| {
        ((f64::from(length) * scale).ceil() as u32).clamp(1, limit)
    };
    let (width, height) = if width_scale < height_scale {
        (
            frame_width,
            scaled(source.height(), width_scale, frame_height),
        )
    } else if width_scale > height_scale {
        (
            scaled(source.width(), height_scale, frame_width),
            frame_height,
        )
    } else {
        (frame_width, frame_height)
    };
    let picture = resize(source, width, height, FilterType::Triangle);
    let (horizontal_padding, vertical_padding) =
        ((frame_width - width) / 2, (frame_height - height) / 2);

    let [red, green, blue, _] = format.back_color;
    let background = image::Rgb([red, green, blue]);
    let mut pixels = Vec::with_capacity(format.slot_len() as usize);
    for y in 0..frame_height {
        for x in 0..format.row_pixels() {
            let inside = x >= horizontal_padding
                && x - horizontal_padding < width
                && y >= vertical_padding
                && y - vertical_padding < height;
            let pixel = if inside {
                *picture.get_pixel(x - horizontal_padding, y - vertical_padding)
            } else {
                background
            };
            pixels.extend_from_slice(&pack_565(pixel.0, format.pixels));
        }
    }

    let to_i16 = |value: u32| i16::try_from(value).unwrap_or(i16::MAX);
    Thumb {
        pixels,
        width: to_i16(horizontal_padding + width),
        height: to_i16(vertical_padding + height),
        horizontal_padding: to_i16(horizontal_padding),
        vertical_padding: to_i16(vertical_padding),
    }
}

fn pack_565([red, green, blue]: [u8; 3], pixels: PixelFormat) -> [u8; 2] {
    let packed = (u16::from(red >> 3) << 11) | (u16::from(green >> 2) << 5) | u16::from(blue >> 3);
    match pixels {
        PixelFormat::Rgb565Le => packed.to_le_bytes(),
        PixelFormat::Rgb565Be => packed.to_be_bytes(),
    }
}

#[cfg(test)]
mod tests {
    use image::{ImageFormat, Rgb};

    use super::*;

    fn format(width: u32, height: u32) -> ArtworkFormat {
        ArtworkFormat {
            format_id: 1,
            width,
            height,
            pixels: PixelFormat::Rgb565Le,
            back_color: [0, 0, 0, 0],
            row_bytes_alignment: 0,
        }
    }

    fn solid(width: u32, height: u32, color: [u8; 3]) -> RgbImage {
        RgbImage::from_pixel(width, height, Rgb(color))
    }

    fn pixel_at(thumb: &Thumb, format: &ArtworkFormat, x: u32, y: u32) -> u16 {
        let at = ((y * format.row_pixels() + x) * 2) as usize;
        u16::from_le_bytes([thumb.pixels[at], thumb.pixels[at + 1]])
    }

    #[test]
    fn a_square_picture_fills_a_square_frame() {
        let format = format(8, 8);

        let thumb = render(&solid(100, 100, [255, 0, 0]), &format);

        assert_eq!(
            (
                thumb.width,
                thumb.height,
                thumb.horizontal_padding,
                thumb.vertical_padding
            ),
            (8, 8, 0, 0)
        );
        assert_eq!(thumb.pixels.len(), format.slot_len() as usize);
        assert_eq!(
            pixel_at(&thumb, &format, 3, 3),
            0xf800,
            "pure red in RGB565"
        );
    }

    #[test]
    fn a_wide_picture_is_letterboxed_and_centred() {
        let format = format(10, 10);

        let thumb = render(&solid(40, 20, [0, 255, 0]), &format);

        // 40x20 scaled into 10x10 becomes 10x5 with 2 rows (5 / 2) of padding above.
        assert_eq!((thumb.horizontal_padding, thumb.vertical_padding), (0, 2));
        assert_eq!(
            (thumb.width, thumb.height),
            (10, 7),
            "sizes include the leading padding"
        );
        assert_eq!(
            pixel_at(&thumb, &format, 5, 0),
            0,
            "the background above the picture"
        );
        assert_eq!(pixel_at(&thumb, &format, 5, 4), 0x07e0, "pure green");
        assert_eq!(pixel_at(&thumb, &format, 5, 9), 0, "and below it");
    }

    #[test]
    fn a_tall_picture_is_pillarboxed() {
        let format = format(10, 10);

        let thumb = render(&solid(20, 40, [0, 0, 255]), &format);

        assert_eq!((thumb.horizontal_padding, thumb.vertical_padding), (2, 0));
        assert_eq!(pixel_at(&thumb, &format, 0, 5), 0);
        assert_eq!(pixel_at(&thumb, &format, 5, 5), 0x001f, "pure blue");
    }

    #[test]
    fn the_background_colour_fills_the_margins_and_the_alignment_padding() {
        let format = ArtworkFormat {
            back_color: [255, 255, 255, 0],
            row_bytes_alignment: 32,
            ..format(10, 10)
        };

        let thumb = render(&solid(40, 20, [0, 0, 0]), &format);

        assert_eq!(format.row_pixels(), 16);
        assert_eq!(thumb.pixels.len(), 16 * 10 * 2);
        assert_eq!(pixel_at(&thumb, &format, 5, 0), 0xffff, "white margin");
        assert_eq!(
            pixel_at(&thumb, &format, 15, 5),
            0xffff,
            "white alignment padding"
        );
        assert_eq!(pixel_at(&thumb, &format, 5, 5), 0, "black picture");
    }

    #[test]
    fn big_endian_formats_swap_the_bytes() {
        let format = ArtworkFormat {
            pixels: PixelFormat::Rgb565Be,
            ..format(4, 4)
        };

        let thumb = render(&solid(4, 4, [255, 0, 0]), &format);

        assert_eq!(&thumb.pixels[..2], &[0xf8, 0x00]);
    }

    #[test]
    fn jpeg_and_png_decode_and_everything_else_does_not() {
        let image = solid(6, 4, [10, 20, 30]);
        for container in [ImageFormat::Png, ImageFormat::Jpeg] {
            let mut bytes = Vec::new();
            image
                .write_to(&mut Cursor::new(&mut bytes), container)
                .unwrap();

            let decoded = decode(&bytes).unwrap();

            assert_eq!(decoded.dimensions(), (6, 4));
        }
        assert!(decode(b"GIF89a not really").is_none());
        assert!(decode(&[]).is_none());
    }
}
