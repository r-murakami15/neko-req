//! nekoreq-curl: cURL コマンドの解析（`parse`）と書き出し（`export`）。
//!
//! - 解析: コマンド文字列 → `ParsedCurl`（core の `RequestSpec` に近い構造。`to_request_spec` で変換）。
//!   読んだけれど使わなかったオプションは `ParsedCurl::warnings` に入る。
//! - 書き出し: core の `RequestSpec`（変数を展開済みの値）→ bash（または Windows cmd）で貼り付けて動く cURL。
//! - OpenAPI の path テンプレートとの照合に使う `match_path_template` も置く（照合する側は別のクレート）。

mod error;
mod export;
mod lexer;
mod model;
mod options;
mod parse;
mod url_parts;

use serde::{Deserialize, Serialize};
use specta::Type;

pub use error::{CurlError, Position};
pub use export::{export, ExportOptions, FormStyle};
pub use lexer::detect_shell;
pub use model::{CurlWarning, ParsedCurl, WarningKind, BASE_URL_VAR};
pub use parse::{parse, parse_as};
pub use url_parts::{match_path_template, PathMatch};

/// コマンドの書き方（引用符と行継続の規則）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Shell {
    /// bash・zsh（シングルクォート、`\` の行継続、`$'...'`）。
    #[default]
    Bash,
    /// Windows の cmd.exe（ダブルクォート、`^` の行継続とエスケープ）。
    Cmd,
}
