//! 解析のエラー。メッセージは画面にそのまま出せる日本語。

/// 引用符が閉じていない場所（1 始まり）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

/// `parse` が返すエラー（取り込めない入力）。取り込めるが注意が要るものは `CurlWarning` で返す。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CurlError {
    /// 空、または空白だけ。
    #[error("cURL コマンドが空です")]
    Empty,
    /// 引用符が閉じていない。
    #[error("引用符（{quote}）が閉じられていません{}", .position.map(|p| format!("（{} 行目 {} 文字目から）", p.line, p.column)).unwrap_or_default())]
    UnterminatedQuote {
        quote: char,
        position: Option<Position>,
    },
    /// 値が要るオプションの値がない（`-H` で終わっているなど）。
    #[error("オプション {option} の値がありません")]
    MissingArgument { option: String },
    /// URL がない。
    #[error("URL が見つかりません。cURL コマンドに URL を含めてください")]
    NoUrl,
    /// URL を読めない。
    #[error("URL を読めません: {0}")]
    InvalidUrl(String),
    /// その他の構文エラー。
    #[error("コマンドを読めません: {0}")]
    Syntax(String),
}
