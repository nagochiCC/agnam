use super::preferences::SettingsDialog;
use super::shortcuts::ShortcutsDialog;
use super::{AppSender, Msg};
use adw::prelude::*;

pub(crate) const OPEN_FILE_ACTION: &str = "app.open-file";
pub(crate) const SEARCH_LIBRARY_ACTION: &str = "app.search-library";
pub(crate) const SETTINGS_ACTION: &str = "app.settings";
pub(crate) const SHORTCUTS_ACTION: &str = "app.shortcuts";
pub(crate) const FULLSCREEN_ACTION: &str = "app.fullscreen";
pub(crate) const ABOUT_ACTION: &str = "app.about";
pub(crate) const NEXT_PAGE_ACTION: &str = "app.next-page";
pub(crate) const PREV_PAGE_ACTION: &str = "app.prev-page";
pub(crate) const NEXT_SINGLE_PAGE_ACTION: &str = "app.next-single";
pub(crate) const PREV_SINGLE_PAGE_ACTION: &str = "app.prev-single";
pub(crate) const NEXT_FILE_ACTION: &str = "app.next-file";
pub(crate) const PREV_FILE_ACTION: &str = "app.prev-file";

pub(crate) const OPEN_FILE_ACCELERATORS: &[&str] = &["<Primary>o"];
pub(crate) const SEARCH_LIBRARY_ACCELERATORS: &[&str] = &["<Primary>f"];
pub(crate) const SETTINGS_ACCELERATORS: &[&str] = &["<Primary>comma"];
pub(crate) const SHORTCUTS_ACCELERATORS: &[&str] = &["<Primary>question"];
pub(crate) const FULLSCREEN_ACCELERATORS: &[&str] = &["F11"];

pub(crate) const VIEWER_ACCELERATORS: &[(&str, &[&str])] = &[
    (NEXT_PAGE_ACTION, &["Left", "space"]),
    (PREV_PAGE_ACTION, &["Right", "BackSpace"]),
    (NEXT_SINGLE_PAGE_ACTION, &["<Shift>Left", "<Shift>space"]),
    (
        PREV_SINGLE_PAGE_ACTION,
        &["<Shift>Right", "<Shift>BackSpace"],
    ),
    (NEXT_FILE_ACTION, &["Down", "bracketright"]),
    (PREV_FILE_ACTION, &["Up", "bracketleft"]),
];

pub(crate) fn viewer_accelerators(action_name: &str) -> Option<&'static [&'static str]> {
    VIEWER_ACCELERATORS
        .iter()
        .find_map(|(name, accelerators)| (*name == action_name).then_some(*accelerators))
}

pub(crate) fn accelerators_for_action(action_name: &str) -> Option<&'static [&'static str]> {
    viewer_accelerators(action_name).or_else(|| match action_name {
        OPEN_FILE_ACTION => Some(OPEN_FILE_ACCELERATORS),
        SEARCH_LIBRARY_ACTION => Some(SEARCH_LIBRARY_ACCELERATORS),
        SETTINGS_ACTION => Some(SETTINGS_ACCELERATORS),
        SHORTCUTS_ACTION => Some(SHORTCUTS_ACCELERATORS),
        FULLSCREEN_ACTION => Some(FULLSCREEN_ACCELERATORS),
        _ => None,
    })
}

pub(crate) fn set_viewer_accelerators_enabled(app: &gtk::Application, enabled: bool) {
    for (action_name, accelerators) in VIEWER_ACCELERATORS {
        app.set_accels_for_action(action_name, if enabled { accelerators } else { &[] });
    }
}

pub(crate) fn main_menu() -> gtk::gio::Menu {
    let menu = gtk::gio::Menu::new();
    menu.append(Some("ファイルを開く…"), Some(OPEN_FILE_ACTION));

    let application_section = gtk::gio::Menu::new();
    application_section.append(Some("フルスクリーン"), Some(FULLSCREEN_ACTION));
    application_section.append(Some("キーボードショートカット"), Some(SHORTCUTS_ACTION));
    menu.append_section(None, &application_section);

    let settings_section = gtk::gio::Menu::new();
    settings_section.append(Some("設定"), Some(SETTINGS_ACTION));
    settings_section.append(Some("Agnam について"), Some(ABOUT_ACTION));
    menu.append_section(None, &settings_section);
    menu
}

