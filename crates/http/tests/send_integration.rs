//! http クレートの結合テスト。公開 API（`send`・`send_with_limits`・`CancellationToken`）だけを使い、
//! wiremock と、生の TCP で応答を返す小さなサーバーで確かめる。
//! 既存の単体テスト（crates/http/src/tests.rs）と重ならない観点: 時間の内訳の整合、HEAD・204・304、
//! chunked、gzip・br、charset、10 MB の境界、同時送信と中止、ボディの実際の形、エラーに秘密が出ないこと。

use std::future::Future;
use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nekoreq_core::{
    build_request, Body, BodyEncoding, DynamicSource, HeaderPair, KeyValue, MultipartPart,
    MultipartValue, ParamIn, PreparedBody, PreparedRequest, RequestSpec, SendError, SendSettings,
    VarLayer, VarScope, BODY_PREVIEW_LIMIT,
};
use nekoreq_http::{send, CancellationToken, SentResponse};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---- 道具 ----

fn prepared(method: &str, url: &str) -> PreparedRequest {
    PreparedRequest {
        method: method.into(),
        url: url.into(),
        headers: vec![],
        body: PreparedBody::None,
        secrets: vec![],
    }
}

fn hdr(name: &str, value: &str) -> HeaderPair {
    HeaderPair {
        name: name.into(),
        value: value.into(),
    }
}

async fn send_default(req: &PreparedRequest) -> Result<SentResponse, SendError> {
    send(req, &SendSettings::default(), &CancellationToken::new()).await
}

fn find_header<'a>(r: &'a SentResponse, name: &str) -> Option<&'a str> {
    r.data
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}

/// 接続ごとに `handler(stream, リクエストのヘッダー部)` を走らせる生のサーバー。ポートを返す。
async fn raw_server<F, Fut>(handler: F) -> u16
where
    F: Fn(TcpStream, String) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handler = Arc::new(handler);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let handler = handler.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                handler(stream, head).await;
            });
        }
    });
    port
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn brotli(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut w = brotli::CompressorWriter::new(&mut out, 4096, 5, 22);
        w.write_all(data).unwrap();
    }
    out
}

// ---- 時間の内訳 ----

#[tokio::test]
async fn timing_parts_never_exceed_the_total_for_get_post_and_redirects() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(120))
                .set_body_string("ok"),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/post"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/r1"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/slow"))
        .mount(&server)
        .await;

    let base = format!("http://localhost:{}", server.address().port());
    let mut post = prepared("POST", &format!("{base}/post"));
    post.body = PreparedBody::Bytes(vec![b'x'; 300_000]);
    let reqs = [
        prepared("GET", &format!("{base}/slow")),
        post,
        prepared("GET", &format!("{base}/r1")),
    ];
    for req in &reqs {
        let r = send_default(req).await.unwrap().data;
        let t = &r.timing;
        let parts: f64 = [
            t.dns_ms,
            t.connect_ms,
            t.tls_ms,
            t.send_ms,
            t.wait_ms,
            t.download_ms,
        ]
        .into_iter()
        .flatten()
        .sum();
        assert!(
            parts <= t.total_ms + 2.0,
            "{} {}: 内訳 {parts} > 合計 {} ({t:?})",
            req.method,
            req.url,
            t.total_ms
        );
        for v in [
            t.dns_ms,
            t.connect_ms,
            t.tls_ms,
            t.send_ms,
            t.wait_ms,
            t.download_ms,
        ]
        .into_iter()
        .flatten()
        {
            assert!(v >= 0.0, "{t:?}");
        }
    }
    // 遅い応答では「応答待ち」に遅れが出る（ダウンロードではなく）
    let r = send_default(&reqs[0]).await.unwrap().data;
    assert!(r.timing.wait_ms.unwrap() >= 100.0, "{:?}", r.timing);
    assert!(r.timing.total_ms >= 120.0, "{:?}", r.timing);
    // リダイレクトを追ったら、合計は 2 区間ぶん、内訳は最後の区間
    let r = send_default(&reqs[2]).await.unwrap().data;
    assert_eq!(r.redirect_count, 1);
    assert!(r.final_url.ends_with("/slow"));
}

