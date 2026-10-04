use crate::archive::{
    is_cover_image, is_ignored_filesystem_path, is_image_ext, is_viewable_path,
    sort_paths_naturally,
};
use std::io;
use std::path::{Path, PathBuf};

/// Resolve a dropped local path to the file that should enter the normal loader.
pub(super) fn resolve_dropped_path(path: &Path) -> io::Result<Option<PathBuf>> {
    if path.is_dir() {
        return first_viewable_file(path);
    }

    Ok(
        (path.is_file() && !is_ignored_filesystem_path(path) && is_viewable_path(path))
            .then(|| path.to_path_buf()),
    )
}

fn first_viewable_file(directory: &Path) -> io::Result<Option<PathBuf>> {
    let mut images = Vec::new();
    let mut archives = Vec::new();

    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() || is_ignored_filesystem_path(&path) {
            continue;
        }
        if is_image_ext(&path) {
            if !is_cover_image(&path) {
                images.push(path);
            }
        } else if is_viewable_path(&path) {
            archives.push(path);
        }
    }

    let candidates = if images.is_empty() {
        &mut archives
    } else {
        &mut images
    };
    sort_paths_naturally(candidates);
    Ok(candidates.first().cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(directory: &Path, name: &str) {
        std::fs::write(directory.join(name), []).unwrap();
    }

    fn selected_name(directory: &Path) -> Option<String> {
        resolve_dropped_path(directory)
            .unwrap()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
    }

    #[test]
    fn image_only_folder_selects_naturally_first_image() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["10.jpg", "2.jpg", "1.jpg"] {
            touch(directory.path(), name);
        }

        assert_eq!(selected_name(directory.path()).as_deref(), Some("1.jpg"));
    }

    #[test]
    fn archive_only_folder_selects_naturally_first_archive() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["10.zip", "2.zip", "1.zip"] {
            touch(directory.path(), name);
        }

        assert_eq!(selected_name(directory.path()).as_deref(), Some("1.zip"));
    }

    #[test]
    fn image_is_preferred_over_archive() {
        let directory = tempfile::tempdir().unwrap();
        touch(directory.path(), "01.zip");
        touch(directory.path(), "002.jpg");
        touch(directory.path(), "001.jpg");

        assert_eq!(selected_name(directory.path()).as_deref(), Some("001.jpg"));
    }

    #[test]
    fn cover_image_is_skipped_in_favor_of_naturally_first_archive() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["cover.jpg", "01.zip", "02.zip"] {
            touch(directory.path(), name);
        }
        assert_eq!(selected_name(directory.path()).as_deref(), Some("01.zip"));

        let mixed_case_directory = tempfile::tempdir().unwrap();
        for name in ["Cover.PNG", "10.zip", "2.zip"] {
            touch(mixed_case_directory.path(), name);
        }
        assert_eq!(
            selected_name(mixed_case_directory.path()).as_deref(),
            Some("2.zip")
        );
    }

    #[test]
    fn non_cover_image_is_still_preferred_over_archive() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["cover.jpg", "002.jpg", "001.jpg", "01.zip"] {
            touch(directory.path(), name);
        }

        assert_eq!(selected_name(directory.path()).as_deref(), Some("001.jpg"));
    }

    #[test]
    fn cover_only_folder_produces_no_target() {
        let directory = tempfile::tempdir().unwrap();
        touch(directory.path(), "cover.jpg");

        assert_eq!(selected_name(directory.path()), None);
    }

    #[test]
    fn directly_dropped_cover_image_is_selected() {
        let directory = tempfile::tempdir().unwrap();
        let cover = directory.path().join("cover.jpg");
        touch(directory.path(), "cover.jpg");

        assert_eq!(resolve_dropped_path(&cover).unwrap(), Some(cover));
    }

    #[test]
    fn names_containing_cover_are_not_excluded() {
        for name in ["cover-art.jpg", "mycover.jpg"] {
            let directory = tempfile::tempdir().unwrap();
            touch(directory.path(), name);
            assert_eq!(selected_name(directory.path()).as_deref(), Some(name));
        }
    }

    #[test]
    fn unsupported_files_produce_no_target() {
        let directory = tempfile::tempdir().unwrap();
        touch(directory.path(), "notes.txt");
        touch(directory.path(), "animation.gif");

        assert_eq!(selected_name(directory.path()), None);
    }

    #[test]
    fn child_folders_are_not_searched() {
        let directory = tempfile::tempdir().unwrap();
        let child = directory.path().join("chapter");
        std::fs::create_dir(&child).unwrap();
        touch(&child, "01.jpg");

        assert_eq!(selected_name(directory.path()), None);
    }

    #[test]
    fn supported_extensions_are_case_insensitive_via_existing_predicates() {
        for name in ["01.JPG", "01.JpEg", "01.PNG", "01.WeBp"] {
            let image_directory = tempfile::tempdir().unwrap();
            touch(image_directory.path(), name);
            assert_eq!(selected_name(image_directory.path()).as_deref(), Some(name));
        }

        let archive_directory = tempfile::tempdir().unwrap();
        touch(archive_directory.path(), "01.CBZ");
        assert_eq!(
            selected_name(archive_directory.path()).as_deref(),
            Some("01.CBZ")
        );
    }
}