pub(crate) fn register(
    main_window: &adw::ApplicationWindow,
    settings_dialog: &SettingsDialog,
    sender: AppSender,
) -> gtk::gio::SimpleAction {
    let app = main_window
        .application()
        .or_else(|| gtk::gio::Application::default().and_downcast::<gtk::Application>())
        .expect("Agnam main window must belong to an application");
    app.set_accels_for_action(OPEN_FILE_ACTION, OPEN_FILE_ACCELERATORS);
    app.set_accels_for_action(SEARCH_LIBRARY_ACTION, SEARCH_LIBRARY_ACCELERATORS);
    app.set_accels_for_action(SHORTCUTS_ACTION, SHORTCUTS_ACCELERATORS);
    app.set_accels_for_action(SETTINGS_ACTION, SETTINGS_ACCELERATORS);
    app.set_accels_for_action(FULLSCREEN_ACTION, FULLSCREEN_ACCELERATORS);
    set_viewer_accelerators_enabled(&app, false);

    let action_group = gtk::gio::SimpleActionGroup::new();

    let settings_action = gtk::gio::SimpleAction::new("settings", None);
    {
        let dialog = settings_dialog.dialog().clone();
        let main_window = main_window.clone();
        settings_action.connect_activate(move |_, _| dialog.present(Some(&main_window)));
    }
    action_group.add_action(&settings_action);

    let open_file_action = gtk::gio::SimpleAction::new("open-file", None);
    {
        let sender = sender.clone();
        open_file_action.connect_activate(move |_, _| sender.input(Msg::OpenFile));
    }
    action_group.add_action(&open_file_action);

    let search_library_action = gtk::gio::SimpleAction::new("search-library", None);
    {
        let sender = sender.clone();
        search_library_action.connect_activate(move |_, _| sender.input(Msg::ToggleSearchPanel));
    }
    action_group.add_action(&search_library_action);

    let shortcuts_dialog = ShortcutsDialog::new(main_window, &app);
    let shortcuts_action = gtk::gio::SimpleAction::new("shortcuts", None);
    shortcuts_action.connect_activate(move |_, _| shortcuts_dialog.present());
    action_group.add_action(&shortcuts_action);

    let fullscreen_action = gtk::gio::SimpleAction::new("fullscreen", None);
    {
        let main_window = main_window.clone();
        fullscreen_action.connect_activate(move |_, _| {
            if main_window.is_fullscreen() {
                main_window.unfullscreen();
            } else {
                main_window.fullscreen();
            }
        });
    }
    action_group.add_action(&fullscreen_action);

    let about_dialog = adw::AboutDialog::builder()
        .application_name("Agnam")
        .developer_name("nagochiCC")
        .version(env!("CARGO_PKG_VERSION"))
        .build();
    let about_action = gtk::gio::SimpleAction::new("about", None);
    {
        let main_window = main_window.clone();
        about_action.connect_activate(move |_, _| about_dialog.present(Some(&main_window)));
    }
    action_group.add_action(&about_action);

    let next_action = gtk::gio::SimpleAction::new("next-page", None);
    {
        let sender = sender.clone();
        next_action.connect_activate(move |_, _| sender.input(Msg::NextPage));
    }
    action_group.add_action(&next_action);

    let prev_action = gtk::gio::SimpleAction::new("prev-page", None);
    {
        let sender = sender.clone();
        prev_action.connect_activate(move |_, _| sender.input(Msg::PrevPage));
    }
    action_group.add_action(&prev_action);

    let next_single_action = gtk::gio::SimpleAction::new("next-single", None);
    {
        let sender = sender.clone();
        next_single_action.connect_activate(move |_, _| sender.input(Msg::NextSinglePage));
    }
    action_group.add_action(&next_single_action);

    let prev_single_action = gtk::gio::SimpleAction::new("prev-single", None);
    {
        let sender = sender.clone();
        prev_single_action.connect_activate(move |_, _| sender.input(Msg::PrevSinglePage));
    }
    action_group.add_action(&prev_single_action);

    let next_file_action = gtk::gio::SimpleAction::new("next-file", None);
    {
        let sender = sender.clone();
        next_file_action.connect_activate(move |_, _| sender.input(Msg::NextFile));
    }
    action_group.add_action(&next_file_action);

    let prev_file_action = gtk::gio::SimpleAction::new("prev-file", None);
    {
        prev_file_action.connect_activate(move |_, _| sender.input(Msg::PrevFile));
    }
    action_group.add_action(&prev_file_action);

    main_window.insert_action_group("app", Some(&action_group));
    search_library_action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_accelerators_match_the_viewer_actions() {
        let expected = [
            (NEXT_PAGE_ACTION, &["Left", "space"][..]),
            (PREV_PAGE_ACTION, &["Right", "BackSpace"][..]),
            (
                NEXT_SINGLE_PAGE_ACTION,
                &["<Shift>Left", "<Shift>space"][..],
            ),
            (
                PREV_SINGLE_PAGE_ACTION,
                &["<Shift>Right", "<Shift>BackSpace"][..],
            ),
            (NEXT_FILE_ACTION, &["Down", "bracketright"][..]),
            (PREV_FILE_ACTION, &["Up", "bracketleft"][..]),
        ];

        assert_eq!(VIEWER_ACCELERATORS.len(), expected.len());
        for (action_name, accelerators) in expected {
            assert_eq!(viewer_accelerators(action_name), Some(accelerators));
        }
    }

    #[test]
    fn library_search_has_its_own_action_name() {
        assert_eq!(SEARCH_LIBRARY_ACTION, "app.search-library");
        assert_eq!(SEARCH_LIBRARY_ACCELERATORS, &["<Primary>f"]);
        assert_eq!(
            accelerators_for_action(SEARCH_LIBRARY_ACTION),
            Some(SEARCH_LIBRARY_ACCELERATORS)
        );
    }
}