#[tokio::test]
async fn a_delayed_body_shows_up_as_download_time_not_wait_time() {
    let port = raw_server(|mut s, _| async move {
        s.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 10\r\n\r\nabcde",
        )
        .await
        .unwrap();
        s.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        s.write_all(b"fghij").await.unwrap();
    })
    .await;
    let r = send_default(&prepared("GET", &format!("http://127.0.0.1:{port}/")))
        .await
        .unwrap();
    assert_eq!(r.data.body.content, "abcdefghij");
    let t = &r.data.timing;
    assert!(t.download_ms.unwrap() >= 250.0, "{t:?}");
    assert!(t.wait_ms.unwrap() < 200.0, "{t:?}");
}

// ---- HEAD / 204 / 304 / chunked ----

#[tokio::test]
async fn head_returns_headers_only_even_with_a_content_length() {
    let port = raw_server(|mut s, head| async move {
        assert!(head.starts_with("HEAD /x HTTP/1.1"), "{head}");
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 12345\r\nETag: \"abc\"\r\n\r\n").await.unwrap();
        // 接続は開いたまま（ボディを待ってしまう実装だとここで固まる）
        tokio::time::sleep(Duration::from_secs(3)).await;
    })
    .await;
    let start = Instant::now();
    let r = send_default(&prepared("HEAD", &format!("http://127.0.0.1:{port}/x")))
        .await
        .unwrap();
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "HEAD でボディを待っている"
    );
    assert_eq!(r.data.status, 200);
    assert_eq!(r.data.body.size, 0);
    assert_eq!(r.data.body.content, "");
    assert_eq!(r.data.body.encoding, BodyEncoding::Text);
    assert_eq!(find_header(&r, "content-length"), Some("12345"));
    assert_eq!(find_header(&r, "etag"), Some("\"abc\""));
    assert_eq!(
        r.data.body.content_type.as_deref(),
        Some("application/json")
    );
}

#[tokio::test]
async fn no_content_and_not_modified_have_empty_bodies_and_standard_reasons() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/d"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/nm"))
        .respond_with(ResponseTemplate::new(304).insert_header("etag", "\"v1\""))
        .mount(&server)
        .await;
    let base = server.uri();
    let r = send_default(&prepared("DELETE", &format!("{base}/d")))
        .await
        .unwrap()
        .data;
    assert_eq!(
        (
            r.status,
            r.status_text.as_str(),
            r.body.size,
            r.body.content.as_str()
        ),
        (204, "No Content", 0, "")
    );
    assert_eq!(r.body.encoding, BodyEncoding::Text);

    // 304 は redirect ではない（追わない）
    let r = send_default(&prepared("GET", &format!("{base}/nm")))
        .await
        .unwrap()
        .data;
    assert_eq!(
        (r.status, r.status_text.as_str(), r.body.size),
        (304, "Not Modified", 0)
    );
    assert_eq!(r.redirect_count, 0);
    assert!(r.headers.iter().any(|h| h.name == "etag"));
}

