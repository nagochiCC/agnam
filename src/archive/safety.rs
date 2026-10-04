use crate::error::AppError;
use std::path::{Component, Path, PathBuf};

pub(crate) fn is_normal_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

pub(super) fn safe_archive_output_path(
    destination: &Path,
    entry_path: &Path,
) -> Result<PathBuf, AppError> {
    let mut output_path = destination.to_path_buf();
    let mut has_normal_component = false;

    for component in entry_path.components() {
        match component {
            Component::Normal(part) => {
                output_path.push(part);
                has_normal_component = true;
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AppError::Archive(format!(
                    "安全でないアーカイブ内パスです: {}",
                    entry_path.display()
                )));
            }
        }
    }

    if !has_normal_component {
        return Err(AppError::Archive(format!(
            "空のアーカイブ内パスです: {}",
            entry_path.display()
        )));
    }

    Ok(output_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_normal_relative_path_accepts_only_non_empty_normal_components() {
        for path in ["page.png", "nested/page.png", "a/b/c.cbz"] {
            assert!(is_normal_relative_path(Path::new(path)));
        }
        for path in [
            "",
            ".",
            "./nested/page.png",
            "../page.png",
            "nested/../page.png",
            "/page.png",
        ] {
            assert!(!is_normal_relative_path(Path::new(path)));
        }

        #[cfg(windows)]
        assert!(!is_normal_relative_path(Path::new(r"C:\page.png")));
    }

    #[test]
    fn accepts_only_relative_normal_components() {
        let destination = Path::new("output");
        assert_eq!(
            safe_archive_output_path(destination, Path::new("./nested/page.png")).unwrap(),
            destination.join("nested/page.png")
        );

        assert!(safe_archive_output_path(destination, Path::new("../page.png")).is_err());
        assert!(safe_archive_output_path(destination, Path::new("nested/../../page.png")).is_err());
        assert!(safe_archive_output_path(destination, Path::new("/page.png")).is_err());
        assert!(safe_archive_output_path(destination, Path::new(".")).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn rejects_windows_prefixes() {
        assert!(safe_archive_output_path(Path::new("output"), Path::new(r"C:\page.png")).is_err());
    }
}
