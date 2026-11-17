//! リクエストの組み立て。method・URL・パラメータ・ヘッダー・ボディ・認証と変数の集まりから、
//! 送信用に確定した `PreparedRequest` を作る。
//!
//! - 無効（`enabled = false`）の行は送らない。
//! - URL: `{{変数}}` を展開 → パスパラメータ `{id}`（パーセントエンコードして埋める）→ クエリを追加。
//!   スキームがなければ `http://` を補う。
//! - 同じ名前のヘッダーを手で入れていれば、自動で付けるもの（Authorization・Content-Type・Cookie）より
//!   手入力を優先する。手入力の Authorization があるときは認証の設定は使わない（認証欄の変数も展開しない）。
//!   multipart だけは境界の文字列が要るので、手入力の Content-Type は無視して自動で決める。
//! - 展開できない変数があれば組み立てずに、場所と名前を返す（送信しない）。

use base64::Engine;
use percent_encoding::utf8_percent_encode;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::history::HeaderPair;
use crate::redact::{redact_header_value, redact_text};
use crate::request::{ApiKeyIn, AuthOverride, Body, KeyValue, MultipartValue, ParamIn};
use crate::request_encode::{PATH_SEGMENT, QUERY_COMPONENT};
use crate::vars::{expand, DynamicSource, RowPart, UnresolvedVar, VarLocation, VarScope};

/// アプリが付ける User-Agent。
pub const DEFAULT_USER_AGENT: &str = concat!("NekoReq/", env!("CARGO_PKG_VERSION"));
/// 手で指定しなければ付く Accept。
pub const DEFAULT_ACCEPT: &str = "*/*";
/// 手で指定しなければ付く Accept-Encoding。プレビュー・実際の送信・送信バイト数の見積もりで同じ値を使う
/// （応答の展開は http クレートが自前で行う）。
pub const DEFAULT_ACCEPT_ENCODING: &str = "gzip, br, deflate";

/// 送るリクエストの入力（画面で編集している内容）。`url` と各値に `{{変数}}` を書ける。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RequestSpec {
    pub method: String,
    /// `{{baseUrl}}/api/v1/clients/{clientId}` のようなテンプレート。
    pub url: String,
    /// クエリ・パス・ヘッダー・Cookie のパラメータ（`location` で区別）。
    pub params: Vec<KeyValue>,
    pub body: Body,
    /// `None` と `AuthOverride::None` はどちらも認証なし。
    pub auth: Option<AuthOverride>,
}

impl From<&crate::request::AdHocRequest> for RequestSpec {
    fn from(r: &crate::request::AdHocRequest) -> Self {
        Self {
            method: r.method.clone(),
            url: r.url.clone(),
            params: r.params.clone(),
            body: r.body.clone(),
            auth: r.auth.clone(),
        }
    }
}

/// 送信用に確定したリクエスト。値にはシークレットの実際の値が入っているので、
/// 画面・ログ・履歴に出すときは `redact` で伏せること。
#[derive(Clone, PartialEq, Eq)]
pub struct PreparedRequest {
    /// 大文字。
    pub method: String,
    /// クエリまで確定した URL（エンコード済み）。
    pub url: String,
    /// 手入力のヘッダーと、認証・ボディから自動で足したヘッダー（順番どおり）。
    /// User-Agent・Accept・Accept-Encoding は送信側（http クレート）が足す。
    pub headers: Vec<HeaderPair>,
    pub body: PreparedBody,
    /// 差し込んだシークレットの値（伏せ字にする対象）。
    pub secrets: Vec<String>,
}

// シークレットの値を持つので、Debug に出さない
impl std::fmt::Debug for PreparedRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedRequest")
            .field("method", &self.method)
            .field("url", &redact_text(&self.url, &self.secrets))
            .field("headers", &self.headers.len())
            .finish_non_exhaustive()
    }
}

