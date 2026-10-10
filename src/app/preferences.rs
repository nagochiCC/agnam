use super::{AppSender, Msg};
use crate::settings::{
    ClickMode, LIBRARY_BOOK_HEIGHT_MAX, LIBRARY_BOOK_HEIGHT_MIN, PreviewPositionMode,
    StartupBehavior, UserSettings,
};
use crate::viewer::{ThumbnailGenerationSpeed, ViewMode};
use adw::prelude::*;
use gtk::glib::SignalHandlerId;

trait SwitchRowSync {
    type Handler;

    fn block_sync_signal(&self, handler: &Self::Handler);
    fn set_sync_active(&self, active: bool);
    fn unblock_sync_signal(&self, handler: &Self::Handler);
}

impl SwitchRowSync for adw::SwitchRow {
    type Handler = SignalHandlerId;

    fn block_sync_signal(&self, handler: &Self::Handler) {
        self.block_signal(handler);
    }

    fn set_sync_active(&self, active: bool) {
        self.set_active(active);
    }

    fn unblock_sync_signal(&self, handler: &Self::Handler) {
        self.unblock_signal(handler);
    }
}

fn sync_switch_row<R: SwitchRowSync>(row: &R, handler: &R::Handler, active: bool) {
    row.block_sync_signal(handler);
    row.set_sync_active(active);
    row.unblock_sync_signal(handler);
}

pub(super) struct SettingsDialog {
    dialog: adw::PreferencesDialog,
    archive_expansion_limit_row: adw::ComboRow,
    archive_expansion_limit_handler: SignalHandlerId,
    click_mode_row: adw::ComboRow,
    show_document_boundary_page_row: adw::SwitchRow,
    scale_up_row: adw::SwitchRow,
    smart_crop_row: adw::SwitchRow,
    view_mode_row: adw::SwitchRow,
    header_auto_hide_row: adw::SwitchRow,
    slider_auto_hide_row: adw::SwitchRow,
    thumbnails_enabled_row: adw::SwitchRow,
    preview_position_row: adw::ComboRow,
    thumbnail_generation_speed_row: adw::ComboRow,
    bookshelf_root_row: adw::ActionRow,
    bookshelf_root_button: gtk::Button,
    library_book_height_row: adw::SpinRow,
    startup_behavior_row: adw::ComboRow,
    click_mode_handler: SignalHandlerId,
    show_document_boundary_page_handler: SignalHandlerId,
    scale_up_handler: SignalHandlerId,
    smart_crop_handler: SignalHandlerId,
    view_mode_handler: SignalHandlerId,
    header_auto_hide_handler: SignalHandlerId,
    slider_auto_hide_handler: SignalHandlerId,
    thumbnails_enabled_handler: SignalHandlerId,
    preview_position_handler: SignalHandlerId,
    thumbnail_generation_speed_handler: SignalHandlerId,
    library_book_height_handler: SignalHandlerId,
    startup_behavior_handler: SignalHandlerId,
}

