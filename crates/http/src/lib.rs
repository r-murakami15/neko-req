//! nekoreq-http: リクエストの送信、時間の計測（DNS・接続・TLS・応答待ち・ダウンロード）、中止。
//! reqwest（rustls）と tokio を使う。
//!
//! - `send` は `PreparedRequest`（core が組み立てた、変数展開済みのリクエスト）を 1 回送る。
//!   送信ごとに新しい `reqwest::Client` を作る（接続は使い回さない）ので、DNS・TCP・TLS の時間が毎回測れる。
//!   `CancellationToken` を `cancel()` すると、送信中でも止まる（`SendError::Cancelled`）。
//! - タイムアウトは接続から本文を読み終えるまでの全体（既定 30 秒。0 で無制限）。
//! - リダイレクトの追従（最大 10 回）と証明書の検証は `SendSettings` で切り替える。リダイレクトは自前で追う
//!   （別の origin へ飛ぶときは `Authorization`・`Cookie`・`Proxy-Authorization`・`X-XSRF-TOKEN` を外す）。
//! - プロキシ・CA 証明書・クライアント証明書・ボディの上限は `Transport`（`send_with_transport`）で渡す。
//!   `send` は何も足さない（プロキシなし・OS の証明書ストアだけ・10 MB まで表示）。
//! - レスポンスのボディは 10 MB を超えたら先頭だけを `ResponseData` に入れ、全体は `SentResponse.body` に持つ
//!   （ファイルへの保存用）。512 MB を超える応答は `SendError::TooLarge`。
//! - 応答のヘッダーは受け取ったまま（`Content-Encoding`・`Content-Length` を含む）返す。ボディの展開
//!   （gzip・deflate・br）は `decode` で自前で行う（reqwest の自動展開は使わない）。`receivedBytes` は
//!   転送されたバイト数（展開前）、`body.size` は展開後。Cookie は扱わない（M5 で core 側が扱う）。
//! - エラーの文には、URL のクエリ・フラグメント・ユーザー情報を載せず、リクエストのシークレットも伏せる。
//! - 時間の測り方は `timing` を参照。測れない区間は `None`。

mod body;
mod decode;
mod fetch;
mod timing;
mod tls;
mod transport;

use std::time::{Duration, Instant};

use bytes::Bytes;
use nekoreq_core::{
    redact_text, ConnectionInfo, HeaderPair, PreparedRequest, ResponseBody, ResponseData,
    SendError, SendSettings, BODY_PREVIEW_LIMIT, DEFAULT_ACCEPT, DEFAULT_ACCEPT_ENCODING,
    DEFAULT_USER_AGENT,
};
use reqwest::header::{HeaderName, HeaderValue, ACCEPT_ENCODING, CONTENT_TYPE};

use crate::timing::{compute, Recorder, TimingLayer, TimingResolver};

pub use fetch::{fetch_bytes, fetch_bytes_at, fetch_bytes_with, FetchError};
pub use rustls::pki_types::CertificateDer;
pub use transport::{
    host_matches, parse_ca_pem, ClientIdentity, ProxyChoice, RedirectCookies, Transport,
};

/// 送信を止めるための合図（`cancel()` で止まる）。
pub use tokio_util::sync::CancellationToken;

/// 追うリダイレクトの上限。
pub const MAX_REDIRECTS: usize = 10;

/// 受け取るボディの上限（これを超える応答はエラー）。
pub const MAX_BODY_BYTES: usize = 512 * 1024 * 1024;

/// 送信して受け取ったもの。
#[derive(Debug, Clone)]
pub struct SentResponse {
    /// 画面へ渡す内容（ボディは先頭 10 MB まで）。
    pub data: ResponseData,
    /// ボディの全体（展開後）。「ファイルに保存」に使う。
    pub body: Bytes,
    /// 追ってきたリダイレクトの途中のレスポンス（最後のレスポンスは含まない）の `Set-Cookie`。
    /// `data.headers` は最後のレスポンスのヘッダーだけなので、途中で発行された SESSION などはここから読む。
    pub redirect_hops: Vec<RedirectHop>,
}

