//! 時間の計測。reqwest 単体では DNS・接続・TLS の時間が取れないので、接続部分を包んで測る。
//!
//! - DNS 解決: 独自の `Resolve`（`TimingResolver`）が、OS の名前解決の前後で時刻を記録する。
//!   IP アドレスを直接書いた URL では名前解決をしないので記録がなく、`None`（画面で「—」）になる。
//! - 接続全体（DNS + TCP + TLS）: `connector_layer` に差す `TimingLayer` が、接続の開始と終了を記録する。
//! - TLS の開始: rustls の `ClientSessionStore`（`TlsStartStore`）が、ClientHello を作る時点
//!   （TCP 接続が終わってハンドシェイクを始める時点）で呼ばれるのを利用する。
//!   TCP 接続 = TLS 開始 − DNS 終了、TLS ハンドシェイク = 接続終了 − TLS 開始。
//! - リクエスト送信: 測らない（常に `None`）。ボディは `Bytes` のまま渡す（ストリームだと 307 / 308 で
//!   送り直せない）ので、送り終えた時刻が取れない。送信の時間は「応答待ち」に含まれる。
//! - 応答待ち・ダウンロード: レスポンスのヘッダーが届いた時刻と、本文を読み終えた時刻から求める。
//! - リダイレクトを追うたびに「区間」を作り直し、時間の内訳は最後の区間（最終的な応答）のもの。
//!   合計時間だけは最初から最後まで。

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};
use std::time::Instant;

use nekoreq_core::Timing;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use rustls::client::ClientSessionStore;
use rustls::client::Tls12ClientSessionValue;
use rustls::client::Tls13ClientSessionValue;
use rustls::pki_types::ServerName;
use rustls::NamedGroup;
use tower_layer::Layer;
use tower_service::Service;

/// 1 回のリクエスト（リダイレクトの区間ごと）の時刻。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Marks {
    /// この区間の開始。
    pub hop_start: Instant,
    pub dns_start: Option<Instant>,
    pub dns_end: Option<Instant>,
    pub connect_start: Option<Instant>,
    pub connect_end: Option<Instant>,
    pub tls_start: Option<Instant>,
}

impl Marks {
    fn new(now: Instant) -> Self {
        Self {
            hop_start: now,
            dns_start: None,
            dns_end: None,
            connect_start: None,
            connect_end: None,
            tls_start: None,
        }
    }
}

/// 時刻の記録先。1 回の送信ごとに 1 つ作り、リゾルバ・接続のレイヤー・TLS に配る。
#[derive(Debug)]
pub(crate) struct Recorder {
    marks: Mutex<Marks>,
    redirects: AtomicU32,
}

