use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArchiveFormat {
    Zip,
    Rar,
    SevenZip,
    Tar,
    Lha,
}

impl ArchiveFormat {
    pub(crate) const ALL: [Self; 5] = [Self::Zip, Self::Rar, Self::SevenZip, Self::Tar, Self::Lha];

    pub(crate) const fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Zip => &["zip", "cbz"],
            Self::Rar => &["rar", "cbr"],
            Self::SevenZip => &["7z", "cb7"],
            Self::Tar => &["tar", "cbt"],
            Self::Lha => &["lzh", "lha"],
        }
    }

    pub(crate) fn all_extensions() -> impl Iterator<Item = &'static str> {
        Self::ALL
            .into_iter()
            .flat_map(|format| format.extensions().iter().copied())
    }

    pub(crate) fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        Self::ALL.into_iter().find(|format| {
            format
                .extensions()
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_format_recognizes_its_extensions_case_insensitively() {
        let cases = [
            (ArchiveFormat::Zip, ["zip", "cbz"]),
            (ArchiveFormat::Rar, ["rar", "cbr"]),
            (ArchiveFormat::SevenZip, ["7z", "cb7"]),
            (ArchiveFormat::Tar, ["tar", "cbt"]),
            (ArchiveFormat::Lha, ["lzh", "lha"]),
        ];

        for (format, extensions) in cases {
            assert_eq!(format.extensions(), extensions);
            for extension in extensions {
                assert_eq!(
                    ArchiveFormat::from_path(Path::new(&format!("pages.{extension}"))),
                    Some(format)
                );
                assert_eq!(
                    ArchiveFormat::from_path(Path::new(&format!(
                        "pages.{}",
                        extension.to_uppercase()
                    ))),
                    Some(format)
                );
            }
        }
    }

    #[test]
    fn all_extensions_come_from_the_format_definitions() {
        assert_eq!(
            ArchiveFormat::all_extensions().collect::<Vec<_>>(),
            vec![
                "zip", "cbz", "rar", "cbr", "7z", "cb7", "tar", "cbt", "lzh", "lha"
            ]
        );
        assert!(ArchiveFormat::from_path(Path::new("pages.png")).is_none());
        assert!(ArchiveFormat::from_path(Path::new("pages")).is_none());
    }
}
