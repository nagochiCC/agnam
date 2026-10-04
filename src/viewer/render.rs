use super::page_paintable::PagePaintable;
use super::smart_crop::{self, CropRect};
use crate::document::ImageLayout;
use gtk::prelude::*;

const RGBA_CHANNEL_COUNT: usize = 4;

#[cfg(test)]
thread_local! {
    pub(crate) static ANALYSIS_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static CPU_DECODE_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(super) static DOWNLOAD_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(super) enum RenderedAsset {
    Single(gtk::gdk::Paintable),
    Spread {
        right: gtk::gdk::Paintable,
        left: gtk::gdk::Paintable,
    },
}

/// Only the lightweight analysis survives the CPU worker; decoded pixels are
/// released before publication and never become Viewer texture backing.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CropAnalysis {
    pub(crate) layout: ImageLayout,
    pub(crate) crop_result: Option<CropRect>,
}

pub(super) fn analyze_smart_crop_asset(bytes: &[u8], layout: ImageLayout) -> Option<CropAnalysis> {
    analyze_smart_crop_decoded_cancellable(decode_rgba(bytes)?, layout, || false).ok()
}

pub(super) fn analyze_smart_crop_decoded_cancellable(
    (pixels, width, height): (Vec<u8>, usize, usize),
    layout: ImageLayout,
    cancelled: impl FnMut() -> bool,
) -> Result<CropAnalysis, ()> {
    analyze_rgba_cancellable(&pixels, width, height, layout, cancelled)
}

pub(super) fn analyze_decoded_image_cancellable(
    decoded: &image::DynamicImage,
    layout: ImageLayout,
    cancelled: impl FnMut() -> bool,
) -> Result<CropAnalysis, ()> {
    let rgba;
    let pixels = if let Some(pixels) = decoded.as_rgba8() {
        pixels
    } else {
        rgba = decoded.to_rgba8();
        &rgba
    };
    analyze_rgba_cancellable(
        pixels.as_raw(),
        pixels.width() as usize,
        pixels.height() as usize,
        layout,
        cancelled,
    )
}

pub(super) fn analyze_rgba_cancellable(
    pixels: &[u8],
    width: usize,
    height: usize,
    layout: ImageLayout,
    mut cancelled: impl FnMut() -> bool,
) -> Result<CropAnalysis, ()> {
    #[cfg(test)]
    ANALYSIS_COUNT.set(ANALYSIS_COUNT.get() + 1);
    let stride = width.checked_mul(RGBA_CHANNEL_COUNT).ok_or(())?;
    let crop_result = match smart_crop::detect_rgba_cancellable(
        pixels,
        stride,
        width,
        height,
        layout == ImageLayout::Spread,
        &mut cancelled,
    ) {
        smart_crop::DetectResult::Complete(crop) => crop,
        smart_crop::DetectResult::Cancelled => return Err(()),
    };
    if cancelled() {
        return Err(());
    }
    Ok(CropAnalysis {
        layout,
        crop_result,
    })
}

/// Compatibility fallback for a file accepted by GDK but rejected by the CPU
/// decoder. Download only for uncached analysis; keep the original texture.
pub(super) fn analyze_texture(
    texture: &gtk::gdk::Texture,
    layout: ImageLayout,
) -> Option<CropAnalysis> {
    let (pixels, stride, width, height) = download_rgba(texture)?;
    Some(CropAnalysis {
        layout,
        crop_result: smart_crop::detect_rgba(
            &pixels,
            stride,
            width,
            height,
            layout == ImageLayout::Spread,
        ),
    })
}

