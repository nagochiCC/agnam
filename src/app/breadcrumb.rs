use super::library::path_is_within_root;
use super::{AppSender, Msg};
use crate::archive::is_image_ext;
use crate::document::ImageSource;
use adw::prelude::*;
use std::path::{Path, PathBuf};

pub(super) const LIBRARY_ROOT_ICON: &str = "go-home-symbolic";
const LIBRARY_ROOT_LABEL: &str = "本棚トップ";
const MIN_SEGMENT_WIDTH_CHARS: i32 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BreadcrumbSegment {
    pub(super) label: String,
    pub(super) icon: Option<&'static str>,
    pub(super) target: Option<BreadcrumbTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BreadcrumbTarget {
    Library(PathBuf),
    Archive(crate::archive::ArchiveLocation),
}

fn library_root_segment(target: Option<PathBuf>) -> BreadcrumbSegment {
    BreadcrumbSegment {
        label: LIBRARY_ROOT_LABEL.into(),
        icon: Some(LIBRARY_ROOT_ICON),
        target: target.map(BreadcrumbTarget::Library),
    }
}

pub(super) fn parent_target(segments: &[BreadcrumbSegment]) -> Option<BreadcrumbTarget> {
    segments
        .iter()
        .rev()
        .find_map(|segment| segment.target.clone())
}

pub(super) fn library_segments(root: &Path, current: &Path) -> Vec<BreadcrumbSegment> {
    if !path_is_within_root(root, current) {
        return Vec::new();
    }

    let mut segments = vec![library_root_segment(
        (current != root).then(|| root.to_path_buf()),
    )];
    let mut target = root.to_path_buf();
    let relative = current.strip_prefix(root).expect("root checked above");
    let components = relative.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        target.push(component.as_os_str());
        segments.push(BreadcrumbSegment {
            label: component.as_os_str().to_string_lossy().into_owned(),
            icon: None,
            target: (index + 1 != components.len())
                .then(|| BreadcrumbTarget::Library(target.clone())),
        });
    }
    segments
}

pub(super) fn viewer_segments(
    root: Option<&Path>,
    document_path: &Path,
    current_source: Option<&ImageSource>,
) -> Vec<BreadcrumbSegment> {
    if is_image_ext(document_path)
        && let (Some(root), Some(folder)) = (root, document_path.parent())
        && folder != root
        && path_is_within_root(root, folder)
    {
        return viewer_document_segments(Some(root), folder);
    }

    let mut segments = viewer_document_segments(root, document_path);

    if is_image_ext(document_path)
        && let Some(current_path) = current_source.and_then(ImageSource::as_file_path)
        && let Some(current_name) = current_path.file_name()
        && let Some(current_segment) = segments.last_mut()
    {
        current_segment.label = current_name.to_string_lossy().into_owned();
    }

    segments
}

fn viewer_document_segments(root: Option<&Path>, path: &Path) -> Vec<BreadcrumbSegment> {
    let Some(root) = root else {
        return vec![current_file_segment(path)];
    };
    if !path_is_within_root(root, path) {
        return vec![
            library_root_segment(Some(root.to_path_buf())),
            current_file_segment(path),
        ];
    }

    let mut segments = vec![library_root_segment(Some(root.to_path_buf()))];
    let mut target = root.to_path_buf();
    let relative = path.strip_prefix(root).expect("root checked above");
    let components = relative.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        target.push(component.as_os_str());
        let is_file = index + 1 == components.len();
        segments.push(BreadcrumbSegment {
            label: component.as_os_str().to_string_lossy().into_owned(),
            icon: None,
            target: (!is_file).then(|| BreadcrumbTarget::Library(target.clone())),
        });
    }
    segments
}

