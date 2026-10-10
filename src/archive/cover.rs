use super::format::ArchiveFormat;
use super::formats;
use super::nested::images_from_extracted_root;
use super::{
    is_cover_image, is_ignored_filesystem_path, is_image_document_entry_path, is_image_ext,
    is_normal_relative_path, sort_paths_naturally,
};
use crate::error::AppError;
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ArchiveImageId {
    pub(crate) archives: Vec<PathBuf>,
    pub(crate) image: PathBuf,
}

impl ArchiveImageId {
    pub(crate) fn display_path(&self) -> PathBuf {
        self.archives
            .iter()
            .chain(std::iter::once(&self.image))
            .fold(PathBuf::new(), |path, component| path.join(component))
    }

    pub(crate) fn is_valid(&self) -> bool {
        self.archives
            .iter()
            .all(|path| is_normal_relative_path(path))
            && is_normal_relative_path(&self.image)
            && is_image_ext(&self.image)
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ArchiveEntryLoadError {
    #[error("指定された表紙画像が見つかりません")]
    Missing,
    #[error(transparent)]
    Other(#[from] AppError),
}

/// Returns the bytes of the image used as a bookshelf cover source.
///
/// Normal archives select a root/wrapper cover from metadata, then read only
/// that entry or the naturally first supported image. Nested archives reuse
/// the bounded, safe temporary extraction path used by Viewer.
#[cfg(test)]
pub(crate) fn load_cover_source_bytes(
    limit: crate::archive::ArchiveExpansionLimit,
    path: &Path,
) -> Result<Vec<u8>, AppError> {
    load_cover_source_bytes_with_cancel(limit, path, &|| false)
}

pub(crate) fn load_cover_source_bytes_with_cancel(
    limit: crate::archive::ArchiveExpansionLimit,
    path: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<u8>, AppError> {
    if cancelled() {
        return Err(AppError::ArchiveCancelled);
    }
    if is_image_ext(path) {
        return Ok(std::fs::read(path)?);
    }

    let Some(format) = ArchiveFormat::from_path(path) else {
        return Err(AppError::Archive(
            "表紙を取得できないファイル形式です".to_string(),
        ));
    };

    let mut budget = crate::archive::resource::ResourceBudget::for_expansion_limit(limit);
    budget.set_cancel_check(cancelled);
    if let Some(bytes) = formats::automatic_cover_hint_bytes(path, &mut budget)? {
        return Ok(bytes);
    }
    let has_nested_archives = formats::has_nested_archives(format, path);
    if !has_nested_archives {
        return formats::first_image_bytes(path, &mut budget)?
            .ok_or_else(|| AppError::Archive("アーカイブに対応画像がありません".to_string()));
    }

    let temp_directory = tempfile::Builder::new()
        .prefix("agnam-bookshelf-cover-")
        .tempdir()?;
    formats::extract_to_dir_with_budget(path, temp_directory.path(), &mut budget)?;

    // A cover at the archive's own level (or its transparent wrapper) is a
    // cover hint.  Do this before expanding nested archives, so a nested
    // archive's cover can never be promoted to its parent.
    if let Some(cover) = automatic_archive_cover_path(temp_directory.path())? {
        return Ok(std::fs::read(cover)?);
    }

    let images = images_from_extracted_root(temp_directory.path(), &mut budget)?;
    let first = images
        .first()
        .ok_or_else(|| AppError::Archive("アーカイブに対応画像がありません".to_string()))?;
    Ok(std::fs::read(&first.physical_path)?)
}

/// Returns the automatic cover for an image book.  Unlike the viewer entry
/// point, `cover.*` is deliberately included and wins over all other images.
pub(crate) fn cover_image_in_folder(folder: &Path) -> Option<std::path::PathBuf> {
    let mut first_cover: Option<PathBuf> = None;
    let mut first_image: Option<PathBuf> = None;
    for entry in std::fs::read_dir(folder).ok()?.filter_map(Result::ok) {
        if !entry.file_type().is_ok_and(|file_type| file_type.is_file()) {
            continue;
        }
        let path = entry.path();
        let first = if is_cover_image(&path) {
            &mut first_cover
        } else if is_image_document_entry_path(&path) {
            &mut first_image
        } else {
            continue;
        };
        if first.as_ref().is_none_or(|current| {
            natord::compare(
                &path.file_name().unwrap().to_string_lossy(),
                &current.file_name().unwrap().to_string_lossy(),
            ) == Ordering::Less
        }) {
            *first = Some(path);
        }
    }
    first_cover.or(first_image)
}

/// Loads a stable relative candidate from an archive.  This intentionally
/// uses the existing bounded nested extraction path; only the relative entry
/// identifier is persisted, never its temporary extraction location.
#[cfg(test)]
pub(crate) fn load_archive_entry_bytes(
    limit: crate::archive::ArchiveExpansionLimit,
    archive: &Path,
    id: &ArchiveImageId,
) -> Result<Vec<u8>, ArchiveEntryLoadError> {
    load_archive_entry_bytes_with_cancel(limit, archive, id, &|| false)
}

pub(crate) fn load_archive_entry_bytes_with_cancel(
    limit: crate::archive::ArchiveExpansionLimit,
    archive: &Path,
    id: &ArchiveImageId,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<u8>, ArchiveEntryLoadError> {
    if cancelled() {
        return Err(AppError::ArchiveCancelled.into());
    }
    if !id.is_valid() || !archive.is_file() {
        return Err(ArchiveEntryLoadError::Missing);
    }
    let temporary = tempfile::Builder::new()
        .prefix("agnam-cover-entry-")
        .tempdir()
        .map_err(AppError::from)?;
    let mut current_archive = archive.to_path_buf();
    let mut budget = crate::archive::resource::ResourceBudget::for_expansion_limit(limit);
    budget.set_cancel_check(cancelled);
    for (depth, nested_entry) in id.archives.iter().enumerate() {
        if depth > 10 {
            return Err(
                AppError::Archive("アーカイブの階層が深すぎます（最大10階層まで）".into()).into(),
            );
        }
        let destination = temporary.path().join(format!("level-{depth}"));
        let nested = formats::materialize_nested_entry(
            &current_archive,
            nested_entry,
            &destination,
            &mut budget,
        )?
        .ok_or(ArchiveEntryLoadError::Missing)?;
        if !nested.is_file() || ArchiveFormat::from_path(&nested).is_none() {
            return Err(ArchiveEntryLoadError::Missing);
        }
        current_archive = nested;
    }
    match formats::cover_entry_bytes(&current_archive, &id.image, &mut budget)? {
        formats::CoverEntryBytes::Bytes(bytes) => return Ok(bytes),
        formats::CoverEntryBytes::Missing => return Err(ArchiveEntryLoadError::Missing),
        formats::CoverEntryBytes::Unsupported => {}
    }
    let destination = temporary.path().join("image-level");
    std::fs::create_dir_all(&destination).map_err(AppError::from)?;
    formats::extract_to_dir_with_budget(&current_archive, &destination, &mut budget)?;
    let image = destination.join(&id.image);
    if !image.is_file() || !is_image_ext(&image) {
        return Err(ArchiveEntryLoadError::Missing);
    }
    std::fs::read(image)
        .map_err(AppError::from)
        .map_err(ArchiveEntryLoadError::from)
}

fn automatic_archive_cover_path(root: &Path) -> Result<Option<std::path::PathBuf>, AppError> {
    let mut level = root.to_path_buf();
    loop {
        let entries = std::fs::read_dir(&level)?
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        let mut covers = entries
            .iter()
            .map(|entry| entry.path())
            .filter(|path| is_cover_image(path))
            .collect::<Vec<_>>();
        sort_paths_naturally(&mut covers);
        if let Some(cover) = covers.into_iter().next() {
            return Ok(Some(cover));
        }

        let meaningful = entries
            .into_iter()
            .filter(|entry| !is_ignored_filesystem_path(&entry.path()))
            .collect::<Vec<_>>();
        let directories = meaningful
            .iter()
            .filter(|entry| entry.file_type().ok().is_some_and(|kind| kind.is_dir()))
            .collect::<Vec<_>>();
        let has_file = meaningful
            .iter()
            .any(|entry| entry.file_type().ok().is_some_and(|kind| kind.is_file()));
        if has_file || directories.len() != 1 {
            return Ok(None);
        }
        level = directories[0].path();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn png(red: u8) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 3, image::Rgb([red, 0, 0])))
            .write_to(&mut output, image::ImageFormat::Png)
            .unwrap();
        output.into_inner()
    }

    #[test]
    fn missing_nested_cover_candidate_is_distinct_from_corrupt_payload() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("outer.rar");
        let id = ArchiveImageId {
            archives: vec![PathBuf::from("inner.rar")],
            image: PathBuf::from("page.png"),
        };
        std::fs::write(
            &path,
            super::super::formats::rar_native::stored_rar(&[("page.png", b"image")]),
        )
        .unwrap();
        assert!(matches!(
            load_archive_entry_bytes(Default::default(), &path, &id),
            Err(ArchiveEntryLoadError::Missing)
        ));
        std::fs::write(
            &path,
            super::super::formats::rar_native::stored_rar(&[("inner.rar", b"invalid")]),
        )
        .unwrap();
        assert!(matches!(
            load_archive_entry_bytes(Default::default(), &path, &id),
            Err(ArchiveEntryLoadError::Other(AppError::Archive(_)))
        ));
    }

    #[test]
    fn cover_generation_cancel_probe_reaches_nested_materialization() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("outer.rar");
        let inner = super::super::formats::rar_native::stored_rar(&[("page.png", b"image")]);
        std::fs::write(
            &path,
            super::super::formats::rar_native::stored_rar(&[("inner.rar", &inner)]),
        )
        .unwrap();
        let id = ArchiveImageId {
            archives: vec![PathBuf::from("inner.rar")],
            image: PathBuf::from("page.png"),
        };
        let calls = std::cell::Cell::new(0);
        assert_eq!(
            load_archive_entry_bytes_with_cancel(Default::default(), &path, &id, &|| {
                calls.set(calls.get() + 1);
                false
            })
            .unwrap(),
            b"image"
        );
        let stop_at = calls.get() / 2;
        assert!(stop_at > 1);
        calls.set(0);
        assert!(matches!(
            load_archive_entry_bytes_with_cancel(Default::default(), &path, &id, &|| {
                calls.set(calls.get() + 1);
                calls.get() >= stop_at
            }),
            Err(ArchiveEntryLoadError::Other(AppError::ArchiveCancelled))
        ));
        assert_eq!(calls.get(), stop_at);
    }

    #[test]
    fn nested_archive_uses_the_first_image_in_viewer_order() {
        let nested_cursor = Cursor::new(Vec::new());
        let mut nested = zip::ZipWriter::new(nested_cursor);
        nested
            .start_file("2.png", zip::write::SimpleFileOptions::default())
            .unwrap();
        nested.write_all(&png(2)).unwrap();
        nested
            .start_file("1.png", zip::write::SimpleFileOptions::default())
            .unwrap();
        nested.write_all(&png(1)).unwrap();
        let nested_bytes = nested.finish().unwrap().into_inner();

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("outer.cbz");
        let mut outer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        outer
            .start_file("nested.cbz", zip::write::SimpleFileOptions::default())
            .unwrap();
        outer.write_all(&nested_bytes).unwrap();
        outer.finish().unwrap();

        let bytes = load_cover_source_bytes(Default::default(), &path).unwrap();
        assert_eq!(
            image::load_from_memory(&bytes)
                .unwrap()
                .to_rgb8()
                .get_pixel(0, 0)[0],
            1
        );
    }

    #[test]
    fn normal_archive_returns_the_naturally_first_real_image() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("normal.cbz");
        let mut archive = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (name, bytes) in [
            ("10.png", png(10)),
            ("__MACOSX/0.png", png(200)),
            ("2.png", png(2)),
            ("1.png", png(1)),
        ] {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(&bytes).unwrap();
        }
        archive.finish().unwrap();

        let bytes = load_cover_source_bytes(Default::default(), &path).unwrap();
        assert_eq!(
            image::load_from_memory(&bytes)
                .unwrap()
                .to_rgb8()
                .get_pixel(0, 0)[0],
            1
        );
    }