/// 送るボディ。ファイルは送信時に読む。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedBody {
    None,
    Bytes(Vec<u8>),
    /// 中身をそのまま送るファイルのパス。
    File(String),
    Multipart {
        boundary: String,
        parts: Vec<PreparedPart>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPart {
    pub name: String,
    pub value: PreparedPartValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedPartValue {
    Text(String),
    File(String),
}

/// 組み立てに失敗した理由。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    /// 展開できない変数がある（送信しない）。
    #[error("展開できない変数があります: {}", .0.iter().map(|v| v.describe()).collect::<Vec<_>>().join("、"))]
    Unresolved(Vec<UnresolvedVar>),
    /// URL が空、または読めない。
    #[error("{0}")]
    InvalidUrl(String),
    /// 認証の設定が正しくない（Basic のユーザー名に `:` など）。
    #[error("{0}")]
    InvalidAuth(String),
}

/// 自動で付くヘッダー（画面のヘッダータブの「自動で付与されるヘッダー」）。値は伏せ字済み。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AutoHeader {
    pub name: String,
    pub value: String,
    /// 付く理由（「認証」「ボディ」「アプリ既定」「URL から」）。
    pub source: String,
}

/// リクエストに書かれている変数の名前（`{{ }}` の中身。動的変数 `$...` は除く。出てきた順、重複なし）。
/// 無効の行は送らないので数えない。シークレットをキーチェーンから読む範囲を、使うものだけに絞るのに使う。
pub fn referenced_variables(spec: &RequestSpec) -> Vec<String> {
    let mut texts: Vec<&str> = vec![&spec.url];
    for p in spec.params.iter().filter(|p| p.enabled) {
        texts.push(&p.name);
        texts.push(&p.value);
    }
    match &spec.body {
        Body::None => {}
        Body::Json(t) | Body::Text(t) | Body::Binary(t) => texts.push(t),
        Body::Form(rows) => {
            for r in rows.iter().filter(|r| r.enabled) {
                texts.push(&r.name);
                texts.push(&r.value);
            }
        }
        Body::Multipart(parts) => {
            for part in parts.iter().filter(|p| p.enabled) {
                texts.push(&part.name);
                match &part.value {
                    MultipartValue::Text(t) | MultipartValue::File(t) => texts.push(t),
                }
            }
        }
    }
    match &spec.auth {
        None | Some(AuthOverride::None) => {}
        Some(AuthOverride::Bearer { token }) => texts.push(token),
        Some(AuthOverride::Basic { username, password }) => {
            texts.push(username);
            texts.push(password);
        }
        Some(AuthOverride::ApiKey { name, value, .. }) => {
            texts.push(name);
            texts.push(value);
        }
    }
    let mut names: Vec<String> = Vec::new();
    for text in texts {
        for n in crate::vars::variable_names(text) {
            if !n.starts_with('$') && !names.contains(&n) {
                names.push(n);
            }
        }
    }
    names
}

/// 展開後の URL などの確認用。シークレットの値は `••••` に置き換えてある。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RequestPreview {
    /// 展開後の URL。展開できない変数があるときは `{{name}}` を残した途中の姿。
    pub url: String,
    /// 展開できない変数（場所つき）。空でなければ送信できない。
    pub unresolved: Vec<UnresolvedVar>,
    /// URL にあるのに値が入っていないパスパラメータ（送信はできる。警告用）。パスパラメータの行がない `{id}`、
    /// 値が空のもの、値が `.` / `..` のもの（URL の仕様上 1 区間として送れないので置き換えない）。
    pub missing_path_params: Vec<String>,
    /// URL を読めないときの理由（送信できない）。
    pub url_error: Option<String>,
    pub auto_headers: Vec<AutoHeader>,
}

/// 組み立ての途中結果（プレビューと送信で共通）。
struct Assembly {
    prepared: PreparedRequest,
    unresolved: Vec<UnresolvedVar>,
    missing_path_params: Vec<String>,
    url_error: Option<String>,
    auth_error: Option<String>,
    auto: Vec<AutoHeader>,
}

