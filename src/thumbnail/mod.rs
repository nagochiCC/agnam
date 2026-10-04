mod disk_cache;
mod schedule;
pub(crate) mod worker;

#[cfg(test)]
use crate::archive::image_loader::ThumbnailImageLoader;
#[cfg(test)]
use crate::document::ImageAsset;
use crate::document::{ImageLayout, PagePart};
use std::path::Path;

pub(crate) const THUMBNAIL_HEIGHT: u32 = 200;
const MIN_JPEG_THUMBNAIL_DECODE_HEIGHT: usize = 175;
pub(crate) const THUMBNAIL_BYTES_PER_PIXEL: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Thumbnail {
    pub(crate) pixels: Vec<u8>,
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) stride: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AssetThumbnails {
    Single(Thumbnail),
    Spread { right: Thumbnail, left: Thumbnail },
}

pub(crate) fn make_memory_asset_thumbnails(
    bytes: &gtk::glib::Bytes,
    layout: ImageLayout,
) -> Option<Vec<(usize, Thumbnail)>> {
    let mut decompressor = turbojpeg::Decompressor::new().ok()?;
    match make_asset_thumbnails_from_bytes(&mut decompressor, bytes.as_ref(), layout)? {
        AssetThumbnails::Single(thumbnail) => Some(vec![(0, thumbnail)]),
        AssetThumbnails::Spread { right, left } => Some(vec![(0, right), (1, left)]),
    }
}

pub(crate) struct ProgressiveThumbnailCacheAttempt {
    cache: disk_cache::ThumbnailDiskCache,
    entry: Option<disk_cache::CacheEntry>,
    layout: ImageLayout,
}

impl ProgressiveThumbnailCacheAttempt {
    pub(crate) fn new(document: &Path, physical_index: usize, layout: ImageLayout) -> Self {
        Self::from_cache(
            disk_cache::ThumbnailDiskCache::for_document(document),
            physical_index,
            layout,
        )
    }

    fn from_cache(
        cache: disk_cache::ThumbnailDiskCache,
        physical_index: usize,
        layout: ImageLayout,
    ) -> Self {
        let entry = cache.progressive_entry(physical_index, layout);
        Self {
            cache,
            entry,
            layout,
        }
    }

    pub(crate) fn load(&self) -> Option<Vec<(usize, Thumbnail)>> {
        match self.cache.load(self.entry.as_ref()?)? {
            AssetThumbnails::Single(thumbnail) if self.layout == ImageLayout::Single => {
                Some(vec![(0, thumbnail)])
            }
            AssetThumbnails::Spread { right, left } if self.layout == ImageLayout::Spread => {
                Some(vec![(0, right), (1, left)])
            }
            _ => None,
        }
    }

    pub(crate) fn save_later(self, thumbnails: Vec<(usize, Thumbnail)>) {
        let Some(entry) = self.entry else { return };
        let mut thumbnails = thumbnails.into_iter();
        let asset = match (
            self.layout,
            thumbnails.next(),
            thumbnails.next(),
            thumbnails.next(),
        ) {
            (ImageLayout::Single, Some((0, thumbnail)), None, None) => {
                AssetThumbnails::Single(thumbnail)
            }
            (ImageLayout::Spread, Some((0, right)), Some((1, left)), None) => {
                AssetThumbnails::Spread { right, left }
            }
            _ => return,
        };
        self.cache.save_later(entry, asset);
    }
}

#[cfg(test)]
fn make_asset_thumbnails_with_loader(
    loader: &mut ThumbnailImageLoader,
    decompressor: &mut turbojpeg::Decompressor,
    asset: &ImageAsset,
) -> Option<AssetThumbnails> {
    let bytes = loader.load_image_bytes(&asset.source)?;
    make_asset_thumbnails_from_bytes(decompressor, &bytes, asset.layout)
}

fn make_asset_thumbnails_from_bytes(
    decompressor: &mut turbojpeg::Decompressor,
    bytes: &[u8],
    layout: ImageLayout,
) -> Option<AssetThumbnails> {
    make_asset_thumbnails_from_bytes_shared(decompressor, bytes, layout, None)
}

