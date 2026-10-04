use super::{is_archive_ext, is_ignored_filesystem_path, is_image_ext};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

pub(crate) fn find_sibling_file(current_path: &Path, next: bool) -> Option<PathBuf> {
    let entity_path = if is_archive_ext(current_path) {
        current_path.to_path_buf()
    } else if let Some(parent) = current_path.parent() {
        parent.to_path_buf()
    } else {
        return None;
    };

    let parent_directory = sibling_container(current_path)?;
    let mut entries = Vec::new();
    if let Ok(read_directory) = std::fs::read_dir(parent_directory) {
        for entry in read_directory.filter_map(Result::ok) {
            let path = entry.path();
            if is_ignored_filesystem_path(&path) {
                continue;
            }

            let is_dir = entry.file_type().map_or_else(
                |_| path.is_dir(),
                |file_type| file_type.is_dir() || file_type.is_symlink() && path.is_dir(),
            );
            if is_dir || is_archive_ext(&path) {
                entries.push(path);
            }
        }
    }
    let current_index = entries.iter().position(|path| path == &entity_path)?;
    let current_name = entity_path.file_name()?.to_string_lossy();
    let mut target_index = None;
    for (index, path) in entries.iter().enumerate() {
        if index == current_index {
            continue;
        }
        let name = path.file_name()?.to_string_lossy();
        let order = natord::compare(&name, &current_name);
        let eligible = if next {
            order == Ordering::Greater || order == Ordering::Equal && index > current_index
        } else {
            order == Ordering::Less || order == Ordering::Equal && index < current_index
        };
        if !eligible {
            continue;
        }
        let better = target_index.is_none_or(|best: usize| {
            let best_name = entries[best].file_name().unwrap().to_string_lossy();
            let order = natord::compare(&name, &best_name);
            if next {
                order == Ordering::Less || order == Ordering::Equal && index < best
            } else {
                order == Ordering::Greater || order == Ordering::Equal && index > best
            }
        });
        if better {
            target_index = Some(index);
        }
    }

    let target_path = entries.get(target_index?)?;
    if target_path.is_dir() {
        find_first_image_in_dir(target_path)
    } else {
        Some(target_path.clone())
    }
}

pub(crate) fn sibling_container(current_path: &Path) -> Option<&Path> {
    if is_archive_ext(current_path) {
        current_path.parent()
    } else {
        current_path.parent()?.parent()
    }
}

fn find_first_image_in_dir(directory: &Path) -> Option<PathBuf> {
    let mut entries = Vec::new();
    if let Ok(read_directory) = std::fs::read_dir(directory) {
        for entry in read_directory.filter_map(Result::ok) {
            let path = entry.path();
            if !is_ignored_filesystem_path(&path) {
                entries.push(path);
            }
        }
    }
    entries.sort_by(|left, right| {
        natord::compare(
            &left.file_name().unwrap().to_string_lossy(),
            &right.file_name().unwrap().to_string_lossy(),
        )
    });

    for path in entries {
        if path.is_dir() {
            if let Some(image) = find_first_image_in_dir(&path) {
                return Some(image);
            }
        } else if is_image_ext(&path) {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::sort_paths_naturally;

    #[test]
    fn navigates_directories_naturally_and_skips_hidden_entries() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("book1");
        let second = root.path().join("book2");
        let hidden = root.path().join(".book-hidden");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::create_dir_all(&hidden).unwrap();
        std::fs::write(first.join("1.jpg"), []).unwrap();
        std::fs::write(second.join("10.jpg"), []).unwrap();
        std::fs::write(second.join("2.jpg"), []).unwrap();
        std::fs::write(hidden.join("0.jpg"), []).unwrap();

        assert_eq!(
            find_sibling_file(&first.join("1.jpg"), true),
            Some(second.join("2.jpg"))
        );
        assert_eq!(
            find_sibling_file(&second.join("2.jpg"), false),
            Some(first.join("1.jpg"))
        );
    }

    #[test]
    fn navigates_archive_files_as_individual_items() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("volume2.cbz");
        let second = root.path().join("volume10.cbz");
        std::fs::write(&first, []).unwrap();
        std::fs::write(&second, []).unwrap();

        assert_eq!(find_sibling_file(&first, true), Some(second.clone()));
        assert_eq!(find_sibling_file(&second, false), Some(first));
        assert_eq!(find_sibling_file(&second, true), None);
    }

    #[test]
    fn sibling_selection_matches_natural_sort_in_both_directions() {
        let directory = tempfile::tempdir().unwrap();
        for name in [
            "volume01.cbz",
            "volume1.cbz",
            "volume02.cbz",
            "volume2.cbz",
            "volume10.cbz",
            "volume11.cbz",
        ] {
            std::fs::write(directory.path().join(name), []).unwrap();
        }
        let mut ordered = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        sort_paths_naturally(&mut ordered);
        for (index, path) in ordered.iter().enumerate() {
            assert_eq!(
                find_sibling_file(path, false),
                index.checked_sub(1).map(|i| ordered[i].clone())
            );
            assert_eq!(
                find_sibling_file(path, true),
                ordered.get(index + 1).cloned()
            );
        }
    }

    #[test]
    fn image_folder_entry_uses_the_same_natural_order() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["page01.jpg", "page1.jpg", "page2.jpg", "page10.jpg"] {
            std::fs::write(directory.path().join(name), []).unwrap();
        }
        let mut ordered = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        sort_paths_naturally(&mut ordered);
        assert_eq!(
            find_first_image_in_dir(directory.path()),
            Some(ordered[0].clone())
        );
    }

    #[test]
    fn sibling_container_matches_archive_and_image_book_entities() {
        let root = Path::new("/books");
        assert_eq!(sibling_container(&root.join("01.cbz")), Some(root));
        assert_eq!(
            sibling_container(&root.join("01").join("001.jpg")),
            Some(root)
        );
        let work = root.join("work");
        assert_eq!(
            sibling_container(&work.join("01.cbz")),
            Some(work.as_path())
        );
        assert_eq!(
            sibling_container(&work.join("01").join("001.jpg")),
            Some(work.as_path())
        );
    }
}