/// 送信用のリクエストを組み立てる。
pub fn build_request(
    spec: &RequestSpec,
    scope: &VarScope,
    dynamics: &dyn DynamicSource,
) -> Result<PreparedRequest, BuildError> {
    let a = assemble(spec, scope, dynamics);
    if !a.unresolved.is_empty() {
        return Err(BuildError::Unresolved(a.unresolved));
    }
    if let Some(e) = a.url_error {
        return Err(BuildError::InvalidUrl(e));
    }
    if let Some(e) = a.auth_error {
        return Err(BuildError::InvalidAuth(e));
    }
    Ok(a.prepared)
}

/// 展開後の URL と、展開できない変数の一覧を作る（送信はしない）。
pub fn preview(
    spec: &RequestSpec,
    scope: &VarScope,
    dynamics: &dyn DynamicSource,
) -> RequestPreview {
    let a = assemble(spec, scope, dynamics);
    let secrets = &a.prepared.secrets;
    let mut auto_headers: Vec<AutoHeader> = a
        .auto
        .iter()
        .map(|h| AutoHeader {
            name: h.name.clone(),
            value: redact_text(&redact_header_value(&h.name, &h.value), secrets),
            source: h.source.clone(),
        })
        .collect();
    let has = |name: &str| {
        a.prepared
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case(name))
    };
    for (name, value) in [
        ("User-Agent", DEFAULT_USER_AGENT),
        ("Accept", DEFAULT_ACCEPT),
        ("Accept-Encoding", DEFAULT_ACCEPT_ENCODING),
    ] {
        if !has(name) {
            auto_headers.push(AutoHeader {
                name: name.to_string(),
                value: value.to_string(),
                source: "アプリ既定".to_string(),
            });
        }
    }
    if !has("Host") {
        if let Some(host) = host_header(&a.prepared.url) {
            auto_headers.push(AutoHeader {
                name: "Host".to_string(),
                value: host,
                source: "URL から".to_string(),
            });
        }
    }
    RequestPreview {
        url: display_missing_path_params(
            redact_text(&a.prepared.url, secrets),
            &a.missing_path_params,
        ),
        unresolved: a.unresolved,
        missing_path_params: a.missing_path_params,
        url_error: a.url_error,
        auto_headers,
    }
}

/// 値が入らなかった `{name}` は、URL を整えたときに `%7Bname%7D` になっているので、
/// 画面のプレビューでは `{name}` のまま見せる（送る URL は変えない）。
fn display_missing_path_params(url: String, missing: &[String]) -> String {
    let mut url = url;
    for name in missing {
        let encoded = utf8_percent_encode(name, QUERY_COMPONENT).to_string();
        url = url.replace(&format!("%7B{encoded}%7D"), &format!("{{{name}}}"));
    }
    url
}

/// `Host` ヘッダーになる値（`host` または `host:port`）。URL を読めなければ `None`。
fn host_header(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let host = u.host_str()?;
    Some(match u.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

struct Cx<'a> {
    scope: &'a VarScope,
    dynamics: &'a dyn DynamicSource,
    unresolved: Vec<UnresolvedVar>,
    secrets: Vec<String>,
}

impl Cx<'_> {
    fn expand(&mut self, template: &str, location: VarLocation) -> String {
        let e = expand(template, self.scope, self.dynamics);
        for name in e.unresolved {
            let var = UnresolvedVar {
                name,
                location: location.clone(),
            };
            if !self.unresolved.contains(&var) {
                self.unresolved.push(var);
            }
        }
        for s in e.secrets {
            self.add_secret(&s);
        }
        e.text
    }

    fn add_secret(&mut self, secret: &str) {
        if !secret.is_empty() && !self.secrets.iter().any(|s| s == secret) {
            self.secrets.push(secret.to_string());
        }
    }
}

/// `location` の行を、行番号（1 始まり）つきで返す。
fn rows_of(params: &[KeyValue], location: ParamIn) -> impl Iterator<Item = (u32, &KeyValue)> + '_ {
    params
        .iter()
        .filter(move |p| p.location == location)
        .enumerate()
        .map(|(i, p)| (u32::try_from(i + 1).unwrap_or(u32::MAX), p))
}

fn param_loc(location: ParamIn, row: u32, part: RowPart) -> VarLocation {
    VarLocation::Param {
        location,
        row,
        part,
    }
}