#[tokio::test]
async fn chunked_transfer_is_assembled_and_counted() {
    let port = raw_server(|mut s, _| async move {
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
        for chunk in ["こんにちは", ", ", "world", "!"] {
            s.write_all(format!("{:x}\r\n{}\r\n", chunk.len(), chunk).as_bytes()).await.unwrap();
            s.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        s.write_all(b"0\r\n\r\n").await.unwrap();
    })
    .await;
    let r = send_default(&prepared("GET", &format!("http://127.0.0.1:{port}/")))
        .await
        .unwrap();
    assert_eq!(r.data.body.content, "こんにちは, world!");
    assert_eq!(r.data.body.size as usize, "こんにちは, world!".len());
    assert_eq!(&r.body[..], "こんにちは, world!".as_bytes());
    assert_eq!(find_header(&r, "transfer-encoding"), Some("chunked"));
    assert!(find_header(&r, "content-length").is_none());
    assert!(r.data.received_bytes > r.data.body.size);
}

#[tokio::test]
async fn a_connection_closed_in_the_middle_of_the_body_is_an_error_not_a_hang() {
    let port = raw_server(|mut s, _| async move {
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nshort")
            .await
            .unwrap();
        // ここで切る
    })
    .await;
    let start = Instant::now();
    let e = send_default(&prepared("GET", &format!("http://127.0.0.1:{port}/")))
        .await
        .unwrap_err();
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(!e.message().is_empty());
    assert!(
        matches!(e, SendError::Other { .. } | SendError::Connect { .. }),
        "{e:?}"
    );
}

#[tokio::test]
async fn a_server_that_closes_without_a_response_is_an_error() {
    let port = raw_server(|s, _| async move { drop(s) }).await;
    let e = send_default(&prepared("GET", &format!("http://127.0.0.1:{port}/")))
        .await
        .unwrap_err();
    assert!(!e.message().is_empty(), "{e:?}");
}

// ---- 圧縮 ----

#[tokio::test]
async fn gzip_deflate_and_br_bodies_are_decompressed_and_headers_and_sizes_are_as_received() {
    let plain = "{\"message\":\"ネコ\"}".repeat(2000);
    let zlib = {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(plain.as_bytes()).unwrap();
        e.finish().unwrap()
    };
    for (enc, body) in [
        ("gzip", gzip(plain.as_bytes())),
        ("br", brotli(plain.as_bytes())),
        ("deflate", zlib),
    ] {
        let compressed_len = body.len();
        assert!(compressed_len < plain.len() / 5, "圧縮の効く入力のはず");
        let body2 = body.clone();
        let port = raw_server(move |mut s, head| {
            let body = body2.clone();
            async move {
                let lower = head.to_ascii_lowercase();
                assert!(lower.replace(' ', "").contains("accept-encoding:gzip,br,deflate"), "{head}");
                let h = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Encoding: {enc}\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                s.write_all(h.as_bytes()).await.unwrap();
                s.write_all(&body).await.unwrap();
            }
        })
        .await;
        let r = send_default(&prepared("GET", &format!("http://127.0.0.1:{port}/")))
            .await
            .unwrap();
        assert_eq!(r.data.body.content, plain, "{enc}");
        assert_eq!(
            r.data.body.size as usize,
            plain.len(),
            "{enc}: size は展開後"
        );
        assert_eq!(r.body.len(), plain.len());
        // ヘッダーは受け取ったまま（展開したあとも content-encoding・content-length を残す）
        assert_eq!(find_header(&r, "content-encoding"), Some(enc));
        assert_eq!(
            find_header(&r, "content-length"),
            Some(compressed_len.to_string().as_str()),
            "{enc}"
        );
        // receivedBytes は「ステータス行 + ヘッダー + 転送されたボディ（展開前）」の近似
        let r_bytes = r.data.received_bytes as usize;
        assert!(r_bytes >= compressed_len, "{enc}: {r_bytes}");
        assert!(r_bytes < compressed_len + 1024, "{enc}: {r_bytes}");
        assert!(r_bytes < plain.len(), "{enc}: 展開後より小さい");
    }
}

#[tokio::test]
async fn a_corrupt_gzip_body_is_a_readable_error_without_the_request_url() {
    let port = raw_server(|mut s, _| async move {
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 20\r\n\r\nthis is not gzip data").await.unwrap();
    })
    .await;
    let e = send_default(&prepared(
        "GET",
        &format!("http://127.0.0.1:{port}/p?token=GZIP-SECRET"),
    ))
    .await
    .unwrap_err();
    assert!(!e.message().is_empty());
    assert!(!e.message().contains("GZIP-SECRET"), "{}", e.message());
}

// ---- 文字コード・バイナリ ----

#[tokio::test]
async fn content_types_with_charset_are_text_when_the_bytes_are_utf8() {
    let server = MockServer::start().await;
    for (p, ct) in [
        ("/a", "application/json; charset=utf-8"),
        ("/b", "application/json;charset=UTF-8"),
        ("/c", "application/problem+json; charset=\"utf-8\""),
        ("/d", "text/plain; charset=ISO-8859-1"),
    ] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw("{\"a\":\"日本語\"}".as_bytes().to_vec(), ct),
            )
            .mount(&server)
            .await;
    }
    for p in ["/a", "/b", "/c", "/d"] {
        let r = send_default(&prepared("GET", &format!("{}{p}", server.uri())))
            .await
            .unwrap()
            .data;
        assert_eq!(r.body.encoding, BodyEncoding::Text, "{p}");
        assert_eq!(r.body.content, "{\"a\":\"日本語\"}", "{p}");
        assert!(r.body.content_type.is_some());
    }
}