/// リダイレクトの途中のレスポンス 1 つ分の `Set-Cookie`（値が入ったまま。画面へは返さない）。
#[derive(Debug, Clone)]
pub struct RedirectHop {
    /// このレスポンスを返した URL。
    pub url: String,
    /// このレスポンスの `Set-Cookie` ヘッダー（なければ空）。
    pub set_cookies: Vec<HeaderPair>,
}

/// ボディの大きさの上限。
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// `ResponseData` に入れるボディの先頭の大きさ。
    pub preview_bytes: usize,
    /// 受け取る全体の上限。
    pub max_body_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            preview_bytes: BODY_PREVIEW_LIMIT,
            max_body_bytes: MAX_BODY_BYTES,
        }
    }
}

/// リクエストを 1 回送る（プロキシなし・OS の証明書ストアだけ）。
pub async fn send(
    request: &PreparedRequest,
    settings: &SendSettings,
    cancel: &CancellationToken,
) -> Result<SentResponse, SendError> {
    send_with_transport(request, settings, cancel, &Transport::default()).await
}

/// `send` のボディの上限を指定する版（テスト用にも使う）。
pub async fn send_with_limits(
    request: &PreparedRequest,
    settings: &SendSettings,
    cancel: &CancellationToken,
    limits: Limits,
) -> Result<SentResponse, SendError> {
    let transport = Transport {
        limits,
        ..Transport::default()
    };
    send_with_transport(request, settings, cancel, &transport).await
}

/// プロキシ・CA 証明書・クライアント証明書・ボディの上限（`Transport`）を指定して送る。
pub async fn send_with_transport(
    request: &PreparedRequest,
    settings: &SendSettings,
    cancel: &CancellationToken,
    transport: &Transport,
) -> Result<SentResponse, SendError> {
    let work = async {
        if settings.timeout_ms == 0 {
            return run(request, settings, transport).await;
        }
        let limit = Duration::from_millis(u64::from(settings.timeout_ms));
        match tokio::time::timeout(limit, run(request, settings, transport)).await {
            Ok(result) => result,
            Err(_) => Err(SendError::Timeout {
                message: format!(
                    "タイムアウトしました（{}）",
                    format_duration_ms(settings.timeout_ms)
                ),
            }),
        }
    };
    let result = tokio::select! {
        biased;
        () = cancel.cancelled() => Err(SendError::cancelled()),
        result = work => result,
    };
    // どの経路のエラーでも、メッセージにシークレットを残さない
    result.map_err(|e| redact_error(e, &request.secrets))
}

/// エラーのメッセージのシークレット（URL・ヘッダーに入れた値）を伏せる。
fn redact_error(error: SendError, secrets: &[String]) -> SendError {
    let r = |m: String| redact_text(&m, secrets);
    match error {
        SendError::Unresolved { message, vars } => SendError::Unresolved {
            message: r(message),
            vars,
        },
        SendError::InvalidRequest { message } => SendError::InvalidRequest {
            message: r(message),
        },
        SendError::BodyFile { message } => SendError::BodyFile {
            message: r(message),
        },
        SendError::Connect { message } => SendError::Connect {
            message: r(message),
        },
        SendError::Tls { message } => SendError::Tls {
            message: r(message),
        },
        SendError::Timeout { message } => SendError::Timeout {
            message: r(message),
        },
        SendError::Cancelled { message } => SendError::Cancelled {
            message: r(message),
        },
        SendError::TooLarge { message } => SendError::TooLarge {
            message: r(message),
        },
        SendError::Other { message } => SendError::Other {
            message: r(message),
        },
    }
}

fn format_duration_ms(ms: u32) -> String {
    if ms % 1000 == 0 {
        format!("{} 秒", ms / 1000)
    } else {
        format!("{ms} ミリ秒")
    }
}