pub(super) fn render_asset(
    texture: gtk::gdk::Texture,
    layout: ImageLayout,
    crop: Option<CropRect>,
) -> Option<RenderedAsset> {
    let width = usize::try_from(texture.width()).ok()?;
    let height = usize::try_from(texture.height()).ok()?;
    let crop = crop.unwrap_or_else(|| CropRect::full(width, height));
    // Only vertical crop is valid; splitting always uses the physical center.
    if crop.x != 0
        || crop.width != width
        || crop.height == 0
        || crop.y.checked_add(crop.height)? > height
    {
        return None;
    }
    match layout {
        ImageLayout::Single if crop == CropRect::full(width, height) => {
            Some(RenderedAsset::Single(texture.upcast()))
        }
        ImageLayout::Single => Some(RenderedAsset::Single(
            PagePaintable::new(&texture, crop).upcast(),
        )),
        ImageLayout::Spread => {
            let left_width = width / 2;
            if left_width == 0 {
                return None;
            }
            Some(RenderedAsset::Spread {
                right: PagePaintable::new(
                    &texture,
                    CropRect {
                        x: left_width,
                        width: width - left_width,
                        ..crop
                    },
                )
                .upcast(),
                left: PagePaintable::new(
                    &texture,
                    CropRect {
                        width: left_width,
                        ..crop
                    },
                )
                .upcast(),
            })
        }
    }
}

pub(super) fn decode_rgba(bytes: &[u8]) -> Option<(Vec<u8>, usize, usize)> {
    #[cfg(test)]
    CPU_DECODE_COUNT.set(CPU_DECODE_COUNT.get() + 1);
    let format = image::guess_format(bytes).ok()?;
    match format {
        image::ImageFormat::Jpeg => decode_jpeg_rgba(bytes),
        image::ImageFormat::Png | image::ImageFormat::WebP => {
            let image = image::load_from_memory_with_format(bytes, format)
                .ok()?
                .into_rgba8();
            let width = usize::try_from(image.width()).ok()?;
            let height = usize::try_from(image.height()).ok()?;
            (width > 0 && height > 0).then(|| (image.into_raw(), width, height))
        }
        _ => None,
    }
}

fn decode_jpeg_rgba(bytes: &[u8]) -> Option<(Vec<u8>, usize, usize)> {
    let mut decompressor = turbojpeg::Decompressor::new().ok()?;
    let header = decompressor.read_header(bytes).ok()?;
    if header.width == 0 || header.height == 0 {
        return None;
    }
    let pitch = header
        .width
        .checked_mul(turbojpeg::PixelFormat::RGBA.size())?;
    let pixels_len = pitch.checked_mul(header.height)?;
    let mut image = turbojpeg::Image {
        pixels: vec![0; pixels_len],
        width: header.width,
        pitch,
        height: header.height,
        format: turbojpeg::PixelFormat::RGBA,
    };
    decompressor.decompress(bytes, image.as_deref_mut()).ok()?;
    Some((image.pixels, image.width, image.height))
}

