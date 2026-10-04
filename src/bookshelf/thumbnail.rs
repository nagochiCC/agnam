use image::imageops::FilterType;

pub(crate) const COVER_MAX_WIDTH: u32 = 468;
pub(crate) const COVER_MAX_HEIGHT: u32 = 312;
const BYTES_PER_PIXEL: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThumbnailData {
    pub(crate) pixels: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) stride: usize,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ThumbnailGenerationError {
    #[error("未対応の表紙画像形式です")]
    UnsupportedImageFormat,
    #[error("表紙画像をデコードできませんでした: {0}")]
    Image(#[from] image::ImageError),
    #[error("JPEG画像をデコードできませんでした: {0}")]
    Jpeg(String),
    #[error("表紙画像の寸法が不正です")]
    InvalidDimensions,
}

pub(crate) fn generate_from_bytes(bytes: &[u8]) -> Result<ThumbnailData, ThumbnailGenerationError> {
    Ok(generate_from_bytes_with_cancel(bytes, &|| false)?.expect("uncancelled thumbnail"))
}

pub(crate) fn generate_from_bytes_with_cancel(
    bytes: &[u8],
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<ThumbnailData>, ThumbnailGenerationError> {
    if cancelled() {
        return Ok(None);
    }
    let source = decode_source_image(bytes)?;
    if cancelled() {
        return Ok(None);
    }
    if source.width() == 0 || source.height() == 0 {
        return Err(ThumbnailGenerationError::InvalidDimensions);
    }
    let cover = cover_from_source(source)?;
    if cancelled() {
        return Ok(None);
    }
    thumbnail_data(cover).map(Some)
}

pub(crate) fn fit_thumbnail_data(
    thumbnail: ThumbnailData,
    max_width: u32,
    max_height: u32,
) -> Result<ThumbnailData, ThumbnailGenerationError> {
    let expected_stride = thumbnail
        .width
        .checked_mul(BYTES_PER_PIXEL as u32)
        .and_then(|stride| usize::try_from(stride).ok())
        .ok_or(ThumbnailGenerationError::InvalidDimensions)?;
    let expected_len = expected_stride
        .checked_mul(
            usize::try_from(thumbnail.height)
                .map_err(|_| ThumbnailGenerationError::InvalidDimensions)?,
        )
        .ok_or(ThumbnailGenerationError::InvalidDimensions)?;
    if thumbnail.width == 0
        || thumbnail.height == 0
        || max_width == 0
        || max_height == 0
        || thumbnail.stride != expected_stride
        || thumbnail.pixels.len() != expected_len
    {
        return Err(ThumbnailGenerationError::InvalidDimensions);
    }

    let (width, height) =
        fitted_dimensions(thumbnail.width, thumbnail.height, max_width, max_height)?;
    if (width, height) == (thumbnail.width, thumbnail.height) {
        return Ok(thumbnail);
    }

    let source = image::RgbImage::from_raw(thumbnail.width, thumbnail.height, thumbnail.pixels)
        .ok_or(ThumbnailGenerationError::InvalidDimensions)?;
    thumbnail_data(image::imageops::resize(
        &source,
        width,
        height,
        FilterType::Triangle,
    ))
}

fn decode_source_image(bytes: &[u8]) -> Result<image::RgbImage, ThumbnailGenerationError> {
    let format = image::guess_format(bytes)?;
    match format {
        image::ImageFormat::Jpeg => decode_jpeg(bytes),
        image::ImageFormat::Png | image::ImageFormat::WebP => {
            Ok(image::load_from_memory_with_format(bytes, format)?.into_rgb8())
        }
        _ => Err(ThumbnailGenerationError::UnsupportedImageFormat),
    }
}

fn decode_jpeg(bytes: &[u8]) -> Result<image::RgbImage, ThumbnailGenerationError> {
    let mut decompressor = turbojpeg::Decompressor::new()
        .map_err(|error| ThumbnailGenerationError::Jpeg(error.to_string()))?;
    let header = decompressor
        .read_header(bytes)
        .map_err(|error| ThumbnailGenerationError::Jpeg(error.to_string()))?;
    if header.width == 0 || header.height == 0 {
        return Err(ThumbnailGenerationError::InvalidDimensions);
    }

    let (crop_width, crop_height) =
        center_crop_dimensions(header.width as u32, header.height as u32)?;
    let (cover_width, cover_height) =
        fitted_dimensions(crop_width, crop_height, COVER_MAX_WIDTH, COVER_MAX_HEIGHT)?;
    let scaling_factor = turbojpeg::Decompressor::supported_scaling_factors()
        .into_iter()
        .filter(|factor| factor.num() <= factor.denom())
        .filter(|factor| {
            let Ok((crop_width, crop_height)) = center_crop_dimensions(
                factor.scale(header.width) as u32,
                factor.scale(header.height) as u32,
            ) else {
                return false;
            };
            crop_width >= cover_width && crop_height >= cover_height
        })
        .min_by_key(|factor| (factor.scale(header.width), factor.scale(header.height)))
        .unwrap_or(turbojpeg::ScalingFactor::ONE);
    let scaled_header = header.scaled(scaling_factor);
    let pitch = scaled_header
        .width
        .checked_mul(turbojpeg::PixelFormat::RGB.size())
        .ok_or(ThumbnailGenerationError::InvalidDimensions)?;
    let pixels_len = pitch
        .checked_mul(scaled_header.height)
        .ok_or(ThumbnailGenerationError::InvalidDimensions)?;
    let mut decoded = turbojpeg::Image {
        pixels: vec![0; pixels_len],
        width: scaled_header.width,
        pitch,
        height: scaled_header.height,
        format: turbojpeg::PixelFormat::RGB,
    };
    decompressor
        .set_scaling_factor(scaling_factor)
        .and_then(|_| decompressor.decompress(bytes, decoded.as_deref_mut()))
        .map_err(|error| ThumbnailGenerationError::Jpeg(error.to_string()))?;

    image::RgbImage::from_raw(
        scaled_header.width as u32,
        scaled_header.height as u32,
        decoded.pixels,
    )
    .ok_or(ThumbnailGenerationError::InvalidDimensions)
}

fn cover_from_source(source: image::RgbImage) -> Result<image::RgbImage, ThumbnailGenerationError> {
    let (crop_width, crop_height) = center_crop_dimensions(source.width(), source.height())?;
    let crop_x = (source.width() - crop_width) / 2;
    let (width, height) =
        fitted_dimensions(crop_width, crop_height, COVER_MAX_WIDTH, COVER_MAX_HEIGHT)?;

    let needs_crop = crop_width != source.width();
    let needs_resize = width != crop_width || height != crop_height;
    if !needs_crop && !needs_resize {
        return Ok(source);
    }
    if needs_resize {
        let crop = image::imageops::crop_imm(&source, crop_x, 0, crop_width, crop_height);
        return Ok(image::imageops::resize(
            &*crop,
            width,
            height,
            FilterType::Triangle,
        ));
    }

    Ok(image::imageops::crop_imm(&source, crop_x, 0, crop_width, crop_height).to_image())
}

fn center_crop_dimensions(width: u32, height: u32) -> Result<(u32, u32), ThumbnailGenerationError> {
    if width == 0 || height == 0 {
        return Err(ThumbnailGenerationError::InvalidDimensions);
    }
    if u64::from(width) * 2 <= u64::from(height) * 3 {
        return Ok((width, height));
    }
    Ok((((u64::from(height) * 3) / 2) as u32, height))
}

fn fitted_dimensions(
    width: u32,
    height: u32,
    max_width: u32,
    max_height: u32,
) -> Result<(u32, u32), ThumbnailGenerationError> {
    if width == 0 || height == 0 || max_width == 0 || max_height == 0 {
        return Err(ThumbnailGenerationError::InvalidDimensions);
    }
    if width <= max_width && height <= max_height {
        return Ok((width, height));
    }

    let width_limited_height = u64::from(height) * u64::from(max_width) / u64::from(width);
    if width_limited_height <= u64::from(max_height) {
        Ok((max_width, width_limited_height.max(1) as u32))
    } else {
        let height_limited_width = u64::from(width) * u64::from(max_height) / u64::from(height);
        Ok((height_limited_width.max(1) as u32, max_height))
    }
}

fn thumbnail_data(image: image::RgbImage) -> Result<ThumbnailData, ThumbnailGenerationError> {
    let stride = image
        .width()
        .checked_mul(BYTES_PER_PIXEL as u32)
        .and_then(|stride| usize::try_from(stride).ok())
        .ok_or(ThumbnailGenerationError::InvalidDimensions)?;
    Ok(ThumbnailData {
        width: image.width(),
        height: image.height(),
        stride,
        pixels: image.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn encoded(format: image::ImageFormat, width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 251) as u8, ((x + y) % 251) as u8])
        });
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut bytes, format)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn cover_preserves_portrait_aspect_and_crops_wide_landscapes() {
        let portrait = generate_from_bytes(&encoded(image::ImageFormat::Png, 400, 800)).unwrap();
        assert_eq!((portrait.width, portrait.height), (156, 312));

        let landscape = generate_from_bytes(&encoded(image::ImageFormat::Png, 800, 400)).unwrap();
        assert_eq!((landscape.width, landscape.height), (468, 312));
        assert_eq!(
            landscape.pixels.len(),
            landscape.stride * landscape.height as usize
        );
    }

    #[test]
    fn cover_preserves_three_to_two_aspect() {
        let cover = generate_from_bytes(&encoded(image::ImageFormat::Png, 600, 400)).unwrap();
        assert_eq!((cover.width, cover.height), (468, 312));
    }

    #[test]
    fn cover_does_not_upscale_small_images() {
        let result = generate_from_bytes(&encoded(image::ImageFormat::Png, 80, 120)).unwrap();
        assert_eq!((result.width, result.height), (80, 120));

        let wide = generate_from_bytes(&encoded(image::ImageFormat::Png, 80, 40)).unwrap();
        assert_eq!((wide.width, wide.height), (60, 40));
    }

    #[test]
    fn crop_dimensions_leave_three_to_two_and_narrower_images_unchanged() {
        assert_eq!(center_crop_dimensions(600, 400).unwrap(), (600, 400));
        assert_eq!(center_crop_dimensions(400, 800).unwrap(), (400, 800));
    }

    #[test]
    fn crop_dimensions_center_crop_images_wider_than_three_to_two() {
        assert_eq!(center_crop_dimensions(800, 400).unwrap(), (600, 400));
    }

    #[test]
    fn optimized_cover_matches_crop_then_triangle_resize_pixels() {
        for (width, height) in [(400, 800), (600, 400), (800, 400), (80, 120), (80, 40)] {
            let source = image::RgbImage::from_fn(width, height, |x, y| {
                image::Rgb([
                    (x % 251) as u8,
                    (y % 251) as u8,
                    ((x.wrapping_mul(3) + y.wrapping_mul(5)) % 251) as u8,
                ])
            });
            let (crop_width, crop_height) = center_crop_dimensions(width, height).unwrap();
            let crop_x = (width - crop_width) / 2;
            let legacy_crop =
                image::imageops::crop_imm(&source, crop_x, 0, crop_width, crop_height).to_image();
            let (out_width, out_height) =
                fitted_dimensions(crop_width, crop_height, COVER_MAX_WIDTH, COVER_MAX_HEIGHT)
                    .unwrap();
            let expected = if (out_width, out_height) == (crop_width, crop_height) {
                legacy_crop
            } else {
                image::imageops::resize(&legacy_crop, out_width, out_height, FilterType::Triangle)
            };

            assert_eq!(cover_from_source(source).unwrap(), expected);
        }
    }

    #[test]
    fn png_jpeg_and_webp_are_supported() {
        for format in [
            image::ImageFormat::Png,
            image::ImageFormat::Jpeg,
            image::ImageFormat::WebP,
        ] {
            let result = generate_from_bytes(&encoded(format, 240, 360)).unwrap();
            assert!(result.width > 0);
            assert!(result.height > 0);
        }
    }

    fn thumbnail(width: u32, height: u32) -> ThumbnailData {
        thumbnail_data(image::RgbImage::new(width, height)).unwrap()
    }

    #[test]
    fn fitted_thumbnail_respects_history_and_search_bounds() {
        let history = fit_thumbnail_data(thumbnail(468, 312), 72, 100).unwrap();
        assert_eq!((history.width, history.height), (72, 48));
        assert_eq!(history.stride, history.width as usize * BYTES_PER_PIXEL);
        assert_eq!(
            history.pixels.len(),
            history.stride * history.height as usize
        );

        let search = fit_thumbnail_data(thumbnail(468, 312), 47, 66).unwrap();
        assert_eq!((search.width, search.height), (47, 31));
        assert!(search.width <= 47 && search.height <= 66);
    }

    #[test]
    fn fitted_thumbnail_preserves_portrait_and_landscape_aspects() {
        let portrait = fit_thumbnail_data(thumbnail(156, 312), 72, 100).unwrap();
        assert_eq!((portrait.width, portrait.height), (50, 100));

        let landscape = fit_thumbnail_data(thumbnail(468, 312), 72, 100).unwrap();
        assert_eq!((landscape.width, landscape.height), (72, 48));
    }

    #[test]
    fn fitted_thumbnail_does_not_upscale_and_reuses_the_original_buffer() {
        let original = thumbnail(40, 60);
        let pointer = original.pixels.as_ptr();
        let fitted = fit_thumbnail_data(original, 72, 100).unwrap();
        assert_eq!((fitted.width, fitted.height), (40, 60));
        assert_eq!(fitted.pixels.as_ptr(), pointer);
    }

    #[test]
    fn fitted_thumbnail_rejects_invalid_dimensions_and_buffers() {
        assert!(fit_thumbnail_data(thumbnail(1, 1), 0, 100).is_err());
        assert!(fit_thumbnail_data(thumbnail(1, 1), 72, 0).is_err());
        assert!(
            fit_thumbnail_data(
                ThumbnailData {
                    pixels: Vec::new(),
                    width: 0,
                    height: 1,
                    stride: 0,
                },
                72,
                100,
            )
            .is_err()
        );
        assert!(
            fit_thumbnail_data(
                ThumbnailData {
                    pixels: vec![0; 2],
                    width: 1,
                    height: 1,
                    stride: 3,
                },
                72,
                100,
            )
            .is_err()
        );
    }
}
