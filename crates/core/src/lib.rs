//! nekoreq-core: ドメインモデル。
//!
//! 設計書「データモデル」の型を置く。yaml から作る `Operation`（読み取り専用）と、
//! 自分が入れた `RequestDraft` を分けて持ち、両者は `OpKey` で結びつける。
//!
//! - すべての型は serde（camelCase）と `specta::Type` を derive する。tauri-specta が
//!   `src/bindings.ts` に TypeScript の型として書き出す。
//! - ID は文字列の UUID（`new_id`）。日時は UTC の RFC 3339 文字列。
//! - `i64` / `u64` は TypeScript に書き出せないため使わない（時間・サイズは `u32` / `f64`）。
//! - シークレットの値はこのモデルに持たせない（`VarValue::Secret` は印だけ）。
//!
//! M2 で足したもの: 変数展開（`vars`）、リクエストの組み立て（`build`）、伏せ字処理（`redact`）、
//! 送信の設定と結果の型（`response`）。ここは通信も保存もしない（入出力は http・storage・secrets の仕事）。

pub mod build;
pub mod environment;
pub mod history;
pub mod operation;
pub mod project;
pub mod redact;
pub mod request;
mod request_encode;
pub mod response;
pub mod spec;
pub mod vars;

pub use build::{
    build_request, preview, referenced_variables, AutoHeader, BuildError, PreparedBody,
    PreparedPart, PreparedPartValue, PreparedRequest, RequestPreview, RequestSpec, DEFAULT_ACCEPT,
    DEFAULT_ACCEPT_ENCODING, DEFAULT_USER_AGENT,
};
pub use environment::{EnvVar, Environment, VarValue, GLOBAL_ENVIRONMENT_NAME};
pub use history::{HeaderPair, HistoryEntry, Timing};
pub use operation::{
    match_operations, MediaTypeSpec, OpKey, OpKeyParseError, Operation, OperationMatching,
    ParamSpec, RequestBodySpec, ResponseSpec, SecurityRef, SecurityRequirement,
};
pub use project::{ApiSource, Project, SourceFile, SourceKind};
pub use redact::{is_sensitive_header, redact_header_value, redact_headers, redact_text, MASK};
pub use request::{
    AdHocRequest, ApiKeyIn, AuthOverride, Body, KeyValue, MultipartPart, MultipartValue, ParamIn,
    RequestDraft,
};

pub use response::{
    BodyEncoding, ConnectionInfo, ResponseBody, ResponseData, SendError, SendOutcome, SendSettings,
    BODY_PREVIEW_LIMIT, DEFAULT_TIMEOUT_MS,
};
pub use spec::{
    ImportOptions, SecuritySchemeInfo, SecuritySchemeKind, ServerInfo, SpecMeta, TagInfo,
};
pub use vars::{
    expand, DynamicSource, Expanded, RowPart, ScopedValue, SystemDynamics, UnresolvedVar, VarLayer,
    VarLocation, VarScope,
};

/// 新しい ID（UUID v4 の文字列）を作る。
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_id_is_uuid_string() {
        let id = new_id();
        assert_eq!(id.len(), 36);
        assert!(uuid::Uuid::parse_str(&id).is_ok());
    }
}