fn assemble(spec: &RequestSpec, scope: &VarScope, dynamics: &dyn DynamicSource) -> Assembly {
    let mut cx = Cx {
        scope,
        dynamics,
        unresolved: Vec::new(),
        secrets: Vec::new(),
    };
    let method = spec.method.trim().to_ascii_uppercase();

    // パスパラメータ
    let mut path_values: Vec<(String, String)> = Vec::new();
    for (row, p) in rows_of(&spec.params, ParamIn::Path) {
        if !p.enabled {
            continue;
        }
        let name = cx.expand(&p.name, param_loc(ParamIn::Path, row, RowPart::Name));
        let value = cx.expand(&p.value, param_loc(ParamIn::Path, row, RowPart::Value));
        if !name.trim().is_empty() {
            path_values.push((name.trim().to_string(), value));
        }
    }

    // URL（変数 → パスパラメータ）
    let expanded_url = cx.expand(spec.url.trim(), VarLocation::Url);
    let (url_text, missing_path_params) = replace_path_params(&expanded_url, &path_values);

    // クエリ
    let mut query: Vec<(String, String)> = Vec::new();
    for (row, p) in rows_of(&spec.params, ParamIn::Query) {
        if !p.enabled {
            continue;
        }
        let name = cx.expand(&p.name, param_loc(ParamIn::Query, row, RowPart::Name));
        let value = cx.expand(&p.value, param_loc(ParamIn::Query, row, RowPart::Value));
        if !name.is_empty() {
            query.push((name, value));
        }
    }

    // ヘッダー（手入力）
    let mut headers: Vec<HeaderPair> = Vec::new();
    for (row, p) in rows_of(&spec.params, ParamIn::Header) {
        if !p.enabled {
            continue;
        }
        let name = cx.expand(&p.name, param_loc(ParamIn::Header, row, RowPart::Name));
        let value = cx.expand(&p.value, param_loc(ParamIn::Header, row, RowPart::Value));
        if !name.trim().is_empty() {
            headers.push(HeaderPair {
                name: name.trim().to_string(),
                value,
            });
        }
    }
    let mut auto: Vec<AutoHeader> = Vec::new();

    // Cookie
    let mut cookies: Vec<String> = Vec::new();
    for (row, p) in rows_of(&spec.params, ParamIn::Cookie) {
        if !p.enabled {
            continue;
        }
        let name = cx.expand(&p.name, param_loc(ParamIn::Cookie, row, RowPart::Name));
        let value = cx.expand(&p.value, param_loc(ParamIn::Cookie, row, RowPart::Value));
        if !name.trim().is_empty() {
            cookies.push(format!("{}={}", name.trim(), value));
        }
    }

    // 認証
    let has = |headers: &[HeaderPair], name: &str| {
        headers.iter().any(|h| h.name.eq_ignore_ascii_case(name))
    };
    let auth_loc = |field: &str| VarLocation::Auth {
        field: field.to_string(),
    };
    let mut auth_error: Option<String> = None;
    // 手入力の Authorization があれば、認証の設定は使わない（認証欄の未解決の変数で止めない）
    let manual_authorization = has(&headers, "Authorization");
    match &spec.auth {
        None | Some(AuthOverride::None) => {}
        Some(AuthOverride::Bearer { .. } | AuthOverride::Basic { .. }) if manual_authorization => {}
        Some(AuthOverride::Bearer { token }) => {
            let token = cx.expand(token, auth_loc("token"));
            add_auto(
                &mut headers,
                &mut auto,
                "Authorization",
                format!("Bearer {}", token.trim()),
                "認証（Bearer）",
            );
        }
        Some(AuthOverride::Basic { username, password }) => {
            let user = cx.expand(username, auth_loc("username"));
            let pass = cx.expand(password, auth_loc("password"));
            if user.contains(':') {
                // RFC 7617: ユーザー名に ':' は使えない（受け取り側が最初の ':' で分けるため）
                auth_error =
                    Some("Basic 認証のユーザー名に「:」は使えません（RFC 7617）".to_string());
            } else {
                let encoded =
                    base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
                // Authorization をエコーする応答（httpbin 型）でも履歴に残らないよう、伏せる対象に加える
                cx.add_secret(&encoded);
                if !pass.is_empty() {
                    cx.add_secret(&format!("{user}:{pass}"));
                }
                add_auto(
                    &mut headers,
                    &mut auto,
                    "Authorization",
                    format!("Basic {encoded}"),
                    "認証（Basic）",
                );
            }
        }
        Some(AuthOverride::ApiKey {
            location,
            name,
            value,
        }) => {
            let name = cx.expand(name, auth_loc("name"));
            let name = name.trim().to_string();
            // 同じ名前の手入力ヘッダーがあれば、そちらが使われるので値は展開しない
            let overridden = matches!(location, ApiKeyIn::Header) && has(&headers, &name);
            if !name.is_empty() && !overridden {
                let value = cx.expand(value, auth_loc("value"));
                match location {
                    ApiKeyIn::Header => {
                        add_auto(&mut headers, &mut auto, &name, value, "認証（API キー）")
                    }
                    ApiKeyIn::Query => query.push((name, value)),
                    ApiKeyIn::Cookie => cookies.push(format!("{name}={value}")),
                }
            } else if name.is_empty() {
                // 名前がなくても、値の未解決の変数は知らせる
                cx.expand(value, auth_loc("value"));
            }
        }
    }

    // Cookie ヘッダーにまとめる（手入力の Cookie ヘッダーがあればその後ろに足す）
    if !cookies.is_empty() {
        let joined = cookies.join("; ");
        if let Some(h) = headers
            .iter_mut()
            .find(|h| h.name.eq_ignore_ascii_case("Cookie"))
        {
            if h.value.trim().is_empty() {
                h.value = joined;
            } else {
                h.value = format!("{}; {joined}", h.value.trim_end_matches(';').trim_end());
            }
        } else {
            add_auto(&mut headers, &mut auto, "Cookie", joined, "Cookie・認証");
        }
    }

    // ボディ
    let (body, content_type) = build_body(&spec.body, &mut cx);
    if let Some(ct) = content_type {
        let is_multipart = ct.starts_with("multipart/");
        if is_multipart {
            // 境界の文字列が合わないと壊れるので、手入力の Content-Type は使わない
            headers.retain(|h| !h.name.eq_ignore_ascii_case("Content-Type"));
        }
        if !has(&headers, "Content-Type") {
            add_auto(&mut headers, &mut auto, "Content-Type", ct, "ボディ");
        }
    }

    // URL を確定
    let unresolved = std::mem::take(&mut cx.unresolved);
    let keep: Vec<&str> = unresolved.iter().map(|u| u.name.as_str()).collect();
    let (url, url_error) = finish_url(&url_text, &query, unresolved.is_empty(), &keep);

    Assembly {
        prepared: PreparedRequest {
            method,
            url,
            headers,
            body,
            secrets: cx.secrets,
        },
        unresolved,
        missing_path_params,
        url_error,
        auth_error,
        auto,
    }
}

