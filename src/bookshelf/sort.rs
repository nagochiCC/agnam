#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum LibrarySortKey {
    #[default]
    Name,
    Modified,
    Created,
}

impl LibrarySortKey {
    pub(crate) fn storage_value(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Modified => "modified",
            Self::Created => "created",
        }
    }

    pub(crate) fn from_storage_value(value: &str) -> Option<Self> {
        match value {
            "name" => Some(Self::Name),
            "modified" => Some(Self::Modified),
            "created" => Some(Self::Created),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum LibrarySortDirection {
    #[default]
    Ascending,
    Descending,
}

impl LibrarySortDirection {
    pub(crate) fn storage_value(self) -> &'static str {
        match self {
            Self::Ascending => "ascending",
            Self::Descending => "descending",
        }
    }

    pub(crate) fn from_storage_value(value: &str) -> Option<Self> {
        match value {
            "ascending" => Some(Self::Ascending),
            "descending" => Some(Self::Descending),
            _ => None,
        }
    }
}
