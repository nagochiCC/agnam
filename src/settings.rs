use crate::bookshelf::{LibrarySortDirection, LibrarySortKey};
use crate::viewer::{ThumbnailGenerationSpeed, ViewMode};
use gtk::glib;
use std::fs;
use std::path::{Path, PathBuf};

const SETTINGS_DIRECTORY: &str = "agnam";
const SETTINGS_FILENAME: &str = "settings.ini";
const SETTINGS_GROUP: &str = "Settings";
const SESSION_GROUP: &str = "Session";

pub(crate) const WINDOW_WIDTH_DEFAULT: i32 = 900;
pub(crate) const WINDOW_HEIGHT_DEFAULT: i32 = 700;

pub(crate) const LIBRARY_BOOK_HEIGHT_MIN: i32 = 81;
pub(crate) const LIBRARY_BOOK_HEIGHT_DEFAULT: i32 = 130;
pub(crate) const LIBRARY_BOOK_HEIGHT_MAX: i32 = 262;
const LEGACY_LIBRARY_FOLDER_PREVIEW_HEIGHT_MIN: i32 = 104;
const LEGACY_LIBRARY_FOLDER_PREVIEW_HEIGHT_MAX: i32 = 312;
const SETTINGS_FORMAT_VERSION: i32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LastSession {
    pub(crate) document_path: PathBuf,
    pub(crate) page_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ClickMode {
    #[default]
    ButtonBased,
    AreaBased,
}

impl ClickMode {
    fn storage_value(self) -> &'static str {
        match self {
            Self::ButtonBased => "button-based",
            Self::AreaBased => "area-based",
        }
    }

    fn from_storage_value(value: &str) -> Option<Self> {
        match value {
            "button-based" => Some(Self::ButtonBased),
            "area-based" => Some(Self::AreaBased),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PreviewPositionMode {
    #[default]
    FollowPointer,
    Centered,
}

impl PreviewPositionMode {
    fn storage_value(self) -> &'static str {
        match self {
            Self::Centered => "centered",
            Self::FollowPointer => "follow-pointer",
        }
    }

    fn from_storage_value(value: &str) -> Option<Self> {
        match value {
            "centered" => Some(Self::Centered),
            "follow-pointer" => Some(Self::FollowPointer),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum StartupBehavior {
    #[default]
    BookshelfTop,
    RestoreLastSession,
}

impl StartupBehavior {
    fn storage_value(self) -> &'static str {
        match self {
            Self::BookshelfTop => "bookshelf-top",
            Self::RestoreLastSession => "restore-last-session",
        }
    }

    fn from_storage_value(value: &str) -> Option<Self> {
        match value {
            "bookshelf-top" => Some(Self::BookshelfTop),
            "restore-last-session" => Some(Self::RestoreLastSession),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UserSettings {
    pub(crate) archive_expansion_limit: crate::archive::ArchiveExpansionLimit,
    pub(crate) click_mode: ClickMode,
    pub(crate) show_document_boundary_page: bool,
    pub(crate) scale_up: bool,
    pub(crate) smart_crop: bool,
    pub(crate) view_mode: ViewMode,
    pub(crate) slider_auto_hide: bool,
    pub(crate) header_auto_hide: bool,
    pub(crate) thumbnails_enabled: bool,
    pub(crate) thumbnail_generation_speed: ThumbnailGenerationSpeed,
    pub(crate) preview_position: PreviewPositionMode,
    pub(crate) bookshelf_root: Option<PathBuf>,
    pub(crate) library_book_height: i32,
    pub(crate) library_sort_key: LibrarySortKey,
    pub(crate) library_sort_direction: LibrarySortDirection,
    pub(crate) startup_behavior: StartupBehavior,
    pub(crate) last_session: Option<LastSession>,
    pub(crate) window_width: i32,
    pub(crate) window_height: i32,
    pub(crate) window_maximized: bool,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            archive_expansion_limit: Default::default(),
            click_mode: ClickMode::default(),
            show_document_boundary_page: true,
            scale_up: true,
            smart_crop: false,
            view_mode: ViewMode::default(),
            slider_auto_hide: true,
            header_auto_hide: false,
            thumbnails_enabled: true,
            thumbnail_generation_speed: ThumbnailGenerationSpeed::Normal,
            preview_position: PreviewPositionMode::default(),
            bookshelf_root: None,
            library_book_height: LIBRARY_BOOK_HEIGHT_DEFAULT,
            library_sort_key: LibrarySortKey::default(),
            library_sort_direction: LibrarySortDirection::default(),
            startup_behavior: StartupBehavior::default(),
            last_session: None,
            window_width: WINDOW_WIDTH_DEFAULT,
            window_height: WINDOW_HEIGHT_DEFAULT,
            window_maximized: false,
        }
    }
}

impl UserSettings {
    pub(crate) fn load() -> Self {
        Self::load_or_default_from_path(&settings_path())
    }

    pub(crate) fn save(&self) {
        let path = settings_path();
        if let Err(error) = self.save_to_path(&path) {
            eprintln!("設定を保存できませんでした ({}): {error}", path.display());
        }
    }

    pub(crate) fn update_window_state(
        &mut self,
        width: i32,
        height: i32,
        maximized: bool,
        fullscreened: bool,
    ) {
        if fullscreened {
            return;
        }
        self.window_maximized = maximized;

        if !maximized && width > 0 && height > 0 {
            self.window_width = width;
            self.window_height = height;
        }
    }

    fn load_or_default_from_path(path: &Path) -> Self {
        if !path.exists() {
            return Self::default();
        }

        match Self::load_from_path(path) {
            Ok(settings) => settings,
            Err(error) => {
                eprintln!(
                    "設定を読み込めないためデフォルト値を使用します ({}): {error}",
                    path.display()
                );
                Self::default()
            }
        }
    }

    fn load_from_path(path: &Path) -> Result<Self, glib::Error> {
        let key_file = glib::KeyFile::new();
        key_file.load_from_file(path, glib::KeyFileFlags::NONE)?;
        let defaults = Self::default();

        let archive_expansion_limit = key_file
            .integer(SETTINGS_GROUP, "archive-expansion-limit-gib")
            .ok()
            .and_then(crate::archive::ArchiveExpansionLimit::from_gib)
            .unwrap_or(defaults.archive_expansion_limit);

        let click_mode = key_file
            .string(SETTINGS_GROUP, "click-mode")
            .ok()
            .and_then(|value| ClickMode::from_storage_value(value.as_str()))
            .unwrap_or(defaults.click_mode);
        let show_document_boundary_page = key_file
            .boolean(SETTINGS_GROUP, "show-document-boundary-page")
            .unwrap_or(defaults.show_document_boundary_page);
        let scale_up = key_file
            .boolean(SETTINGS_GROUP, "scale-up")
            .unwrap_or(defaults.scale_up);
        let smart_crop = key_file
            .boolean(SETTINGS_GROUP, "smart-crop")
            .unwrap_or(defaults.smart_crop);
        let view_mode = key_file
            .string(SETTINGS_GROUP, "view-mode")
            .ok()
            .and_then(|value| match value.as_str() {
                "single" => Some(ViewMode::Single),
                "spread" => Some(ViewMode::Spread),
                _ => None,
            })
            .unwrap_or(defaults.view_mode);
        let slider_auto_hide = key_file
            .string(SETTINGS_GROUP, "slider-bar-mode")
            .ok()
            .and_then(|value| match value.as_str() {
                "overlay-auto-hide" => Some(true),
                "overlay-always-visible" | "reserved-space" => Some(false),
                _ => None,
            })
            .unwrap_or_else(|| {
                key_file
                    .boolean(SETTINGS_GROUP, "slider-auto-hide")
                    .unwrap_or(defaults.slider_auto_hide)
            });
        let header_auto_hide = key_file
            .boolean(SETTINGS_GROUP, "header-auto-hide")
            .unwrap_or(defaults.header_auto_hide);
        let thumbnails_enabled = key_file
            .boolean(SETTINGS_GROUP, "thumbnails-enabled")
            .unwrap_or(defaults.thumbnails_enabled);
        let thumbnail_generation_speed = key_file
            .string(SETTINGS_GROUP, "thumbnail-generation-speed")
            .ok()
            .and_then(|value| thumbnail_generation_speed_from_storage_value(value.as_str()))
            .unwrap_or(defaults.thumbnail_generation_speed);
        let preview_position = key_file
            .string(SETTINGS_GROUP, "preview-position")
            .ok()
            .and_then(|value| PreviewPositionMode::from_storage_value(value.as_str()))
            .unwrap_or(defaults.preview_position);
        let bookshelf_root = key_file
            .string(SETTINGS_GROUP, "bookshelf-root")
            .ok()
            .filter(|value| !value.is_empty())
            .map(|value| PathBuf::from(value.as_str()));
        let settings_format_version = key_file
            .integer(SETTINGS_GROUP, "format-version")
            .unwrap_or(1);
        let library_book_height = if settings_format_version >= SETTINGS_FORMAT_VERSION {
            load_bounded_integer(
                &key_file,
                "library-book-height",
                LIBRARY_BOOK_HEIGHT_MIN,
                LIBRARY_BOOK_HEIGHT_MAX,
                defaults.library_book_height,
            )
        } else if settings_format_version >= 2 {
            load_bounded_integer(
                &key_file,
                "library-folder-preview-height",
                LIBRARY_BOOK_HEIGHT_MIN,
                LIBRARY_BOOK_HEIGHT_MAX,
                defaults.library_book_height,
            )
        } else {
            key_file
                .integer(SETTINGS_GROUP, "library-folder-preview-height")
                .ok()
                .filter(|height| {
                    (LEGACY_LIBRARY_FOLDER_PREVIEW_HEIGHT_MIN
                        ..=LEGACY_LIBRARY_FOLDER_PREVIEW_HEIGHT_MAX)
                        .contains(height)
                })
                .map(legacy_folder_preview_image_height)
                .unwrap_or(defaults.library_book_height)
        };
        let library_sort_key = key_file
            .string(SETTINGS_GROUP, "library-sort-key")
            .ok()
            .and_then(|value| LibrarySortKey::from_storage_value(value.as_str()))
            .unwrap_or(defaults.library_sort_key);
        let library_sort_direction = key_file
            .string(SETTINGS_GROUP, "library-sort-direction")
            .ok()
            .and_then(|value| LibrarySortDirection::from_storage_value(value.as_str()))
            .unwrap_or(defaults.library_sort_direction);
        let startup_behavior = key_file
            .string(SETTINGS_GROUP, "startup-behavior")
            .ok()
            .and_then(|value| StartupBehavior::from_storage_value(value.as_str()))
            .unwrap_or(defaults.startup_behavior);
        let last_session = load_last_session(&key_file);
        let window_width = load_positive_integer(&key_file, "window-width", defaults.window_width);
        let window_height =
            load_positive_integer(&key_file, "window-height", defaults.window_height);
        let window_maximized = key_file
            .boolean(SETTINGS_GROUP, "window-maximized")
            .unwrap_or(defaults.window_maximized);

        Ok(Self {
            archive_expansion_limit,
            click_mode,
            show_document_boundary_page,
            scale_up,
            smart_crop,
            view_mode,
            slider_auto_hide,
            header_auto_hide,
            thumbnails_enabled,
            thumbnail_generation_speed,
            preview_position,
            bookshelf_root,
            library_book_height,
            library_sort_key,
            library_sort_direction,
            startup_behavior,
            last_session,
            window_width,
            window_height,
            window_maximized,
        })
    }

    fn save_to_path(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let key_file = glib::KeyFile::new();
        key_file.set_integer(
            SETTINGS_GROUP,
            "archive-expansion-limit-gib",
            self.archive_expansion_limit.gib(),
        );
        key_file.set_integer(SETTINGS_GROUP, "format-version", SETTINGS_FORMAT_VERSION);
        key_file.set_string(
            SETTINGS_GROUP,
            "click-mode",
            self.click_mode.storage_value(),
        );
        key_file.set_boolean(
            SETTINGS_GROUP,
            "show-document-boundary-page",
            self.show_document_boundary_page,
        );
        key_file.set_boolean(SETTINGS_GROUP, "scale-up", self.scale_up);
        key_file.set_boolean(SETTINGS_GROUP, "smart-crop", self.smart_crop);
        key_file.set_string(
            SETTINGS_GROUP,
            "view-mode",
            match self.view_mode {
                ViewMode::Single => "single",
                ViewMode::Spread => "spread",
            },
        );
        key_file.set_boolean(SETTINGS_GROUP, "slider-auto-hide", self.slider_auto_hide);
        key_file.set_boolean(SETTINGS_GROUP, "header-auto-hide", self.header_auto_hide);
        key_file.set_boolean(
            SETTINGS_GROUP,
            "thumbnails-enabled",
            self.thumbnails_enabled,
        );
        key_file.set_string(
            SETTINGS_GROUP,
            "thumbnail-generation-speed",
            thumbnail_generation_speed_storage_value(self.thumbnail_generation_speed),
        );
        key_file.set_string(
            SETTINGS_GROUP,
            "preview-position",
            self.preview_position.storage_value(),
        );
        if let Some(root) = &self.bookshelf_root {
            key_file.set_string(
                SETTINGS_GROUP,
                "bookshelf-root",
                root.to_string_lossy().as_ref(),
            );
        }
        key_file.set_integer(
            SETTINGS_GROUP,
            "library-book-height",
            self.library_book_height,
        );
        key_file.set_string(
            SETTINGS_GROUP,
            "library-sort-key",
            self.library_sort_key.storage_value(),
        );
        key_file.set_string(
            SETTINGS_GROUP,
            "library-sort-direction",
            self.library_sort_direction.storage_value(),
        );
        key_file.set_string(
            SETTINGS_GROUP,
            "startup-behavior",
            self.startup_behavior.storage_value(),
        );
        key_file.set_integer(SETTINGS_GROUP, "window-width", self.window_width);
        key_file.set_integer(SETTINGS_GROUP, "window-height", self.window_height);
        key_file.set_boolean(SETTINGS_GROUP, "window-maximized", self.window_maximized);
        if let Some(session) = &self.last_session {
            key_file.set_string(
                SESSION_GROUP,
                "last-document",
                session.document_path.to_string_lossy().as_ref(),
            );
            key_file.set_string(SESSION_GROUP, "last-page", &session.page_index.to_string());
        }
        key_file.save_to_file(path)?;
        Ok(())
    }
}

fn thumbnail_generation_speed_storage_value(speed: ThumbnailGenerationSpeed) -> &'static str {
    match speed {
        ThumbnailGenerationSpeed::Low => "low",
        ThumbnailGenerationSpeed::Normal => "normal",
        ThumbnailGenerationSpeed::High => "high",
    }
}

fn thumbnail_generation_speed_from_storage_value(value: &str) -> Option<ThumbnailGenerationSpeed> {
    match value {
        "low" => Some(ThumbnailGenerationSpeed::Low),
        "normal" => Some(ThumbnailGenerationSpeed::Normal),
        "high" => Some(ThumbnailGenerationSpeed::High),
        _ => None,
    }
}

fn load_positive_integer(key_file: &glib::KeyFile, key: &str, default: i32) -> i32 {
    key_file
        .integer(SETTINGS_GROUP, key)
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn load_bounded_integer(
    key_file: &glib::KeyFile,
    key: &str,
    minimum: i32,
    maximum: i32,
    default: i32,
) -> i32 {
    key_file
        .integer(SETTINGS_GROUP, key)
        .ok()
        .filter(|value| (minimum..=maximum).contains(value))
        .unwrap_or(default)
}

fn legacy_folder_preview_image_height(outer_height: i32) -> i32 {
    let tab_height = (outer_height / 9).clamp(8, 20);
    let inset = (outer_height / 20).clamp(6, 18);
    outer_height - tab_height - inset * 2
}

fn load_last_session(key_file: &glib::KeyFile) -> Option<LastSession> {
    let document_path = key_file.string(SESSION_GROUP, "last-document").ok()?;
    if document_path.is_empty() {
        return None;
    }
    let page_index = key_file
        .string(SESSION_GROUP, "last-page")
        .ok()?
        .parse::<usize>()
        .ok()?;
    Some(LastSession {
        document_path: PathBuf::from(document_path.as_str()),
        page_index,
    })
}

fn settings_path() -> PathBuf {
    glib::user_config_dir()
        .join(SETTINGS_DIRECTORY)
        .join(SETTINGS_FILENAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_expansion_limits_round_trip_and_invalid_values_use_four_gib() {
        use crate::archive::ArchiveExpansionLimit;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.ini");
        for limit in ArchiveExpansionLimit::ALL {
            let expected = UserSettings {
                archive_expansion_limit: limit,
                ..Default::default()
            };
            expected.save_to_path(&path).unwrap();
            assert_eq!(UserSettings::load_from_path(&path).unwrap(), expected);
            assert!(
                fs::read_to_string(&path)
                    .unwrap()
                    .contains(&format!("archive-expansion-limit-gib={}", limit.gib()))
            );
        }
        for value in [
            "",
            "0",
            "1",
            "3",
            "32",
            "-2",
            "unlimited",
            "4 GiB",
            "4294967296",
        ] {
            fs::write(
                &path,
                format!("[Settings]\nscale-up=false\narchive-expansion-limit-gib={value}\n"),
            )
            .unwrap();
            let settings = UserSettings::load_from_path(&path).unwrap();
            assert_eq!(
                settings.archive_expansion_limit,
                ArchiveExpansionLimit::GiB4,
                "{value}"
            );
            assert!(!settings.scale_up);
        }
        fs::write(&path, "[Settings]\nscale-up=false\n").unwrap();
        assert_eq!(
            UserSettings::load_from_path(&path)
                .unwrap()
                .archive_expansion_limit,
            ArchiveExpansionLimit::GiB4
        );
    }

    #[test]
    fn defaults_match_the_existing_application_behavior() {
        assert!(!UserSettings::default().header_auto_hide);
        assert!(!UserSettings::default().smart_crop);
        assert_eq!(
            UserSettings::default(),
            UserSettings {
                archive_expansion_limit: crate::archive::ArchiveExpansionLimit::GiB4,
                click_mode: ClickMode::default(),
                show_document_boundary_page: true,
                scale_up: true,
                smart_crop: false,
                view_mode: ViewMode::default(),
                slider_auto_hide: true,
                header_auto_hide: false,
                thumbnails_enabled: true,
                thumbnail_generation_speed: ThumbnailGenerationSpeed::Normal,
                preview_position: PreviewPositionMode::default(),
                bookshelf_root: None,
                library_book_height: LIBRARY_BOOK_HEIGHT_DEFAULT,
                library_sort_key: LibrarySortKey::Name,
                library_sort_direction: LibrarySortDirection::Ascending,
                startup_behavior: StartupBehavior::BookshelfTop,
                last_session: None,
                window_width: WINDOW_WIDTH_DEFAULT,
                window_height: WINDOW_HEIGHT_DEFAULT,
                window_maximized: false,
            }
        );
    }

    #[test]
    fn missing_settings_file_uses_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let settings = UserSettings::load_or_default_from_path(&directory.path().join("missing"));

        assert_eq!(settings, UserSettings::default());
    }

    #[test]
    fn settings_round_trip_through_the_key_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/settings.ini");
        let expected = UserSettings {
            archive_expansion_limit: crate::archive::ArchiveExpansionLimit::GiB8,
            click_mode: ClickMode::AreaBased,
            show_document_boundary_page: false,
            scale_up: true,
            smart_crop: true,
            view_mode: ViewMode::Spread,
            slider_auto_hide: true,
            header_auto_hide: true,
            thumbnails_enabled: false,
            thumbnail_generation_speed: ThumbnailGenerationSpeed::Normal,
            preview_position: PreviewPositionMode::FollowPointer,
            bookshelf_root: Some(PathBuf::from("/books/manga")),
            library_book_height: 140,
            library_sort_key: LibrarySortKey::Created,
            library_sort_direction: LibrarySortDirection::Descending,
            startup_behavior: StartupBehavior::RestoreLastSession,
            last_session: Some(LastSession {
                document_path: PathBuf::from("/books/漫画 1.cbz"),
                page_index: 37,
            }),
            window_width: 1200,
            window_height: 800,
            window_maximized: true,
        };

        expected.save_to_path(&path).unwrap();

        assert_eq!(UserSettings::load_or_default_from_path(&path), expected);

        let serialized = fs::read_to_string(path).unwrap();
        assert!(!serialized.contains("spread-shift"));
        assert!(serialized.contains("slider-auto-hide=true"));
        assert!(serialized.contains("header-auto-hide=true"));
        assert!(serialized.contains("thumbnail-generation-speed=normal"));
        assert!(serialized.contains("show-document-boundary-page=false"));
        assert!(serialized.contains("smart-crop=true"));
        assert!(!serialized.contains("sidebar-visible"));
        assert!(!serialized.contains("slider-bar-mode"));
        assert!(serialized.contains("bookshelf-root=/books/manga"));
        assert!(serialized.contains("format-version=3"));
        assert!(serialized.contains("library-book-height=140"));
        assert!(!serialized.contains("library-folder-preview-height"));
        assert!(!serialized.contains("library-cover-height"));
        assert!(serialized.contains("library-sort-key=created"));
        assert!(serialized.contains("library-sort-direction=descending"));
        assert!(serialized.contains("startup-behavior=restore-last-session"));
        assert!(serialized.contains("window-width=1200"));
        assert!(serialized.contains("window-height=800"));
        assert!(serialized.contains("window-maximized=true"));
        assert!(serialized.contains("[Session]"));
        assert!(serialized.contains("last-document=/books/漫画 1.cbz"));
        assert!(serialized.contains("last-page=37"));
    }

    #[test]
    fn legacy_folder_preview_height_is_migrated_to_image_height() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.ini");

        fs::write(
            &path,
            "[Settings]\nformat-version=1\nlibrary-folder-preview-height=222\n",
        )
        .unwrap();
        assert_eq!(
            UserSettings::load_or_default_from_path(&path).library_book_height,
            180
        );

        fs::write(&path, "[Settings]\nlibrary-folder-preview-height=222\n").unwrap();
        assert_eq!(
            UserSettings::load_or_default_from_path(&path).library_book_height,
            180
        );
    }

    #[test]
    fn version_two_folder_height_migrates_to_the_unified_book_height() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.ini");
        fs::write(
            &path,
            "[Settings]\nformat-version=2\nlibrary-folder-preview-height=180\nlibrary-cover-height=222\n",
        )
        .unwrap();

        let settings = UserSettings::load_or_default_from_path(&path);
        assert_eq!(settings.library_book_height, 180);
        settings.save_to_path(&path).unwrap();
        let serialized = fs::read_to_string(path).unwrap();
        assert!(serialized.contains("format-version=3"));
        assert!(serialized.contains("library-book-height=180"));
        assert!(!serialized.contains("library-folder-preview-height"));
    }

    #[test]
    fn document_boundary_page_setting_round_trips_in_both_states() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.ini");

        for enabled in [false, true] {
            let expected = UserSettings {
                show_document_boundary_page: enabled,
                ..UserSettings::default()
            };
            expected.save_to_path(&path).unwrap();
            assert_eq!(UserSettings::load_or_default_from_path(&path), expected);
        }
    }

    #[test]
    fn display_settings_round_trip_in_both_states() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.ini");

        for view_mode in [ViewMode::Single, ViewMode::Spread] {
            for thumbnails_enabled in [false, true] {
                for slider_auto_hide in [false, true] {
                    for header_auto_hide in [false, true] {
                        for preview_position in [
                            PreviewPositionMode::FollowPointer,
                            PreviewPositionMode::Centered,
                        ] {
                            let expected = UserSettings {
                                view_mode,
                                slider_auto_hide,
                                header_auto_hide,
                                thumbnails_enabled,
                                preview_position,
                                ..UserSettings::default()
                            };
                            expected.save_to_path(&path).unwrap();
                            assert_eq!(UserSettings::load_or_default_from_path(&path), expected);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn corrupted_settings_file_uses_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.ini");
        fs::write(&path, "[Settings\ninvalid").unwrap();

        assert_eq!(
            UserSettings::load_or_default_from_path(&path),
            UserSettings::default()
        );
    }

    #[test]
    fn invalid_individual_values_do_not_override_valid_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.ini");
        fs::write(
            &path,
            "[Settings]\nclick-mode=unknown\nscale-up=true\nview-mode=unknown\nthumbnails-enabled=invalid\nthumbnail-generation-speed=invalid\npreview-position=unknown\nlibrary-sort-key=unknown\nlibrary-sort-direction=unknown\nstartup-behavior=unknown\n",
        )
        .unwrap();

        assert_eq!(
            UserSettings::load_or_default_from_path(&path),
            UserSettings {
                scale_up: true,
                ..UserSettings::default()
            }
        );
    }

    #[test]
    fn old_settings_file_uses_defaults_for_new_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.ini");
        fs::write(
            &path,
            "[Settings]\nformat-version=1\nclick-mode=area-based\nscale-up=true\npreview-position=follow-pointer\n",
        )
        .unwrap();

        assert_eq!(
            UserSettings::load_or_default_from_path(&path),
            UserSettings {
                archive_expansion_limit: crate::archive::ArchiveExpansionLimit::GiB4,
                click_mode: ClickMode::AreaBased,
                show_document_boundary_page: true,
                scale_up: true,
                smart_crop: false,
                view_mode: ViewMode::Spread,
                slider_auto_hide: true,
                header_auto_hide: false,
                thumbnails_enabled: true,
                thumbnail_generation_speed: ThumbnailGenerationSpeed::Normal,
                preview_position: PreviewPositionMode::FollowPointer,
                bookshelf_root: None,
                library_book_height: LIBRARY_BOOK_HEIGHT_DEFAULT,
                library_sort_key: LibrarySortKey::Name,
                library_sort_direction: LibrarySortDirection::Ascending,
                startup_behavior: StartupBehavior::BookshelfTop,
                last_session: None,
                window_width: WINDOW_WIDTH_DEFAULT,
                window_height: WINDOW_HEIGHT_DEFAULT,
                window_maximized: false,
            }
        );
        assert!(UserSettings::load_or_default_from_path(&path).show_document_boundary_page);
        assert_eq!(
            UserSettings::load_or_default_from_path(&path).thumbnail_generation_speed,
            ThumbnailGenerationSpeed::Normal
        );
    }

    #[test]
    fn thumbnail_generation_speed_defaults_to_normal() {
        assert_eq!(
            UserSettings::default().thumbnail_generation_speed,
            ThumbnailGenerationSpeed::Normal
        );
    }

    #[test]
    fn thumbnail_generation_speed_round_trips_all_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.ini");

        for speed in [
            ThumbnailGenerationSpeed::Low,
            ThumbnailGenerationSpeed::Normal,
            ThumbnailGenerationSpeed::High,
        ] {
            let expected = UserSettings {
                thumbnail_generation_speed: speed,
                ..UserSettings::default()
            };
            expected.save_to_path(&path).unwrap();
            assert_eq!(
                UserSettings::load_or_default_from_path(&path).thumbnail_generation_speed,
                speed
            );
        }
    }

    #[test]
    fn settings_without_header_auto_hide_keep_it_disabled() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.ini");
        fs::write(&path, "[Settings]\nscale-up=false\n").unwrap();

        let settings = UserSettings::load_or_default_from_path(&path);
        assert!(!settings.header_auto_hide);
        assert!(!settings.scale_up);
    }

    #[test]
    fn obsolete_sidebar_key_is_ignored() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.ini");
        fs::write(&path, "[Settings]\nscale-up=false\nsidebar-visible=false\n").unwrap();

        let settings = UserSettings::load_or_default_from_path(&path);
        assert!(!settings.scale_up);

        settings.save_to_path(&path).unwrap();
        assert!(
            !fs::read_to_string(path)
                .unwrap()
                .contains("sidebar-visible")
        );
    }

    #[test]
    fn maximized_state_does_not_replace_the_normal_window_size() {
        let mut settings = UserSettings::default();

        settings.update_window_state(1200, 800, false, false);
        assert_eq!((settings.window_width, settings.window_height), (1200, 800));

        settings.update_window_state(1200, 800, true, false);
        settings.update_window_state(1920, 1080, true, false);
        assert_eq!((settings.window_width, settings.window_height), (1200, 800));
        assert!(settings.window_maximized);

        settings.update_window_state(1200, 800, false, false);
        assert_eq!((settings.window_width, settings.window_height), (1200, 800));
        assert!(!settings.window_maximized);

        settings.update_window_state(1300, 850, false, false);
        settings.update_window_state(1300, 850, true, false);
        settings.update_window_state(1920, 1080, true, false);
        assert_eq!((settings.window_width, settings.window_height), (1300, 850));
    }

    #[test]
    fn fullscreen_state_does_not_replace_normal_window_state() {
        let mut settings = UserSettings::default();
        settings.update_window_state(1200, 800, false, false);
        settings.update_window_state(1200, 800, true, false);

        settings.update_window_state(1920, 1080, false, true);

        assert_eq!((settings.window_width, settings.window_height), (1200, 800));
        assert!(settings.window_maximized);
    }

    #[test]
    fn invalid_or_missing_window_values_use_existing_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.ini");
        fs::write(
            &path,
            "[Settings]\nwindow-width=0\nwindow-height=invalid\nwindow-maximized=invalid\n",
        )
        .unwrap();

        let settings = UserSettings::load_or_default_from_path(&path);
        assert_eq!(settings.window_width, WINDOW_WIDTH_DEFAULT);
        assert_eq!(settings.window_height, WINDOW_HEIGHT_DEFAULT);
        assert!(!settings.window_maximized);
    }

    #[test]
    fn unused_legacy_key_is_ignored_and_removed_on_save() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.ini");
        let retired_key = ["library", "display", "mode"].join("-");
        fs::write(
            &path,
            format!("[Settings]\nscale-up=false\n{retired_key}=retired\n"),
        )
        .unwrap();

        let settings = UserSettings::load_or_default_from_path(&path);
        assert!(!settings.scale_up);

        settings.save_to_path(&path).unwrap();
        assert!(!fs::read_to_string(path).unwrap().contains(&retired_key));
    }

    #[test]
    fn invalid_library_book_height_uses_the_default() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.ini");

        for height in ["80", "263", "invalid"] {
            fs::write(
                &path,
                format!(
                    "[Settings]\nformat-version=3\nscale-up=false\nlibrary-book-height={height}\n"
                ),
            )
            .unwrap();
            let settings = UserSettings::load_or_default_from_path(&path);
            assert_eq!(settings.library_book_height, LIBRARY_BOOK_HEIGHT_DEFAULT);
            assert!(!settings.scale_up);
        }
    }

    #[test]
    fn incomplete_or_invalid_session_is_ignored() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.ini");

        for session in [
            "last-document=/books/1.cbz\n",
            "last-page=10\n",
            "last-document=\nlast-page=10\n",
            "last-document=/books/1.cbz\nlast-page=invalid\n",
            "last-document=/books/1.cbz\nlast-page=999999999999999999999999999999999999\n",
        ] {
            fs::write(
                &path,
                format!("[Settings]\nscale-up=false\n[Session]\n{session}"),
            )
            .unwrap();
            let settings = UserSettings::load_or_default_from_path(&path);
            assert_eq!(settings.last_session, None);
            assert!(!settings.scale_up);
        }
    }

    #[test]
    fn ordinary_settings_save_preserves_session() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.ini");
        let session = LastSession {
            document_path: PathBuf::from("/本 棚/作品/01.cbz"),
            page_index: usize::MAX,
        };
        let mut settings = UserSettings {
            last_session: Some(session.clone()),
            ..UserSettings::default()
        };
        settings.save_to_path(&path).unwrap();
        settings.scale_up = false;
        settings.save_to_path(&path).unwrap();

        let loaded = UserSettings::load_or_default_from_path(&path);
        assert_eq!(loaded.last_session, Some(session));
        assert!(!loaded.scale_up);
    }

    #[test]
    fn previous_slider_modes_migrate_to_the_two_current_states() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.ini");

        for (old_mode, expected_auto_hide) in [
            ("overlay-auto-hide", true),
            ("overlay-always-visible", false),
            ("reserved-space", false),
        ] {
            fs::write(&path, format!("[Settings]\nslider-bar-mode={old_mode}\n")).unwrap();
            assert_eq!(
                UserSettings::load_or_default_from_path(&path).slider_auto_hide,
                expected_auto_hide
            );
        }
    }
}