    #[test]
    fn archive_root_and_wrapper_cover_take_priority_without_promoting_nested_cover() {
        let directory = tempfile::tempdir().unwrap();
        let wrapped = directory.path().join("wrapped.cbz");
        let mut archive = zip::ZipWriter::new(std::fs::File::create(&wrapped).unwrap());
        for (name, bytes) in [
            ("book/002.png", png(2)),
            ("book/Cover.PNG", png(9)),
            ("book/001.png", png(1)),
        ] {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(&bytes).unwrap();
        }
        archive.finish().unwrap();
        let bytes = load_cover_source_bytes(Default::default(), &wrapped).unwrap();
        assert_eq!(
            image::load_from_memory(&bytes)
                .unwrap()
                .to_rgb8()
                .get_pixel(0, 0)[0],
            9
        );

        let nested_cursor = Cursor::new(Vec::new());
        let mut nested = zip::ZipWriter::new(nested_cursor);
        nested
            .start_file("cover.png", zip::write::SimpleFileOptions::default())
            .unwrap();
        nested.write_all(&png(7)).unwrap();
        nested
            .start_file("001.png", zip::write::SimpleFileOptions::default())
            .unwrap();
        nested.write_all(&png(3)).unwrap();
        let nested_bytes = nested.finish().unwrap().into_inner();
        let outer = directory.path().join("outer.cbz");
        let mut archive = zip::ZipWriter::new(std::fs::File::create(&outer).unwrap());
        archive
            .start_file("01.cbz", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(&nested_bytes).unwrap();
        archive.finish().unwrap();
        let bytes = load_cover_source_bytes(Default::default(), &outer).unwrap();
        assert_eq!(
            image::load_from_memory(&bytes)
                .unwrap()
                .to_rgb8()
                .get_pixel(0, 0)[0],
            3
        );
    }

    #[test]
    fn folder_cover_is_preferred_but_not_a_viewer_entry() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("Cover.PNG"), png(8)).unwrap();
        std::fs::write(directory.path().join("001.png"), png(1)).unwrap();
        assert_eq!(
            cover_image_in_folder(directory.path()),
            Some(directory.path().join("Cover.PNG"))
        );
        assert_eq!(
            super::super::first_direct_image_path(directory.path()),
            Some(directory.path().join("001.png"))
        );
    }

    #[test]
    fn folder_cover_and_image_fallback_keep_natural_order() {
        let directory = tempfile::tempdir().unwrap();
        let cover_folder = directory.path().join("with-cover");
        let image_folder = directory.path().join("without-cover");
        std::fs::create_dir_all(&cover_folder).unwrap();
        std::fs::create_dir_all(&image_folder).unwrap();
        for name in ["Cover.PNG", "cover.jpg", "2.jpg"] {
            std::fs::write(cover_folder.join(name), []).unwrap();
        }
        for name in ["10.jpg", "2.jpg"] {
            std::fs::write(image_folder.join(name), []).unwrap();
        }
        let mut covers = vec![
            cover_folder.join("Cover.PNG"),
            cover_folder.join("cover.jpg"),
        ];
        sort_paths_naturally(&mut covers);
        assert_eq!(
            cover_image_in_folder(&cover_folder),
            Some(covers[0].clone())
        );
        assert_eq!(
            cover_image_in_folder(&image_folder),
            Some(image_folder.join("2.jpg"))
        );
    }

    #[test]
    fn archive_without_supported_images_returns_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("empty.cbz");
        let mut archive = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        archive
            .start_file("memo.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"memo").unwrap();
        archive.finish().unwrap();

        assert!(load_cover_source_bytes(Default::default(), &path).is_err());
    }
}