impl SettingsDialog {
    pub(super) fn new(settings: &UserSettings, sender: AppSender) -> Self {
        let click_mode_options = gtk::StringList::new(&["マウスボタン", "クリック位置"]);
        let click_mode_row = adw::ComboRow::builder()
            .title("ページ移動方式")
            .model(&click_mode_options)
            .selected(click_mode_index(settings.click_mode))
            .build();

        let scale_up_row = adw::SwitchRow::builder()
            .title("小さい画像を拡大する")
            .active(settings.scale_up)
            .build();

        let smart_crop_row = adw::SwitchRow::builder()
            .title("スマートcrop")
            .subtitle("画像外周の不要な余白を自動的に除去します")
            .active(settings.smart_crop)
            .build();

        let view_mode_row = adw::SwitchRow::builder()
            .title("見開き表示")
            .active(settings.view_mode == ViewMode::Spread)
            .build();

        let viewing_group = adw::PreferencesGroup::builder().title("ページ表示").build();
        viewing_group.add(&view_mode_row);
        viewing_group.add(&scale_up_row);
        viewing_group.add(&smart_crop_row);

        let page_navigation_group = adw::PreferencesGroup::builder().title("ページ移動").build();
        page_navigation_group.add(&click_mode_row);

        let show_document_boundary_page_row = adw::SwitchRow::builder()
            .title("ファイル切り替えページを表示")
            .subtitle("ページ送りで前後のファイルへ移動する際に表示します")
            .active(settings.show_document_boundary_page)
            .build();
        page_navigation_group.add(&show_document_boundary_page_row);

        let header_auto_hide_row = adw::SwitchRow::builder()
            .title("ヘッダーバーを自動的に隠す")
            .active(settings.header_auto_hide)
            .build();

        let slider_auto_hide_row = adw::SwitchRow::builder()
            .title("スライダーを自動的に隠す")
            .active(settings.slider_auto_hide)
            .build();

        let display_group = adw::PreferencesGroup::builder()
            .title("インターフェース")
            .build();
        display_group.add(&header_auto_hide_row);
        display_group.add(&slider_auto_hide_row);

        let thumbnails_enabled_row = adw::SwitchRow::builder()
            .title("ページプレビューを表示")
            .subtitle("スライダー操作時にページのプレビューを表示します")
            .active(settings.thumbnails_enabled)
            .build();

        let preview_position_options = gtk::StringList::new(&["マウス追従", "中央"]);
        let preview_position_row = adw::ComboRow::builder()
            .title("プレビューの表示位置")
            .model(&preview_position_options)
            .selected(preview_position_index(settings.preview_position))
            .sensitive(settings.thumbnails_enabled)
            .build();

        let thumbnail_generation_speed_options = gtk::StringList::new(&["低速", "標準", "高速"]);
        let thumbnail_generation_speed_row = adw::ComboRow::builder()
            .title("サムネイル生成速度")
            .subtitle("速度を下げるとCPU負荷と発熱を抑えます")
            .model(&thumbnail_generation_speed_options)
            .selected(thumbnail_generation_speed_index(
                settings.thumbnail_generation_speed,
            ))
            .sensitive(settings.thumbnails_enabled)
            .build();

        let page_preview_group = adw::PreferencesGroup::builder()
            .title("ページプレビュー")
            .build();
        page_preview_group.add(&thumbnails_enabled_row);
        page_preview_group.add(&preview_position_row);
        page_preview_group.add(&thumbnail_generation_speed_row);

        let bookshelf_root_row = adw::ActionRow::builder()
            .title("本棚フォルダ")
            .subtitle(bookshelf_root_text(settings))
            .build();

        let bookshelf_root_button = gtk::Button::with_label(bookshelf_root_button_text(settings));
        bookshelf_root_button.set_valign(gtk::Align::Center);
        bookshelf_root_row.add_suffix(&bookshelf_root_button);
        bookshelf_root_row.set_activatable_widget(Some(&bookshelf_root_button));

        let bookshelf_group = adw::PreferencesGroup::builder().title("本棚").build();
        bookshelf_group.add(&bookshelf_root_row);

        let book_height_adjustment = gtk::Adjustment::new(
            f64::from(settings.library_book_height),
            f64::from(LIBRARY_BOOK_HEIGHT_MIN),
            f64::from(LIBRARY_BOOK_HEIGHT_MAX),
            1.0,
            10.0,
            0.0,
        );

        let library_book_height_row = adw::SpinRow::new(Some(&book_height_adjustment), 1.0, 0);
        library_book_height_row.set_title("本棚の本の高さ");
        library_book_height_row.set_subtitle("本棚に表示する本の基準高さ");
        bookshelf_group.add(&library_book_height_row);

        let startup_behavior_options =
            gtk::StringList::new(&["本棚トップ", "前回の閲覧位置を復元"]);
        let startup_behavior_row = adw::ComboRow::builder()
            .title("起動時に表示する画面")
            .model(&startup_behavior_options)
            .selected(startup_behavior_index(settings.startup_behavior))
            .build();

        let startup_group = adw::PreferencesGroup::builder().title("起動").build();
        startup_group.add(&startup_behavior_row);

        let archive_expansion_limit_row = adw::ComboRow::builder()
            .title("最大展開データ量")
            .subtitle("上限を引き上げると一時ディスク使用量が増える場合があります")
            .model(&gtk::StringList::new(&[
                "2 GiB", "4 GiB", "8 GiB", "16 GiB",
            ]))
            .selected(archive_expansion_limit_index(
                settings.archive_expansion_limit,
            ))
            .build();
        let archive_group = adw::PreferencesGroup::builder().title("アーカイブ").build();
        archive_group.add(&archive_expansion_limit_row);

        let archive_expansion_limit_handler =
            archive_expansion_limit_row.connect_selected_notify({
                let sender = sender.clone();
                move |row| {
                    if let Some(limit) =
                        crate::archive::ArchiveExpansionLimit::ALL.get(row.selected() as usize)
                    {
                        sender.input(Msg::SetArchiveExpansionLimit(*limit));
                    }
                }
            });

        let page = adw::PreferencesPage::builder().title("設定").build();
        page.add(&viewing_group);
        page.add(&page_navigation_group);
        page.add(&display_group);
        page.add(&page_preview_group);
        page.add(&archive_group);
        page.add(&bookshelf_group);
        page.add(&startup_group);

        let dialog = adw::PreferencesDialog::builder()
            .content_width(560)
            .content_height(650)
            .build();
        dialog.add(&page);

        let click_mode_handler = click_mode_row.connect_selected_notify({
            let sender = sender.clone();
            move |row| {
                if let Some(mode) = click_mode_from_index(row.selected()) {
                    sender.input(Msg::SetClickMode(mode));
                }
            }
        });

        let show_document_boundary_page_handler = show_document_boundary_page_row
            .connect_active_notify({
                let sender = sender.clone();
                move |row| sender.input(Msg::SetShowDocumentBoundaryPage(row.is_active()))
            });

        let scale_up_handler = scale_up_row.connect_active_notify({
            let sender = sender.clone();
            move |row| sender.input(Msg::SetScaleUp(row.is_active()))
        });

        let smart_crop_handler = smart_crop_row.connect_active_notify({
            let sender = sender.clone();
            move |row| sender.input(Msg::SetSmartCrop(row.is_active()))
        });

        let view_mode_handler = view_mode_row.connect_active_notify({
            let sender = sender.clone();
            move |row| {
                let mode = if row.is_active() {
                    ViewMode::Spread
                } else {
                    ViewMode::Single
                };
                sender.input(Msg::SetViewMode(mode));
            }
        });

        let slider_auto_hide_handler = slider_auto_hide_row.connect_active_notify({
            let sender = sender.clone();
            move |row| sender.input(Msg::SetSliderAutoHide(row.is_active()))
        });

        let header_auto_hide_handler = header_auto_hide_row.connect_active_notify({
            let sender = sender.clone();
            move |row| sender.input(Msg::SetHeaderAutoHide(row.is_active()))
        });

        let thumbnails_enabled_handler = thumbnails_enabled_row.connect_active_notify({
            let sender = sender.clone();
            move |row| sender.input(Msg::SetThumbnailsEnabled(row.is_active()))
        });

        let preview_position_handler = preview_position_row.connect_selected_notify({
            let sender = sender.clone();
            move |row| {
                if let Some(mode) = preview_position_from_index(row.selected()) {
                    sender.input(Msg::SetPreviewPosition(mode));
                }
            }
        });

        let thumbnail_generation_speed_handler = thumbnail_generation_speed_row
            .connect_selected_notify({
                let sender = sender.clone();
                move |row| {
                    if let Some(speed) = thumbnail_generation_speed_from_index(row.selected()) {
                        sender.input(Msg::SetThumbnailGenerationSpeed(speed));
                    }
                }
            });

        let startup_behavior_handler = startup_behavior_row.connect_selected_notify({
            let sender = sender.clone();
            move |row| {
                if let Some(behavior) = startup_behavior_from_index(row.selected()) {
                    sender.input(Msg::SetStartupBehavior(behavior));
                }
            }
        });

        let library_book_height_handler = library_book_height_row.connect_value_notify({
            let sender = sender.clone();
            move |row| sender.input(Msg::SetLibraryBookHeight(row.value() as i32))
        });

        bookshelf_root_button.connect_clicked(move |_| sender.input(Msg::SelectBookshelfRoot));

        Self {
            archive_expansion_limit_row,
            archive_expansion_limit_handler,
            dialog,
            click_mode_row,
            show_document_boundary_page_row,
            scale_up_row,
            smart_crop_row,
            view_mode_row,
            header_auto_hide_row,
            slider_auto_hide_row,
            thumbnails_enabled_row,
            preview_position_row,
            thumbnail_generation_speed_row,
            bookshelf_root_row,
            bookshelf_root_button,
            library_book_height_row,
            startup_behavior_row,
            click_mode_handler,
            show_document_boundary_page_handler,
            scale_up_handler,
            smart_crop_handler,
            view_mode_handler,
            header_auto_hide_handler,
            slider_auto_hide_handler,
            thumbnails_enabled_handler,
            preview_position_handler,
            thumbnail_generation_speed_handler,
            library_book_height_handler,
            startup_behavior_handler,
        }
    }

