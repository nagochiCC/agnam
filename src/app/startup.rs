use crate::settings::{LastSession, StartupBehavior, UserSettings};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StartupAction {
    SelectBookshelfRoot,
    ShowBookshelf(PathBuf),
    Restore {
        bookshelf_root: PathBuf,
        session: LastSession,
    },
}

pub(super) fn startup_action(settings: &UserSettings) -> StartupAction {
    let Some(bookshelf_root) = settings.bookshelf_root.clone() else {
        return StartupAction::SelectBookshelfRoot;
    };

    match (settings.startup_behavior, settings.last_session.clone()) {
        (StartupBehavior::RestoreLastSession, Some(session)) => StartupAction::Restore {
            bookshelf_root,
            session,
        },
        _ => StartupAction::ShowBookshelf(bookshelf_root),
    }
}

pub(super) fn update_last_session(
    last_session: &mut Option<LastSession>,
    document_path: Option<&Path>,
    page_count: usize,
    current_index: usize,
) -> bool {
    let Some(document_path) = document_path else {
        return false;
    };
    if page_count == 0 || current_index >= page_count {
        return false;
    }
    *last_session = Some(LastSession {
        document_path: document_path.to_owned(),
        page_index: current_index,
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(root: Option<&str>, behavior: StartupBehavior, session: bool) -> UserSettings {
        UserSettings {
            bookshelf_root: root.map(PathBuf::from),
            startup_behavior: behavior,
            last_session: session.then(|| LastSession {
                document_path: PathBuf::from("/books/01.cbz"),
                page_index: 12,
            }),
            ..UserSettings::default()
        }
    }

    #[test]
    fn root_selection_takes_priority_over_restore() {
        assert_eq!(
            startup_action(&settings(None, StartupBehavior::RestoreLastSession, true)),
            StartupAction::SelectBookshelfRoot
        );
    }

    #[test]
    fn bookshelf_top_and_missing_session_open_the_root() {
        for settings in [
            settings(Some("/books"), StartupBehavior::BookshelfTop, true),
            settings(Some("/books"), StartupBehavior::RestoreLastSession, false),
        ] {
            assert_eq!(
                startup_action(&settings),
                StartupAction::ShowBookshelf(PathBuf::from("/books"))
            );
        }
    }

    #[test]
    fn restore_action_keeps_the_root_and_session() {
        let settings = settings(Some("/books"), StartupBehavior::RestoreLastSession, true);
        assert_eq!(
            startup_action(&settings),
            StartupAction::Restore {
                bookshelf_root: PathBuf::from("/books"),
                session: settings.last_session.unwrap(),
            }
        );
    }

    #[test]
    fn valid_viewer_state_updates_snapshot_but_missing_document_preserves_it() {
        let previous = LastSession {
            document_path: PathBuf::from("/old.cbz"),
            page_index: 7,
        };
        let mut session = Some(previous.clone());
        assert!(!update_last_session(&mut session, None, 0, 0));
        assert_eq!(session, Some(previous));

        assert!(update_last_session(
            &mut session,
            Some(Path::new("/new.cbz")),
            20,
            11,
        ));
        assert_eq!(
            session,
            Some(LastSession {
                document_path: PathBuf::from("/new.cbz"),
                page_index: 11,
            })
        );
        assert!(!update_last_session(
            &mut session,
            Some(Path::new("/empty.cbz")),
            0,
            0,
        ));
    }
}