#[tokio::test]
async fn non_utf8_charsets_are_converted_to_text_and_the_kept_body_has_the_original_bytes() {
    // Shift_JIS の「日本語」
    let sjis: Vec<u8> = vec![0x93, 0xFA, 0x96, 0x7B, 0x8C, 0xEA];
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/s"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(sjis.clone(), "text/plain; charset=Shift_JIS"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/unknown"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(sjis.clone(), "text/plain; charset=x-nope"),
        )
        .mount(&server)
        .await;
    let r = send_default(&prepared("GET", &format!("{}/s", server.uri())))
        .await
        .unwrap();
    assert_eq!(r.data.body.encoding, BodyEncoding::Text);
    assert_eq!(r.data.body.content, "日本語");
    assert_eq!(r.data.body.size, 6, "size は元のバイト数");
    assert_eq!(&r.body[..], &sjis[..], "保存用の全体は元のバイトのまま");
    // 変換できないものだけ base64
    let r = send_default(&prepared("GET", &format!("{}/unknown", server.uri())))
        .await
        .unwrap();
    assert_eq!(r.data.body.encoding, BodyEncoding::Base64);
    assert_eq!(&r.body[..], &sjis[..]);
}

#[tokio::test]
async fn invalid_utf8_inside_a_response_header_value_is_kept_lossily() {
    let port = raw_server(|mut s, _| async move {
        let mut h = b"HTTP/1.1 200 OK\r\nX-Name: ".to_vec();
        h.extend_from_slice("ネコ".as_bytes());
        h.extend_from_slice(b"\r\nX-Bad: \xff\xfe\r\nContent-Length: 0\r\n\r\n");
        s.write_all(&h).await.unwrap();
    })
    .await;
    let r = send_default(&prepared("GET", &format!("http://127.0.0.1:{port}/")))
        .await
        .unwrap();
    assert_eq!(find_header(&r, "x-name"), Some("ネコ"));
    assert!(find_header(&r, "x-bad").is_some());
}

// ---- 10 MB の境界 ----

#[tokio::test]
async fn exactly_ten_megabytes_is_not_truncated_and_one_more_byte_is() {
    let limit = BODY_PREVIEW_LIMIT;
    assert_eq!(limit, 10 * 1024 * 1024);
    let server = MockServer::start().await;
    for (p, n) in [
        ("/exact", limit),
        ("/plus1", limit + 1),
        ("/minus1", limit - 1),
    ] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_raw(vec![b'a'; n], "text/plain"))
            .mount(&server)
            .await;
    }
    let get = |p: &'static str| {
        let u = format!("{}{p}", server.uri());
        async move { send_default(&prepared("GET", &u)).await.unwrap() }
    };
    let r = get("/exact").await;
    assert!(!r.data.body.truncated);
    assert_eq!(r.data.body.content.len(), limit);
    assert_eq!(r.data.body.size as usize, limit);
    assert_eq!(r.data.body.encoding, BodyEncoding::Text);

    let r = get("/minus1").await;
    assert!(!r.data.body.truncated);
    assert_eq!(r.data.body.content.len(), limit - 1);

    let r = get("/plus1").await;
    assert!(r.data.body.truncated);
    assert_eq!(r.data.body.content.len(), limit);
    assert_eq!(r.data.body.size as usize, limit + 1, "size は切る前の全体");
    assert_eq!(r.body.len(), limit + 1, "保存用は全体");
}

#[tokio::test]
async fn truncating_in_the_middle_of_a_multibyte_character_keeps_valid_text() {
    // 「あ」は 3 バイト。10 MB（3 の倍数でない）で切ると文字の途中になる
    let n = BODY_PREVIEW_LIMIT / 3 + 10;
    let text = "あ".repeat(n);
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(text.clone().into_bytes(), "text/plain; charset=utf-8"),
        )
        .mount(&server)
        .await;
    let r = send_default(&prepared("GET", &server.uri())).await.unwrap();
    assert!(r.data.body.truncated);
    assert_eq!(r.data.body.encoding, BodyEncoding::Text);
    assert!(r.data.body.content.len() <= BODY_PREVIEW_LIMIT);
    assert!(r.data.body.content.len() > BODY_PREVIEW_LIMIT - 3);
    assert!(text.starts_with(&r.data.body.content));
}

