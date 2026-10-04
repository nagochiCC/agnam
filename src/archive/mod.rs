mod access;
mod content;
mod cover;
mod format;
mod formats;
pub(crate) mod image_loader;
mod loader;
pub(crate) mod navigation;
mod nested;
mod probe_cache;
mod resource;
mod safety;

#[cfg(test)]
pub(crate) use probe_cache::{backend_probe_count, track_backend_probes_for};

use crate::error::AppError;
pub(crate) use access::{
    ArchiveAccessStrategy, archive_access_strategy, archive_supports_sequential_progress,
};
pub(crate) use content::{
    ArchiveContentItemKind, ArchiveContentLevel, ArchiveEntryReader, ArchiveLocation,
    cover_only_paths,
};
pub(crate) use cover::{
    ArchiveEntryLoadError, ArchiveImageId, cover_image_in_folder, load_archive_entry_bytes,
    load_cover_source_bytes,
};
pub(crate) use format::ArchiveFormat;
pub(crate) use loader::{
    first_direct_image_path, is_image_document_entry_path, load_document_from_path,
    load_document_from_path_with_sequential_progress, stream_sequential_archive_images,
};
pub(crate) use resource::ResourceLimitKind;
pub(crate) use safety::is_normal_relative_path;
use std::path::{Path, PathBuf};

use crate::document::{ImageLayout, ImageSource};
use gtk::glib;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug)]
struct SequentialImageBackingState {
    source: ImageSource,
    temp_dir: Option<Arc<tempfile::TempDir>>,
}

#[derive(Debug, Clone)]
pub(crate) struct SequentialImageBacking(Arc<std::sync::Mutex<SequentialImageBackingState>>);

impl SequentialImageBacking {
    pub(crate) fn memory(bytes: glib::Bytes) -> Self {
        Self(Arc::new(std::sync::Mutex::new(
            SequentialImageBackingState {
                source: ImageSource::Memory(bytes),
                temp_dir: None,
            },
        )))
    }

    pub(crate) fn source(&self) -> ImageSource {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .source
            .clone()
    }

    pub(crate) fn temp_dir(&self) -> Option<Arc<tempfile::TempDir>> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .temp_dir
            .clone()
    }

    pub(crate) fn replace_with_file(&self, path: PathBuf, temp_dir: Arc<tempfile::TempDir>) {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        state.source = ImageSource::File(path);
        state.temp_dir = Some(temp_dir);
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ProgressiveArchiveCancelToken {
    cancelled: Arc<AtomicBool>,
}

impl ProgressiveArchiveCancelToken {
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

#[derive(Debug)]
pub(crate) enum ProgressiveArchiveLoadOutcome<T> {
    Complete(T),
    Cancelled,
}

#[derive(Debug, Clone)]
pub(crate) struct ProgressiveArchiveImage {
    pub(crate) physical_index: usize,
    pub(crate) total_physical_images: usize,
    pub(crate) backing: SequentialImageBacking,
    pub(crate) layout: ImageLayout,
}

/// One decoded entry from a sequential archive contents scan. Unlike
/// `ProgressiveArchiveImage`, this carries no Viewer layout or retained image
/// collection; callers consume the bytes before the callback returns.
#[derive(Debug, Clone)]
pub(crate) struct SequentialArchiveImage {
    pub(crate) natural_index: usize,
    pub(crate) total_images: usize,
    pub(crate) bytes: glib::Bytes,
}

const IMAGE_DIMENSION_PROBE_SIZE: usize = 8192;
pub(crate) const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

pub(crate) fn sequential_image_path(directory: &Path, physical_index: usize) -> PathBuf {
    directory.join(format!("image-{physical_index:020}.bin"))
}

pub(crate) fn is_image_ext(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

pub(crate) fn is_cover_image(path: &Path) -> bool {
    is_image_ext(path)
        && path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem.eq_ignore_ascii_case("cover"))
}

fn is_macos_metadata(entry_name: &str) -> bool {
    is_macos_metadata_path(Path::new(entry_name))
}

fn is_macos_metadata_path(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "__MACOSX")
        || path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("._"))
}

fn is_wide_from_bytes(buffer: &[u8]) -> bool {
    let cursor = std::io::Cursor::new(buffer);
    image::ImageReader::new(cursor)
        .with_guessed_format()
        .ok()
        .and_then(|reader| reader.into_dimensions().ok())
        .is_some_and(|(width, height)| width > height)
}

