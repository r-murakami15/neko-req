//! 送信の設定と結果。画面へ渡す型（specta）と、ボディの切り詰め・テキスト判定。

use base64::Engine;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::history::{HeaderPair, Timing};
use crate::vars::UnresolvedVar;

/// 既定のタイムアウト（ミリ秒）。
pub const DEFAULT_TIMEOUT_MS: u32 = 30_000;

/// 画面に返すボディの上限（バイト）。これを超えたら先頭だけを返す。
pub const BODY_PREVIEW_LIMIT: usize = 10 * 1024 * 1024;

/// 送信の設定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SendSettings {
    /// リクエスト全体（接続から本文を読み終えるまで）のタイムアウト。
    pub timeout_ms: u32,
    /// リダイレクト（最大 10 回）を追う。
    pub follow_redirects: bool,
    /// TLS 証明書を検証する。`false` で自己署名証明書なども通す。
    pub verify_certificates: bool,
}

impl Default for SendSettings {
    fn default() -> Self {
        Self {
            timeout_ms: DEFAULT_TIMEOUT_MS,
            follow_redirects: true,
            verify_certificates: true,
        }
    }
}

/// ボディの文字列の入り方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum BodyEncoding {
    /// テキスト（JSON 整形・Raw 表示ができる）。UTF-8 以外の文字コード（Shift_JIS など）は UTF-8 に変換してある。
    Text,
    /// テキストではないボディの base64（画面では「バイナリ」として扱う）。
    Base64,
}

/// レスポンスのボディ。`content` は先頭 10 MB まで。全体は Rust 側が保持していて、
/// `response_save_body` でファイルに保存できる。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ResponseBody {
    pub content_type: Option<String>,
    pub encoding: BodyEncoding,
    pub content: String,
    /// ボディ全体のバイト数（切り詰める前。圧縮は展開した後。文字コードを変換する前の元のバイト数）。
    pub size: u32,
    /// 10 MB を超えて、`content` が先頭だけのとき `true`。
    pub truncated: bool,
}

impl ResponseBody {
    pub fn empty(content_type: Option<String>) -> Self {
        Self {
            content_type,
            encoding: BodyEncoding::Text,
            content: String::new(),
            size: 0,
            truncated: false,
        }
    }

    /// 受け取ったバイト列から、画面に返す形を作る。
    ///
    /// - テキスト: Content-Type が明らかなバイナリ（画像・音声・動画・octet-stream・zip・pdf など）でなく、
    ///   UTF-8 として読めて、先頭 8 KB に NUL がない。UTF-8 として読めないときは、Content-Type の
    ///   `charset`（Shift_JIS・EUC-JP など）で文字列に変換できればテキスト（`content` は UTF-8）にする。
    ///   どちらでも読めないものだけ base64。
    /// - `preview_limit` を超えるぶんは切り捨てる（テキストは文字の途中で切らない）。
    pub fn from_bytes(content_type: Option<&str>, bytes: &[u8], preview_limit: usize) -> Self {
        let truncated = bytes.len() > preview_limit;
        let preview = &bytes[..bytes.len().min(preview_limit)];
        let size = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        let content_type_owned = content_type.map(str::to_string);

        if !declares_binary(content_type) {
            if let Some(text) = as_text(preview, truncated) {
                return Self {
                    content_type: content_type_owned,
                    encoding: BodyEncoding::Text,
                    content: text.to_string(),
                    size,
                    truncated,
                };
            }
            if let Some(text) = decode_with_charset(content_type, preview, truncated) {
                return Self {
                    content_type: content_type_owned,
                    encoding: BodyEncoding::Text,
                    content: text,
                    size,
                    truncated,
                };
            }
        }
        Self {
            content_type: content_type_owned,
            encoding: BodyEncoding::Base64,
            content: base64::engine::general_purpose::STANDARD.encode(preview),
            size,
            truncated,
        }
    }
}

fn declares_binary(content_type: Option<&str>) -> bool {
    let Some(ct) = content_type else {
        return false;
    };
    let ct = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    ct.starts_with("image/")
        || ct.starts_with("audio/")
        || ct.starts_with("video/")
        || ct.starts_with("font/")
        || matches!(
            ct.as_str(),
            "application/octet-stream"
                | "application/zip"
                | "application/pdf"
                | "application/gzip"
                | "application/x-gzip"
                | "application/x-tar"
                | "application/x-7z-compressed"
                | "application/vnd.ms-excel"
                | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        )
}

