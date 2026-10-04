use crate::archive::{is_image_document_entry_path, is_image_ext};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum HistoryIdentity {
    BookshelfWork(PathBuf),
    Document(PathBuf),
}

impl HistoryIdentity {
    pub(super) fn storage_kind(&self) -> &'static str {
        match self {
            Self::BookshelfWork(_) => "bookshelf-work",
            Self::Document(_) => "document",
        }
    }

    pub(super) fn path(&self) -> &Path {
        match self {
            Self::BookshelfWork(path) | Self::Document(path) => path,
        }
    }
}

pub(crate) fn history_identity_for_document(
    document_path: &Path,
    bookshelf_root: Option<&Path>,
) -> HistoryIdentity {
    classify_document(document_path, bookshelf_root).0
}

pub(super) fn classify_document(
    document_path: &Path,
    bookshelf_root: Option<&Path>,
) -> (HistoryIdentity, Option<PathBuf>, String) {
    if let (Some(root), Some(parent)) = (bookshelf_root, document_path.parent())
        && document_path.starts_with(root)
        && parent != root
    {
        let work_path = if is_image_ext(document_path) {
            image_work_path(parent, root).to_path_buf()
        } else {
            parent.to_path_buf()
        };
        return (
            HistoryIdentity::BookshelfWork(work_path.clone()),
            Some(work_path.clone()),
            display_name(&work_path, false),
        );
    }

    (
        HistoryIdentity::Document(document_path.to_path_buf()),
        None,
        display_name(document_path, true),
    )
}

fn image_work_path<'a>(image_folder: &'a Path, bookshelf_root: &Path) -> &'a Path {
    let Some(candidate) = image_folder
        .parent()
        .filter(|candidate| *candidate != bookshelf_root && candidate.starts_with(bookshelf_root))
    else {
        return image_folder;
    };
    if image_folder
        .file_name()
        .is_some_and(|name| is_volume_directory_name(&name.to_string_lossy()))
        || has_sibling_image_document(candidate, image_folder)
    {
        candidate
    } else {
        image_folder
    }
}

fn has_sibling_image_document(parent: &Path, image_folder: &Path) -> bool {
    fs::read_dir(parent).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            let path = entry.path();
            path != image_folder
                && path.is_dir()
                && fs::read_dir(&path).is_ok_and(|images| {
                    images.filter_map(Result::ok).any(|image| {
                        image.file_type().is_ok_and(|file_type| file_type.is_file())
                            && is_image_document_entry_path(&image.path())
                    })
                })
        })
    })
}

pub(crate) fn is_volume_directory_name(name: &str) -> bool {
    fn is_ascii_number(value: &str) -> bool {
        !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
    }

    if is_ascii_number(name) {
        return true;
    }
    if let Some(number) = name.strip_suffix('巻') {
        return is_ascii_number(number.strip_prefix('第').unwrap_or(number));
    }

    let lowercase = name.to_ascii_lowercase();
    ["volume", "vol"].into_iter().any(|prefix| {
        lowercase.strip_prefix(prefix).is_some_and(|number| {
            let number = number
                .strip_prefix('.')
                .or_else(|| number.strip_prefix(' '))
                .unwrap_or(number);
            is_ascii_number(number)
        })
    })
}

pub(super) fn display_name(path: &Path, remove_extension: bool) -> String {
    let preferred = if remove_extension {
        path.file_stem()
    } else {
        path.file_name()
    };
    preferred
        .filter(|name| !name.is_empty())
        .or_else(|| path.file_name().filter(|name| !name.is_empty()))
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| {
            let fallback = path.to_string_lossy().into_owned();
            if fallback.is_empty() {
                "名称不明".to_owned()
            } else {
                fallback
            }
        })
}
