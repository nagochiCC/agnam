use super::formats;
use super::nested::{extract_images_with_identities, has_nested_archives};
use super::sort_paths_naturally;
use super::{
    ArchiveAccessStrategy, ProgressiveArchiveCancelToken, ProgressiveArchiveLoadOutcome,
    archive_access_strategy, archive_supports_sequential_progress,
};
use super::{is_cover_image, is_image_ext};
use crate::document::{Document, ImageLayout, ImageSource};
use crate::error::AppError;
use std::path::{Path, PathBuf};

/// 与えられたパスから表示用の `Document` と初期ページインデックスを返します。
pub(crate) fn load_document_from_path(path: &Path) -> Result<(Document, usize), AppError> {
    match load_document_from_path_impl(path, None)? {
        ProgressiveArchiveLoadOutcome::Complete(document) => Ok(document),
        ProgressiveArchiveLoadOutcome::Cancelled => unreachable!("non-progressive load"),
    }
}

/// Progressive通知に対応するSequential strategyの非nested archiveについて、
/// 各画像を共通形式で通知しながら読み込みます。通知非対応backendとnested archiveは
/// 従来の一括読み込み経路を使用します。
pub(crate) fn load_document_from_path_with_sequential_progress(
    path: &Path,
    cancel_token: &ProgressiveArchiveCancelToken,
    mut on_image: impl FnMut(super::ProgressiveArchiveImage),
) -> Result<ProgressiveArchiveLoadOutcome<(Document, usize)>, AppError> {
    if cancel_token.is_cancelled() {
        return Ok(ProgressiveArchiveLoadOutcome::Cancelled);
    }

    load_document_from_path_impl(path, Some((&mut on_image, cancel_token)))
}

/// Sequential archive contents専用に画像を1件ずつ通知します。Viewer用の
/// `Document` や全画像collectionは構築しません。
pub(crate) fn stream_sequential_archive_images(
    path: &Path,
    cancel_token: &ProgressiveArchiveCancelToken,
    mut on_image: impl FnMut(super::SequentialArchiveImage),
) -> Result<ProgressiveArchiveLoadOutcome<()>, AppError> {
    if cancel_token.is_cancelled() {
        return Ok(ProgressiveArchiveLoadOutcome::Cancelled);
    }
    if archive_access_strategy(path)? != Some(ArchiveAccessStrategy::Sequential)
        || !archive_supports_sequential_progress(path)
    {
        return Err(AppError::Archive(
            "逐次archive thumbnail streamに対応していません".into(),
        ));
    }
    formats::stream_sequential_images(path, cancel_token, &mut on_image)
}