/// 同じ名前のヘッダーがなければ足し、自動で付いたものとして記録する。
fn add_auto(
    headers: &mut Vec<HeaderPair>,
    auto: &mut Vec<AutoHeader>,
    name: &str,
    value: String,
    source: &str,
) {
    if headers.iter().any(|h| h.name.eq_ignore_ascii_case(name)) {
        return;
    }
    auto.push(AutoHeader {
        name: name.to_string(),
        value: value.clone(),
        source: source.to_string(),
    });
    headers.push(HeaderPair {
        name: name.to_string(),
        value,
    });
}

fn build_body(body: &Body, cx: &mut Cx<'_>) -> (PreparedBody, Option<String>) {
    match body {
        Body::None => (PreparedBody::None, None),
        Body::Json(text) => {
            let text = cx.expand(text, VarLocation::Body);
            if text.trim().is_empty() {
                (PreparedBody::None, None)
            } else {
                (
                    PreparedBody::Bytes(text.into_bytes()),
                    Some("application/json".to_string()),
                )
            }
        }
        Body::Text(text) => {
            let text = cx.expand(text, VarLocation::Body);
            if text.is_empty() {
                (PreparedBody::None, None)
            } else {
                (
                    PreparedBody::Bytes(text.into_bytes()),
                    Some("text/plain; charset=utf-8".to_string()),
                )
            }
        }
        Body::Form(rows) => {
            let mut ser = url::form_urlencoded::Serializer::new(String::new());
            let mut any = false;
            for (i, row) in rows.iter().enumerate() {
                if !row.enabled {
                    continue;
                }
                let n = u32::try_from(i + 1).unwrap_or(u32::MAX);
                let name = cx.expand(
                    &row.name,
                    VarLocation::BodyForm {
                        row: n,
                        part: RowPart::Name,
                    },
                );
                let value = cx.expand(
                    &row.value,
                    VarLocation::BodyForm {
                        row: n,
                        part: RowPart::Value,
                    },
                );
                if !name.is_empty() {
                    ser.append_pair(&name, &value);
                    any = true;
                }
            }
            if any {
                (
                    PreparedBody::Bytes(ser.finish().into_bytes()),
                    Some("application/x-www-form-urlencoded".to_string()),
                )
            } else {
                (PreparedBody::None, None)
            }
        }
        Body::Multipart(parts) => {
            let mut out = Vec::new();
            for (i, part) in parts.iter().enumerate() {
                if !part.enabled {
                    continue;
                }
                let n = u32::try_from(i + 1).unwrap_or(u32::MAX);
                let name = cx.expand(
                    &part.name,
                    VarLocation::BodyMultipart {
                        row: n,
                        part: RowPart::Name,
                    },
                );
                if name.is_empty() {
                    continue;
                }
                let value = match &part.value {
                    MultipartValue::Text(t) => PreparedPartValue::Text(cx.expand(
                        t,
                        VarLocation::BodyMultipart {
                            row: n,
                            part: RowPart::Value,
                        },
                    )),
                    MultipartValue::File(path) => PreparedPartValue::File(cx.expand(
                        path,
                        VarLocation::BodyMultipart {
                            row: n,
                            part: RowPart::Value,
                        },
                    )),
                };
                out.push(PreparedPart { name, value });
            }
            if out.is_empty() {
                return (PreparedBody::None, None);
            }
            let boundary = format!("----NekoReqBoundary{}", uuid::Uuid::new_v4().simple());
            let ct = format!("multipart/form-data; boundary={boundary}");
            (
                PreparedBody::Multipart {
                    boundary,
                    parts: out,
                },
                Some(ct),
            )
        }
        Body::Binary(path) => {
            let path = cx.expand(path, VarLocation::Body);
            if path.trim().is_empty() {
                (PreparedBody::None, None)
            } else {
                (
                    PreparedBody::File(path),
                    Some("application/octet-stream".to_string()),
                )
            }
        }
    }
}