pub(super) fn archive_segments(
    root: Option<&Path>,
    location: &crate::archive::ArchiveLocation,
) -> Vec<BreadcrumbSegment> {
    let mut segments = vec![library_root_segment(root.map(Path::to_path_buf))];
    segments.push(BreadcrumbSegment {
        label: location
            .archive
            .file_name()
            .unwrap_or(location.archive.as_os_str())
            .to_string_lossy()
            .into_owned(),
        icon: None,
        target: (!location.archives.is_empty() || !location.directory.as_os_str().is_empty()).then(
            || {
                BreadcrumbTarget::Archive(crate::archive::ArchiveLocation::root(
                    location.archive.clone(),
                ))
            },
        ),
    });
    let mut chain = Vec::new();
    for (index, archive) in location.archives.iter().enumerate() {
        let mut containing_directory = PathBuf::new();
        if let Some(parent) = archive.parent() {
            for component in parent.components() {
                containing_directory.push(component.as_os_str());
                segments.push(BreadcrumbSegment {
                    label: component.as_os_str().to_string_lossy().into_owned(),
                    icon: None,
                    target: Some(BreadcrumbTarget::Archive(crate::archive::ArchiveLocation {
                        archive: location.archive.clone(),
                        archives: chain.clone(),
                        directory: containing_directory.clone(),
                    })),
                });
            }
        }
        chain.push(archive.clone());
        let is_current =
            index + 1 == location.archives.len() && location.directory.as_os_str().is_empty();
        segments.push(BreadcrumbSegment {
            label: archive
                .file_name()
                .unwrap_or(archive.as_os_str())
                .to_string_lossy()
                .into_owned(),
            icon: None,
            target: (!is_current).then(|| {
                BreadcrumbTarget::Archive(crate::archive::ArchiveLocation {
                    archive: location.archive.clone(),
                    archives: chain.clone(),
                    directory: PathBuf::new(),
                })
            }),
        });
    }
    let components = location.directory.components().collect::<Vec<_>>();
    let mut directory = PathBuf::new();
    for (index, component) in components.iter().enumerate() {
        directory.push(component.as_os_str());
        segments.push(BreadcrumbSegment {
            label: component.as_os_str().to_string_lossy().into_owned(),
            icon: None,
            target: (index + 1 != components.len()).then(|| {
                BreadcrumbTarget::Archive(crate::archive::ArchiveLocation {
                    archive: location.archive.clone(),
                    archives: location.archives.clone(),
                    directory: directory.clone(),
                })
            }),
        });
    }
    segments
}

fn current_file_segment(path: &Path) -> BreadcrumbSegment {
    BreadcrumbSegment {
        label: path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned(),
        icon: None,
        target: None,
    }
}

fn segment_content(segment: &BreadcrumbSegment) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.set_valign(gtk::Align::Center);

    if let Some(icon) = segment.icon {
        content.append(&gtk::Image::from_icon_name(icon));
    }
    if segment.icon == Some(LIBRARY_ROOT_ICON) {
        content.set_tooltip_text(Some(&segment.label));
        return content;
    }
    let label = gtk::Label::new(Some(&segment.label));
    label.set_tooltip_text(Some(&segment.label));
    if segment.icon.is_none() {
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_width_chars(MIN_SEGMENT_WIDTH_CHARS);
    }
    content.append(&label);
    content
}

fn separator() -> gtk::Label {
    let separator = gtk::Label::new(Some("›"));
    separator.add_css_class("breadcrumb-separator");
    separator
}

#[derive(Default)]
struct BreadcrumbRenderState {
    segments: Option<Vec<BreadcrumbSegment>>,
}

impl BreadcrumbRenderState {
    fn update(&mut self, segments: &[BreadcrumbSegment]) -> bool {
        if self.segments.as_deref() == Some(segments) {
            return false;
        }
        self.segments = Some(segments.to_vec());
        true
    }

    fn current(&self) -> Option<&[BreadcrumbSegment]> {
        self.segments.as_deref()
    }
}

#[derive(Default)]
pub(super) struct BreadcrumbBar {
    containers: Vec<gtk::Box>,
    rendered: BreadcrumbRenderState,
}

impl BreadcrumbBar {
    pub(super) fn attach(&mut self, container: gtk::Box, sender: &AppSender) {
        if let Some(segments) = self.rendered.current() {
            Self::render_container(&container, segments, sender);
        }
        self.containers.push(container);
    }