async fn run(
    request: &PreparedRequest,
    settings: &SendSettings,
    transport: &Transport,
) -> Result<SentResponse, SendError> {
    let limits = transport.limits;
    let body = body::load_body(&request.body).await?;
    let body_len = body.as_ref().map_or(0, Bytes::len);

    let recorder = Recorder::new();
    // クライアント証明書は、最初の送り先のホストに合うものを使う（リダイレクト先でも同じ設定のまま）
    let first_url = reqwest::Url::parse(&request.url).ok();
    let identity = first_url.as_ref().and_then(|u| transport.identity_for(u));
    let client = build_client(settings, &recorder, transport, identity)?;
    let hop = Hop::first(request, body)?;

    let start = Instant::now();
    recorder.begin();
    let (mut response, redirect_hops) =
        send_following(&client, request, hop, settings, &recorder, transport).await?;
    let headers_at = Instant::now();

    let status = response.status();
    let version = response.version();
    let remote_addr = response.remote_addr().map(|a| a.to_string());
    let final_url = response.url().clone();
    let headers: Vec<HeaderPair> = response
        .headers()
        .iter()
        .map(|(name, value)| HeaderPair {
            name: name.as_str().to_string(),
            value: header_value_to_string(value),
        })
        .collect();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .map(header_value_to_string);

    // 転送されたまま（圧縮されていれば圧縮後）のボディ
    let mut wire: Vec<u8> = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if wire.len() + chunk.len() > limits.max_body_bytes {
                    return Err(too_large(limits));
                }
                wire.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(e) => return Err(map_error(&e, &request.url, settings)),
        }
    }
    let done_at = Instant::now();
    let wire_len = wire.len();

    let encodings = decode::content_encodings(&headers);
    let buf = if encodings.is_empty() {
        wire
    } else {
        let max = limits.max_body_bytes;
        tokio::task::spawn_blocking(move || decode::decode_body(wire, &encodings, max))
            .await
            .map_err(|e| SendError::Other {
                message: format!("レスポンスのボディを展開できません: {e}"),
            })?
            .map_err(|e| match e {
                decode::DecodeError::TooLarge => too_large(limits),
                decode::DecodeError::Corrupt(name) => SendError::Other {
                    message: format!(
                        "レスポンスのボディを展開できません（{name} の形式が正しくありません）"
                    ),
                },
            })?
    };

    let timing = compute(&recorder.snapshot(), start, headers_at, done_at);
    let status_text = status.canonical_reason().unwrap_or("").to_string();
    let received_bytes =
        estimate_received(version, status.as_u16(), &status_text, &headers, wire_len);
    let sent_bytes = estimate_sent(request, &final_url, body_len);
    let body_view = ResponseBody::from_bytes(content_type.as_deref(), &buf, limits.preview_bytes);

    Ok(SentResponse {
        data: ResponseData {
            status: status.as_u16(),
            status_text,
            headers,
            body: body_view,
            timing,
            connection: ConnectionInfo {
                remote_addr,
                http_version: version_name(version).to_string(),
                tls: final_url.scheme() == "https",
            },
            final_url: final_url.to_string(),
            redirect_count: recorder.redirect_count(),
            sent_bytes: clamp_u32(sent_bytes),
            received_bytes: clamp_u32(received_bytes),
        },
        body: Bytes::from(buf),
        redirect_hops,
    })
}

fn too_large(limits: Limits) -> SendError {
    SendError::TooLarge {
        message: format!(
            "レスポンスのボディが大きすぎます（上限 {} MB）",
            limits.max_body_bytes / (1024 * 1024)
        ),
    }
}