// ---- 同時送信と中止 ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_concurrent_sends_each_get_their_own_response() {
    let server = MockServer::start().await;
    for i in 0..24u32 {
        Mock::given(method("GET"))
            .and(path(format!("/r/{i}")))
            .respond_with(
                ResponseTemplate::new(200 + (i % 3) as u16)
                    .set_delay(Duration::from_millis(u64::from(30 + (i * 37) % 200)))
                    .set_body_string(format!("body-{i}")),
            )
            .mount(&server)
            .await;
    }
    let mut set = tokio::task::JoinSet::new();
    for i in 0..24u32 {
        let url = format!("{}/r/{i}", server.uri());
        set.spawn(async move {
            let r = send_default(&prepared("GET", &url)).await.unwrap();
            (
                i,
                r.data.status,
                r.data.body.content.clone(),
                r.data.final_url.clone(),
            )
        });
    }
    let mut seen = 0;
    while let Some(res) = set.join_next().await {
        let (i, status, body, final_url) = res.unwrap();
        assert_eq!(status, 200 + (i % 3) as u16);
        assert_eq!(body, format!("body-{i}"));
        assert!(final_url.ends_with(&format!("/r/{i}")));
        seen += 1;
    }
    assert_eq!(seen, 24);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_one_send_does_not_disturb_the_others() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(600))
                .set_body_string("slow"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/fast"))
        .respond_with(ResponseTemplate::new(200).set_body_string("fast"))
        .mount(&server)
        .await;

    let victim = CancellationToken::new();
    let t = victim.clone();
    let victim_url = format!("{}/slow", server.uri());
    let h_victim = tokio::spawn(async move {
        send(&prepared("GET", &victim_url), &SendSettings::default(), &t).await
    });
    let mut others = Vec::new();
    for i in 0..5 {
        let url = format!(
            "{}/{}",
            server.uri(),
            if i % 2 == 0 { "slow" } else { "fast" }
        );
        others.push(tokio::spawn(async move {
            send_default(&prepared("GET", &url)).await
        }));
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    victim.cancel();

    let started = Instant::now();
    let err = h_victim.await.unwrap().unwrap_err();
    assert!(matches!(err, SendError::Cancelled { .. }), "{err:?}");
    assert!(
        started.elapsed() < Duration::from_millis(400),
        "中止がすぐ効く"
    );
    for (i, h) in others.into_iter().enumerate() {
        let r = h.await.unwrap().unwrap();
        assert_eq!(
            r.data.body.content,
            if i % 2 == 0 { "slow" } else { "fast" }
        );
    }
}

#[tokio::test]
async fn cancelling_after_the_send_finished_changes_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("done"))
        .mount(&server)
        .await;
    let token = CancellationToken::new();
    let r = send(
        &prepared("GET", &server.uri()),
        &SendSettings::default(),
        &token,
    )
    .await
    .unwrap();
    token.cancel();
    assert_eq!(r.data.body.content, "done");
    assert!(token.is_cancelled());
}