/// `{name}` を、同じ名前のパスパラメータの値（パーセントエンコード済み）に置き換える。
/// `{{...}}`（展開できなかった変数）は触らない。値が空のもの・該当する行がないもの・値が `.` / `..` のものは
/// `{name}` のまま残し、その名前を返す（警告用）。
///
/// `.` / `..` を 1 区間の値として送ることはできない（`%2e%2e` も URL の仕様上「1 つ上へ」と同じに扱われ、
/// 送る前の整形でパスが化ける）ので、置き換えずに警告にする。
fn replace_path_params(url: &str, values: &[(String, String)]) -> (String, Vec<String>) {
    let mut out = String::with_capacity(url.len());
    let mut missing: Vec<String> = Vec::new();
    let mut rest = url;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(after_open) = tail.strip_prefix("{{") {
            // `{{name}}` はそのままコピー（閉じがなければ `{{` だけ）
            match after_open.find("}}") {
                Some(end) => {
                    out.push_str(&tail[..end + 4]);
                    rest = &after_open[end + 2..];
                }
                None => {
                    out.push_str("{{");
                    rest = after_open;
                }
            }
            continue;
        }
        match tail.find('}') {
            Some(end) => {
                let name = &tail[1..end];
                match values.iter().find(|(n, _)| n == name) {
                    Some((_, v)) if !v.is_empty() && v != "." && v != ".." => {
                        out.push_str(&utf8_percent_encode(v, PATH_SEGMENT).to_string());
                    }
                    found => {
                        // 値が空・`.` / `..`、または該当する行がない
                        let name_like = !name.is_empty()
                            && name
                                .chars()
                                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'));
                        if (found.is_some() || name_like) && !missing.iter().any(|m| m == name) {
                            missing.push(name.to_string());
                        }
                        out.push_str(&tail[..end + 1]);
                    }
                }
                rest = &tail[end + 1..];
            }
            None => {
                out.push('{');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    (out, missing)
}

/// `://` が最初の `/` `?` `#` より前にあれば、スキームがある（クエリの中の `://` は数えない）。
fn has_scheme(url: &str) -> bool {
    match url.find("://") {
        Some(i) => url.find(['/', '?', '#']).is_none_or(|d| i < d),
        None => false,
    }
}

/// クエリの名前・値をパーセントエンコードする。`keep` にある名前の `{{name}}`（展開できなかった変数）は、
/// 画面のプレビューで `{{name}}` のまま見せるため、エンコードしない（展開できないときの URL は送らない）。
fn encode_query_component(text: &str, keep: &[&str]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) if keep.contains(&after[..end].trim()) => {
                out.push_str(&utf8_percent_encode(&rest[..start], QUERY_COMPONENT).to_string());
                out.push_str(&rest[start..start + end + 4]);
                rest = &after[end + 2..];
            }
            _ => {
                // 変数でない `{{`: ここまでをエンコードして、1 文字進めて探し直す
                out.push_str(&utf8_percent_encode(&rest[..start + 1], QUERY_COMPONENT).to_string());
                rest = &rest[start + 1..];
            }
        }
    }
    out.push_str(&utf8_percent_encode(rest, QUERY_COMPONENT).to_string());
    out
}