fn build_client(
    settings: &SendSettings,
    recorder: &std::sync::Arc<Recorder>,
    transport: &Transport,
    identity: Option<&ClientIdentity>,
) -> Result<reqwest::Client, SendError> {
    let tls = tls::client_config(
        settings.verify_certificates,
        recorder.clone(),
        &transport.ca_certs,
        identity,
    )
    .map_err(|message| SendError::Other { message })?;

    let builder = reqwest::Client::builder()
        // リダイレクトは `send_following` が自前で追う（別の origin へ X-XSRF-TOKEN などを送らないため）
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(DEFAULT_USER_AGENT)
        .dns_resolver(TimingResolver {
            recorder: recorder.clone(),
        })
        .connector_layer(TimingLayer {
            recorder: recorder.clone(),
        })
        .tls_backend_preconfigured(tls);
    let builder = apply_proxy(builder, &transport.proxy)?;
    builder.build().map_err(|e| SendError::Other {
        message: format!("HTTP クライアントを作れませんでした: {}", error_chain(&e)),
    })
}

/// 設定のプロキシに関わらず、常にプロキシを通さない宛先（ローカルの API を叩くアプリなので、
/// `localhost`・`*.localhost`・`127.0.0.0/8`・`::1` は直接つなぐ）。reqwest の `NO_PROXY` 形式。
const ALWAYS_DIRECT: &str = "localhost,.localhost,127.0.0.0/8,::1";

/// `host` がループバック（`localhost`・`*.localhost`・`127.x.x.x`・`::1`）か。
fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']).trim_end_matches('.');
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return ip.is_loopback();
    }
    let lower = host.to_ascii_lowercase();
    lower == "localhost" || lower.ends_with(".localhost")
}

/// プロキシの設定を反映する。
///
/// - `System`: OS の設定と環境変数（reqwest が読むのと同じ。`hyper-util` の `Matcher::from_system`）に従う。
///   ただしループバックの宛先は常にプロキシを通さない（`NO_PROXY` に書かれていなくても）。
/// - `None`: プロキシを使わない。
/// - `Manual`: 指定したプロキシ。通さないホストに、ループバックを常に足す。
pub(crate) fn apply_proxy(
    builder: reqwest::ClientBuilder,
    proxy: &ProxyChoice,
) -> Result<reqwest::ClientBuilder, SendError> {
    Ok(match proxy {
        ProxyChoice::System => builder.proxy(system_proxy()),
        ProxyChoice::None => builder.no_proxy(),
        ProxyChoice::Manual {
            url,
            username,
            password,
            no_proxy,
        } => {
            let mut p = reqwest::Proxy::all(url.as_str()).map_err(|e| SendError::Other {
                message: format!(
                    "プロキシの URL を読めません（{}）: {e}",
                    strip_url_secrets(url)
                ),
            })?;
            if let Some(user) = username.as_deref().filter(|u| !u.is_empty()) {
                p = p.basic_auth(user, password.as_deref().unwrap_or(""));
            }
            let mut list = no_proxy.join(",");
            if !list.is_empty() {
                list.push(',');
            }
            list.push_str(ALWAYS_DIRECT);
            p = p.no_proxy(reqwest::NoProxy::from_string(&list));
            // 手で指定したときは、環境変数の設定より優先する（`proxy()` を呼ぶとシステムのプロキシは読まれない）
            builder.proxy(p)
        }
    })
}

/// 「OS の設定を使う」ためのプロキシ。設定は作るときに 1 度読む。ループバックの宛先は通さない。
fn system_proxy() -> reqwest::Proxy {
    let matcher = hyper_util::client::proxy::matcher::Matcher::from_system();
    reqwest::Proxy::custom(move |url: &reqwest::Url| {
        if url.host_str().is_some_and(is_loopback_host) {
            return None;
        }
        let uri: http::Uri = url.as_str().parse().ok()?;
        let hit = matcher.intercept(&uri)?;
        let mut target = reqwest::Url::parse(&hit.uri().to_string()).ok()?;
        // 認証（`http://user:pass@proxy:8080` の形で設定されたもの）は URL に戻して reqwest に渡す
        if let Some((user, pass)) = hit.raw_auth() {
            let _ = target.set_username(user);
            let _ = target.set_password(Some(pass));
        } else if let Some((user, pass)) = hit.basic_auth().and_then(decode_basic) {
            let _ = target.set_username(&user);
            let _ = target.set_password(Some(&pass));
        }
        Some(target)
    })
}

