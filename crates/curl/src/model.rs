//! 解析結果の型。core の `RequestSpec`・`KeyValue`・`Body`・`AuthOverride` に近い形にしてある。

use nekoreq_core::{AuthOverride, Body, KeyValue, ParamIn, RequestSpec};
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::url_parts::{origin_key, split_base_prefix};

/// 環境の `baseUrl` 変数に置き換えるときの変数名。
pub const BASE_URL_VAR: &str = "baseUrl";

/// 画面に出す注意の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum WarningKind {
    /// 読んで捨てたオプション（`--compressed`、`-k`、`-L`、`-o` など）。「無視したオプション」として出す。
    Ignored,
    /// 知らないオプション（捨てた）。
    Unknown,
    /// アプリで扱えない指定（`-b` のファイル名、`@-`（標準入力）、`;filename=` など）。取り込んだ内容から欠ける。
    Unsupported,
    /// 取り込みでの読み替え（スキームの補完、`-u` のパスワード欠け、`-b` より Cookie ヘッダーが優先など）。
    Adjusted,
}

/// 取り込めたが注意が要ること。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CurlWarning {
    pub kind: WarningKind,
    /// 該当するオプション（`-k`、`--compressed` など。特定のオプションがないときは空）。
    pub option: String,
    /// 画面にそのまま出せる説明。
    pub message: String,
}

impl CurlWarning {
    pub(crate) fn new(
        kind: WarningKind,
        option: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            option: option.into(),
            message: message.into(),
        }
    }
}

/// cURL コマンドの解析結果。
///
/// - `base` + `path` が URL（クエリは `query` に分けてある）。`base` は `https://host:8080` の形で、
///   スキームがなければ `http://` を補う。`{{baseUrl}}/api/x` のように変数で始まる URL は `{{baseUrl}}` が `base`。
/// - ヘッダーのうち `Cookie` は `cookies` に、`Authorization: Bearer` / `Basic` は `auth` に移してある。
///   `Content-Type` はボディの種類から core が自動で付けるものと同じ値なら `headers` から外してある。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ParsedCurl {
    /// 大文字。
    pub method: String,
    /// スキーム + ホスト + ポート（パスの前まで。末尾に `/` は付かない）。
    pub base: String,
    /// `/` で始まるパス（クエリ・フラグメントを除く。`{id}` のようなテンプレートとの照合に使う）。空のこともある。
    pub path: String,
    /// URL のクエリと `-G` で移した `-d` の値（`location` は `Query`）。
    pub query: Vec<KeyValue>,
    /// `location` は `Header`。
    pub headers: Vec<KeyValue>,
    /// `-b` と `Cookie` ヘッダーから作った行（`location` は `Cookie`）。
    pub cookies: Vec<KeyValue>,
    pub body: Body,
    pub auth: Option<AuthOverride>,
    /// 無視したオプションなど（出てきた順）。
    pub warnings: Vec<CurlWarning>,
}

impl ParsedCurl {
    /// クエリなしの URL（`base` + `path`）。
    pub fn url(&self) -> String {
        format!("{}{}", self.base, self.path)
    }

    /// `query`・`headers`・`cookies` を 1 つにした行（`RequestSpec.params` の並び）。
    pub fn params(&self) -> Vec<KeyValue> {
        self.query
            .iter()
            .chain(&self.headers)
            .chain(&self.cookies)
            .cloned()
            .collect()
    }

    /// core の `RequestSpec` にする。`replace_base` が真なら URL の `base` を `{{baseUrl}}` に置き換える
    /// （呼び出し側が、環境の `baseUrl` と `base` が一致すると確かめたときに使う）。
    pub fn to_request_spec(&self, replace_base: bool) -> RequestSpec {
        let url = if replace_base {
            format!("{{{{{BASE_URL_VAR}}}}}{}", self.path)
        } else {
            self.url()
        };
        self.spec_with_url(url)
    }

    /// 環境の `baseUrl`（`http://localhost:8080` や、パスを含む `https://example.com/api`）と照合し、
    /// 一致すれば URL の先頭を `{{baseUrl}}` に置き換えた `RequestSpec` を返す。
    /// 一致するのは、スキーム・ホスト・ポートが同じ（大文字小文字と既定ポートの違いは無視）で、
    /// `baseUrl` にパスがあるときはそのパスで始まる（区切りの `/` ごと）場合。一致しなければ `None`。
    pub fn to_request_spec_for_base(&self, env_base_url: &str) -> Option<RequestSpec> {
        let rest = self.path_after_base(env_base_url)?;
        Some(self.spec_with_url(format!("{{{{{BASE_URL_VAR}}}}}{rest}")))
    }

    /// `env_base_url` と一致するか（`to_request_spec_for_base` と同じ判定）。
    pub fn matches_base(&self, env_base_url: &str) -> bool {
        self.path_after_base(env_base_url).is_some()
    }

    /// `env_base_url` のあとに続く path（`/` で始まる、または空）。一致しなければ `None`。
    /// OpenAPI の path テンプレートとの照合には、この戻り値（`baseUrl` にパスがあるときはそれを除いたもの）を使う。
    pub fn path_after_base(&self, env_base_url: &str) -> Option<String> {
        let (origin, prefix) = split_base_prefix(env_base_url)?;
        if origin_key(&self.base)? != origin_key(&origin)? {
            return None;
        }
        if prefix.is_empty() {
            return Some(self.path.clone());
        }
        let rest = self.path.strip_prefix(prefix.as_str())?;
        if rest.is_empty() || rest.starts_with('/') {
            Some(rest.to_string())
        } else {
            None
        }
    }

    fn spec_with_url(&self, url: String) -> RequestSpec {
        RequestSpec {
            method: self.method.clone(),
            url,
            params: self.params(),
            body: self.body.clone(),
            auth: self.auth.clone(),
        }
    }
}

impl From<&ParsedCurl> for RequestSpec {
    fn from(p: &ParsedCurl) -> Self {
        p.to_request_spec(false)
    }
}

pub(crate) fn row(location: ParamIn, name: &str, value: &str) -> KeyValue {
    KeyValue {
        location,
        name: name.to_string(),
        value: value.to_string(),
        enabled: true,
    }
}