impl Recorder {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            marks: Mutex::new(Marks::new(Instant::now())),
            redirects: AtomicU32::new(0),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Marks> {
        self.marks.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// 送信を始める。区間の開始を「いま」にする（クライアントを作る時間を含めない）。
    pub fn begin(&self) {
        self.lock().hop_start = Instant::now();
    }

    pub fn snapshot(&self) -> Marks {
        *self.lock()
    }

    pub fn redirect_count(&self) -> u32 {
        self.redirects.load(Ordering::Relaxed)
    }

    /// リダイレクトを追う。これまでの記録を捨てて、新しい区間を始める。
    pub fn new_hop(&self) {
        self.redirects.fetch_add(1, Ordering::Relaxed);
        *self.lock() = Marks::new(Instant::now());
    }

    pub fn dns_started(&self) {
        let mut m = self.lock();
        m.dns_start = Some(Instant::now());
        m.dns_end = None;
    }

    pub fn dns_ended(&self) {
        self.lock().dns_end = Some(Instant::now());
    }

    pub fn connect_started(&self) {
        let mut m = self.lock();
        m.connect_start = Some(Instant::now());
        m.connect_end = None;
        m.dns_start = None;
        m.dns_end = None;
        m.tls_start = None;
    }

    pub fn connect_ended(&self) {
        self.lock().connect_end = Some(Instant::now());
    }

    /// TLS のハンドシェイクを始めた（最初の 1 回だけ記録する）。
    pub fn tls_started(&self) {
        let mut m = self.lock();
        if m.connect_start.is_some() && m.tls_start.is_none() {
            m.tls_start = Some(Instant::now());
        }
    }
}

fn ms(from: Instant, to: Instant) -> f64 {
    to.saturating_duration_since(from).as_secs_f64() * 1000.0
}

/// 記録から時間の内訳を求める。
///
/// - `start`: 送信を始めた時刻、`headers_at`: レスポンスのヘッダーが届いた時刻、`done_at`: 本文を読み終えた時刻。
/// - 接続を作らなかった（使い回した）区間では DNS・TCP・TLS はすべて `None`。
pub(crate) fn compute(
    marks: &Marks,
    start: Instant,
    headers_at: Instant,
    done_at: Instant,
) -> Timing {
    let dns_ms = match (marks.dns_start, marks.dns_end) {
        (Some(a), Some(b)) => Some(ms(a, b)),
        _ => None,
    };
    let connect_origin = marks.dns_end.or(marks.connect_start);
    let (connect_ms, tls_ms) = match (marks.connect_start, marks.connect_end) {
        (Some(_), Some(end)) => match (marks.tls_start, connect_origin) {
            (Some(tls), Some(origin)) => (Some(ms(origin, tls)), Some(ms(tls, end))),
            (None, Some(origin)) => (Some(ms(origin, end)), None),
            _ => (None, None),
        },
        _ => (None, None),
    };
    let after_connect = marks.connect_end.unwrap_or(marks.hop_start);
    Timing {
        dns_ms,
        connect_ms,
        tls_ms,
        send_ms: None,
        wait_ms: Some(ms(after_connect, headers_at)),
        download_ms: Some(ms(headers_at, done_at)),
        total_ms: ms(start, done_at),
    }
}

/// DNS の時間を測るリゾルバ。OS の名前解決（getaddrinfo）をそのまま使う。
#[derive(Debug, Clone)]
pub(crate) struct TimingResolver {
    pub recorder: Arc<Recorder>,
}

impl Resolve for TimingResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let recorder = self.recorder.clone();
        let host = name.as_str().to_string();
        Box::pin(async move {
            recorder.dns_started();
            // ポートは 0 のままにする（reqwest が URL のポートか既定のポートに置き換える）
            let result = tokio::net::lookup_host((host, 0u16)).await;
            recorder.dns_ended();
            let addrs: Vec<SocketAddr> = result?.collect();
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// 接続（DNS + TCP + TLS）の開始と終了を記録するレイヤー。
#[derive(Debug, Clone)]
pub(crate) struct TimingLayer {
    pub recorder: Arc<Recorder>,
}

impl<S> Layer<S> for TimingLayer {
    type Service = TimingService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        TimingService {
            inner,
            recorder: self.recorder.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TimingService<S> {
    inner: S,
    recorder: Arc<Recorder>,
}

impl<S, Req> Service<Req> for TimingService<S>
where
    S: Service<Req>,
    S::Future: Send + 'static,
    S::Response: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Req) -> Self::Future {
        let recorder = self.recorder.clone();
        recorder.connect_started();
        let fut = self.inner.call(req);
        Box::pin(async move {
            let result = fut.await;
            recorder.connect_ended();
            result
        })
    }
}

/// rustls が TLS ハンドシェイクを始めるとき（ClientHello を作るとき）に呼ぶセッション置き場を装い、
/// 開始の時刻だけを記録する。セッションの再利用はしない（何も覚えない）。
#[derive(Debug)]
pub(crate) struct TlsStartStore {
    pub recorder: Arc<Recorder>,
}

impl ClientSessionStore for TlsStartStore {
    fn set_kx_hint(&self, _: ServerName<'static>, _: NamedGroup) {}

    fn kx_hint(&self, _: &ServerName<'_>) -> Option<NamedGroup> {
        self.recorder.tls_started();
        None
    }

    fn set_tls12_session(&self, _: ServerName<'static>, _: Tls12ClientSessionValue) {}

    fn tls12_session(&self, _: &ServerName<'_>) -> Option<Tls12ClientSessionValue> {
        self.recorder.tls_started();
        None
    }

    fn remove_tls12_session(&self, _: &ServerName<'static>) {}

    fn insert_tls13_ticket(&self, _: ServerName<'static>, _: Tls13ClientSessionValue) {}

    fn take_tls13_ticket(&self, _: &ServerName<'static>) -> Option<Tls13ClientSessionValue> {
        self.recorder.tls_started();
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn plain_http_fresh_connection() {
        let t0 = Instant::now();
        let marks = Marks {
            hop_start: t0,
            dns_start: Some(at(t0, 1)),
            dns_end: Some(at(t0, 3)),
            connect_start: Some(at(t0, 1)),
            connect_end: Some(at(t0, 8)),
            tls_start: None,
        };
        let t = compute(&marks, t0, at(t0, 38), at(t0, 41));
        assert_eq!(t.dns_ms.map(f64::round), Some(2.0));
        assert_eq!(t.connect_ms.map(f64::round), Some(5.0));
        assert_eq!(t.tls_ms, None);
        assert_eq!(t.send_ms, None);
        assert_eq!(t.wait_ms.map(f64::round), Some(30.0));
        assert_eq!(t.download_ms.map(f64::round), Some(3.0));
        assert_eq!(t.total_ms.round(), 41.0);
    }

    #[test]
    fn https_splits_tcp_and_tls() {
        let t0 = Instant::now();
        let marks = Marks {
            hop_start: t0,
            dns_start: Some(at(t0, 0)),
            dns_end: Some(at(t0, 10)),
            connect_start: Some(at(t0, 0)),
            connect_end: Some(at(t0, 60)),
            tls_start: Some(at(t0, 30)),
        };
        let t = compute(&marks, t0, at(t0, 100), at(t0, 110));
        assert_eq!(t.dns_ms.map(f64::round), Some(10.0));
        assert_eq!(t.connect_ms.map(f64::round), Some(20.0));
        assert_eq!(t.tls_ms.map(f64::round), Some(30.0));
        assert_eq!(t.send_ms, None);
        assert_eq!(t.wait_ms.map(f64::round), Some(40.0));
    }

    #[test]
    fn reused_connection_has_no_connection_phases() {
        let t0 = Instant::now();
        let marks = Marks::new(t0);
        let t = compute(&marks, t0, at(t0, 5), at(t0, 6));
        assert_eq!(t.dns_ms, None);
        assert_eq!(t.connect_ms, None);
        assert_eq!(t.tls_ms, None);
        assert_eq!(t.wait_ms.map(f64::round), Some(5.0));
    }

    #[test]
    fn ip_literal_has_no_dns_but_has_connect() {
        let t0 = Instant::now();
        let marks = Marks {
            hop_start: t0,
            dns_start: None,
            dns_end: None,
            connect_start: Some(at(t0, 0)),
            connect_end: Some(at(t0, 4)),
            tls_start: None,
        };
        let t = compute(&marks, t0, at(t0, 10), at(t0, 10));
        assert_eq!(t.dns_ms, None);
        assert_eq!(t.connect_ms.map(f64::round), Some(4.0));
    }

    #[test]
    fn recorder_tracks_hops_and_tls_start_once() {
        let r = Recorder::new();
        r.tls_started(); // 接続の前は無視
        assert!(r.snapshot().tls_start.is_none());
        r.connect_started();
        r.tls_started();
        let first = r.snapshot().tls_start;
        assert!(first.is_some());
        r.tls_started();
        assert_eq!(r.snapshot().tls_start, first, "最初の 1 回だけ");
        r.new_hop();
        assert_eq!(r.redirect_count(), 1);
        assert!(r.snapshot().connect_start.is_none());
    }
}