/// `Basic base64(user:pass)` のヘッダー値から、ユーザー名とパスワードを取り出す。
fn decode_basic(value: &http::HeaderValue) -> Option<(String, String)> {
    use base64::Engine as _;
    let text = value.to_str().ok()?;
    let encoded = text.strip_prefix("Basic ")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let decoded = String::from_utf8(bytes).ok()?;
    let (user, pass) = decoded.split_once(':')?;
    Some((user.to_string(), pass.to_string()))
}

/// 1 回の要求（リダイレクトのたびに作り直す）。
struct Hop {
    method: reqwest::Method,
    /// 最初だけ、書かれたままの文字列（読めなければ `InvalidRequest`）。リダイレクト先は読み済みの URL。
    url: HopUrl,
    headers: Vec<(HeaderName, HeaderValue)>,
    body: Option<Bytes>,
}

enum HopUrl {
    Text(String),
    Parsed(reqwest::Url),
}

impl Hop {
    fn first(request: &PreparedRequest, body: Option<Bytes>) -> Result<Self, SendError> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes()).map_err(|_| {
            SendError::InvalidRequest {
                message: format!("method を読めません: {}", request.method),
            }
        })?;
        let mut headers = Vec::with_capacity(request.headers.len() + 1);
        let mut has_accept_encoding = false;
        for h in &request.headers {
            let name = HeaderName::from_bytes(h.name.as_bytes()).map_err(|_| {
                SendError::InvalidRequest {
                    message: format!("ヘッダー名に使えない文字があります: {}", h.name),
                }
            })?;
            let value = HeaderValue::from_bytes(h.value.as_bytes()).map_err(|_| {
                SendError::InvalidRequest {
                    message: format!("ヘッダー {} の値に使えない文字があります", h.name),
                }
            })?;
            has_accept_encoding |= name == ACCEPT_ENCODING;
            headers.push((name, value));
        }
        // reqwest の自動展開は使わないので、Accept-Encoding は自分で付ける（プレビューと同じ値）
        if !has_accept_encoding {
            headers.push((
                ACCEPT_ENCODING,
                HeaderValue::from_static(DEFAULT_ACCEPT_ENCODING),
            ));
        }
        Ok(Self {
            method,
            url: HopUrl::Text(request.url.clone()),
            headers,
            // `Bytes` のまま渡す。ストリームで渡すと、307 / 308 のリダイレクトでボディを送り直せない
            // （そのため「リクエスト送信」の時間は測れず、`None` になる）。
            body: body.filter(|b| !b.is_empty()),
        })
    }

    fn builder(&self, client: &reqwest::Client) -> reqwest::RequestBuilder {
        let mut builder = match &self.url {
            HopUrl::Text(t) => client.request(self.method.clone(), t.as_str()),
            HopUrl::Parsed(u) => client.request(self.method.clone(), u.clone()),
        };
        for (name, value) in &self.headers {
            builder = builder.header(name.clone(), value.clone());
        }
        if let Some(bytes) = &self.body {
            builder = builder.body(bytes.clone());
        }
        builder
    }
}