fn make_asset_thumbnails_from_bytes_shared(
    decompressor: &mut turbojpeg::Decompressor,
    bytes: &[u8],
    layout: ImageLayout,
    shared: Option<(
        &crate::viewer::SmartCropPreparationHandle,
        u64,
        crate::document::AssetId,
    )>,
) -> Option<AssetThumbnails> {
    let format = image::guess_format(bytes).ok()?;
    let image = match format {
        image::ImageFormat::Jpeg => decode_jpeg_at_thumbnail_scale(decompressor, bytes)?,
        image::ImageFormat::Png | image::ImageFormat::WebP => {
            if let Some((handle, generation, asset_id)) = shared {
                let decoded = handle.decode_png_webp(generation, asset_id, bytes)?;
                thumbnail_scale_image(&decoded)?
            } else {
                decode_image_at_thumbnail_scale(bytes, format)?
            }
        }
        _ => return None,
    };
    match layout {
        ImageLayout::Single => Some(AssetThumbnails::Single(thumbnail_from_rgb_image(image)?)),
        ImageLayout::Spread => Some(AssetThumbnails::Spread {
            right: thumbnail_for_part(&image, PagePart::Right)?,
            left: thumbnail_for_part(&image, PagePart::Left)?,
        }),
    }
}

fn decode_jpeg_at_thumbnail_scale(
    decompressor: &mut turbojpeg::Decompressor,
    bytes: &[u8],
) -> Option<image::RgbImage> {
    let header = decompressor.read_header(bytes).ok()?;
    if header.width == 0 || header.height == 0 {
        return None;
    }

    let target_width = header
        .width
        .checked_mul(THUMBNAIL_HEIGHT as usize)?
        .checked_div(header.height)?
        .max(1);
    let target_height = THUMBNAIL_HEIGHT as usize;
    let scaling_factor =
        select_thumbnail_scaling_factor(header.width, header.height, target_width, target_height)?;
    let scaled_header = header.scaled(scaling_factor);
    let pitch = scaled_header
        .width
        .checked_mul(turbojpeg::PixelFormat::RGB.size())?;
    let pixels_len = pitch.checked_mul(scaled_header.height)?;
    let mut image = turbojpeg::Image {
        pixels: vec![0; pixels_len],
        width: scaled_header.width,
        pitch,
        height: scaled_header.height,
        format: turbojpeg::PixelFormat::RGB,
    };

    decompressor.set_scaling_factor(scaling_factor).ok()?;
    decompressor.decompress(bytes, image.as_deref_mut()).ok()?;

    let image = image::RgbImage::from_raw(
        scaled_header.width.try_into().ok()?,
        scaled_header.height.try_into().ok()?,
        image.pixels,
    )?;

    resize_to_thumbnail_height(image)
}

fn decode_image_at_thumbnail_scale(
    bytes: &[u8],
    format: image::ImageFormat,
) -> Option<image::RgbImage> {
    let image = image::load_from_memory_with_format(bytes, format).ok()?;
    thumbnail_scale_image(&image)
}

fn thumbnail_scale_image(image: &image::DynamicImage) -> Option<image::RgbImage> {
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 {
        return None;
    }

    let image = if height > THUMBNAIL_HEIGHT {
        let target_width = u64::from(width)
            .checked_mul(u64::from(THUMBNAIL_HEIGHT))?
            .checked_div(u64::from(height))?
            .max(1)
            .try_into()
            .ok()?;

        image.thumbnail_exact(target_width, THUMBNAIL_HEIGHT)
    } else {
        return Some(image.to_rgb8());
    };

    Some(image.into_rgb8())
}

fn resize_to_thumbnail_height(image: image::RgbImage) -> Option<image::RgbImage> {
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 {
        return None;
    }
    if height <= THUMBNAIL_HEIGHT {
        return Some(image);
    }

    let target_width = u64::from(width)
        .checked_mul(u64::from(THUMBNAIL_HEIGHT))?
        .checked_div(u64::from(height))?
        .max(1)
        .try_into()
        .ok()?;

    Some(image::imageops::resize(
        &image,
        target_width,
        THUMBNAIL_HEIGHT,
        image::imageops::FilterType::Triangle,
    ))
}