    pub(super) fn dialog(&self) -> &adw::PreferencesDialog {
        &self.dialog
    }

    pub(super) fn sync(&self, settings: &UserSettings) {
        self.archive_expansion_limit_row
            .block_signal(&self.archive_expansion_limit_handler);
        self.archive_expansion_limit_row
            .set_selected(archive_expansion_limit_index(
                settings.archive_expansion_limit,
            ));
        self.archive_expansion_limit_row
            .unblock_signal(&self.archive_expansion_limit_handler);
        self.click_mode_row.block_signal(&self.click_mode_handler);
        self.click_mode_row
            .set_selected(click_mode_index(settings.click_mode));
        self.click_mode_row.unblock_signal(&self.click_mode_handler);

        sync_switch_row(
            &self.show_document_boundary_page_row,
            &self.show_document_boundary_page_handler,
            settings.show_document_boundary_page,
        );

        sync_switch_row(
            &self.scale_up_row,
            &self.scale_up_handler,
            settings.scale_up,
        );

        sync_switch_row(
            &self.smart_crop_row,
            &self.smart_crop_handler,
            settings.smart_crop,
        );

        sync_switch_row(
            &self.view_mode_row,
            &self.view_mode_handler,
            settings.view_mode == ViewMode::Spread,
        );

        sync_switch_row(
            &self.header_auto_hide_row,
            &self.header_auto_hide_handler,
            settings.header_auto_hide,
        );

        sync_switch_row(
            &self.slider_auto_hide_row,
            &self.slider_auto_hide_handler,
            settings.slider_auto_hide,
        );

        sync_switch_row(
            &self.thumbnails_enabled_row,
            &self.thumbnails_enabled_handler,
            settings.thumbnails_enabled,
        );
        self.preview_position_row
            .set_sensitive(settings.thumbnails_enabled);
        self.thumbnail_generation_speed_row
            .set_sensitive(settings.thumbnails_enabled);

        self.preview_position_row
            .block_signal(&self.preview_position_handler);
        self.preview_position_row
            .set_selected(preview_position_index(settings.preview_position));
        self.preview_position_row
            .unblock_signal(&self.preview_position_handler);

        self.thumbnail_generation_speed_row
            .block_signal(&self.thumbnail_generation_speed_handler);
        self.thumbnail_generation_speed_row
            .set_selected(thumbnail_generation_speed_index(
                settings.thumbnail_generation_speed,
            ));
        self.thumbnail_generation_speed_row
            .unblock_signal(&self.thumbnail_generation_speed_handler);

        self.bookshelf_root_row
            .set_subtitle(&bookshelf_root_text(settings));
        self.bookshelf_root_button
            .set_label(bookshelf_root_button_text(settings));

        self.library_book_height_row
            .block_signal(&self.library_book_height_handler);
        self.library_book_height_row
            .set_value(f64::from(settings.library_book_height));
        self.library_book_height_row
            .unblock_signal(&self.library_book_height_handler);

        self.startup_behavior_row
            .block_signal(&self.startup_behavior_handler);
        self.startup_behavior_row
            .set_selected(startup_behavior_index(settings.startup_behavior));
        self.startup_behavior_row
            .unblock_signal(&self.startup_behavior_handler);
    }
}