/// 送って、リダイレクトを追う（`settings.follow_redirects` のとき。最大 `MAX_REDIRECTS` 回）。
/// 戻り値は最後のレスポンス（追った回数は `recorder` が数える）。
///
/// - 301・302 の POST と、303 の HEAD 以外は GET に変え、ボディと `Content-*` を外す。307・308 はそのまま送り直す。
/// - 別の origin（スキーム・ホスト・ポート）へ飛ぶときは、`Authorization`・`Cookie`・`Proxy-Authorization`・
///   `X-XSRF-TOKEN` を外す（セッションや CSRF トークンを別のサイトへ送らない）。
async fn send_following(
    client: &reqwest::Client,
    request: &PreparedRequest,
    first: Hop,
    settings: &SendSettings,
    recorder: &std::sync::Arc<Recorder>,
    transport: &Transport,
) -> Result<(reqwest::Response, Vec<RedirectHop>), SendError> {
    let mut hop = first;
    let mut redirects = 0u32;
    let mut hops: Vec<RedirectHop> = Vec::new();
    loop {
        let response = hop
            .builder(client)
            .send()
            .await
            .map_err(|e| map_error(&e, &request.url, settings))?;
        if !settings.follow_redirects {
            return Ok((response, hops));
        }
        let status = response.status().as_u16();
        if !matches!(status, 301 | 302 | 303 | 307 | 308) {
            return Ok((response, hops));
        }
        let Some(location) = response.headers().get(reqwest::header::LOCATION) else {
            return Ok((response, hops));
        };
        let location = header_value_to_string(location);
        let current = response.url().clone();
        let Ok(next) = current.join(location.trim()) else {
            return Ok((response, hops));
        };
        if !matches!(next.scheme(), "http" | "https") {
            return Ok((response, hops));
        }
        if redirects as usize >= MAX_REDIRECTS {
            return Err(SendError::Other {
                message: format!(
                    "リダイレクトを追えません: リダイレクトが {MAX_REDIRECTS} 回を超えました"
                ),
            });
        }
        redirects += 1;
        recorder.new_hop();

        let to_get = match status {
            301 | 302 => hop.method == reqwest::Method::POST,
            303 => hop.method != reqwest::Method::HEAD && hop.method != reqwest::Method::GET,
            _ => false,
        };
        if to_get {
            hop.method = reqwest::Method::GET;
            hop.body = None;
            hop.headers.retain(|(n, _)| {
                !matches!(
                    n.as_str(),
                    "content-type" | "content-length" | "content-encoding" | "transfer-encoding"
                )
            });
        }
        // 途中のレスポンスの Set-Cookie（ログイン後の 302 など）。呼び出し側が自動更新に使う
        let set_cookies: Vec<HeaderPair> = response
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .map(|v| HeaderPair {
                name: "set-cookie".to_string(),
                value: header_value_to_string(v),
            })
            .collect();
        if !same_origin(&current, &next) {
            hop.headers
                .retain(|(n, _)| !is_credential_header(n.as_str()));
        } else if let Some(rc) = &transport.redirect_cookies {
            // 同じ origin への追従では、新しい SESSION・XSRF-TOKEN を付ける（別の origin には付けない）
            if nekoreq_core::origin_of(current.as_str()).as_deref() == Some(rc.origin.as_str()) {
                apply_redirect_cookies(&mut hop, rc, &set_cookies);
            }
        }
        hops.push(RedirectHop {
            url: current.to_string(),
            set_cookies,
        });
        hop.url = HopUrl::Parsed(next);
        drop(response);
    }
}