fn download_rgba(texture: &gtk::gdk::Texture) -> Option<(gtk::glib::Bytes, usize, usize, usize)> {
    #[cfg(test)]
    DOWNLOAD_COUNT.set(DOWNLOAD_COUNT.get() + 1);
    let width = usize::try_from(texture.width()).ok()?;
    let height = usize::try_from(texture.height()).ok()?;
    let mut downloader = gtk::gdk::TextureDownloader::new(texture);
    downloader.set_format(gtk::gdk::MemoryFormat::R8g8b8a8);
    let (bytes, stride) = downloader.download_bytes();
    Some((bytes, stride, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn encoded_bytes(image: image::DynamicImage, format: image::ImageFormat) -> gtk::glib::Bytes {
        let mut output = Cursor::new(Vec::new());
        image.write_to(&mut output, format).unwrap();
        gtk::glib::Bytes::from_owned(output.into_inner())
    }

    fn bordered_bytes(width: u32) -> gtk::glib::Bytes {
        encoded_bytes(
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(width, 70, |x, y| {
                if (8..width - 15).contains(&x) && (12..57).contains(&y) {
                    image::Rgba([x as u8, 0, 0, 255])
                } else {
                    image::Rgba([255, 255, 255, 255])
                }
            })),
            image::ImageFormat::Png,
        )
    }

    #[test]
    fn single_off_and_cached_no_crop_keep_the_normal_texture() {
        let texture = gtk::gdk::Texture::from_bytes(&bordered_bytes(80)).unwrap();
        let RenderedAsset::Single(page) =
            render_asset(texture.clone(), ImageLayout::Single, None).unwrap()
        else {
            panic!()
        };
        assert_eq!(page, texture.clone().upcast::<gtk::gdk::Paintable>());
        assert_eq!((page.intrinsic_width(), page.intrinsic_height()), (80, 70));
    }

    #[test]
    fn single_crop_keeps_source_texture_and_reports_cropped_size() {
        let bytes = bordered_bytes(80);
        let crop = analyze_smart_crop_asset(&bytes, ImageLayout::Single)
            .unwrap()
            .crop_result
            .unwrap();
        let texture = gtk::gdk::Texture::from_bytes(&bytes).unwrap();
        let RenderedAsset::Single(page) =
            render_asset(texture.clone(), ImageLayout::Single, Some(crop)).unwrap()
        else {
            panic!()
        };
        let page = page.downcast::<PagePaintable>().unwrap();
        assert_eq!(page.region(), crop);
        assert_eq!(page.texture(), texture);
        assert_eq!(page.intrinsic_width(), 80);
        assert_eq!(page.intrinsic_height(), crop.height as i32);
        assert!(crop.height < 70);
    }

    #[test]
    fn spread_off_and_on_share_the_original_texture_and_physical_center() {
        for width in [80, 81] {
            let bytes = bordered_bytes(width);
            let texture = gtk::gdk::Texture::from_bytes(&bytes).unwrap();
            let detected = analyze_smart_crop_asset(&bytes, ImageLayout::Spread)
                .unwrap()
                .crop_result;
            assert!(detected.is_some());
            for crop in [None, detected] {
                let RenderedAsset::Spread { right, left } =
                    render_asset(texture.clone(), ImageLayout::Spread, crop).unwrap()
                else {
                    panic!()
                };
                let right = right.downcast::<PagePaintable>().unwrap();
                let left = left.downcast::<PagePaintable>().unwrap();
                assert_eq!(right.texture(), texture);
                assert_eq!(left.texture(), texture);
                assert_eq!(right.region().x, width as usize / 2);
                assert_eq!(left.region().x, 0);
                assert_eq!(
                    right.intrinsic_width() + left.intrinsic_width(),
                    width as i32
                );
                assert_eq!(right.region().y, left.region().y);
                assert_eq!(
                    right.intrinsic_height(),
                    crop.map_or(70, |crop| crop.height as i32)
                );
            }
        }
    }

    #[test]
    fn invalid_crops_and_unsplittable_spreads_are_rejected() {
        let texture = gtk::gdk::Texture::from_bytes(&bordered_bytes(80)).unwrap();
        for crop in [
            CropRect {
                x: 1,
                ..CropRect::full(80, 70)
            },
            CropRect::full(79, 70),
            CropRect::full(80, 0),
            CropRect {
                y: 1,
                ..CropRect::full(80, 70)
            },
            CropRect {
                y: usize::MAX,
                height: 2,
                ..CropRect::full(80, 70)
            },
        ] {
            for layout in [ImageLayout::Single, ImageLayout::Spread] {
                assert!(render_asset(texture.clone(), layout, Some(crop)).is_none());
            }
        }
        let narrow = encoded_bytes(
            image::DynamicImage::new_rgba8(1, 2),
            image::ImageFormat::Png,
        );
        let texture = gtk::gdk::Texture::from_bytes(&narrow).unwrap();
        assert!(render_asset(texture, ImageLayout::Spread, None).is_none());
    }

    #[test]
    fn gdk_only_decoder_fallback_keeps_the_original_texture_and_caches_analysis() {
        let bytes = encoded_bytes(
            image::DynamicImage::new_rgb8(80, 70),
            image::ImageFormat::Bmp,
        );
        assert!(analyze_smart_crop_asset(&bytes, ImageLayout::Single).is_none());
        let texture = gtk::gdk::Texture::from_bytes(&bytes).unwrap();
        let analysis = analyze_texture(&texture, ImageLayout::Single).unwrap();
        let RenderedAsset::Single(page) =
            render_asset(texture.clone(), analysis.layout, analysis.crop_result).unwrap()
        else {
            panic!()
        };
        assert_eq!(page, texture.upcast::<gtk::gdk::Paintable>());

        let mut document = crate::document::Document::new("book".into(), None);
        let asset = document.add_asset(
            crate::document::ImageSource::Memory(bytes),
            ImageLayout::Single,
        );
        let mut session = crate::viewer::ViewerSession::new(crate::viewer::ViewMode::Single);
        session.replace_document(document, 0);
        let downloads = DOWNLOAD_COUNT.get();
        assert!(
            session
                .current_textures(true)
                .0
                .unwrap()
                .is::<gtk::gdk::Texture>()
        );
        assert_eq!(DOWNLOAD_COUNT.get(), downloads + 1);
        assert_eq!(session.cached_crop_result(asset), Some(None));
        let counts = (CPU_DECODE_COUNT.get(), DOWNLOAD_COUNT.get());
        session.clear_textures();
        assert!(
            session
                .current_textures(true)
                .0
                .unwrap()
                .is::<gtk::gdk::Texture>()
        );
        assert_eq!((CPU_DECODE_COUNT.get(), DOWNLOAD_COUNT.get()), counts);
    }

    #[test]
    fn cpu_decode_supports_png_jpeg_and_webp() {
        let rgba = image::RgbaImage::from_fn(17, 13, |x, y| {
            image::Rgba([x as u8 * 7, y as u8 * 9, (x + y) as u8, 200])
        });
        for format in [
            image::ImageFormat::Png,
            image::ImageFormat::Jpeg,
            image::ImageFormat::WebP,
        ] {
            let bytes = encoded_bytes(image::DynamicImage::ImageRgba8(rgba.clone()), format);
            let (pixels, width, height) = decode_rgba(bytes.as_ref()).unwrap();
            assert_eq!((width, height), (17, 13));
            assert_eq!(pixels.len(), 17 * 13 * RGBA_CHANNEL_COUNT);
            if format == image::ImageFormat::Jpeg {
                assert!(pixels.chunks_exact(4).all(|pixel| pixel[3] == 255));
            }
        }
    }

    #[test]
    fn cpu_decode_keeps_crop_decisions_equal_to_the_previous_gtk_download_path() {
        let rgba = image::RgbaImage::from_fn(320, 480, |x, y| {
            if (35..285).contains(&x) && (60..420).contains(&y) {
                image::Rgba([
                    ((x * 13 + y * 3) % 90) as u8,
                    ((x * 7 + y * 5) % 90) as u8,
                    ((x + y * 11) % 90) as u8,
                    255,
                ])
            } else {
                image::Rgba([248, 246, 243, 255])
            }
        });
        for format in [
            image::ImageFormat::Png,
            image::ImageFormat::Jpeg,
            image::ImageFormat::WebP,
        ] {
            let bytes = encoded_bytes(image::DynamicImage::ImageRgba8(rgba.clone()), format);
            let (cpu_pixels, cpu_width, cpu_height) = decode_rgba(bytes.as_ref()).unwrap();
            let gtk_texture = gtk::gdk::Texture::from_bytes(&bytes).unwrap();
            let (gtk_pixels, gtk_stride, gtk_width, gtk_height) =
                download_rgba(&gtk_texture).unwrap();

            assert_eq!((cpu_width, cpu_height), (gtk_width, gtk_height));
            assert_eq!(
                smart_crop::detect_rgba(
                    &cpu_pixels,
                    cpu_width * RGBA_CHANNEL_COUNT,
                    cpu_width,
                    cpu_height,
                    false,
                ),
                smart_crop::detect_rgba(&gtk_pixels, gtk_stride, gtk_width, gtk_height, false,),
                "format={format:?}"
            );
        }
    }
}