fn load_document_from_path_impl(
    path: &Path,
    progressive: Option<(
        &mut dyn FnMut(super::ProgressiveArchiveImage),
        &ProgressiveArchiveCancelToken,
    )>,
) -> Result<ProgressiveArchiveLoadOutcome<(Document, usize)>, AppError> {
    if is_image_ext(path) {
        if is_cover_image(path) {
            let is_wide = image::image_dimensions(path)
                .map(|(width, height)| width > height)
                .unwrap_or(false);
            let mut document = Document::new(path.to_path_buf(), None);
            document.add_asset(
                ImageSource::File(path.to_path_buf()),
                ImageLayout::from_is_wide(is_wide),
            );
            return Ok(ProgressiveArchiveLoadOutcome::Complete((document, 0)));
        }
        if let Some(parent) = path.parent() {
            let entries = image_paths_in_folder(parent)?;
            let document = build_document_from_paths(path.to_path_buf(), entries, None);
            let initial_index = document
                .assets
                .iter()
                .find(|asset| asset.source.as_file_path() == Some(path))
                .map(|asset| asset.first_page)
                .unwrap_or(0);
            return Ok(ProgressiveArchiveLoadOutcome::Complete((
                document,
                initial_index,
            )));
        }
        return Err(AppError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "親ディレクトリが見つかりません",
        )));
    }

    let needs_temp_directory = has_nested_archives(path);

    if needs_temp_directory {
        if progressive
            .as_ref()
            .is_some_and(|(_, cancel_token)| cancel_token.is_cancelled())
        {
            return Ok(ProgressiveArchiveLoadOutcome::Cancelled);
        }
        let temp_directory = tempfile::Builder::new().prefix("agnam-").tempdir()?;
        let mut budget = crate::archive::resource::ResourceBudget::default();
        let entries = extract_images_with_identities(path, temp_directory.path(), &mut budget)?;

        if progressive
            .as_ref()
            .is_some_and(|(_, cancel_token)| cancel_token.is_cancelled())
        {
            return Ok(ProgressiveArchiveLoadOutcome::Cancelled);
        }
        let document = build_document_from_archive_images(
            path.to_path_buf(),
            entries,
            Some(std::sync::Arc::new(temp_directory)),
        );
        return Ok(ProgressiveArchiveLoadOutcome::Complete((document, 0)));
    }

    let document = match progressive {
        Some((on_image, cancel_token))
            if archive_supports_sequential_progress(path)
                && archive_access_strategy(path)? == Some(ArchiveAccessStrategy::Sequential) =>
        {
            formats::load_sequential_document_with_progress(path, cancel_token, on_image)?
        }
        Some((_, cancel_token)) if cancel_token.is_cancelled() => {
            ProgressiveArchiveLoadOutcome::Cancelled
        }
        _ => ProgressiveArchiveLoadOutcome::Complete(formats::load_document(path)?),
    };
    Ok(match document {
        ProgressiveArchiveLoadOutcome::Complete(document) => {
            ProgressiveArchiveLoadOutcome::Complete((document, 0))
        }
        ProgressiveArchiveLoadOutcome::Cancelled => ProgressiveArchiveLoadOutcome::Cancelled,
    })
}

fn image_paths_in_folder(folder_path: &Path) -> Result<Vec<PathBuf>, AppError> {
    let entries = std::fs::read_dir(folder_path)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_image_document_entry_path(path))
        .collect::<Vec<_>>();
    Ok(entries)
}

pub(crate) fn first_direct_image_path(folder_path: &Path) -> Option<PathBuf> {
    let mut entries = std::fs::read_dir(folder_path)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            let path = entry.path();
            (file_type.is_file() && is_image_document_entry_path(&path)).then_some(path)
        })
        .collect::<Vec<_>>();
    sort_paths_naturally(&mut entries);
    entries.into_iter().next()
}

pub(crate) fn is_image_document_entry_path(path: &Path) -> bool {
    is_image_ext(path) && !is_cover_image(path)
}

/// 画像パスのリストから、物理画像と論理ページを分離した `Document` を構築します。
fn build_document_from_paths(
    document_path: PathBuf,
    mut paths: Vec<PathBuf>,
    temp_dir: Option<std::sync::Arc<tempfile::TempDir>>,
) -> Document {
    sort_paths_naturally(&mut paths);

    let mut document = Document::new(document_path, temp_dir);
    for path in paths {
        let is_wide = image::image_dimensions(&path)
            .map(|(width, height)| width > height)
            .unwrap_or(false);
        document.add_asset(ImageSource::File(path), ImageLayout::from_is_wide(is_wide));
    }
    document
}