/// リダイレクトの途中のレスポンスの `Set-Cookie` で、追従先のリクエストの `SESSION`・`XSRF-TOKEN`
/// （Cookie ヘッダー）と `X-XSRF-TOKEN` を更新する。消す指示（`Max-Age=0` など）なら外す。
fn apply_redirect_cookies(hop: &mut Hop, rc: &RedirectCookies, set_cookies: &[HeaderPair]) {
    use nekoreq_core::{
        cookie_changes, is_csrf_safe_method, CookieChange, SESSION_COOKIE, XSRF_COOKIE,
    };
    let changes = cookie_changes(set_cookies, chrono::Utc::now());
    let mut wanted: Vec<(&str, CookieChange)> = Vec::new();
    if rc.session {
        wanted.extend(changes.session.map(|c| (SESSION_COOKIE, c)));
    }
    if rc.csrf {
        wanted.extend(changes.xsrf.map(|c| (XSRF_COOKIE, c)));
    }
    if wanted.is_empty() {
        return;
    }
    let cookie_name = HeaderName::from_static("cookie");
    let mut pairs: Vec<String> = hop
        .headers
        .iter()
        .find(|(n, _)| *n == cookie_name)
        .map(|(_, v)| header_value_to_string(v))
        .unwrap_or_default()
        .split(';')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    let name_of = |pair: &str| {
        pair.split_once('=')
            .map_or(pair, |(n, _)| n)
            .trim()
            .to_string()
    };
    for (name, change) in &wanted {
        pairs.retain(|p| name_of(p) != *name);
        if let CookieChange::Set(value) = change {
            pairs.push(format!("{name}={value}"));
        }
    }
    hop.headers.retain(|(n, _)| *n != cookie_name);
    if !pairs.is_empty() {
        if let Ok(v) = HeaderValue::from_str(&pairs.join("; ")) {
            hop.headers.push((cookie_name, v));
        }
    }
    // CSRF のヘッダー（GET・HEAD・OPTIONS 以外か、すでにあるとき）
    if let Some((_, change)) = wanted.iter().find(|(n, _)| *n == XSRF_COOKIE) {
        let header = HeaderName::from_static("x-xsrf-token");
        let had = hop.headers.iter().any(|(n, _)| *n == header);
        match change {
            CookieChange::Set(value) if had || !is_csrf_safe_method(hop.method.as_str()) => {
                if let Ok(v) = HeaderValue::from_str(value) {
                    hop.headers.retain(|(n, _)| *n != header);
                    hop.headers.push((header, v));
                }
            }
            CookieChange::Clear => hop.headers.retain(|(n, _)| *n != header),
            CookieChange::Set(_) => {}
        }
    }
}

/// 別の origin へのリダイレクトで外すヘッダー（小文字）。
fn is_credential_header(name: &str) -> bool {
    matches!(
        name,
        "authorization" | "cookie" | "proxy-authorization" | "x-xsrf-token"
    )
}

fn same_origin(a: &reqwest::Url, b: &reqwest::Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

fn header_value_to_string(v: &HeaderValue) -> String {
    match v.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => String::from_utf8_lossy(v.as_bytes()).into_owned(),
    }
}

fn version_name(v: reqwest::Version) -> &'static str {
    match v {
        reqwest::Version::HTTP_09 => "HTTP/0.9",
        reqwest::Version::HTTP_10 => "HTTP/1.0",
        reqwest::Version::HTTP_11 => "HTTP/1.1",
        reqwest::Version::HTTP_2 => "HTTP/2",
        reqwest::Version::HTTP_3 => "HTTP/3",
        _ => "HTTP",
    }
}

fn clamp_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// 送ったバイト数の近似: リクエスト行 + ヘッダー（自動で付くものを含む）+ 空行 + ボディ。
fn estimate_sent(request: &PreparedRequest, url: &reqwest::Url, body_len: usize) -> usize {
    let target = match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_string(),
    };
    let mut n = format!("{} {} HTTP/1.1\r\n", request.method, target).len();
    let has = |name: &str| {
        request
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case(name))
    };
    for h in &request.headers {
        n += h.name.len() + 2 + h.value.len() + 2;
    }
    let host = match url.port() {
        Some(p) => format!("{}:{p}", url.host_str().unwrap_or("")),
        None => url.host_str().unwrap_or("").to_string(),
    };
    for (name, value) in [
        ("Host", host.as_str()),
        ("User-Agent", DEFAULT_USER_AGENT),
        ("Accept", DEFAULT_ACCEPT),
        ("Accept-Encoding", DEFAULT_ACCEPT_ENCODING),
    ] {
        if !has(name) {
            n += name.len() + 2 + value.len() + 2;
        }
    }
    if body_len > 0 && !has("Content-Length") {
        n += "Content-Length".len() + 2 + body_len.to_string().len() + 2;
    }
    n + 2 + body_len
}