/// UTF-8 のテキストなら、そのスライス（切り詰めで末尾の文字が欠けていれば、欠けた文字を除く）。
fn as_text(bytes: &[u8], truncated: bool) -> Option<&str> {
    let head = &bytes[..bytes.len().min(8192)];
    if head.contains(&0) {
        return None;
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => Some(s),
        // 切り詰めのせいで末尾の文字が途中で終わっているだけなら、その手前までをテキストにする
        Err(e) if truncated && e.error_len().is_none() => {
            std::str::from_utf8(&bytes[..e.valid_up_to()]).ok()
        }
        Err(_) => None,
    }
}

/// Content-Type の `charset` が UTF-8 以外のとき、その文字コードで文字列に変換する。
/// 文字コードを知らない・変換できない（不正なバイトがある）ときは `None`。
/// 切り詰めで末尾の文字が途中で終わっているときは、その手前までを変換する。
fn decode_with_charset(
    content_type: Option<&str>,
    bytes: &[u8],
    truncated: bool,
) -> Option<String> {
    let label = charset_of(content_type?)?;
    let encoding = encoding_rs::Encoding::for_label(label.as_bytes())?;
    if encoding == encoding_rs::UTF_8 {
        return None;
    }
    // UTF-16 は NUL を含むのが普通。それ以外は NUL があればバイナリ
    let utf16 = encoding == encoding_rs::UTF_16LE || encoding == encoding_rs::UTF_16BE;
    if !utf16 && bytes[..bytes.len().min(8192)].contains(&0) {
        return None;
    }
    // 切り詰めた末尾の欠けた文字（最長 3 バイト）は落として試す
    let cut_max = if truncated { 3 } else { 0 };
    for cut in 0..=cut_max.min(bytes.len()) {
        let head = &bytes[..bytes.len() - cut];
        if let Some(text) = encoding.decode_without_bom_handling_and_without_replacement(head) {
            return Some(text.into_owned());
        }
    }
    None
}

/// Content-Type の `charset=` の値（引用符・空白を除く）。
fn charset_of(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|param| {
        let (name, value) = param.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches('"').trim().to_string())
    })
}

/// 接続の情報。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    /// 接続先の `IP:ポート`（IPv6 は `[::1]:8080`）。取れなければ `None`。
    pub remote_addr: Option<String>,
    /// `HTTP/1.1` / `HTTP/2` など。
    pub http_version: String,
    /// TLS（https）で接続した。
    pub tls: bool,
}

/// 受け取ったレスポンス。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ResponseData {
    pub status: u16,
    /// 理由句（`OK` など）。サーバーが返した語ではなく、ステータスコードの標準の理由句（知らないコードは空）。
    pub status_text: String,
    /// 受け取った順・重複を保ったヘッダー。サーバーが返したまま（`content-encoding`・`content-length` も含む）。
    pub headers: Vec<HeaderPair>,
    pub body: ResponseBody,
    pub timing: Timing,
    pub connection: ConnectionInfo,
    /// リダイレクトを追ったあとの最終的な URL（伏せ字にしていない。シークレットがクエリに入っていれば含まれる）。
    pub final_url: String,
    /// 追ったリダイレクトの回数。
    pub redirect_count: u32,
    /// 送ったバイト数（リクエスト行 + ヘッダー + ボディを HTTP/1.1 の書式で数えた近似）。
    pub sent_bytes: u32,
    /// 受け取ったバイト数（ステータス行 + ヘッダー + 転送されたボディ（展開前）の近似）。展開後の大きさは `body.size`。
    pub received_bytes: u32,
}

/// 送信の失敗。画面には `message` をそのまま出せる。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type, thiserror::Error)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum SendError {
    /// 展開できない変数がある（送信していない）。
    #[error("{message}")]
    Unresolved {
        message: String,
        vars: Vec<UnresolvedVar>,
    },
    /// URL・method・ヘッダーなど、リクエストの形が正しくない（送信していない）。
    #[error("{message}")]
    InvalidRequest { message: String },
    /// ボディに指定したファイルを読めない（送信していない）。
    #[error("{message}")]
    BodyFile { message: String },
    /// 接続できない（名前の解決・接続の拒否・切断など）。
    #[error("{message}")]
    Connect { message: String },
    /// TLS（証明書の検証・ハンドシェイク）の失敗。
    #[error("{message}")]
    Tls { message: String },
    /// タイムアウト。
    #[error("{message}")]
    Timeout { message: String },
    /// 「中止」で止めた。
    #[error("{message}")]
    Cancelled { message: String },
    /// 受け取るボディが大きすぎる。
    #[error("{message}")]
    TooLarge { message: String },
    /// ほかの失敗。
    #[error("{message}")]
    Other { message: String },
}

