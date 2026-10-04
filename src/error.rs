#[derive(Debug, thiserror::Error)]
pub(crate) enum AppError {
    #[error("ファイルを開けませんでした: {0}")]
    Io(#[from] std::io::Error),
    #[error("アーカイブの読み込みに失敗しました: {0}")]
    Archive(String),
    #[error("アーカイブのresource safety limitに達しました: {0}")]
    ArchiveResourceLimit(#[from] crate::archive::ResourceLimitKind),
}