fn bookshelf_root_text(settings: &UserSettings) -> String {
    settings
        .bookshelf_root
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "未設定".into())
}

fn bookshelf_root_button_text(settings: &UserSettings) -> &'static str {
    if settings.bookshelf_root.is_some() {
        "変更…"
    } else {
        "選択…"
    }
}

fn click_mode_index(mode: ClickMode) -> u32 {
    match mode {
        ClickMode::ButtonBased => 0,
        ClickMode::AreaBased => 1,
    }
}

fn click_mode_from_index(index: u32) -> Option<ClickMode> {
    match index {
        0 => Some(ClickMode::ButtonBased),
        1 => Some(ClickMode::AreaBased),
        _ => None,
    }
}

fn preview_position_index(mode: PreviewPositionMode) -> u32 {
    match mode {
        PreviewPositionMode::FollowPointer => 0,
        PreviewPositionMode::Centered => 1,
    }
}

fn preview_position_from_index(index: u32) -> Option<PreviewPositionMode> {
    match index {
        0 => Some(PreviewPositionMode::FollowPointer),
        1 => Some(PreviewPositionMode::Centered),
        _ => None,
    }
}

fn thumbnail_generation_speed_index(speed: ThumbnailGenerationSpeed) -> u32 {
    match speed {
        ThumbnailGenerationSpeed::Low => 0,
        ThumbnailGenerationSpeed::Normal => 1,
        ThumbnailGenerationSpeed::High => 2,
    }
}