fn select_thumbnail_scaling_factor(
    width: usize,
    height: usize,
    target_width: usize,
    target_height: usize,
) -> Option<turbojpeg::ScalingFactor> {
    if width <= target_width && height <= target_height {
        return Some(turbojpeg::ScalingFactor::ONE);
    }

    turbojpeg::Decompressor::supported_scaling_factors()
        .into_iter()
        .filter(|factor| factor.num() <= factor.denom())
        .filter(|factor| {
            let meets_target =
                factor.scale(width) >= target_width && factor.scale(height) >= target_height;
            let acceptable_eighth = *factor == turbojpeg::ScalingFactor::ONE_EIGHTH
                && factor.scale(height) >= MIN_JPEG_THUMBNAIL_DECODE_HEIGHT;
            meets_target || acceptable_eighth
        })
        .min_by_key(|factor| (factor.scale(width), factor.scale(height)))
}

fn thumbnail_for_part(image: &image::RgbImage, part: PagePart) -> Option<Thumbnail> {
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 {
        return None;
    }

    let image = match part {
        PagePart::Whole => image.clone(),
        PagePart::Right => {
            image::imageops::crop_imm(image, width / 2, 0, width / 2, height).to_image()
        }
        PagePart::Left => image::imageops::crop_imm(image, 0, 0, width / 2, height).to_image(),
    };

    thumbnail_from_rgb_image(image)
}

