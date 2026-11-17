use thiserror::Error;

/// storage クレートのエラー。
#[derive(Debug, Error)]
pub enum StorageError {
    /// SQLite のエラー。
    #[error("データベースのエラー: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// データフォルダの作成などのファイル操作のエラー。
    #[error("ファイル操作のエラー: {0}")]
    Io(#[from] std::io::Error),
    /// 保存済みの JSON や文字列が壊れている。
    #[error("保存されたデータを読めません: {0}")]
    Corrupt(String),
    /// このアプリより新しい版が作ったデータベース。
    #[error("データベースの版（{found}）がこのアプリの対応する版（{supported}）より新しいです")]
    SchemaTooNew { found: u32, supported: u32 },
    /// 名前が空、または長すぎる。
    #[error("{0}")]
    InvalidName(String),
    /// 入力が正しくない（変数名・グローバル環境への操作など）。メッセージは画面にそのまま出せる。
    #[error("{0}")]
    Invalid(String),
    /// 指定の ID が見つからない。
    #[error("{what}が見つかりません: {id}")]
    NotFound { what: &'static str, id: String },
}

pub type Result<T> = std::result::Result<T, StorageError>;
