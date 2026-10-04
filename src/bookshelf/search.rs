use super::{is_bookshelf_item_path, read_visible_entries};
use crate::archive::is_image_document_entry_path;
use crate::history::{HistoryIdentity, history_identity_for_document};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SearchIndex {
    items: Vec<SearchItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SearchItemId(usize);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SearchItem {
    Folder(SearchFolder),
    Archive(SearchArchive),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SearchFolder {
    pub(crate) path: PathBuf,
    pub(crate) display_name: String,
    pub(crate) normalized_key: String,
    pub(crate) parent_relative: PathBuf,
    pub(crate) cover_source: PathBuf,
    pub(crate) image_document_entry: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SearchArchive {
    pub(crate) path: PathBuf,
    pub(crate) display_name: String,
    pub(crate) normalized_key: String,
    pub(crate) parent_relative: PathBuf,
}

impl SearchIndex {
    /// Builds an in-memory snapshot by walking the bookshelf root once.
    pub(crate) fn build(root: &Path) -> io::Result<Self> {
        let mut documents = Vec::new();
        let mut image_document_entries = HashMap::new();
        scan_documents(root, &mut documents, &mut image_document_entries)?;

        // The history classifier defines which documents belong to one work.
        // Keep only the first naturally encountered document as that work's
        // representative cover source; root-level documents remain standalone.
        let mut work_documents = HashMap::new();
        let mut items = Vec::new();
        for document in documents {
            match history_identity_for_document(&document, Some(root)) {
                HistoryIdentity::BookshelfWork(work) => {
                    work_documents.entry(work).or_insert(document);
                }
                HistoryIdentity::Document(path) => items.push(archive_item(root, path)),
            }
        }
        for (path, representative_document) in work_documents {
            let image_document_entry = image_document_entries.remove(&path);
            items.push(SearchItem::Folder(folder_item(
                root,
                path,
                representative_document,
                image_document_entry,
            )));
        }
        items.sort_by(compare_search_items);
        Ok(Self { items })
    }

    #[cfg(test)]
    pub(crate) fn items(&self) -> &[SearchItem] {
        &self.items
    }

    pub(crate) fn item(&self, id: SearchItemId) -> Option<&SearchItem> {
        self.items.get(id.0)
    }

    /// Searches the snapshot without accessing the filesystem.
    pub(crate) fn search(&self, query: &str) -> Vec<SearchItemId> {
        let query = normalize_search_text(query);
        if query.is_empty() {
            return Vec::new();
        }

        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.normalized_key().contains(&query))
            .map(|(position, _)| SearchItemId(position))
            .collect()
    }
}

impl SearchItem {
    pub(crate) fn normalized_key(&self) -> &str {
        match self {
            Self::Folder(folder) => &folder.normalized_key,
            Self::Archive(archive) => &archive.normalized_key,
        }
    }

    fn display_name(&self) -> &str {
        match self {
            Self::Folder(folder) => &folder.display_name,
            Self::Archive(archive) => &archive.display_name,
        }
    }

    fn path(&self) -> &Path {
        match self {
            Self::Folder(folder) => &folder.path,
            Self::Archive(archive) => &archive.path,
        }
    }
}

pub(crate) fn normalize_search_text(text: &str) -> String {
    text.nfkc()
        .flat_map(char::to_lowercase)
        .map(katakana_to_hiragana)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn katakana_to_hiragana(character: char) -> char {
    if ('ァ'..='ヶ').contains(&character) {
        char::from_u32(character as u32 - 0x60).unwrap_or(character)
    } else {
        character
    }
}

fn scan_documents(
    directory: &Path,
    documents: &mut Vec<PathBuf>,
    image_document_entries: &mut HashMap<PathBuf, PathBuf>,
) -> io::Result<()> {
    for entry in read_visible_entries(directory)? {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_file() {
            if is_image_document_entry_path(&path) {
                if !image_document_entries.contains_key(directory) {
                    image_document_entries.insert(directory.to_path_buf(), path.clone());
                    documents.push(path);
                }
            } else if is_bookshelf_item_path(&path) {
                documents.push(path);
            }
        } else if file_type.is_dir() {
            scan_documents(&path, documents, image_document_entries)?;
        }
    }
    Ok(())
}

fn folder_item(
    root: &Path,
    path: PathBuf,
    representative_document: PathBuf,
    image_document_entry: Option<PathBuf>,
) -> SearchFolder {
    let cover_source = image_document_entry
        .clone()
        .unwrap_or(representative_document);
    let display_name = file_name(&path);
    SearchFolder {
        normalized_key: normalize_search_text(&display_name),
        display_name,
        parent_relative: parent_relative(root, &path),
        path,
        cover_source,
        image_document_entry,
    }
}

fn archive_item(root: &Path, path: PathBuf) -> SearchItem {
    let display_name = file_name(&path);
    SearchItem::Archive(SearchArchive {
        normalized_key: normalize_search_text(&display_name),
        display_name,
        parent_relative: parent_relative(root, &path),
        path,
    })
}

fn parent_relative(root: &Path, path: &Path) -> PathBuf {
    path.parent()
        .and_then(|parent| parent.strip_prefix(root).ok())
        .unwrap_or(Path::new(""))
        .to_path_buf()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn compare_search_items(left: &SearchItem, right: &SearchItem) -> Ordering {
    natord::compare(left.display_name(), right.display_name())
        .then_with(|| {
            natord::compare(
                &left.path().to_string_lossy(),
                &right.path().to_string_lossy(),
            )
        })
        .then_with(|| left.path().cmp(right.path()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn touch(path: &Path) {
        fs::write(path, []).unwrap();
    }

    fn write_zip(path: &Path, entry_names: &[&str]) {
        let mut archive = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for entry_name in entry_names {
            archive
                .start_file(*entry_name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(b"content").unwrap();
        }
        archive.finish().unwrap();
    }

    fn folders(index: &SearchIndex) -> Vec<&SearchFolder> {
        index
            .items()
            .iter()
            .filter_map(|item| match item {
                SearchItem::Folder(folder) => Some(folder),
                SearchItem::Archive(_) => None,
            })
            .collect()
    }

    fn archives(index: &SearchIndex) -> Vec<&SearchArchive> {
        index
            .items()
            .iter()
            .filter_map(|item| match item {
                SearchItem::Folder(_) => None,
                SearchItem::Archive(archive) => Some(archive),
            })
            .collect()
    }

    #[test]
    fn normalization_folds_width_case_katakana_and_symbols() {
        for text in [
            "ブラック・ジャック",
            "ブラックジャック",
            "ぶらっく・じゃっく",
            "ﾌﾞﾗｯｸｼﾞｬｯｸ",
        ] {
            assert_eq!(normalize_search_text(text), "ぶらっくじゃっく");
        }
        assert_eq!(normalize_search_text("ＡＫＩＲＡ"), "akira");
        assert_eq!(normalize_search_text("ヴァイオリン"), "ゔぁいおりん");
        assert!(normalize_search_text("・!?＿-").is_empty());
    }

    #[test]
    fn archive_volumes_are_one_work_and_containers_are_not_items() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("作者/作品");
        fs::create_dir_all(&work).unwrap();
        write_zip(&work.join("01.cbz"), &["001.jpg"]);
        write_zip(&work.join("02.zip"), &["001.jpg"]);

        let index = SearchIndex::build(root.path()).unwrap();
        assert_eq!(folders(&index).len(), 1);
        assert_eq!(folders(&index)[0].path, work);
        assert_eq!(folders(&index)[0].display_name, "作品");
        assert_eq!(
            folders(&index)[0].cover_source,
            root.path().join("作者/作品/01.cbz")
        );
        assert!(archives(&index).is_empty());
        assert!(index.search("作者").is_empty());
        assert!(index.search("01").is_empty());
        assert!(index.search("02").is_empty());
    }

    #[test]
    fn image_book_volumes_are_one_work() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("作者/作品");
        for volume in ["01巻", "02巻"] {
            fs::create_dir_all(work.join(volume)).unwrap();
            touch(&work.join(volume).join("001.jpg"));
        }

        let index = SearchIndex::build(root.path()).unwrap();
        assert_eq!(folders(&index).len(), 1);
        let folder = folders(&index)[0];
        assert_eq!(folder.path, work);
        assert_eq!(
            folder.cover_source,
            root.path().join("作者/作品/01巻/001.jpg")
        );
        assert!(folder.image_document_entry.is_none());
        assert!(index.search("作者").is_empty());
        assert!(index.search("01巻").is_empty());
    }

    #[test]
    fn image_book_pages_are_collected_as_one_logical_document() {
        let root = tempfile::tempdir().unwrap();
        let volume = root.path().join("作品/01巻");
        fs::create_dir_all(&volume).unwrap();
        for page in 1..=100 {
            touch(&volume.join(format!("{page:03}.jpg")));
        }

        let mut documents = Vec::new();
        let mut image_document_entries = HashMap::new();
        scan_documents(root.path(), &mut documents, &mut image_document_entries).unwrap();

        assert_eq!(documents, [volume.join("001.jpg")]);
        assert_eq!(
            image_document_entries.get(&volume),
            Some(&volume.join("001.jpg"))
        );
    }

    #[test]
    fn direct_image_work_keeps_its_viewer_entry() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("作者/作品");
        fs::create_dir_all(&work).unwrap();
        touch(&work.join("001.jpg"));

        let index = SearchIndex::build(root.path()).unwrap();
        let folder = folders(&index)[0];
        assert_eq!(folder.path, work);
        assert_eq!(
            folder.image_document_entry,
            Some(root.path().join("作者/作品/001.jpg"))
        );
        assert_eq!(folder.cover_source, root.path().join("作者/作品/001.jpg"));
    }

    #[test]
    fn root_documents_stay_standalone_including_numeric_names() {
        let root = tempfile::tempdir().unwrap();
        write_zip(&root.path().join("単独作品.zip"), &["001.jpg"]);
        write_zip(&root.path().join("01.zip"), &["001.jpg"]);

        let index = SearchIndex::build(root.path()).unwrap();
        assert_eq!(folders(&index).len(), 0);
        assert_eq!(
            archives(&index)
                .iter()
                .map(|archive| archive.display_name.as_str())
                .collect::<Vec<_>>(),
            ["01.zip", "単独作品.zip"]
        );
    }

    #[test]
    fn query_is_normalized_substring_search_over_index_owned_metadata() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("【ブラック・ジャック】愛蔵版");
        fs::create_dir(&work).unwrap();
        touch(&work.join("001.jpg"));
        let index = SearchIndex::build(root.path()).unwrap();

        let results = index.search("ぶらっく");
        assert_eq!(results.len(), 1);
        assert!(
            matches!(index.item(results[0]), Some(SearchItem::Folder(folder)) if folder.path == work)
        );
        assert!(index.search("!?　").is_empty());
        assert_eq!(
            std::mem::size_of::<SearchItemId>(),
            std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn index_build_reuses_the_bookshelf_scan_archive_probe_cache() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("作品.zip");
        write_zip(&archive, &["001.jpg"]);
        crate::archive::track_backend_probes_for(&archive);

        let bookshelf = super::super::scan_directory(root.path()).unwrap();
        assert_eq!(crate::archive::backend_probe_count(), 1);
        let index = SearchIndex::build(root.path()).unwrap();

        assert_eq!(bookshelf.direct_files, [archive.clone()]);
        assert_eq!(archives(&index)[0].path, archive);
        assert_eq!(crate::archive::backend_probe_count(), 1);
    }

    #[test]
    fn work_keeps_one_representative_cover_source() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("作品");
        fs::create_dir(&work).unwrap();
        write_zip(&work.join("01.zip"), &["001.jpg"]);
        write_zip(&work.join("02.zip"), &["001.jpg"]);

        let index = SearchIndex::build(root.path()).unwrap();
        let folder = folders(&index)[0];
        assert_eq!(folder.cover_source, work.join("01.zip"));
    }

    #[test]
    fn ignored_entries_and_directory_symlinks_are_not_followed() {
        let root = tempfile::tempdir().unwrap();
        let visible = root.path().join("visible");
        fs::create_dir(&visible).unwrap();
        touch(&visible.join("1.jpg"));
        for name in [".hidden", "__MACOSX"] {
            fs::create_dir(root.path().join(name)).unwrap();
            touch(&root.path().join(name).join("1.jpg"));
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.path(), root.path().join("loop")).unwrap();

        let index = SearchIndex::build(root.path()).unwrap();
        assert_eq!(folders(&index).len(), 1);
        assert_eq!(folders(&index)[0].path, visible);
    }
}