/// クエリを足して URL を確定する。`strict` のときは URL として読めるか確かめて整える。
/// `keep` は展開できなかった変数の名前（`strict` でないときだけ使う。`encode_query_component` 参照）。
fn finish_url(
    base: &str,
    query: &[(String, String)],
    strict: bool,
    keep: &[&str],
) -> (String, Option<String>) {
    let base = base.trim();
    if base.is_empty() {
        return (String::new(), Some("URL を入力してください".to_string()));
    }
    let with_scheme = if has_scheme(base) {
        base.to_string()
    } else {
        format!("http://{base}")
    };
    let (before_fragment, fragment) = match with_scheme.split_once('#') {
        Some((a, b)) => (a.to_string(), Some(b.to_string())),
        None => (with_scheme, None),
    };
    let (path_part, existing_query) = match before_fragment.split_once('?') {
        Some((a, b)) => (a.to_string(), b.to_string()),
        None => (before_fragment, String::new()),
    };
    let mut q = existing_query;
    for (k, v) in query {
        if !q.is_empty() && !q.ends_with('&') {
            q.push('&');
        }
        q.push_str(&encode_query_component(k, keep));
        q.push('=');
        q.push_str(&encode_query_component(v, keep));
    }
    let mut assembled = path_part;
    if !q.is_empty() {
        assembled.push('?');
        assembled.push_str(&q);
    }
    if let Some(f) = fragment {
        assembled.push('#');
        assembled.push_str(&f);
    }
    if !strict {
        return (assembled, None);
    }
    match url::Url::parse(&assembled) {
        Ok(u) if matches!(u.scheme(), "http" | "https") && u.host().is_some() => {
            (u.to_string(), None)
        }
        Ok(_) => (
            assembled,
            Some("http または https の URL を指定してください".to_string()),
        ),
        Err(e) => (assembled, Some(format!("URL を読めません（{e}）"))),
    }
}

#[cfg(test)]
mod tests;