    pub(super) fn render(&mut self, segments: &[BreadcrumbSegment], sender: &AppSender) {
        if !self.rendered.update(segments) {
            return;
        }
        for container in &self.containers {
            Self::render_container(container, segments, sender);
        }
    }

    fn render_container(container: &gtk::Box, segments: &[BreadcrumbSegment], sender: &AppSender) {
        while let Some(child) = container.first_child() {
            container.remove(&child);
        }

        for (index, segment) in segments.iter().enumerate() {
            if index > 0 {
                container.append(&separator());
            }

            let content = segment_content(segment);
            let widget = if let Some(target) = &segment.target {
                content.add_css_class("breadcrumb-segment-content");

                let button = gtk::Button::builder()
                    .focusable(false)
                    .valign(gtk::Align::Center)
                    .build();
                button.add_css_class("breadcrumb-segment");
                if segment.icon == Some(LIBRARY_ROOT_ICON) {
                    button.set_tooltip_text(Some(&segment.label));
                }
                button.set_child(Some(&content));

                let target = target.clone();
                let sender = sender.clone();
                button.connect_clicked(move |_| {
                    sender.input(match &target {
                        BreadcrumbTarget::Library(path) => Msg::NavigateLibrary(path.clone()),
                        BreadcrumbTarget::Archive(location) => {
                            Msg::NavigateArchiveContents(location.clone())
                        }
                    });
                });
                button.upcast::<gtk::Widget>()
            } else {
                content.add_css_class("breadcrumb-current");
                content.upcast::<gtk::Widget>()
            };
            container.append(&widget);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library_target(path: &str) -> Option<BreadcrumbTarget> {
        Some(BreadcrumbTarget::Library(PathBuf::from(path)))
    }

    fn labels(segments: &[BreadcrumbSegment]) -> Vec<String> {
        segments
            .iter()
            .map(|segment| segment.label.clone())
            .collect()
    }

    fn segment(
        label: &str,
        icon: Option<&'static str>,
        target: Option<BreadcrumbTarget>,
    ) -> BreadcrumbSegment {
        BreadcrumbSegment {
            label: label.into(),
            icon,
            target,
        }
    }

    #[test]
    fn identical_render_state_does_not_require_rebuilding() {
        let segments = vec![segment(
            "本棚",
            Some(LIBRARY_ROOT_ICON),
            library_target("/books"),
        )];
        let mut state = BreadcrumbRenderState::default();

        assert!(state.update(&segments));
        assert!(!state.update(&segments));
    }

    #[test]
    fn label_target_and_icon_changes_each_require_rebuilding() {
        let mut state = BreadcrumbRenderState::default();
        assert!(state.update(&[segment("A", None, library_target("/a"))]));
        assert!(state.update(&[segment("B", None, library_target("/a"))]));
        assert!(state.update(&[segment("B", None, library_target("/b"))]));
        assert!(state.update(&[segment("B", Some(LIBRARY_ROOT_ICON), library_target("/b"))]));
    }

    #[test]
    fn current_segments_are_available_when_a_new_container_is_attached() {
        let segments = vec![segment("本棚", Some(LIBRARY_ROOT_ICON), None)];
        let mut state = BreadcrumbRenderState::default();
        assert!(state.update(&segments));

        assert_eq!(state.current(), Some(segments.as_slice()));
    }

    #[test]
    fn library_breadcrumbs_are_built_from_root_to_current_directory() {
        let segments = library_segments(Path::new("/books"), Path::new("/books/author/work"));

        assert_eq!(labels(&segments), ["本棚トップ", "author", "work"]);
        assert_eq!(segments[0].icon, Some(LIBRARY_ROOT_ICON));
        assert_eq!(segments[0].target, library_target("/books"));
        assert_eq!(segments[1].target, library_target("/books/author"));
        assert_eq!(segments[2].target, None);
        assert_eq!(parent_target(&segments), library_target("/books/author"));
    }

    #[test]
    fn library_root_is_an_icon_without_a_navigation_target() {
        let segments = library_segments(Path::new("/books"), Path::new("/books"));

        assert_eq!(labels(&segments), ["本棚トップ"]);
        assert_eq!(segments[0].icon, Some(LIBRARY_ROOT_ICON));
        assert_eq!(segments[0].target, None);
        assert_eq!(parent_target(&segments), None);
    }

    #[test]
    fn archive_breadcrumbs_keep_folders_nested_archives_and_current_directory() {
        let location = crate::archive::ArchiveLocation {
            archive: PathBuf::from("/books/book.cbz"),
            archives: vec![PathBuf::from("volumes/01.cbz")],
            directory: PathBuf::from("chapter1/pages"),
        };
        let segments = archive_segments(Some(Path::new("/books")), &location);

        assert_eq!(
            labels(&segments),
            [
                "本棚トップ",
                "book.cbz",
                "volumes",
                "01.cbz",
                "chapter1",
                "pages"
            ]
        );
        assert_eq!(
            segments[2].target,
            Some(BreadcrumbTarget::Archive(crate::archive::ArchiveLocation {
                archive: PathBuf::from("/books/book.cbz"),
                archives: Vec::new(),
                directory: PathBuf::from("volumes"),
            }))
        );
        assert_eq!(segments.last().unwrap().target, None);
        assert_eq!(
            parent_target(&segments),
            Some(BreadcrumbTarget::Archive(crate::archive::ArchiveLocation {
                archive: PathBuf::from("/books/book.cbz"),
                archives: vec![PathBuf::from("volumes/01.cbz")],
                directory: PathBuf::from("chapter1"),
            }))
        );
    }

    #[test]
    fn viewer_breadcrumbs_use_relative_ancestors_inside_root() {
        let segments = viewer_segments(
            Some(Path::new("/books")),
            Path::new("/books/author/work/01.zip"),
            None,
        );

        assert_eq!(
            labels(&segments),
            ["本棚トップ", "author", "work", "01.zip"]
        );
        assert_eq!(segments[2].target, library_target("/books/author/work"));
        assert_eq!(segments[3].target, None);
        assert_eq!(
            parent_target(&segments),
            library_target("/books/author/work")
        );
    }

    #[test]
    fn external_viewer_path_is_not_treated_as_a_library_descendant() {
        let segments = viewer_segments(
            Some(Path::new("/books")),
            Path::new("/other/author/01.zip"),
            None,
        );

        assert_eq!(labels(&segments), ["本棚トップ", "01.zip"]);
        assert_eq!(segments[0].target, library_target("/books"));
        assert_eq!(segments[1].target, None);
    }

    #[test]
    fn image_book_uses_folder_as_current_segment() {
        let current_source = ImageSource::File(PathBuf::from("/books/author/work/002.jpg"));

        let segments = viewer_segments(
            Some(Path::new("/books")),
            Path::new("/books/author/work/001.jpg"),
            Some(&current_source),
        );

        assert_eq!(labels(&segments), ["本棚トップ", "author", "work"]);
        assert_eq!(segments[1].target, library_target("/books/author"));
        assert_eq!(segments[2].target, None);
    }

    #[test]
    fn root_level_image_keeps_current_file_name() {
        let current_source = ImageSource::File(PathBuf::from("/books/002.jpg"));

        let segments = viewer_segments(
            Some(Path::new("/books")),
            Path::new("/books/001.jpg"),
            Some(&current_source),
        );

        assert_eq!(labels(&segments), ["本棚トップ", "002.jpg"]);
        assert_eq!(segments[0].target, library_target("/books"));
        assert_eq!(segments[1].target, None);
    }

    #[test]
    fn archive_document_ignores_file_source_from_temporary_extraction() {
        let extracted_source = ImageSource::File(PathBuf::from("/tmp/agnam/pages/002.jpg"));

        let segments = viewer_segments(
            Some(Path::new("/books")),
            Path::new("/books/author/work.cbz"),
            Some(&extracted_source),
        );

        assert_eq!(labels(&segments), ["本棚トップ", "author", "work.cbz"]);
    }
}
