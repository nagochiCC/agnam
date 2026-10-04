use gtk::glib;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct AssetId(pub(crate) usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PagePart {
    Whole,
    Right,
    Left,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageLayout {
    Single,
    Spread,
}

impl ImageLayout {
    pub(crate) const fn from_is_wide(is_wide: bool) -> Self {
        if is_wide { Self::Spread } else { Self::Single }
    }

    const fn page_count(self) -> usize {
        match self {
            Self::Single => 1,
            Self::Spread => 2,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ImageSource {
    File(PathBuf),
    ArchiveEntry {
        archive_path: PathBuf,
        entry_index: usize,
        entry_name: String,
    },
    Memory(glib::Bytes),
}

impl ImageSource {
    pub(crate) fn as_file_path(&self) -> Option<&Path> {
        match self {
            Self::File(path) => Some(path),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ImageAsset {
    pub(crate) source: ImageSource,
    pub(crate) first_page: usize,
    pub(crate) layout: ImageLayout,
    pub(crate) archive_identity: Option<ArchiveAssetIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveAssetIdentity {
    pub(crate) archives: Vec<PathBuf>,
    pub(crate) image: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Page {
    pub(crate) asset_id: AssetId,
    pub(crate) part: PagePart,
}

#[derive(Debug)]
pub(crate) struct Document {
    pub(crate) path: PathBuf,
    pub(crate) assets: Vec<ImageAsset>,
    pub(crate) pages: Vec<Page>,
    // 所有している間、nested archiveとspilled Sequential archiveの一時fileを存続させる。
    #[allow(dead_code)]
    pub(crate) temp_dir: Option<Arc<tempfile::TempDir>>,
}

impl Document {
    pub(crate) fn new(path: PathBuf, temp_dir: Option<Arc<tempfile::TempDir>>) -> Self {
        Self {
            path,
            assets: Vec::new(),
            pages: Vec::new(),
            temp_dir,
        }
    }

    pub(crate) fn add_asset(&mut self, source: ImageSource, layout: ImageLayout) -> AssetId {
        self.add_asset_with_archive_identity(source, layout, None)
    }

    pub(crate) fn add_archive_asset(
        &mut self,
        source: ImageSource,
        layout: ImageLayout,
        archives: Vec<PathBuf>,
        image: PathBuf,
    ) -> AssetId {
        self.add_asset_with_archive_identity(
            source,
            layout,
            Some(ArchiveAssetIdentity { archives, image }),
        )
    }

    fn add_asset_with_archive_identity(
        &mut self,
        source: ImageSource,
        layout: ImageLayout,
        archive_identity: Option<ArchiveAssetIdentity>,
    ) -> AssetId {
        let asset_id = AssetId(self.assets.len());
        let first_page = self.pages.len();
        self.assets.push(ImageAsset {
            source,
            first_page,
            layout,
            archive_identity,
        });

        match layout {
            ImageLayout::Single => self.pages.push(Page {
                asset_id,
                part: PagePart::Whole,
            }),
            ImageLayout::Spread => {
                self.pages.push(Page {
                    asset_id,
                    part: PagePart::Right,
                });
                self.pages.push(Page {
                    asset_id,
                    part: PagePart::Left,
                });
            }
        }
        asset_id
    }

    pub(crate) fn first_page_for_archive_image(
        &self,
        archives: &[PathBuf],
        image: &Path,
    ) -> Option<usize> {
        self.assets.iter().find_map(|asset| {
            asset
                .archive_identity
                .as_ref()
                .filter(|identity| identity.archives == archives && identity.image == image)
                .map(|_| asset.first_page)
        })
    }

    pub(crate) fn asset_for_page(&self, page_index: usize) -> Option<&ImageAsset> {
        let page = self.pages.get(page_index)?;
        let asset = self.assets.get(page.asset_id.0)?;
        debug_assert!(page_index >= asset.first_page);
        debug_assert!(page_index < asset.first_page + asset.layout.page_count());
        Some(asset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_image_creates_one_asset_and_one_whole_page() {
        let mut document = Document::new(PathBuf::from("book"), None);
        let asset_id = document.add_asset(
            ImageSource::File(PathBuf::from("page.jpg")),
            ImageLayout::Single,
        );

        assert_eq!(document.assets.len(), 1);
        assert_eq!(document.pages.len(), 1);
        assert_eq!(document.assets[0].first_page, 0);
        assert_eq!(document.assets[0].layout, ImageLayout::Single);
        assert_eq!(document.pages[0].asset_id, asset_id);
        assert_eq!(document.pages[0].part, PagePart::Whole);
    }

    #[test]
    fn spread_image_creates_right_then_left_pages_for_the_same_asset() {
        let mut document = Document::new(PathBuf::from("book"), None);
        let asset_id = document.add_asset(
            ImageSource::File(PathBuf::from("spread.jpg")),
            ImageLayout::Spread,
        );

        assert_eq!(document.assets.len(), 1);
        assert_eq!(document.pages.len(), 2);
        assert_eq!(document.assets[0].layout, ImageLayout::Spread);
        assert_eq!(document.pages[0].asset_id, asset_id);
        assert_eq!(document.pages[1].asset_id, asset_id);
        assert_eq!(document.pages[0].part, PagePart::Right);
        assert_eq!(document.pages[1].part, PagePart::Left);
    }

    #[test]
    fn different_images_receive_different_asset_ids_and_keep_page_order() {
        let mut document = Document::new(PathBuf::from("book"), None);
        let first = document.add_asset(
            ImageSource::File(PathBuf::from("1.jpg")),
            ImageLayout::Single,
        );
        let second = document.add_asset(
            ImageSource::File(PathBuf::from("2.jpg")),
            ImageLayout::Spread,
        );

        assert_ne!(first, second);
        assert_eq!(document.assets[0].first_page, 0);
        assert_eq!(document.assets[1].first_page, 1);
        assert_eq!(
            document.pages,
            vec![
                Page {
                    asset_id: first,
                    part: PagePart::Whole,
                },
                Page {
                    asset_id: second,
                    part: PagePart::Right,
                },
                Page {
                    asset_id: second,
                    part: PagePart::Left,
                },
            ]
        );
    }
}