/// 受け取ったバイト数の近似: ステータス行 + ヘッダー + 空行 + 転送されたボディ（展開前）。
fn estimate_received(
    version: reqwest::Version,
    status: u16,
    status_text: &str,
    headers: &[HeaderPair],
    wire_body_len: usize,
) -> usize {
    let mut n = format!("{} {status} {status_text}\r\n", version_name(version)).len();
    for h in headers {
        n += h.name.len() + 2 + h.value.len() + 2;
    }
    n + 2 + wire_body_len
}

/// エラーの原因をたどって「: 」でつなぐ。
fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cur {
        let s = err.to_string();
        if parts.last() != Some(&s) {
            parts.push(s);
        }
        cur = err.source();
    }
    parts.join(": ")
}

/// 接続先（`host:port`）。URL を読めなければ URL そのまま。
fn authority(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(u) => match (u.host_str(), u.port_or_known_default()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.to_string(),
            _ => url.to_string(),
        },
        Err(_) => url.to_string(),
    }
}

/// 文中の URL（`http://`・`https://` から空白や `)` まで）から、ユーザー情報・クエリ・フラグメントを取り除く。
/// reqwest のエラー文（`error sending request for url (…?token=…)`）に URL がそのまま入るため。
pub fn strip_url_secrets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = ["http://", "https://"]
        .iter()
        .filter_map(|scheme| rest.find(scheme))
        .min()
    {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\'' | '>' | ','))
            .unwrap_or(tail.len());
        let (url, after) = tail.split_at(end);
        let scheme_len = url.find("://").map_or(0, |n| n + 3);
        let (scheme, body) = url.split_at(scheme_len);
        let authority_end = body.find(['/', '?', '#']).unwrap_or(body.len());
        let (authority, path_etc) = body.split_at(authority_end);
        let authority = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        let path = path_etc.split(['?', '#']).next().unwrap_or("");
        out.push_str(scheme);
        out.push_str(authority);
        out.push_str(path);
        rest = after;
    }
    out.push_str(rest);
    out
}

fn map_error(e: &reqwest::Error, url: &str, settings: &SendSettings) -> SendError {
    let chain = strip_url_secrets(&error_chain(e));
    let lower = chain.to_ascii_lowercase();
    if e.is_timeout() {
        return SendError::Timeout {
            message: format!(
                "タイムアウトしました（{}）",
                format_duration_ms(settings.timeout_ms)
            ),
        };
    }
    if e.is_builder() {
        return SendError::InvalidRequest {
            message: format!("リクエストを組み立てられません: {chain}"),
        };
    }
    if e.is_redirect() {
        return SendError::Other {
            message: format!("リダイレクトを追えません: {chain}"),
        };
    }
    let is_tls = lower.contains("certificate")
        || lower.contains("handshake")
        || lower.contains("invalid peer")
        || lower.contains("tls");
    if is_tls {
        return SendError::Tls {
            message: format!(
                "TLS で接続できません（{}）: {chain}。自己署名の証明書などは、設定で証明書の検証を切ると通せます",
                authority(url)
            ),
        };
    }
    if e.is_connect() {
        return SendError::Connect {
            message: format!("接続できません（{}）: {chain}", authority(url)),
        };
    }
    if e.is_body() || e.is_decode() {
        return SendError::Other {
            message: format!("レスポンスのボディを読めません: {chain}"),
        };
    }
    SendError::Other {
        message: format!("送信に失敗しました（{}）: {chain}", authority(url)),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod loopback_tests {
    use super::is_loopback_host;

    #[test]
    fn loopback_hosts() {
        for h in [
            "localhost",
            "LOCALHOST",
            "api.localhost",
            "127.0.0.1",
            "127.1.2.3",
            "::1",
            "[::1]",
            "localhost.",
        ] {
            assert!(is_loopback_host(h), "{h}");
        }
        for h in [
            "example.com",
            "notlocalhost",
            "localhost.example.com",
            "10.0.0.1",
            "128.0.0.1",
        ] {
            assert!(!is_loopback_host(h), "{h}");
        }
    }
}
