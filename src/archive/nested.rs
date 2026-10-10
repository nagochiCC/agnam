use super::format::ArchiveFormat;
use super::formats;
use super::{is_archive_ext, is_image_ext};
use crate::error::AppError;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(super) struct NestedImage {
    pub(super) physical_path: PathBuf,
    pub(super) id: super::ArchiveImageId,
}

/// Expands nested archives while retaining a stable logical identity for each
/// image. Generated `_extracted` directory names never enter that identity.
pub(super) fn extract_images_with_identities(
    archive_path: &Path,
    destination: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Vec<NestedImage>, AppError> {
    formats::extract_to_dir_with_budget(archive_path, destination, budget)?;
    let mut images = Vec::new();
    collect_level_images(destination, &[], 0, &mut images, budget)?;
    images.sort_by(|left, right| {
        natord::compare(
            &left.id.display_path().to_string_lossy(),
            &right.id.display_path().to_string_lossy(),
        )
    });
    Ok(images)
}

pub(super) fn images_from_extracted_root(
    root: &Path,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<Vec<NestedImage>, AppError> {
    let mut images = Vec::new();
    collect_level_images(root, &[], 0, &mut images, budget)?;
    images.sort_by(|left, right| {
        natord::compare(
            &left.id.display_path().to_string_lossy(),
            &right.id.display_path().to_string_lossy(),
        )
    });
    Ok(images)
}

fn collect_level_images(
    level_root: &Path,
    archive_chain: &[PathBuf],
    depth: usize,
    output: &mut Vec<NestedImage>,
    budget: &mut crate::archive::resource::ResourceBudget,
) -> Result<(), AppError> {
    budget.check_cancel()?;
    if depth > 10 {
        return Err(AppError::Archive(
            "アーカイブの階層が深すぎます（最大10階層まで）".into(),
        ));
    }
    let mut files = Vec::new();
    collect_level_files(level_root, level_root, &mut files)?;
    let cover_only = super::cover_only_paths(&files);
    let nested = files
        .iter()
        .filter(|path| is_archive_ext(path))
        .cloned()
        .collect::<Vec<_>>();
    for relative in files {
        let physical = level_root.join(&relative);
        if is_image_ext(&relative) && !cover_only.contains(&relative) {
            output.push(NestedImage {
                physical_path: physical,
                id: super::ArchiveImageId {
                    archives: archive_chain.to_vec(),
                    image: relative,
                },
            });
        }
    }
    for (nested_index, relative) in nested.into_iter().enumerate() {
        budget.check_cancel()?;
        let physical = level_root.join(&relative);
        let destination = level_root.join(format!(".agnam-nested-{}", nested_index));
        std::fs::create_dir_all(&destination)?;
        formats::extract_to_dir_with_budget(&physical, &destination, budget)?;
        let mut chain = archive_chain.to_vec();
        chain.push(relative);
        collect_level_images(&destination, &chain, depth + 1, output, budget)?;
    }
    Ok(())
}

fn collect_level_files(
    root: &Path,
    directory: &Path,
    output: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if super::is_ignored_filesystem_path(&path) {
            continue;
        }
        if path.is_dir() {
            collect_level_files(root, &path, output)?;
        } else if let Ok(relative) = path.strip_prefix(root) {
            output.push(relative.to_path_buf());
        }
    }
    Ok(())
}

pub(super) fn has_nested_archives(archive_path: &Path) -> bool {
    let Some(format) = ArchiveFormat::from_path(archive_path) else {
        return false;
    };
    formats::has_nested_archives(format, archive_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn all_backends_materialize_nested_rar_with_shared_budget_and_file_identity() {
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 3, image::Rgb([12, 34, 56])))
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let image = encoded.into_inner();
        let inner = formats::rar_native::stored_rar(&[("page.png", &image)]);
        let directory = tempfile::tempdir().unwrap();
        for extension in ["zip", "rar", "7z", "tar", "lha"] {
            let archive = directory.path().join(format!("outer.{extension}"));
            match extension {
                "zip" => std::fs::write(&archive, zip_bytes(&[("inner.rar", &inner)])).unwrap(),
                "rar" => std::fs::write(
                    &archive,
                    formats::rar_native::stored_rar(&[("inner.rar", &inner)]),
                )
                .unwrap(),
                "7z" => {
                    let mut writer = sevenz_rust::SevenZWriter::create(&archive).unwrap();
                    let mut entry = sevenz_rust::SevenZArchiveEntry::new();
                    entry.name = "inner.rar".into();
                    entry.has_stream = true;
                    writer
                        .push_archive_entry(entry, Some(std::io::Cursor::new(&inner)))
                        .unwrap();
                    writer.finish().unwrap();
                }
                "tar" => {
                    let mut writer = tar::Builder::new(std::fs::File::create(&archive).unwrap());
                    let mut header = tar::Header::new_gnu();
                    header.set_size(inner.len() as u64);
                    header.set_mode(0o644);
                    header.set_cksum();
                    writer
                        .append_data(&mut header, "inner.rar", inner.as_slice())
                        .unwrap();
                    writer.finish().unwrap();
                }
                "lha" => {
                    let mut header = b"-lh0-".to_vec();
                    header.extend_from_slice(&(inner.len() as u32).to_le_bytes());
                    header.extend_from_slice(&(inner.len() as u32).to_le_bytes());
                    header.extend_from_slice(&0u32.to_le_bytes());
                    header.extend_from_slice(&[0x20, 0, 9]);
                    header.extend_from_slice(b"inner.rar");
                    header.extend_from_slice(&[0, 0]);
                    let mut out = vec![
                        header.len() as u8,
                        header.iter().fold(0u8, |sum, &byte| sum.wrapping_add(byte)),
                    ];
                    out.extend_from_slice(&header);
                    out.extend_from_slice(&inner);
                    out.push(0);
                    std::fs::write(&archive, out).unwrap();
                }
                _ => unreachable!(),
            }
            let total = inner.len() as u64 + image.len() as u64;
            for allowance in [total, total - 1] {
                let mut budget = crate::archive::resource::ResourceBudget::new(
                    crate::archive::resource::ResourceLimits {
                        single_entry: image.len() as u64,
                        cumulative: allowance,
                        temp_writes: total,
                        temp_occupancy: total,
                        entries: 4,
                        images: 4,
                    },
                );
                let workspace = tempfile::tempdir().unwrap();
                let result =
                    extract_images_with_identities(&archive, workspace.path(), &mut budget);
                if allowance == total {
                    let images = result.unwrap();
                    assert_eq!(images.len(), 1, "{extension}");
                    assert_eq!(images[0].id.archives, [PathBuf::from("inner.rar")]);
                    assert_eq!(std::fs::read(&images[0].physical_path).unwrap(), image);
                    assert_eq!(budget.temp_counters(), (total, total));
                } else {
                    assert!(
                        matches!(
                            result,
                            Err(AppError::ArchiveResourceLimit(
                                crate::archive::ResourceLimitKind::CumulativeBytes
                            ))
                        ),
                        "{extension}"
                    );
                }
                let root = workspace.path().to_path_buf();
                workspace.close().unwrap();
                budget.release_temp_tree(&root).unwrap();
                assert_eq!(budget.temp_counters().1, 0);
            }
            let workspace = tempfile::tempdir().unwrap();
            let mut counted = crate::archive::resource::ResourceBudget::new(
                crate::archive::resource::ResourceLimits {
                    entries: 1,
                    ..crate::archive::resource::ResourceLimits::PRODUCTION
                },
            );
            assert!(
                matches!(
                    extract_images_with_identities(&archive, workspace.path(), &mut counted),
                    Err(AppError::ArchiveResourceLimit(
                        crate::archive::ResourceLimitKind::EntryCount
                    ))
                ),
                "{extension}"
            );
            let level = crate::archive::ArchiveContentLevel::open(
                Default::default(),
                crate::archive::ArchiveLocation {
                    archive: archive.clone(),
                    archives: vec!["inner.rar".into()],
                    directory: PathBuf::new(),
                },
            )
            .unwrap();
            assert_eq!(
                level
                    .reader
                    .read(Default::default(), Path::new("page.png"))
                    .unwrap(),
                image
            );
            assert_eq!(
                crate::archive::load_archive_entry_bytes(
                    Default::default(),
                    &archive,
                    &crate::archive::ArchiveImageId {
                        archives: vec!["inner.rar".into()],
                        image: "page.png".into(),
                    }
                )
                .unwrap(),
                image
            );
            let (document, _) =
                crate::archive::load_document_from_path(Default::default(), &archive).unwrap();
            assert_eq!(document.assets.len(), 1);
            assert!(document.temp_dir.is_some());
            assert_eq!(document.pages.len(), 1);
            assert!(crate::bookshelf::thumbnail::generate_from_bytes(&image).is_ok());
        }
    }

    #[test]
    fn mixed_rar_zip_recursion_and_forged_archive_fail_without_completed_images() {
        let directory = tempfile::tempdir().unwrap();
        let inner = zip_bytes(&[("page.png", b"image")]);
        let middle = formats::rar_native::stored_rar(&[("inner.zip", &inner)]);
        let archive = directory.path().join("outer.zip");
        std::fs::write(&archive, zip_bytes(&[("middle.rar", &middle)])).unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let images =
            extract_images_with_identities(&archive, workspace.path(), &mut Default::default())
                .unwrap();
        assert_eq!(
            images[0].id.archives,
            [PathBuf::from("middle.rar"), PathBuf::from("inner.zip")]
        );
        std::fs::write(&archive, zip_bytes(&[("forged.rar", b"not an archive")])).unwrap();
        let workspace = tempfile::tempdir().unwrap();
        assert!(
            extract_images_with_identities(&archive, workspace.path(), &mut Default::default())
                .is_err()
        );
        let token = crate::archive::ProgressiveArchiveCancelToken::default();
        token.cancel();
        assert!(matches!(
            crate::archive::load_document_from_path_with_cancel(
                Default::default(),
                &archive,
                &token
            )
            .unwrap(),
            crate::archive::ProgressiveArchiveLoadOutcome::Cancelled
        ));
    }

    #[test]
    fn nested_extraction_shares_byte_budget_and_cleans_partial_temp_tree() {
        let inner = zip_bytes(&[("page.png", b"image")]);
        let outer = zip_bytes(&[("nested.cbz", &inner)]);
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("outer.cbz");
        std::fs::write(&archive, outer).unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let workspace_path = workspace.path().to_path_buf();
        let mut budget = crate::archive::resource::ResourceBudget::new(
            crate::archive::resource::ResourceLimits {
                single_entry: inner.len() as u64,
                cumulative: inner.len() as u64 + 4,
                temp_writes: 1024,
                temp_occupancy: 1024,
                entries: 8,
                images: 8,
            },
        );
        assert!(matches!(
            extract_images_with_identities(&archive, workspace.path(), &mut budget),
            Err(crate::error::AppError::ArchiveResourceLimit(
                crate::archive::ResourceLimitKind::CumulativeBytes
            ))
        ));
        drop(workspace);
        assert!(!workspace_path.exists());
    }

    #[test]
    fn selected_budget_boundary_is_shared_by_outer_and_nested_extraction() {
        use crate::archive::resource::{ArchiveExpansionLimit, ResourceBudget, SINGLE_ENTRY_LIMIT};
        let inner = zip_bytes(&[("page.png", b"image")]);
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("outer.zip");
        std::fs::write(&archive, zip_bytes(&[("inner.zip", &inner)])).unwrap();
        for limit in ArchiveExpansionLimit::ALL {
            for extra in [0, 1] {
                let mut budget = ResourceBudget::for_expansion_limit(limit);
                // Charge previous work without creating a multi-GiB fixture.
                let mut used = limit.bytes() - inner.len() as u64 - 5 + extra;
                while used > 0 {
                    let amount = used.min(SINGLE_ENTRY_LIMIT);
                    budget.account_bytes(amount).unwrap();
                    used -= amount;
                }
                let workspace = tempfile::tempdir().unwrap();
                let root = workspace.path().to_path_buf();
                let result = extract_images_with_identities(&archive, &root, &mut budget);
                if extra == 0 {
                    let images = result.unwrap();
                    assert_eq!(images.len(), 1);
                    assert_eq!(std::fs::read(&images[0].physical_path).unwrap(), b"image");
                    assert_eq!(
                        budget.temp_counters(),
                        (inner.len() as u64 + 5, inner.len() as u64 + 5)
                    );
                } else {
                    assert!(matches!(
                        result,
                        Err(AppError::ArchiveResourceLimit(
                            crate::archive::ResourceLimitKind::CumulativeBytes
                        ))
                    ));
                }
                drop(workspace);
                budget.release_temp_tree(&root).unwrap();
                assert!(!root.exists());
                assert_eq!(budget.temp_counters().1, 0);
            }
        }
    }
}