impl SendError {
    /// 画面に出す日本語のメッセージ。
    pub fn message(&self) -> &str {
        match self {
            SendError::Unresolved { message, .. }
            | SendError::InvalidRequest { message }
            | SendError::BodyFile { message }
            | SendError::Connect { message }
            | SendError::Tls { message }
            | SendError::Timeout { message }
            | SendError::Cancelled { message }
            | SendError::TooLarge { message }
            | SendError::Other { message } => message,
        }
    }

    pub fn cancelled() -> Self {
        SendError::Cancelled {
            message: "送信を中止しました".to_string(),
        }
    }

    /// 展開できない変数から作る。
    pub fn unresolved(vars: Vec<UnresolvedVar>) -> Self {
        let list = vars
            .iter()
            .map(UnresolvedVar::describe)
            .collect::<Vec<_>>()
            .join("、");
        SendError::Unresolved {
            message: format!("展開できない変数があるため送信しませんでした: {list}"),
            vars,
        }
    }
}

/// 送信の結果。成功（レスポンスを受け取った）か失敗。4xx / 5xx も「成功」に入る。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", tag = "status")]
// 1 回の送信につき 1 つしか作らないので、大きさの差は気にしない
#[allow(clippy::large_enum_variant)]
pub enum SendOutcome {
    Success { response: ResponseData },
    Failure { error: SendError },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_is_text_and_size_is_the_full_length() {
        let b = ResponseBody::from_bytes(Some("application/json"), br#"{"a":1}"#, 1024);
        assert_eq!(b.encoding, BodyEncoding::Text);
        assert_eq!(b.content, r#"{"a":1}"#);
        assert_eq!(b.size, 7);
        assert!(!b.truncated);
        assert_eq!(b.content_type.as_deref(), Some("application/json"));
    }

    #[test]
    fn declared_binary_is_base64_even_if_it_looks_like_text() {
        let b = ResponseBody::from_bytes(Some("application/octet-stream"), b"abc", 1024);
        assert_eq!(b.encoding, BodyEncoding::Base64);
        assert_eq!(b.content, "YWJj");
        let b = ResponseBody::from_bytes(Some("image/png; foo=bar"), b"abc", 1024);
        assert_eq!(b.encoding, BodyEncoding::Base64);
    }

    #[test]
    fn invalid_utf8_and_nul_are_binary() {
        let b = ResponseBody::from_bytes(None, &[0xff, 0xfe, 0x00], 1024);
        assert_eq!(b.encoding, BodyEncoding::Base64);
        let b = ResponseBody::from_bytes(Some("text/plain"), b"a\0b", 1024);
        assert_eq!(b.encoding, BodyEncoding::Base64);
        // Content-Type がなくても UTF-8 ならテキスト
        let b = ResponseBody::from_bytes(None, "日本語".as_bytes(), 1024);
        assert_eq!(b.encoding, BodyEncoding::Text);
    }

    #[test]
    fn truncation_keeps_the_head_and_the_full_size() {
        let b = ResponseBody::from_bytes(Some("text/plain"), b"0123456789", 4);
        assert!(b.truncated);
        assert_eq!(b.content, "0123");
        assert_eq!(b.size, 10);
    }

    #[test]
    fn truncation_does_not_split_a_multibyte_character() {
        // 「あ」は 3 バイト。4 バイトで切ると 2 文字目が途中で終わる
        let b = ResponseBody::from_bytes(Some("text/plain"), "ああ".as_bytes(), 4);
        assert_eq!(b.encoding, BodyEncoding::Text);
        assert_eq!(b.content, "あ");
        assert!(b.truncated);
        assert_eq!(b.size, 6);
        // 切り詰めていないのに壊れた UTF-8 はバイナリ
        let b = ResponseBody::from_bytes(Some("text/plain"), &"あ".as_bytes()[..2], 100);
        assert_eq!(b.encoding, BodyEncoding::Base64);
    }

    #[test]
    fn non_utf8_charsets_are_converted_to_text() {
        // Shift_JIS の「日本語」
        let sjis = [0x93, 0xFA, 0x96, 0x7B, 0x8C, 0xEA];
        let b = ResponseBody::from_bytes(Some("text/plain; charset=Shift_JIS"), &sjis, 1024);
        assert_eq!(b.encoding, BodyEncoding::Text);
        assert_eq!(b.content, "日本語");
        assert_eq!(b.size, 6, "size は元のバイト数");
        // EUC-JP（引用符つき・JSON）
        let (euc, _, _) = encoding_rs::EUC_JP.encode("日本語");
        let b = ResponseBody::from_bytes(Some("application/json; charset=\"euc-jp\""), &euc, 1024);
        assert_eq!(b.encoding, BodyEncoding::Text);
        assert_eq!(b.content, "日本語");
        // 知らない文字コード・charset のない非 UTF-8 は base64
        let b = ResponseBody::from_bytes(Some("text/plain; charset=x-unknown"), &sjis, 1024);
        assert_eq!(b.encoding, BodyEncoding::Base64);
        let b = ResponseBody::from_bytes(Some("text/plain"), &sjis, 1024);
        assert_eq!(b.encoding, BodyEncoding::Base64);
        // Shift_JIS として不正なバイトがあれば base64
        let b = ResponseBody::from_bytes(Some("text/plain; charset=Shift_JIS"), &[0x81], 1024);
        assert_eq!(b.encoding, BodyEncoding::Base64);
        // 宣言がバイナリなら変換しない
        let b = ResponseBody::from_bytes(Some("image/png; charset=Shift_JIS"), &sjis, 1024);
        assert_eq!(b.encoding, BodyEncoding::Base64);
    }

    #[test]
    fn charset_conversion_does_not_split_a_character_when_truncated() {
        let sjis = [0x93, 0xFA, 0x96, 0x7B, 0x8C, 0xEA];
        // 3 バイトで切ると 2 文字目が途中で終わる
        let b = ResponseBody::from_bytes(Some("text/plain; charset=Shift_JIS"), &sjis, 3);
        assert_eq!(b.encoding, BodyEncoding::Text);
        assert_eq!(b.content, "日");
        assert!(b.truncated);
        assert_eq!(b.size, 6);
    }

    #[test]
    fn utf16_with_a_declared_charset_is_text() {
        let raw: Vec<u8> = "a ねこ".encode_utf16().flat_map(u16::to_le_bytes).collect();
        let b = ResponseBody::from_bytes(Some("text/plain; charset=utf-16le"), &raw, 1024);
        assert_eq!(b.encoding, BodyEncoding::Text);
        assert_eq!(b.content, "a ねこ");
    }

    #[test]
    fn utf8_bytes_stay_text_even_if_the_declared_charset_says_otherwise() {
        let b = ResponseBody::from_bytes(
            Some("text/plain; charset=ISO-8859-1"),
            "日本語".as_bytes(),
            1024,
        );
        assert_eq!(b.content, "日本語");
    }

    #[test]
    fn empty_body() {
        let b = ResponseBody::from_bytes(None, b"", 10);
        assert_eq!(b.encoding, BodyEncoding::Text);
        assert_eq!(b.size, 0);
        assert_eq!(ResponseBody::empty(None).content, "");
    }

    #[test]
    fn defaults_match_the_spec() {
        let s = SendSettings::default();
        assert_eq!(s.timeout_ms, 30_000);
        assert!(s.follow_redirects && s.verify_certificates);
    }

    #[test]
    fn send_error_wire_shape_and_messages() {
        let e = SendError::cancelled();
        assert_eq!(e.message(), "送信を中止しました");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind": "cancelled", "message": "送信を中止しました"})
        );
        let e = SendError::unresolved(vec![UnresolvedVar {
            name: "baseUrl".into(),
            location: crate::vars::VarLocation::Url,
        }]);
        assert!(
            e.message().contains("{{baseUrl}}（URL）"),
            "{}",
            e.message()
        );
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["kind"], "unresolved");
        assert_eq!(v["vars"][0]["name"], "baseUrl");
    }
}