fn thumbnail_from_rgb_image(image: image::RgbImage) -> Option<Thumbnail> {
    let stride = (image.width() as usize).checked_mul(THUMBNAIL_BYTES_PER_PIXEL)?;
    Some(Thumbnail {
        width: image.width().try_into().ok()?,
        height: image.height().try_into().ok()?,
        stride,
        pixels: image.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{ImageLayout, ImageSource};
    use std::io::{Cursor, Write};

    fn image_asset(source: ImageSource, layout: ImageLayout) -> ImageAsset {
        ImageAsset {
            source,
            first_page: 0,
            layout,
            archive_identity: None,
        }
    }

    fn reference_thumbnail_for_part(image: &image::RgbImage, part: PagePart) -> Option<Thumbnail> {
        let width = image.width();
        let height = image.height();
        if width == 0 || height == 0 {
            return None;
        }

        let image = image::DynamicImage::ImageRgb8(image.clone());
        let image = match part {
            PagePart::Whole => image,
            PagePart::Right => image.crop_imm(width / 2, 0, width / 2, height),
            PagePart::Left => image.crop_imm(0, 0, width / 2, height),
        };
        let image = image.into_rgb8();

        Some(Thumbnail {
            width: image.width().try_into().ok()?,
            height: image.height().try_into().ok()?,
            stride: image.width() as usize * THUMBNAIL_BYTES_PER_PIXEL,
            pixels: image.into_raw(),
        })
    }

    fn assert_same_thumbnail(actual: &Thumbnail, expected: &Thumbnail) {
        assert_eq!(actual.width, expected.width);
        assert_eq!(actual.height, expected.height);
        assert_eq!(actual.stride, expected.stride);
        assert_eq!(actual.pixels, expected.pixels);
    }

    fn encoded_image(image: image::DynamicImage, format: image::ImageFormat) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, format).unwrap();
        bytes.into_inner()
    }

    fn single_thumbnail(bytes: &[u8]) -> Thumbnail {
        let mut decompressor = turbojpeg::Decompressor::new().unwrap();
        let AssetThumbnails::Single(thumbnail) =
            make_asset_thumbnails_from_bytes(&mut decompressor, bytes, ImageLayout::Single)
                .unwrap()
        else {
            panic!("single image should produce one thumbnail");
        };
        thumbnail
    }

    #[test]
    fn rgb_conversion_preserves_pixels_for_all_parts() {
        for width in [4, 5] {
            let image = image::RgbImage::from_fn(width, 3, |x, y| {
                image::Rgb([(x * 31) as u8, (y * 47) as u8, (x + y) as u8])
            });

            for part in [PagePart::Whole, PagePart::Right, PagePart::Left] {
                let expected = reference_thumbnail_for_part(&image, part).unwrap();
                let actual = thumbnail_for_part(&image, part).unwrap();
                assert_same_thumbnail(&actual, &expected);
            }
        }
    }

    #[test]
    fn split_parts_keep_existing_orientation_and_odd_width_crop() {
        let image = image::RgbImage::from_fn(5, 2, |x, _| image::Rgb([((x + 1) * 10) as u8, 0, 0]));

        let right = thumbnail_for_part(&image, PagePart::Right).unwrap();
        let left = thumbnail_for_part(&image, PagePart::Left).unwrap();
        let right_red = right
            .pixels
            .chunks_exact(THUMBNAIL_BYTES_PER_PIXEL)
            .map(|pixel| pixel[0])
            .collect::<Vec<_>>();
        let left_red = left
            .pixels
            .chunks_exact(THUMBNAIL_BYTES_PER_PIXEL)
            .map(|pixel| pixel[0])
            .collect::<Vec<_>>();

        assert_eq!((right.width, right.height), (2, 2));
        assert_eq!((left.width, left.height), (2, 2));
        assert_eq!(right_red, vec![30, 40, 30, 40]);
        assert_eq!(left_red, vec![10, 20, 10, 20]);
    }

    #[test]
    fn png_and_webp_thumbnails_use_the_target_height_and_integer_width() {
        for format in [image::ImageFormat::Png, image::ImageFormat::WebP] {
            let bytes = encoded_image(
                image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                    121,
                    301,
                    image::Rgb([32, 64, 128]),
                )),
                format,
            );
            let thumbnail = single_thumbnail(&bytes);

            assert_eq!((thumbnail.width, thumbnail.height), (80, 200));
            assert_eq!(thumbnail.stride, 80 * THUMBNAIL_BYTES_PER_PIXEL);
            assert_eq!(
                thumbnail.pixels.len(),
                thumbnail.stride * thumbnail.height as usize
            );

            let narrow_bytes = encoded_image(
                image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                    1,
                    401,
                    image::Rgb([32, 64, 128]),
                )),
                format,
            );
            let narrow_thumbnail = single_thumbnail(&narrow_bytes);
            assert_eq!((narrow_thumbnail.width, narrow_thumbnail.height), (1, 200));
        }
    }

    #[test]
    fn png_and_webp_thumbnails_do_not_upscale_small_images() {
        for format in [image::ImageFormat::Png, image::ImageFormat::WebP] {
            let bytes = encoded_image(
                image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                    123,
                    199,
                    image::Rgb([32, 64, 128]),
                )),
                format,
            );
            let thumbnail = single_thumbnail(&bytes);

            assert_eq!((thumbnail.width, thumbnail.height), (123, 199));
            assert_eq!(thumbnail.stride, 123 * THUMBNAIL_BYTES_PER_PIXEL);
            assert_eq!(
                thumbnail.pixels.len(),
                thumbnail.stride * thumbnail.height as usize
            );
        }
    }

    #[test]
    fn png_color_formats_convert_to_rgb_after_resizing() {
        let images = [
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                121,
                301,
                image::Rgb([32, 64, 128]),
            )),
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                121,
                301,
                image::Rgba([32, 64, 128, 192]),
            )),
            image::DynamicImage::ImageLuma8(image::GrayImage::from_pixel(
                121,
                301,
                image::Luma([96]),
            )),
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                123,
                199,
                image::Rgb([32, 64, 128]),
            )),
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                123,
                200,
                image::Rgba([32, 64, 128, 192]),
            )),
            image::DynamicImage::ImageLuma8(image::GrayImage::from_pixel(
                123,
                150,
                image::Luma([96]),
            )),
        ];

        for image in images {
            let expected_width = if image.height() > THUMBNAIL_HEIGHT {
                image.width() * THUMBNAIL_HEIGHT / image.height()
            } else {
                image.width()
            };
            let expected_height = image.height().min(THUMBNAIL_HEIGHT);
            let thumbnail = single_thumbnail(&encoded_image(image, image::ImageFormat::Png));

            assert_eq!(
                (thumbnail.width, thumbnail.height),
                (expected_width as i32, expected_height as i32)
            );
            assert_eq!(
                thumbnail.stride,
                expected_width as usize * THUMBNAIL_BYTES_PER_PIXEL
            );
            assert_eq!(
                thumbnail.pixels.len(),
                thumbnail.stride * thumbnail.height as usize
            );
        }
    }

    #[test]
    fn spread_png_keeps_orientation_odd_width_crop_and_rgb_contract() {
        let image = image::RgbImage::from_fn(14, 300, |x, _| image::Rgb([(x * 16) as u8, 0, 0]));
        let bytes = encoded_image(
            image::DynamicImage::ImageRgb8(image),
            image::ImageFormat::Png,
        );
        let mut decompressor = turbojpeg::Decompressor::new().unwrap();
        let AssetThumbnails::Spread { right, left } =
            make_asset_thumbnails_from_bytes(&mut decompressor, &bytes, ImageLayout::Spread)
                .unwrap()
        else {
            panic!("spread image should produce right and left thumbnails");
        };

        assert_eq!((right.width, right.height, right.stride), (4, 200, 12));
        assert_eq!((left.width, left.height, left.stride), (4, 200, 12));
        assert_eq!(right.width + left.width, 9 - 1);
        assert_eq!(right.pixels.len(), right.stride * right.height as usize);
        assert_eq!(left.pixels.len(), left.stride * left.height as usize);
        let right_red = right
            .pixels
            .chunks_exact(THUMBNAIL_BYTES_PER_PIXEL)
            .take(right.width as usize)
            .map(|pixel| pixel[0]);
        let left_red = left
            .pixels
            .chunks_exact(THUMBNAIL_BYTES_PER_PIXEL)
            .take(left.width as usize)
            .map(|pixel| pixel[0]);
        assert!(right_red.zip(left_red).all(|(right, left)| right > left));
    }

    #[test]
    fn invalid_png_and_webp_data_is_rejected_without_panicking() {
        for (format, bytes) in [
            (image::ImageFormat::Png, &b"\x89PNG\r\n\x1a\n"[..]),
            (image::ImageFormat::WebP, &b"RIFF\x04\0\0\0WEBP"[..]),
        ] {
            let result =
                std::panic::catch_unwind(|| decode_image_at_thumbnail_scale(bytes, format));
            assert!(result.unwrap().is_none());
        }
    }

    #[test]
    fn rejects_unsupported_and_invalid_image_data() {
        let mut bmp = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            4,
            4,
            image::Rgb([32, 64, 128]),
        ))
        .write_to(&mut bmp, image::ImageFormat::Bmp)
        .unwrap();

        let mut decompressor = turbojpeg::Decompressor::new().unwrap();
        assert!(
            make_asset_thumbnails_from_bytes(
                &mut decompressor,
                bmp.get_ref(),
                ImageLayout::Single,
            )
            .is_none()
        );
        assert!(
            make_asset_thumbnails_from_bytes(
                &mut decompressor,
                &[0xff, 0xd8, 0xff, 0xe0, 0, 1],
                ImageLayout::Single,
            )
            .is_none()
        );
    }

    #[test]
    fn limits_thumbnail_height_after_scaled_jpeg_decode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("page.jpg");
        image::RgbImage::from_pixel(800, 1200, image::Rgb([32, 64, 128]))
            .save_with_format(&path, image::ImageFormat::Jpeg)
            .unwrap();

        let asset = image_asset(ImageSource::File(path.clone()), ImageLayout::Single);
        let mut loader = ThumbnailImageLoader::new();
        let mut decompressor = turbojpeg::Decompressor::new().unwrap();
        let AssetThumbnails::Single(thumbnail) =
            make_asset_thumbnails_with_loader(&mut loader, &mut decompressor, &asset).unwrap()
        else {
            panic!("single asset should produce one thumbnail");
        };
        let bytes = std::fs::read(&path).unwrap();
        let decoded = decode_jpeg_at_thumbnail_scale(&mut decompressor, &bytes).unwrap();
        assert_eq!(thumbnail.width, decoded.width() as i32);
        assert_eq!(thumbnail.height, decoded.height() as i32);
        assert_eq!((thumbnail.width, thumbnail.height), (133, 200));
        assert_eq!(thumbnail.height, THUMBNAIL_HEIGHT as i32);
        assert!(thumbnail.width > 0);
        assert!(thumbnail.height > 0);
        assert_eq!(
            thumbnail.stride,
            thumbnail.width as usize * THUMBNAIL_BYTES_PER_PIXEL
        );
        assert_eq!(
            thumbnail.pixels.len(),
            thumbnail.stride * thumbnail.height as usize
        );

        let spread_asset = image_asset(ImageSource::File(path), ImageLayout::Spread);
        let AssetThumbnails::Spread {
            right: right_thumbnail,
            left: left_thumbnail,
        } = make_asset_thumbnails_with_loader(&mut loader, &mut decompressor, &spread_asset)
            .unwrap()
        else {
            panic!("spread asset should produce right and left thumbnails");
        };
        assert!(right_thumbnail.width + left_thumbnail.width <= decoded.width() as i32);
        assert!(decoded.width() as i32 - right_thumbnail.width - left_thumbnail.width <= 1);
        assert_eq!(right_thumbnail.height, decoded.height() as i32);
        assert_eq!(left_thumbnail.height, decoded.height() as i32);
        assert_eq!(
            right_thumbnail.pixels.len(),
            right_thumbnail.stride * right_thumbnail.height as usize
        );
        assert_eq!(
            left_thumbnail.pixels.len(),
            left_thumbnail.stride * left_thumbnail.height as usize
        );
    }

    #[test]
    fn allows_one_eighth_scale_at_the_minimum_jpeg_decode_height() {
        let factor_for_height = |height| {
            let width = 1000;
            let target_width = width * THUMBNAIL_HEIGHT as usize / height;
            select_thumbnail_scaling_factor(width, height, target_width, THUMBNAIL_HEIGHT as usize)
                .unwrap()
        };

        for height in [1600, 1580, 1420, 1400] {
            assert_eq!(
                factor_for_height(height),
                turbojpeg::ScalingFactor::ONE_EIGHTH
            );
        }

        // TurboJPEG rounds scaled dimensions up, so 1399px also yields 175px at 1/8.
        assert_eq!(
            turbojpeg::ScalingFactor::ONE_EIGHTH.scale(1399),
            MIN_JPEG_THUMBNAIL_DECODE_HEIGHT
        );
        assert_eq!(
            factor_for_height(1399),
            turbojpeg::ScalingFactor::ONE_EIGHTH
        );
        assert_eq!(turbojpeg::ScalingFactor::ONE_EIGHTH.scale(1392), 174);
        assert_eq!(
            factor_for_height(1392),
            turbojpeg::ScalingFactor::ONE_QUARTER
        );
        assert_eq!(
            factor_for_height(1200),
            turbojpeg::ScalingFactor::ONE_QUARTER
        );
    }

    #[test]
    fn chooses_the_smallest_scale_that_meets_thumbnail_height_without_upscaling() {
        let eighth =
            select_thumbnail_scaling_factor(1365, 2048, 133, THUMBNAIL_HEIGHT as usize).unwrap();
        assert_eq!((eighth.scale(1365), eighth.scale(2048)), (171, 256));

        let quarter =
            select_thumbnail_scaling_factor(800, 1200, 133, THUMBNAIL_HEIGHT as usize).unwrap();
        assert_eq!((quarter.scale(800), quarter.scale(1200)), (200, 300));

        let original =
            select_thumbnail_scaling_factor(100, 150, 133, THUMBNAIL_HEIGHT as usize).unwrap();
        assert_eq!((original.scale(100), original.scale(150)), (100, 150));
    }

    #[test]
    fn creates_split_thumbnails_from_a_zip_entry() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("pages.cbz");
        let mut jpeg = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            1600,
            800,
            image::Rgb([32, 64, 128]),
        ))
        .write_to(&mut jpeg, image::ImageFormat::Jpeg)
        .unwrap();

        let file = std::fs::File::create(&archive_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("spread.jpg", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&jpeg.into_inner()).unwrap();
        writer.finish().unwrap();

        let asset = image_asset(
            ImageSource::ArchiveEntry {
                archive_path,
                entry_index: 0,
                entry_name: "spread.jpg".into(),
            },
            ImageLayout::Spread,
        );
        let mut loader = ThumbnailImageLoader::new();
        let mut decompressor = turbojpeg::Decompressor::new().unwrap();
        let AssetThumbnails::Spread {
            right: right_thumbnail,
            left: left_thumbnail,
        } = make_asset_thumbnails_with_loader(&mut loader, &mut decompressor, &asset).unwrap()
        else {
            panic!("spread asset should produce right and left thumbnails");
        };
        assert!(right_thumbnail.width > 0);
        assert!(right_thumbnail.height > 0);
        assert_eq!(right_thumbnail.width, left_thumbnail.width);
        assert_eq!(right_thumbnail.height, left_thumbnail.height);
        assert_eq!(
            right_thumbnail.pixels.len(),
            right_thumbnail.stride * right_thumbnail.height as usize
        );
        assert_eq!(
            left_thumbnail.pixels.len(),
            left_thumbnail.stride * left_thumbnail.height as usize
        );
    }

    #[test]
    fn reusable_decompressor_handles_a_valid_jpeg_after_an_error() {
        let mut jpeg = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            800,
            1200,
            image::Rgb([32, 64, 128]),
        ))
        .write_to(&mut jpeg, image::ImageFormat::Jpeg)
        .unwrap();

        let mut decompressor = turbojpeg::Decompressor::new().unwrap();
        assert!(decode_jpeg_at_thumbnail_scale(&mut decompressor, &[0xff, 0xd8]).is_none());
        assert!(decode_jpeg_at_thumbnail_scale(&mut decompressor, jpeg.get_ref()).is_some());
    }

    #[test]
    fn jpeg_final_resize_keeps_the_triangle_filter() {
        let image = image::RgbImage::from_fn(121, 301, |x, y| {
            image::Rgb([(x * 7) as u8, (y * 11) as u8, (x + y) as u8])
        });
        let expected = image::imageops::resize(
            &image,
            80,
            THUMBNAIL_HEIGHT,
            image::imageops::FilterType::Triangle,
        );

        assert_eq!(resize_to_thumbnail_height(image).unwrap(), expected);
    }

    #[test]
    fn memory_pipeline_returns_single_and_spread_page_offsets() {
        let bytes = gtk::glib::Bytes::from_owned(encoded_image(
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                8,
                4,
                image::Rgb([32, 64, 128]),
            )),
            image::ImageFormat::Png,
        ));

        let single = make_memory_asset_thumbnails(&bytes, ImageLayout::Single).unwrap();
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].0, 0);

        let spread = make_memory_asset_thumbnails(&bytes, ImageLayout::Spread).unwrap();
        assert_eq!(spread.len(), 2);
        assert_eq!(
            spread.iter().map(|(offset, _)| *offset).collect::<Vec<_>>(),
            [0, 1]
        );
    }

    #[test]
    fn progressive_cache_uses_the_same_spread_pixels_after_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("book.cbr");
        std::fs::write(&archive, b"archive source").unwrap();
        let cache = disk_cache::ThumbnailDiskCache::new(temp.path().join("cache"), &archive);
        let image = gtk::glib::Bytes::from_owned(encoded_image(
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                5,
                2,
                image::Rgb([32, 64, 128]),
            )),
            image::ImageFormat::Png,
        ));
        let first =
            ProgressiveThumbnailCacheAttempt::from_cache(cache.clone(), 0, ImageLayout::Spread);
        assert!(first.load().is_none());
        let generated = make_memory_asset_thumbnails(&image, ImageLayout::Spread).unwrap();
        first.save_later(generated.clone());
        cache.flush_writes();
        let reopened =
            ProgressiveThumbnailCacheAttempt::from_cache(cache.clone(), 0, ImageLayout::Spread);
        assert_eq!(reopened.load(), Some(generated));
        assert!(
            ProgressiveThumbnailCacheAttempt::from_cache(cache, 1, ImageLayout::Spread)
                .load()
                .is_none()
        );
    }
}
