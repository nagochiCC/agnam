use super::smart_crop::CropRect;
use crate::document::{AssetId, Document, ImageAsset, ImageLayout};
use std::collections::BTreeMap;

// 第2層：アーカイブ展開・ディスクI/Oを回避するための圧縮画像バイトデータのキャッシュ半径
const BYTE_RADIUS: usize = 5;

fn asset_page_distance(asset: &ImageAsset, current_page: usize) -> usize {
    let last_page = match asset.layout {
        ImageLayout::Single => asset.first_page,
        ImageLayout::Spread => asset.first_page + 1,
    };

    if current_page < asset.first_page {
        asset.first_page - current_page
    } else {
        current_page.saturating_sub(last_page)
    }
}

pub(super) struct ImageCache {
    // 第1層：高解像度デコード済みGTKテクスチャ (メモリを多く消費するため、現在周辺の表示単位に絞る)
    textures: BTreeMap<usize, gtk::gdk::Paintable>,
    // 第2層：圧縮状態の画像ファイルバイト列 (メモリをほとんど消費しない: 数百KB〜数MB)
    bytes: BTreeMap<AssetId, gtk::glib::Bytes>,
    // Asset単位の軽量な判定cache。Noneも安全側no-crop結果として保持する。
    crop_results: BTreeMap<AssetId, Option<CropRect>>,
}

impl ImageCache {
    pub(super) fn new() -> Self {
        Self {
            textures: BTreeMap::new(),
            bytes: BTreeMap::new(),
            crop_results: BTreeMap::new(),
        }
    }

    pub(super) fn clear(&mut self) {
        self.textures.clear();
        self.bytes.clear();
        self.crop_results.clear();
    }

    pub(super) fn clear_textures(&mut self) {
        self.textures.clear();
    }

    pub(super) fn has_texture(&self, idx: usize) -> bool {
        self.textures.contains_key(&idx)
    }

    pub(super) fn get_texture(&self, idx: usize) -> Option<gtk::gdk::Paintable> {
        self.textures.get(&idx).cloned()
    }

    pub(super) fn insert_texture(&mut self, idx: usize, texture: gtk::gdk::Paintable) {
        self.textures.insert(idx, texture);
    }

    pub(super) fn get_bytes(&self, asset_id: AssetId) -> Option<gtk::glib::Bytes> {
        self.bytes.get(&asset_id).cloned()
    }

    pub(super) fn insert_bytes(&mut self, asset_id: AssetId, bytes: gtk::glib::Bytes) {
        self.bytes.insert(asset_id, bytes);
    }

    pub(super) fn crop_result(&self, asset_id: AssetId) -> Option<Option<CropRect>> {
        self.crop_results.get(&asset_id).copied()
    }

    pub(super) fn insert_crop_result(&mut self, asset_id: AssetId, crop: Option<CropRect>) {
        self.crop_results.insert(asset_id, crop);
    }

    pub(super) fn trim(
        &mut self,
        retained_texture_indices: &[usize],
        current_page: usize,
        document: &Document,
    ) {
        // 第1層（テクスチャ）は前・現在・次の表示単位に限定する
        self.textures
            .retain(|idx, _| retained_texture_indices.contains(idx));
        // 第2層（バイトデータ）はより広くトリムしてディスクI/O・展開コストを回避
        self.bytes.retain(|asset_id, _| {
            document
                .assets
                .get(asset_id.0)
                .is_some_and(|asset| asset_page_distance(asset, current_page) <= BYTE_RADIUS)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{ImageLayout, ImageSource};
    use gtk::prelude::Cast;
    use std::io::Cursor;

    fn png_bytes(width: u32, height: u32) -> gtk::glib::Bytes {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 0, 0, 255]));
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        gtk::glib::Bytes::from_owned(png.into_inner())
    }

    fn document_with_layouts(layouts: &[ImageLayout]) -> Document {
        let mut document = Document::new("book".into(), None);
        for &layout in layouts {
            document.add_asset(ImageSource::Memory(png_bytes(2, 2)), layout);
        }
        document
    }

