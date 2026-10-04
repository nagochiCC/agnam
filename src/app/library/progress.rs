use crate::favorites::FavoriteIdentity;
use crate::history::HistoryEntry;
use std::path::Path;

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FolderPreviewGeometry {
    pub(super) tab_height: i32,
    pub(super) tab_width: i32,
    pub(super) shoulder_width: i32,
    pub(super) body_top: i32,
    pub(super) horizontal_inset: i32,
    pub(super) vertical_inset: i32,
    pub(super) cover_height: i32,
    pub(super) portrait_width: i32,
    pub(super) landscape_width: i32,
    pub(super) landscape_height: i32,
    pub(super) cover_top: i32,
    pub(super) front_overlap: i32,
    pub(super) front_top: i32,
}

#[cfg(test)]
impl FolderPreviewGeometry {
    pub(super) fn new(height: i32, width: i32) -> Self {
        let tab_height = (height / 9).clamp(8, 20);
        let body_top = tab_height;
        // Content insets are height-derived and kept tighter than the outer
        // folder sizing allowance so the representative cover reads larger.
        let horizontal_inset = (height / 20).clamp(6, 18);
        let vertical_inset = horizontal_inset;
        let cover_height = (height - body_top - vertical_inset * 2).max(1);
        let portrait_width = (cover_height * 2 / 3).max(1);
        let landscape_width = (cover_height * 3 / 2)
            .min((width - horizontal_inset * 2).max(1))
            .max(1);
        let landscape_height = (landscape_width * 2 / 3).max(1);
        let cover_top = body_top + vertical_inset;
        let front_overlap = (height / 7).clamp(10, 15);
        let front_top = (height - vertical_inset - front_overlap).max(0);
        Self {
            tab_height,
            tab_width: (width * 9 / 20).clamp(28, width - 12),
            shoulder_width: tab_height,
            body_top,
            horizontal_inset,
            vertical_inset,
            cover_height,
            portrait_width,
            landscape_width,
            landscape_height,
            cover_top,
            front_overlap,
            front_top,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct BookshelfDisplaySizes {
    pub(super) folder_preview_height: i32,
    pub(super) book_height: i32,
}

impl BookshelfDisplaySizes {
    pub(in crate::app) fn new(book_height: i32) -> Self {
        let folder_outer_height = folder_outer_height_for_cover_height(book_height);
        Self {
            folder_preview_height: folder_outer_height,
            book_height,
        }
    }
}

pub(super) fn folder_outer_height_for_cover_height(cover_height: i32) -> i32 {
    for outer_height in cover_height..=cover_height + 64 {
        let tab_height = (outer_height / 9).clamp(8, 20);
        let inset = (outer_height / 20).clamp(6, 18);
        if outer_height - tab_height - inset * 2 == cover_height {
            return outer_height;
        }
    }
    unreachable!("folder cover height is within the supported settings range")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct LibraryProgress {
    page_index: usize,
    current_page: usize,
    page_count: usize,
}

impl LibraryProgress {
    pub(super) fn new(page_index: usize, page_count: usize) -> Option<Self> {
        if page_count == 0 || page_index >= page_count {
            return None;
        }
        Some(Self {
            page_index,
            current_page: page_index + 1,
            page_count,
        })
    }

    pub(in crate::app) fn page_index(self) -> usize {
        self.page_index
    }

    pub(super) fn fraction(self) -> f64 {
        (self.current_page as f64 / self.page_count as f64).clamp(0.0, 1.0)
    }
}

pub(super) fn library_badge_progress(
    history_entries: &[HistoryEntry],
    identity: &FavoriteIdentity,
) -> Option<LibraryProgress> {
    favorite_badge_progress(history_entries, identity)
}

pub(in crate::app) fn library_progress(
    history_entries: &[HistoryEntry],
    path: &Path,
) -> Option<LibraryProgress> {
    latest_matching_progress(history_entries, |document_path| document_path == path)
}

pub(in crate::app) fn favorite_progress(
    history_entries: &[HistoryEntry],
    identity: &FavoriteIdentity,
) -> Option<LibraryProgress> {
    match identity {
        FavoriteIdentity::FileDocument(path) => library_progress(history_entries, path),
        FavoriteIdentity::ImageFolderDocument(folder) => {
            image_folder_progress(history_entries, folder)
        }
    }
}

fn favorite_badge_progress(
    history_entries: &[HistoryEntry],
    identity: &FavoriteIdentity,
) -> Option<LibraryProgress> {
    match identity {
        FavoriteIdentity::FileDocument(path) => {
            latest_matching_progress(history_entries, |document_path| document_path == path)
        }
        FavoriteIdentity::ImageFolderDocument(folder) => {
            latest_matching_progress(history_entries, |document_path| {
                document_path.parent() == Some(folder.as_path())
            })
        }
    }
}

pub(in crate::app) fn image_folder_progress(
    history_entries: &[HistoryEntry],
    folder: &Path,
) -> Option<LibraryProgress> {
    latest_matching_progress(history_entries, |document_path| {
        document_path.parent() == Some(folder)
    })
}

fn latest_matching_progress(
    history_entries: &[HistoryEntry],
    matches_document: impl Fn(&Path) -> bool,
) -> Option<LibraryProgress> {
    let latest_entry = latest_matching_entry(history_entries, matches_document)?;
    if latest_entry.at_document_end {
        return None;
    }
    LibraryProgress::new(latest_entry.page_index, latest_entry.page_count)
}

fn latest_matching_entry<'a>(
    history_entries: &'a [HistoryEntry],
    matches_document: impl Fn(&Path) -> bool,
) -> Option<&'a HistoryEntry> {
    let matching_entry = history_entries
        .iter()
        .find(|entry| matches_document(&entry.last_document_path))?;
    let latest_entry = history_entries
        .iter()
        .find(|entry| entry.identity == matching_entry.identity)?;
    if !matches_document(&latest_entry.last_document_path) {
        return None;
    }
    Some(latest_entry)
}
