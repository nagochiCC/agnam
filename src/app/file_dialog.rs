use super::{AppSender, Msg};
use crate::archive::{ArchiveFormat, IMAGE_EXTENSIONS};
use crate::covers::CoverBookIdentity;
use gtk::prelude::*;

fn file_filter<'a>(name: &str, extensions: impl IntoIterator<Item = &'a str>) -> gtk::FileFilter {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(name));
    for ext in extensions {
        filter.add_pattern(&format!("*.{ext}"));
        filter.add_pattern(&format!("*.{}", ext.to_ascii_uppercase()));
    }
    filter
}

pub(super) fn show_file_dialog(sender: AppSender) {
    let filter_all = file_filter(
        "対応ファイルすべて",
        ArchiveFormat::all_extensions().chain(IMAGE_EXTENSIONS.iter().copied()),
    );
    let filter_archive = file_filter("アーカイブファイル", ArchiveFormat::all_extensions());
    let filter_image = file_filter("画像ファイル", IMAGE_EXTENSIONS.iter().copied());

    let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter_all);
    filters.append(&filter_archive);
    filters.append(&filter_image);

    let dialog = gtk::FileDialog::builder()
        .title("ファイルを開く")
        .filters(&filters)
        .build();

    dialog.open(
        None::<&gtk::Window>,
        None::<&gtk::gio::Cancellable>,
        move |result: Result<gtk::gio::File, gtk::glib::Error>| {
            let Ok(file) = result else {
                return;
            };
            let Some(path) = file.path() else {
                return;
            };

            sender.input(Msg::PathSelected(path));
        },
    );
}

pub(super) fn show_bookshelf_folder_dialog(sender: AppSender) {
    let dialog = gtk::FileDialog::builder()
        .title("本棚フォルダを選択")
        .accept_label("選択")
        .build();

    dialog.select_folder(
        None::<&gtk::Window>,
        None::<&gtk::gio::Cancellable>,
        move |result: Result<gtk::gio::File, gtk::glib::Error>| {
            let Ok(folder) = result else {
                return;
            };
            let Some(path) = folder.path() else {
                return;
            };
            sender.input(Msg::BookshelfRootSelected(path));
        },
    );
}

pub(super) fn show_external_cover_dialog(
    identity: CoverBookIdentity,
    request_id: u64,
    sender: AppSender,
) {
    let filter = file_filter("表紙に使用できる画像", IMAGE_EXTENSIONS.iter().copied());
    let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder()
        .title("表紙画像を選択")
        .accept_label("選択")
        .filters(&filters)
        .build();
    dialog.open(
        None::<&gtk::Window>,
        None::<&gtk::gio::Cancellable>,
        move |result| {
            let Ok(file) = result else { return };
            let Some(path) = file.path() else { return };
            sender.input(Msg::ExternalCoverSelected {
                identity: identity.clone(),
                request_id,
                path,
            });
        },
    );
}