    #[test]
    fn insert_lookup_and_clear_cover_both_cache_layers() {
        let texture: gtk::gdk::Paintable = gtk::gdk::Texture::from_bytes(&png_bytes(2, 2))
            .unwrap()
            .upcast();
        let bytes = gtk::glib::Bytes::from_static(b"compressed");
        let mut cache = ImageCache::new();

        cache.insert_texture(3, texture);
        cache.insert_bytes(AssetId(2), bytes);

        assert!(cache.has_texture(3));
        assert!(cache.get_texture(3).is_some());
        assert!(cache.get_bytes(AssetId(2)).is_some());
        assert_eq!(cache.get_bytes(AssetId(2)).unwrap().as_ref(), b"compressed");

        cache.clear();

        assert!(!cache.has_texture(3));
        assert!(cache.get_texture(3).is_none());
        assert!(cache.get_bytes(AssetId(2)).is_none());
    }

    #[test]
    fn clearing_textures_keeps_compressed_bytes() {
        let texture: gtk::gdk::Paintable = gtk::gdk::Texture::from_bytes(&png_bytes(2, 2))
            .unwrap()
            .upcast();
        let bytes = gtk::glib::Bytes::from_static(b"compressed");
        let mut cache = ImageCache::new();
        cache.insert_texture(0, texture);
        cache.insert_bytes(AssetId(0), bytes);

        cache.clear_textures();

        assert!(!cache.has_texture(0));
        assert_eq!(cache.get_bytes(AssetId(0)).unwrap().as_ref(), b"compressed");
    }

    #[test]
    fn clearing_textures_keeps_crop_results_but_document_clear_drops_them() {
        let mut cache = ImageCache::new();
        let asset_id = AssetId(3);
        let crop = CropRect {
            x: 0,
            y: 12,
            width: 80,
            height: 50,
        };
        cache.insert_crop_result(asset_id, Some(crop));

        cache.clear_textures();
        assert_eq!(cache.crop_result(asset_id), Some(Some(crop)));

        cache.clear();
        assert_eq!(cache.crop_result(asset_id), None);
    }

    #[test]
    fn texture_trim_keeps_exact_display_unit_indices() {
        let document = Document::new("book".into(), None);
        let texture: gtk::gdk::Paintable = gtk::gdk::Texture::from_bytes(&png_bytes(2, 2))
            .unwrap()
            .upcast();
        let mut cache = ImageCache::new();
        for page_index in 0..=12 {
            cache.insert_texture(page_index, texture.clone());
        }

        cache.trim(&[3, 4, 5, 6, 7, 8], 5, &document);

        assert!(!cache.has_texture(2));
        assert!(cache.has_texture(3));
        assert!(cache.has_texture(4));
        assert!(cache.has_texture(6));
        assert!(cache.has_texture(8));
        assert!(!cache.has_texture(9));
    }

    #[test]
    fn byte_trim_uses_asset_page_ranges_instead_of_asset_id_values() {
        let document = document_with_layouts(&[
            ImageLayout::Spread,
            ImageLayout::Spread,
            ImageLayout::Spread,
            ImageLayout::Spread,
            ImageLayout::Spread,
            ImageLayout::Spread,
            ImageLayout::Single,
        ]);
        let mut cache = ImageCache::new();
        for asset_id in [AssetId(0), AssetId(2), AssetId(3), AssetId(6)] {
            cache.insert_bytes(
                asset_id,
                gtk::glib::Bytes::from_owned(vec![asset_id.0 as u8]),
            );
        }

        cache.trim(&[], 12, &document);

        assert!(cache.get_bytes(AssetId(0)).is_none());
        assert!(cache.get_bytes(AssetId(2)).is_none());
        assert!(cache.get_bytes(AssetId(3)).is_some());
        assert!(cache.get_bytes(AssetId(6)).is_some());
    }
}
