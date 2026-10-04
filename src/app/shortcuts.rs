use super::actions::{
    FULLSCREEN_ACTION, NEXT_FILE_ACTION, NEXT_PAGE_ACTION, NEXT_SINGLE_PAGE_ACTION,
    OPEN_FILE_ACTION, PREV_FILE_ACTION, PREV_PAGE_ACTION, PREV_SINGLE_PAGE_ACTION,
    SEARCH_LIBRARY_ACTION, SETTINGS_ACTION, SHORTCUTS_ACTION, accelerators_for_action,
    viewer_accelerators,
};
use gtk::prelude::*;

const SHORTCUTS_UI: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<interface>
  <object class="GtkShortcutsWindow" id="shortcuts_window">
    <property name="modal">True</property>
    <property name="title">キーボードショートカット</property>
    <child>
      <object class="GtkShortcutsSection">
        <child>
          <object class="GtkShortcutsGroup">
            <property name="title">ページ移動</property>
            <child>
              <object class="GtkShortcutsShortcut" id="next_page">
                <property name="title">次のページ</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut" id="prev_page">
                <property name="title">前のページ</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut" id="next_single_page">
                <property name="title">1ページ進む（微調整）</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut" id="prev_single_page">
                <property name="title">1ページ戻る（微調整）</property>
              </object>
            </child>
          </object>
        </child>
        <child>
          <object class="GtkShortcutsGroup">
            <property name="title">ファイル操作</property>
            <child>
              <object class="GtkShortcutsShortcut" id="search_library">
                <property name="title">本棚を検索</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut" id="open_file">
                <property name="title">ファイルを開く</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut" id="next_file">
                <property name="title">次のファイルへ</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut" id="prev_file">
                <property name="title">前のファイルへ</property>
              </object>
            </child>
          </object>
        </child>
        <child>
          <object class="GtkShortcutsGroup">
            <property name="title">アプリケーション</property>
            <child>
              <object class="GtkShortcutsShortcut" id="fullscreen">
                <property name="title">フルスクリーン</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut" id="show_shortcuts">
                <property name="title">キーボードショートカット</property>
              </object>
            </child>
            <child>
              <object class="GtkShortcutsShortcut" id="show_settings">
                <property name="title">設定</property>
              </object>
            </child>
          </object>
        </child>
      </object>
    </child>
  </object>
</interface>
"#;

pub(super) struct ShortcutsDialog {
    window: gtk::ShortcutsWindow,
}

impl ShortcutsDialog {
    pub(super) fn new(parent: &adw::ApplicationWindow, app: &gtk::Application) -> Self {
        let builder = gtk::Builder::from_string(SHORTCUTS_UI);
        let window = builder
            .object::<gtk::ShortcutsWindow>("shortcuts_window")
            .expect("shortcuts_window must exist in shortcuts UI");

        window.set_application(Some(app));
        set_action_name(&builder, "next_page", NEXT_PAGE_ACTION);
        set_action_name(&builder, "prev_page", PREV_PAGE_ACTION);
        set_action_name(&builder, "next_single_page", NEXT_SINGLE_PAGE_ACTION);
        set_action_name(&builder, "prev_single_page", PREV_SINGLE_PAGE_ACTION);
        set_action_name(&builder, "open_file", OPEN_FILE_ACTION);
        set_action_name(&builder, "search_library", SEARCH_LIBRARY_ACTION);
        set_action_name(&builder, "next_file", NEXT_FILE_ACTION);
        set_action_name(&builder, "prev_file", PREV_FILE_ACTION);
        set_action_name(&builder, "fullscreen", FULLSCREEN_ACTION);
        set_action_name(&builder, "show_shortcuts", SHORTCUTS_ACTION);
        set_action_name(&builder, "show_settings", SETTINGS_ACTION);

        window.set_transient_for(Some(parent));
        window.set_destroy_with_parent(true);
        window.connect_close_request(|window| {
            window.set_visible(false);
            gtk::glib::Propagation::Stop
        });

        Self { window }
    }

    pub(super) fn present(&self) {
        self.window.present();
    }
}

fn set_action_name(builder: &gtk::Builder, id: &str, action_name: &str) {
    let shortcut = builder
        .object::<gtk::ShortcutsShortcut>(id)
        .expect("shortcut must exist in shortcuts UI");
    shortcut.set_action_name(Some(action_name));

    let accelerator = if let Some(accels) = viewer_accelerators(action_name) {
        accels.join(" ")
    } else {
        accelerators_for_action(action_name)
            .unwrap_or_default()
            .join(" ")
    };
    shortcut.set_accelerator(Some(&accelerator));
}