fn thumbnail_generation_speed_from_index(index: u32) -> Option<ThumbnailGenerationSpeed> {
    match index {
        0 => Some(ThumbnailGenerationSpeed::Low),
        1 => Some(ThumbnailGenerationSpeed::Normal),
        2 => Some(ThumbnailGenerationSpeed::High),
        _ => None,
    }
}

fn startup_behavior_index(behavior: StartupBehavior) -> u32 {
    match behavior {
        StartupBehavior::BookshelfTop => 0,
        StartupBehavior::RestoreLastSession => 1,
    }
}

fn startup_behavior_from_index(index: u32) -> Option<StartupBehavior> {
    match index {
        0 => Some(StartupBehavior::BookshelfTop),
        1 => Some(StartupBehavior::RestoreLastSession),
        _ => None,
    }
}

fn archive_expansion_limit_index(limit: crate::archive::ArchiveExpansionLimit) -> u32 {
    crate::archive::ArchiveExpansionLimit::ALL
        .iter()
        .position(|value| *value == limit)
        .unwrap() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn archive_expansion_choices_select_the_default_and_round_trip() {
        use crate::archive::ArchiveExpansionLimit;
        assert_eq!(
            archive_expansion_limit_index(UserSettings::default().archive_expansion_limit),
            1
        );
        for (index, limit) in ArchiveExpansionLimit::ALL.into_iter().enumerate() {
            assert_eq!(archive_expansion_limit_index(limit), index as u32);
            assert_eq!(ArchiveExpansionLimit::from_gib(limit.gib()), Some(limit));
        }
        assert!(ArchiveExpansionLimit::ALL.get(4).is_none());
    }

    #[derive(Default)]
    struct FakeSwitchRow {
        blocked: Cell<bool>,
        active: Cell<bool>,
        notifications: Cell<usize>,
    }

    impl SwitchRowSync for FakeSwitchRow {
        type Handler = ();

        fn block_sync_signal(&self, _handler: &Self::Handler) {
            self.blocked.set(true);
        }

        fn set_sync_active(&self, active: bool) {
            if self.active.replace(active) != active && !self.blocked.get() {
                self.notifications.set(self.notifications.get() + 1);
            }
        }

        fn unblock_sync_signal(&self, _handler: &Self::Handler) {
            self.blocked.set(false);
        }
    }

    #[test]
    fn switch_sync_blocks_change_notifications() {
        let row = FakeSwitchRow::default();

        sync_switch_row(&row, &(), true);

        assert!(row.active.get());
        assert!(!row.blocked.get());
        assert_eq!(row.notifications.get(), 0);
    }

    #[test]
    fn follow_pointer_is_the_first_preview_position_option() {
        assert_eq!(
            preview_position_index(PreviewPositionMode::FollowPointer),
            0
        );
        assert_eq!(
            preview_position_from_index(0),
            Some(PreviewPositionMode::FollowPointer)
        );
        assert_eq!(preview_position_index(PreviewPositionMode::Centered), 1);
        assert_eq!(
            preview_position_from_index(1),
            Some(PreviewPositionMode::Centered)
        );
    }

    #[test]
    fn thumbnail_generation_speed_options_match_the_ui_order() {
        for (index, speed) in [
            ThumbnailGenerationSpeed::Low,
            ThumbnailGenerationSpeed::Normal,
            ThumbnailGenerationSpeed::High,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(thumbnail_generation_speed_index(speed), index as u32);
            assert_eq!(
                thumbnail_generation_speed_from_index(index as u32),
                Some(speed)
            );
        }
        assert_eq!(thumbnail_generation_speed_from_index(3), None);
    }

    #[test]
    fn bookshelf_top_is_the_first_startup_behavior_option() {
        assert_eq!(startup_behavior_index(StartupBehavior::BookshelfTop), 0);
        assert_eq!(
            startup_behavior_from_index(0),
            Some(StartupBehavior::BookshelfTop)
        );
        assert_eq!(
            startup_behavior_index(StartupBehavior::RestoreLastSession),
            1
        );
        assert_eq!(
            startup_behavior_from_index(1),
            Some(StartupBehavior::RestoreLastSession)
        );
        assert_eq!(startup_behavior_from_index(2), None);
    }
}