fn build_document_from_archive_images(
    document_path: PathBuf,
    images: Vec<super::nested::NestedImage>,
    temp_dir: Option<std::sync::Arc<tempfile::TempDir>>,
) -> Document {
    let mut document = Document::new(document_path, temp_dir);
    for image in images {
        let is_wide = image::image_dimensions(&image.physical_path)
            .map(|(width, height)| width > height)
            .unwrap_or(false);
        document.add_archive_asset(
            ImageSource::File(image.physical_path),
            ImageLayout::from_is_wide(is_wide),
            image.id.archives,
            image.id.image,
        );
    }
    document
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::PagePart;
    use std::io::{Cursor, Write};

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            width,
            height,
            image::Rgb([32, 64, 128]),
        ))
        .write_to(&mut output, image::ImageFormat::Png)
        .unwrap();
        output.into_inner()
    }

    fn write_non_solid_7z(path: &Path, entries: Vec<(&str, Vec<u8>)>) {
        let mut writer = sevenz_rust::SevenZWriter::create(path).unwrap();
        for (name, data) in entries {
            let mut entry = sevenz_rust::SevenZArchiveEntry::new();
            entry.name = name.to_string();
            entry.has_stream = true;
            writer
                .push_archive_entry(entry, Some(Cursor::new(data)))
                .unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn image_path_loads_the_naturally_sorted_folder_and_selected_logical_page() {
        let directory = tempfile::tempdir().unwrap();
        for (name, width, height) in [("10.png", 4, 8), ("2.png", 8, 4), ("1.png", 4, 8)] {
            std::fs::write(directory.path().join(name), png(width, height)).unwrap();
        }
        std::fs::write(directory.path().join("ignored.gif"), b"not an image").unwrap();
        let selected = directory.path().join("2.png");

        let (document, initial_index) = load_document_from_path(&selected).unwrap();

        let names = document
            .assets
            .iter()
            .map(|asset| {
                asset
                    .source
                    .as_file_path()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(names, ["1.png", "2.png", "10.png"]);
        assert_eq!(initial_index, 1);
        assert_eq!(document.assets[1].first_page, 1);
        assert_eq!(document.pages[1].part, PagePart::Right);
        assert_eq!(document.pages[2].part, PagePart::Left);
    }

    #[test]
    fn image_folder_document_excludes_cover_images() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("cover.jpg"), png(4, 8)).unwrap();
        std::fs::write(directory.path().join("Cover.PNG"), png(4, 8)).unwrap();
        let selected = directory.path().join("001.png");
        std::fs::write(&selected, png(4, 8)).unwrap();

        let (document, _) = load_document_from_path(&selected).unwrap();
        assert_eq!(document.assets.len(), 1);
        assert_eq!(
            document.assets[0].source.as_file_path(),
            Some(selected.as_path())
        );
    }

    #[test]
    fn explicitly_opened_cover_image_is_a_standalone_document() {
        let directory = tempfile::tempdir().unwrap();
        let cover = directory.path().join("Cover.PNG");
        std::fs::write(&cover, png(8, 4)).unwrap();
        std::fs::write(directory.path().join("001.png"), png(4, 8)).unwrap();

        let (document, initial_index) = load_document_from_path(&cover).unwrap();

        assert_eq!(initial_index, 0);
        assert_eq!(document.path, cover);
        assert_eq!(document.assets.len(), 1);
        assert_eq!(
            document.assets[0].source.as_file_path(),
            Some(cover.as_path())
        );
        assert_eq!(document.pages.len(), 2);
    }

    #[test]
    fn image_document_entry_is_natural_direct_and_missing_safe() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["10.png", "2.png", "1.png"] {
            std::fs::write(directory.path().join(name), png(4, 8)).unwrap();
        }
        let nested = directory.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("0.png"), png(4, 8)).unwrap();
        for name in ["cover.jpg", "Cover.JPEG", "COVER.PNG", "CoVeR.webp"] {
            std::fs::write(directory.path().join(name), png(4, 8)).unwrap();
        }

        assert_eq!(
            first_direct_image_path(directory.path()),
            Some(directory.path().join("1.png"))
        );
        assert_eq!(first_direct_image_path(&nested), Some(nested.join("0.png")));
        assert_eq!(
            first_direct_image_path(&directory.path().join("missing")),
            None
        );

        std::fs::remove_file(nested.join("0.png")).unwrap();
        assert_eq!(first_direct_image_path(&nested), None);
    }

    #[test]
    fn cover_only_and_nested_images_do_not_make_a_direct_image_entry() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["cover.jpg", "Cover.JPEG", "COVER.PNG", "CoVeR.webp"] {
            std::fs::write(directory.path().join(name), png(4, 8)).unwrap();
        }
        let nested = directory.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("001.png"), png(4, 8)).unwrap();

        assert_eq!(first_direct_image_path(directory.path()), None);
    }

    #[test]
    fn loads_non_nested_zip_entries_in_natural_order_without_extracting() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("pages.cbz");
        let file = std::fs::File::create(&archive_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("10.png", options).unwrap();
        writer.write_all(&png(4, 8)).unwrap();
        writer.start_file("2.png", options).unwrap();
        writer.write_all(&png(8, 4)).unwrap();
        writer.start_file("__MACOSX/1.png", options).unwrap();
        writer.write_all(&png(4, 8)).unwrap();
        writer.start_file("._3.png", options).unwrap();
        writer.write_all(&png(4, 8)).unwrap();
        writer.finish().unwrap();

        let (document, initial_index) = load_document_from_path(&archive_path).unwrap();

        assert!(document.temp_dir.is_none());
        assert_eq!(initial_index, 0);
        assert_eq!(document.assets.len(), 2);
        assert_eq!(document.pages.len(), 3);
        assert_eq!(
            document
                .pages
                .iter()
                .map(|page| page.part)
                .collect::<Vec<_>>(),
            vec![PagePart::Right, PagePart::Left, PagePart::Whole]
        );
        assert_eq!(document.pages[0].asset_id, document.pages[1].asset_id);
        assert_ne!(document.pages[1].asset_id, document.pages[2].asset_id);
        let archive_entries = document
            .assets
            .iter()
            .map(|asset| match &asset.source {
                ImageSource::ArchiveEntry {
                    entry_index,
                    entry_name,
                    ..
                } => (*entry_index, entry_name.as_str()),
                _ => panic!("ZIP asset should stay as an ArchiveEntry source"),
            })
            .collect::<Vec<_>>();
        assert_eq!(archive_entries, vec![(1, "2.png"), (0, "10.png")]);
    }

    #[test]
    fn archive_cover_hint_is_excluded_but_deep_cover_keeps_a_page_identity() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("pages.cbz");
        let file = std::fs::File::create(&archive_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for name in ["cover.jpg", "001.png", "chapter/cover.jpg"] {
            writer.start_file(name, options).unwrap();
            writer.write_all(&png(4, 8)).unwrap();
        }
        writer.finish().unwrap();

        let (document, _) = load_document_from_path(&archive_path).unwrap();
        let identities = document
            .assets
            .iter()
            .map(|asset| asset.archive_identity.as_ref().unwrap().image.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            identities,
            [PathBuf::from("001.png"), PathBuf::from("chapter/cover.jpg")]
        );
        assert_eq!(
            document.first_page_for_archive_image(&[], Path::new("chapter/cover.jpg")),
            Some(1)
        );
        assert_eq!(
            document.first_page_for_archive_image(&[], Path::new("cover.jpg")),
            None
        );
    }

    #[test]
    fn transparent_wrapper_cover_hint_is_excluded_from_archive_pages() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("wrapped.cbz");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&archive_path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for name in ["Book/Cover.PNG", "Book/001.png"] {
            writer.start_file(name, options).unwrap();
            writer.write_all(&png(4, 8)).unwrap();
        }
        writer.finish().unwrap();

        let (document, _) = load_document_from_path(&archive_path).unwrap();
        assert_eq!(document.assets.len(), 1);
        assert_eq!(
            document.assets[0]
                .archive_identity
                .as_ref()
                .map(|identity| identity.image.as_path()),
            Some(Path::new("Book/001.png"))
        );
    }

    #[test]
    fn non_solid_seven_zip_candidate_completes_without_progressive_notifications() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("pages.7z");
        write_non_solid_7z(
            &archive_path,
            vec![("002.png", png(8, 4)), ("001.png", png(4, 8))],
        );
        let cancel_token = ProgressiveArchiveCancelToken::default();
        let mut notification_count = 0;

        let outcome =
            load_document_from_path_with_sequential_progress(&archive_path, &cancel_token, |_| {
                notification_count += 1
            })
            .unwrap();
        let ProgressiveArchiveLoadOutcome::Complete((document, initial_index)) = outcome else {
            panic!("non-solid 7z load should complete normally");
        };

        assert_eq!(notification_count, 0);
        assert_eq!(initial_index, 0);
        assert!(document.temp_dir.is_none());
        assert!(
            document
                .assets
                .iter()
                .all(|asset| matches!(&asset.source, ImageSource::ArchiveEntry { .. }))
        );
    }

    #[test]
    fn extracts_nested_zip_pages_into_a_kept_temporary_directory() {
        let nested_cursor = Cursor::new(Vec::new());
        let mut nested_writer = zip::ZipWriter::new(nested_cursor);
        nested_writer
            .start_file("cover.jpg", zip::write::SimpleFileOptions::default())
            .unwrap();
        nested_writer.write_all(&png(4, 8)).unwrap();
        nested_writer
            .start_file("page.png", zip::write::SimpleFileOptions::default())
            .unwrap();
        nested_writer.write_all(&png(4, 8)).unwrap();
        let nested_bytes = nested_writer.finish().unwrap().into_inner();

        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("outer.cbz");
        let file = std::fs::File::create(&archive_path).unwrap();
        let mut outer_writer = zip::ZipWriter::new(file);
        outer_writer
            .start_file("nested.cbz", zip::write::SimpleFileOptions::default())
            .unwrap();
        outer_writer.write_all(&nested_bytes).unwrap();
        outer_writer.finish().unwrap();

        let (document, initial_index) = load_document_from_path(&archive_path).unwrap();

        assert_eq!(initial_index, 0);
        assert_eq!(document.assets.len(), 1);
        assert_eq!(document.pages.len(), 1);
        let temp_dir = document
            .temp_dir
            .as_ref()
            .expect("nested archives need a live temporary directory");
        let ImageSource::File(page_path) = &document.assets[0].source else {
            panic!("nested archive assets should use extracted files");
        };
        assert!(page_path.starts_with(temp_dir.path()));
        assert!(page_path.exists());
        assert_eq!(
            document.assets[0].archive_identity.as_ref(),
            Some(&crate::document::ArchiveAssetIdentity {
                archives: vec![PathBuf::from("nested.cbz")],
                image: PathBuf::from("page.png"),
            })
        );
        assert_eq!(
            document.first_page_for_archive_image(
                &[PathBuf::from("nested.cbz")],
                Path::new("page.png")
            ),
            Some(0)
        );
    }

    #[test]
    fn nested_seven_zip_keeps_the_existing_temporary_extraction_fallback() {
        let nested_cursor = Cursor::new(Vec::new());
        let mut nested_writer = zip::ZipWriter::new(nested_cursor);
        nested_writer
            .start_file("page.png", zip::write::SimpleFileOptions::default())
            .unwrap();
        nested_writer.write_all(&png(4, 8)).unwrap();
        let nested_bytes = nested_writer.finish().unwrap().into_inner();

        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("outer.7z");
        write_non_solid_7z(&archive_path, vec![("nested.cbz", nested_bytes)]);

        let (document, initial_index) = load_document_from_path(&archive_path).unwrap();

        assert_eq!(initial_index, 0);
        assert_eq!(document.assets.len(), 1);
        let temp_dir = document
            .temp_dir
            .as_ref()
            .expect("nested 7z should keep its temporary directory");
        let ImageSource::File(page_path) = &document.assets[0].source else {
            panic!("nested 7z images should use extracted files");
        };
        assert!(page_path.starts_with(temp_dir.path()));
        assert!(page_path.exists());
    }
}
