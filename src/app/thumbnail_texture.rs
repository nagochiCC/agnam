use crate::bookshelf::thumbnail::ThumbnailData;
use gtk::prelude::*;

const THUMBNAIL_MEMORY_FORMAT: gtk::gdk::MemoryFormat = gtk::gdk::MemoryFormat::R8g8b8;

pub(super) fn texture_from_thumbnail(thumbnail: ThumbnailData) -> Option<gtk::gdk::Texture> {
    let width = i32::try_from(thumbnail.width).ok()?;
    let height = i32::try_from(thumbnail.height).ok()?;
    let expected_stride = usize::try_from(thumbnail.width.checked_mul(3)?).ok()?;
    let expected_length = thumbnail.stride.checked_mul(thumbnail.height as usize)?;
    if width <= 0
        || height <= 0
        || thumbnail.stride != expected_stride
        || thumbnail.pixels.len() != expected_length
    {
        return None;
    }
    let bytes = gtk::glib::Bytes::from_owned(thumbnail.pixels);
    Some(
        gtk::gdk::MemoryTexture::new(
            width,
            height,
            THUMBNAIL_MEMORY_FORMAT,
            &bytes,
            thumbnail.stride,
        )
        .upcast(),
    )
}
