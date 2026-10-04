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
}