#[tokio::test]
async fn timeout_zero_means_no_limit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(400))
                .set_body_string("late"),
        )
        .mount(&server)
        .await;
    let s = SendSettings {
        timeout_ms: 0,
        ..SendSettings::default()
    };
    let r = send(
        &prepared("GET", &server.uri()),
        &s,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(r.data.body.content, "late");
    let s = SendSettings {
        timeout_ms: 100,
        ..SendSettings::default()
    };
    let e = send(
        &prepared("GET", &server.uri()),
        &s,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(matches!(e, SendError::Timeout { .. }), "{e:?}");
    assert!(e.message().contains("100 ミリ秒"), "{}", e.message());
    let s = SendSettings {
        timeout_ms: 2000,
        ..SendSettings::default()
    };
    let _ = send(
        &prepared("GET", &server.uri()),
        &s,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
}

// ---- リダイレクト ----

#[tokio::test]
async fn post_redirect_303_becomes_get_and_drops_the_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/s"))
        .respond_with(ResponseTemplate::new(303).insert_header("location", "/final"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/final"))
        .respond_with(ResponseTemplate::new(200).set_body_string("got"))
        .mount(&server)
        .await;
    let mut req = prepared("POST", &format!("{}/s", server.uri()));
    req.headers.push(hdr("Content-Type", "application/json"));
    req.body = PreparedBody::Bytes(b"{\"a\":1}".to_vec());
    let r = send_default(&req).await.unwrap().data;
    assert_eq!(
        (r.status, r.body.content.as_str(), r.redirect_count),
        (200, "got", 1)
    );
}

#[tokio::test]
async fn post_redirects_307_and_308_are_followed_with_the_same_method_and_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/t"))
        .respond_with(ResponseTemplate::new(307).insert_header("location", "/final"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/p"))
        .respond_with(ResponseTemplate::new(308).insert_header("location", "/final"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/final"))
        .respond_with(ResponseTemplate::new(201).set_body_string("posted"))
        .mount(&server)
        .await;
    for p in ["/t", "/p"] {
        let mut req = prepared("POST", &format!("{}{p}", server.uri()));
        req.headers.push(hdr("Content-Type", "application/json"));
        req.body = PreparedBody::Bytes(b"{\"a\":1}".to_vec());
        let r = send_default(&req).await.unwrap().data;
        assert_eq!(
            (r.status, r.body.content.as_str(), r.redirect_count),
            (201, "posted", 1),
            "POST {p}"
        );
    }
    let got = server.received_requests().await.unwrap();
    assert!(got
        .iter()
        .filter(|g| g.url.path() == "/final")
        .all(|g| g.body == b"{\"a\":1}"));
}

// ---- 実際に送られる形（core の組み立て → 送信） ----

struct NoDyn;
impl DynamicSource for NoDyn {
    fn eval(&self, _: &str) -> Option<String> {
        None
    }
}

fn kv(location: ParamIn, name: &str, value: &str) -> KeyValue {
    KeyValue {
        location,
        name: name.into(),
        value: value.into(),
        enabled: true,
    }
}

fn spec(method: &str, url: &str) -> RequestSpec {
    RequestSpec {
        method: method.into(),
        url: url.into(),
        params: vec![],
        body: Body::None,
        auth: None,
    }
}

#[tokio::test]
async fn path_query_and_method_are_exactly_what_the_server_sees() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let mut s = spec("get", &format!("{}/files/{{name}}", server.uri()));
    s.params = vec![
        kv(ParamIn::Path, "name", "請求書 2026/11"),
        kv(ParamIn::Query, "tag", "a"),
        kv(ParamIn::Query, "tag", "b"),
        kv(ParamIn::Query, "q", "ネコ & 犬"),
    ];
    let p = build_request(&s, &VarScope::new(), &NoDyn).unwrap();
    send_default(&p).await.unwrap();
    let got = &server.received_requests().await.unwrap()[0];
    assert_eq!(got.method.as_str(), "GET");
    assert_eq!(
        got.url.path(),
        "/files/%E8%AB%8B%E6%B1%82%E6%9B%B8%202026%2F11"
    );
    assert_eq!(
        got.url.query(),
        Some("tag=a&tag=b&q=%E3%83%8D%E3%82%B3%20%26%20%E7%8A%AC")
    );
}

#[tokio::test]
async fn auth_cookie_and_content_type_headers_on_the_wire() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let mut l = VarLayer::new();
    l.insert("pw", "p:w", true);
    let mut sc = VarScope::new();
    sc.push_layer(l);

    // Basic（ユーザー名に ':' を含む）・Cookie・手入力の Content-Type が自動のものに勝つ
    let mut s = spec("post", &server.uri());
    s.auth = Some(nekoreq_core::AuthOverride::Basic {
        username: "ネコ".into(),
        password: "{{pw}}".into(),
    });
    s.params = vec![
        kv(ParamIn::Cookie, "SESSION", "abc"),
        kv(ParamIn::Cookie, "XSRF-TOKEN", "tok"),
        kv(ParamIn::Header, "content-type", "application/vnd.api+json"),
        kv(ParamIn::Header, "X-Dup", "1"),
        kv(ParamIn::Header, "X-Dup", "2"),
    ];
    s.body = Body::Json("{\"a\":\"日本語\"}".into());
    let p = build_request(&s, &sc, &NoDyn).unwrap();
    send_default(&p).await.unwrap();
    let got = &server.received_requests().await.unwrap()[0];
    assert_eq!(got.method.as_str(), "POST");
    let h = &got.headers;
    assert_eq!(
        h.get("content-type").unwrap().to_str().unwrap(),
        "application/vnd.api+json"
    );
    assert_eq!(
        h.get("cookie").unwrap().to_str().unwrap(),
        "SESSION=abc; XSRF-TOKEN=tok"
    );
    let auth = h
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(auth.strip_prefix("Basic ").unwrap())
        .unwrap();
    assert_eq!(String::from_utf8(decoded).unwrap(), "ネコ:p:w");
    let dups: Vec<_> = h
        .get_all("x-dup")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert_eq!(dups, ["1", "2"]);
    assert_eq!(
        String::from_utf8(got.body.clone()).unwrap(),
        "{\"a\":\"日本語\"}"
    );
    assert_eq!(
        h.get("content-length").unwrap().to_str().unwrap(),
        got.body.len().to_string()
    );
}

#[tokio::test]
async fn form_body_on_the_wire() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let mut s = spec("POST", &server.uri());
    s.body = Body::Form(vec![
        kv(ParamIn::Query, "name", "ネコ 犬"),
        kv(ParamIn::Query, "k", "a&b=c+d"),
    ]);
    send_default(&build_request(&s, &VarScope::new(), &NoDyn).unwrap())
        .await
        .unwrap();
    let got = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        got.headers.get("content-type").unwrap().to_str().unwrap(),
        "application/x-www-form-urlencoded"
    );
    let pairs: Vec<(String, String)> = url::form_urlencoded::parse(&got.body)
        .into_owned()
        .collect();
    assert_eq!(
        pairs,
        [
            ("name".into(), "ネコ 犬".into()),
            ("k".into(), "a&b=c+d".into())
        ]
    );
}

#[tokio::test]
async fn multipart_with_binary_file_and_japanese_names_is_well_formed_on_the_wire() {
    let dir = tempfile::tempdir().unwrap();
    // CRLF・境界に似た並び・UTF-8 でないバイトを含む
    let mut bin: Vec<u8> = vec![0xff, 0xfe, 0x00, b'\r', b'\n', b'-', b'-'];
    bin.extend((0..=255u8).collect::<Vec<_>>());
    let file = dir.path().join("請求書 1.bin");
    std::fs::write(&file, &bin).unwrap();
    let empty = dir.path().join("empty.txt");
    std::fs::write(&empty, b"").unwrap();

    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let mut s = spec("POST", &server.uri());
    s.body = Body::Multipart(vec![
        MultipartPart {
            name: "題名".into(),
            value: MultipartValue::Text("こんにちは\r\n世界".into()),
            enabled: true,
        },
        MultipartPart {
            name: "file".into(),
            value: MultipartValue::File(file.display().to_string()),
            enabled: true,
        },
        MultipartPart {
            name: "e\"mpty".into(),
            value: MultipartValue::File(empty.display().to_string()),
            enabled: true,
        },
    ]);
    send_default(&build_request(&s, &VarScope::new(), &NoDyn).unwrap())
        .await
        .unwrap();

    let got = &server.received_requests().await.unwrap()[0];
    let ct = got
        .headers
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let boundary = ct
        .strip_prefix("multipart/form-data; boundary=")
        .expect(&ct)
        .to_string();
    let body = got.body.clone();
    let delim = format!("--{boundary}").into_bytes();
    assert!(body.starts_with(&delim));
    assert!(body.ends_with(format!("--{boundary}--\r\n").as_bytes()));
    assert_eq!(
        got.headers.get("content-length").unwrap().to_str().unwrap(),
        body.len().to_string()
    );
    // 境界は本文のデータの中に現れない（データは境界の区切りとしてしか出ない）
    let count = body
        .windows(delim.len())
        .filter(|w| *w == delim.as_slice())
        .count();
    assert_eq!(count, 4, "3 パート + 終端");
    // ファイルの中身がそのまま入っている
    assert!(
        body.windows(bin.len()).any(|w| w == bin.as_slice()),
        "バイナリが壊れている"
    );
    let text = String::from_utf8_lossy(&body).to_string();
    assert!(text.contains("name=\"題名\""), "{text}");
    assert!(text.contains("こんにちは\r\n世界"));
    assert!(
        text.contains("name=\"file\"; filename=\"請求書 1.bin\""),
        "{text}"
    );
    assert!(text.contains("Content-Type: application/octet-stream"));
    assert!(
        text.contains("name=\"e%22mpty\"; filename=\"empty.txt\""),
        "{text}"
    );
    assert!(text.contains("Content-Type: text/plain"));
}

#[tokio::test]
async fn a_binary_file_body_is_sent_byte_for_byte_with_octet_stream() {
    let dir = tempfile::tempdir().unwrap();
    let data: Vec<u8> = (0..=255u8).cycle().take(100_000).collect();
    let file = dir.path().join("b.bin");
    std::fs::write(&file, &data).unwrap();
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let mut s = spec("PUT", &server.uri());
    s.body = Body::Binary(file.display().to_string());
    send_default(&build_request(&s, &VarScope::new(), &NoDyn).unwrap())
        .await
        .unwrap();
    let got = &server.received_requests().await.unwrap()[0];
    assert_eq!(got.body, data);
    assert_eq!(
        got.headers.get("content-type").unwrap().to_str().unwrap(),
        "application/octet-stream"
    );
}

#[tokio::test]
async fn default_headers_are_sent_and_a_user_supplied_accept_encoding_is_respected() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    send_default(&prepared("GET", &server.uri())).await.unwrap();
    let mut req = prepared("GET", &server.uri());
    req.headers.push(hdr("Accept-Encoding", "identity"));
    req.headers.push(hdr("Accept", "text/csv"));
    send_default(&req).await.unwrap();
    let got = server.received_requests().await.unwrap();
    assert_eq!(
        got[0].headers.get("accept").unwrap().to_str().unwrap(),
        "*/*"
    );
    // 実際に付く値は、プレビューの DEFAULT_ACCEPT_ENCODING と同じ
    assert_eq!(
        got[0]
            .headers
            .get("accept-encoding")
            .unwrap()
            .to_str()
            .unwrap(),
        nekoreq_core::DEFAULT_ACCEPT_ENCODING
    );
    assert!(got[0]
        .headers
        .get("user-agent")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("NekoReq/"));
    assert_eq!(
        got[1].headers.get("accept").unwrap().to_str().unwrap(),
        "text/csv"
    );
    assert_eq!(
        got[1]
            .headers
            .get("accept-encoding")
            .unwrap()
            .to_str()
            .unwrap(),
        "identity"
    );
}

// ---- エラーにシークレットが出ない ----

#[tokio::test]
async fn connection_errors_do_not_echo_secrets_from_the_url() {
    // 閉じているポートへ（接続拒否）。シークレットは URL のクエリとパスに入っている
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let mut req = prepared(
        "GET",
        &format!("http://127.0.0.1:{port}/v1/CONN-PATH-SECRET?token=CONN-SECRET"),
    );
    req.secrets = vec!["CONN-SECRET".into(), "CONN-PATH-SECRET".into()];
    let e = send_default(&req).await.unwrap_err();
    assert!(matches!(e, SendError::Connect { .. }), "{e:?}");
    let text = format!("{} / {e:?}", e.message());
    assert!(!text.contains("CONN-SECRET"), "{text}");
    assert!(!text.contains("CONN-PATH-SECRET"), "{text}");
}

#[tokio::test]
async fn redirect_loop_errors_do_not_echo_secrets_from_the_url() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", "/loop?token=LOOP-SECRET"),
        )
        .mount(&server)
        .await;
    let mut req = prepared("GET", &format!("{}/loop?token=LOOP-SECRET", server.uri()));
    req.secrets = vec!["LOOP-SECRET".into()];
    let e = send_default(&req).await.unwrap_err();
    assert!(!e.message().is_empty());
    assert!(!e.message().contains("LOOP-SECRET"), "{}", e.message());
}