fn is_archive_ext(path: &Path) -> bool {
    ArchiveFormat::from_path(path).is_some()
}

pub(crate) fn is_viewable_path(path: &Path) -> bool {
    is_image_ext(path) || is_archive_ext(path)
}

pub(crate) fn is_bookshelf_viewable_path(path: &Path) -> Result<bool, AppError> {
    if !is_viewable_path(path) {
        return Ok(false);
    }
    if is_image_ext(path) {
        return Ok(true);
    }

    match ArchiveFormat::from_path(path) {
        Some(format) => probe_cache::probe_viewable_content(path, || {
            formats::has_viewable_content(format, path)
        }),
        None => Ok(false),
    }
}

fn is_viewable_archive_entry(entry_name: &str) -> bool {
    !is_macos_metadata(entry_name)
        && (is_image_ext(Path::new(entry_name)) || is_archive_ext(Path::new(entry_name)))
}

pub(crate) fn is_ignored_filesystem_path(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        || is_macos_metadata_path(path)
}

fn sort_archive_entry_names(names: &mut [String]) {
    names.sort_by(|left, right| natord::compare(left, right));
}

#[derive(Eq, PartialEq)]
struct NaturalPath(Vec<String>);

impl Ord for NaturalPath {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .iter()
            .zip(&other.0)
            .find_map(|(left, right)| {
                let ordering = natord::compare(left, right);
                (ordering != std::cmp::Ordering::Equal).then_some(ordering)
            })
            .unwrap_or_else(|| self.0.len().cmp(&other.0.len()))
    }
}

impl PartialOrd for NaturalPath {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

pub(crate) fn sort_paths_naturally(paths: &mut [PathBuf]) {
    paths.sort_by_cached_key(|path| {
        NaturalPath(
            path.components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect(),
        )
    });
}

fn first_image_entry_name(mut names: Vec<String>) -> Option<String> {
    names.retain(|name| !is_macos_metadata(name) && is_image_ext(Path::new(name)));
    sort_archive_entry_names(&mut names);
    names.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_all_image_extensions_case_insensitively() {
        for &extension in IMAGE_EXTENSIONS {
            assert!(is_image_ext(Path::new(&format!("page.{extension}"))));
            assert!(is_image_ext(Path::new(&format!(
                "page.{}",
                extension.to_uppercase()
            ))));
        }
        assert!(!is_image_ext(Path::new("page.gif")));
        assert!(!is_image_ext(Path::new("page")));
    }

    #[test]
    fn recognizes_cover_stem_only_for_supported_images() {
        for name in ["cover.jpg", "Cover.jpeg", "COVER.PNG", "CoVeR.webp"] {
            assert!(is_cover_image(Path::new(name)));
        }
        for name in ["cover-art.jpg", "mycover.jpg", "cover.gif", "cover"] {
            assert!(!is_cover_image(Path::new(name)));
        }
    }

    #[test]
    fn recognizes_every_directly_viewable_format() {
        for extension in IMAGE_EXTENSIONS
            .iter()
            .copied()
            .chain(ArchiveFormat::all_extensions())
        {
            assert!(is_viewable_path(Path::new(&format!("book.{extension}"))));
            assert!(is_viewable_path(Path::new(&format!(
                "book.{}",
                extension.to_uppercase()
            ))));
        }
        assert!(!is_viewable_path(Path::new("book.txt")));
    }

    #[test]
    fn archive_entry_probe_accepts_images_and_supported_nested_archives_only() {
        for extension in IMAGE_EXTENSIONS
            .iter()
            .copied()
            .chain(ArchiveFormat::all_extensions())
        {
            assert!(is_viewable_archive_entry(&format!("content.{extension}")));
        }
        for name in ["book.epub", "setup.exe", "memo.txt"] {
            assert!(!is_viewable_archive_entry(name));
        }
        assert!(!is_viewable_archive_entry("__MACOSX/cover.jpg"));
        assert!(!is_viewable_archive_entry("._cover.jpg"));
    }

    #[test]
    fn first_image_entry_uses_natural_order_and_ignores_metadata_and_non_images() {
        let first = first_image_entry_name(vec![
            "10.jpg".into(),
            "memo.txt".into(),
            "__MACOSX/0.jpg".into(),
            "._0.jpg".into(),
            "2.png".into(),
            "1.webp".into(),
        ]);

        assert_eq!(first.as_deref(), Some("1.webp"));
    }
}
