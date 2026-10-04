use super::formats;
use crate::error::AppError;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArchiveAccessStrategy {
    RandomAccess,
    Sequential,
}

/// Returns the access strategy selected by the archive backend.
///
/// A backend may inspect an individual archive when its strategy depends on
/// archive metadata. Metadata failures are propagated instead of being treated
/// as a valid strategy. `None` means the path is not a recognized archive.
pub(crate) fn archive_access_strategy(
    path: &Path,
) -> Result<Option<ArchiveAccessStrategy>, AppError> {
    formats::access_strategy(path)
}

/// I/O-free capability check used before starting a background load.
pub(crate) fn archive_supports_sequential_progress(path: &Path) -> bool {
    formats::supports_sequential_progress(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_backends_select_strategy_behind_the_archive_boundary() {
        assert_eq!(
            archive_access_strategy(Path::new("pages.cbz")).unwrap(),
            Some(ArchiveAccessStrategy::RandomAccess)
        );
        assert_eq!(
            archive_access_strategy(Path::new("pages.rar")).unwrap(),
            Some(ArchiveAccessStrategy::Sequential)
        );
        assert_eq!(
            archive_access_strategy(Path::new("pages.tar")).unwrap(),
            Some(ArchiveAccessStrategy::Sequential)
        );
        assert_eq!(
            archive_access_strategy(Path::new("pages.cbt")).unwrap(),
            Some(ArchiveAccessStrategy::Sequential)
        );
        assert_eq!(
            archive_access_strategy(Path::new("pages.lha")).unwrap(),
            Some(ArchiveAccessStrategy::Sequential)
        );
        assert_eq!(
            archive_access_strategy(Path::new("pages.lzh")).unwrap(),
            Some(ArchiveAccessStrategy::Sequential)
        );
        assert_eq!(
            archive_access_strategy(Path::new("pages.png")).unwrap(),
            None
        );
    }

    #[test]
    fn sequential_capability_check_requires_no_archive_io() {
        assert!(archive_supports_sequential_progress(Path::new(
            "missing.rar"
        )));
        assert!(archive_supports_sequential_progress(Path::new(
            "missing.7z"
        )));
        assert!(!archive_supports_sequential_progress(Path::new(
            "missing.cbz"
        )));
        assert!(!archive_supports_sequential_progress(Path::new(
            "missing.tar"
        )));
        assert!(!archive_supports_sequential_progress(Path::new(
            "missing.cbt"
        )));
        assert!(!archive_supports_sequential_progress(Path::new(
            "missing.lha"
        )));
        assert!(!archive_supports_sequential_progress(Path::new(
            "missing.lzh"
        )));
        assert!(!archive_supports_sequential_progress(Path::new(
            "missing.png"
        )));
    }
}