#[tokio::test]
async fn header_errors_name_the_header_but_never_show_its_value() {
    let mut req = prepared("GET", "http://127.0.0.1:9/");
    req.headers
        .push(hdr("X-Token", "HDR-SECRET\r\nInjected: 1"));
    let e = send_default(&req).await.unwrap_err();
    assert!(matches!(e, SendError::InvalidRequest { .. }), "{e:?}");
    assert!(e.message().contains("X-Token"));
    assert!(!e.message().contains("HDR-SECRET"), "{}", e.message());
    let mut req = prepared("GET", "http://127.0.0.1:9/");
    req.headers.push(hdr("Bad Name", "v"));
    assert!(matches!(
        send_default(&req).await.unwrap_err(),
        SendError::InvalidRequest { .. }
    ));
}

#[tokio::test]
async fn timeout_and_cancel_messages_do_not_contain_the_url() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(500)))
        .mount(&server)
        .await;
    let url = format!("{}/x?token=TIMEOUT-SECRET", server.uri());
    let s = SendSettings {
        timeout_ms: 100,
        ..SendSettings::default()
    };
    let e = send(&prepared("GET", &url), &s, &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(e, SendError::Timeout { .. }));
    assert!(!e.message().contains("TIMEOUT-SECRET"));
    let t = CancellationToken::new();
    t.cancel();
    let e = send(&prepared("GET", &url), &SendSettings::default(), &t)
        .await
        .unwrap_err();
    assert!(matches!(e, SendError::Cancelled { .. }));
    assert!(!e.message().contains("TIMEOUT-SECRET"));
}

#[tokio::test]
async fn final_url_after_a_redirect_is_the_last_url() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/a"))
        .respond_with(ResponseTemplate::new(301).insert_header("location", "/b?x=%E3%81%82"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/b"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let r = send_default(&prepared("GET", &format!("{}/a", server.uri())))
        .await
        .unwrap()
        .data;
    assert_eq!(r.final_url, format!("{}/b?x=%E3%81%82", server.uri()));
    assert_eq!(r.redirect_count, 1);
}
