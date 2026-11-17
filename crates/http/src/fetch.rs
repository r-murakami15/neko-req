//! 設定ファイル（OpenAPI の yaml）の取得。送信（`send`）とは別の、単純な GET。
//!
//! - TLS は送信と同じ設定（rustls + OS の証明書ストア）。プロキシは使わない。
//! - アクセストークンは `Authorization: Bearer` で送る。リダイレクト先が別のホストのときは reqwest が
//!   `Authorization` を外す。
//! - エラーの文にはトークンも URL のクエリも載せない。

use std::time::Duration;

use bytes::BytesMut;

use crate::timing::Recorder;
use crate::tls;

/// 取得の失敗。`message` は画面に出せる日本語（URL のクエリ・ユーザー情報は含まない）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct FetchError {
    pub message: String,
    /// HTTP のステータス（接続できなかったときは `None`）。
    pub status: Option<u16>,
}

fn clean_url(url: &url::Url) -> String {
    let mut u = url.clone();
    u.set_query(None);
    u.set_fragment(None);
    let _ = u.set_username("");
    let _ = u.set_password(None);
    u.to_string()
}

fn chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        s.push_str(": ");
        s.push_str(&c.to_string());
        cur = c.source();
    }
    s
}

/// `url` を GET して本文を返す。2xx 以外・`max_bytes` 超えはエラー。
pub async fn fetch_bytes(
    url: &url::Url,
    bearer_token: Option<&str>,
    timeout: Duration,
    max_bytes: usize,
) -> Result<Vec<u8>, FetchError> {
    let shown = clean_url(url);
    let fail = |message: String, status: Option<u16>| FetchError { message, status };

    let tls = tls::client_config(true, Recorder::new())
        .map_err(|m| fail(format!("{m}（{shown}）"), None))?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent(nekoreq_core::DEFAULT_USER_AGENT)
        .tls_backend_preconfigured(tls)
        .build()
        .map_err(|e| {
            fail(
                format!("HTTP クライアントを作れませんでした: {}", chain(&e)),
                None,
            )
        })?;

    let mut req = client.get(url.clone());
    if let Some(token) = bearer_token.filter(|t| !t.is_empty()) {
        req = req.bearer_auth(token);
    }
    let mut resp = req.send().await.map_err(|e| {
        let e = e.without_url();
        fail(
            format!("{shown} を取得できませんでした: {}", chain(&e)),
            None,
        )
    })?;
    let status = resp.status();
    if !status.is_success() {
        let hint = match status.as_u16() {
            401 | 403 | 404 if bearer_token.is_none() => {
                "（非公開のときはアクセストークンを設定してください）"
            }
            401 | 403 => "（アクセストークンが正しいか確かめてください）",
            _ => "",
        };
        return Err(fail(
            format!(
                "{shown} を取得できませんでした: HTTP {}{hint}",
                status.as_u16()
            ),
            Some(status.as_u16()),
        ));
    }
    let mut buf = BytesMut::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| {
        let e = e.without_url();
        fail(format!("{shown} の読み込み中にエラー: {}", chain(&e)), None)
    })? {
        if buf.len() + chunk.len() > max_bytes {
            return Err(fail(
                format!(
                    "{shown} が大きすぎます（上限 {} MB）",
                    max_bytes / (1024 * 1024)
                ),
                None,
            ));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf.to_vec())
}
