//! テスト担当による独立した検証（http クレートの公開 API だけを使う）。
//!
//! - 圧縮されたレスポンスの上限・壊れた圧縮・知らない方式
//! - 文字コード（EUC-JP・UTF-16・不正なラベル）
//! - リダイレクトの回数の境界・メソッドの扱い
//! - 同じ名前のヘッダー（複数の `Set-Cookie`）の順序

use std::io::Write;
use std::time::{Duration, Instant};

use nekoreq_core::{
    BodyEncoding, HeaderPair, PreparedBody, PreparedRequest, SendError, SendSettings,
};
use nekoreq_http::{send_with_transport, CancellationToken, Limits, SentResponse, Transport};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn prepared(m: &str, url: &str, body: PreparedBody) -> PreparedRequest {
    PreparedRequest {
        method: m.into(),
        url: url.into(),
        headers: Vec::<HeaderPair>::new(),
        body,
        secrets: vec![],
    }
}

async fn go(req: &PreparedRequest, t: &Transport) -> Result<SentResponse, SendError> {
    send_with_transport(req, &SendSettings::default(), &CancellationToken::new(), t).await
}

fn small_limit() -> Transport {
    Transport {
        limits: Limits {
            preview_bytes: 1024 * 1024,
            max_body_bytes: 2 * 1024 * 1024,
        },
        ..Transport::default()
    }
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn brotli_bytes(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut w = brotli::CompressorWriter::new(&mut out, 4096, 1, 22);
        w.write_all(data).unwrap();
    }
    out
}

async fn serve(status: u16, headers: &[(&str, &str)], body: Vec<u8>) -> MockServer {
    let server = MockServer::start().await;
    let mut t = ResponseTemplate::new(status).set_body_bytes(body);
    for (k, v) in headers {
        t = t.append_header(*k, *v);
    }
    Mock::given(wiremock::matchers::any())
        .respond_with(t)
        .mount(&server)
        .await;
    server
}

// ---- 圧縮 ----

#[tokio::test]
async fn compression_bombs_are_refused_quickly_for_every_encoding() {
    // 64 MB のゼロ（圧縮すると数十 KB）を、上限 2 MB で受ける
    let zeros = vec![0u8; 64 * 1024 * 1024];
    for (enc, body) in [
        ("gzip", gzip(&zeros)),
        ("deflate", zlib(&zeros)),
        ("br", brotli_bytes(&zeros)),
        // 二重（gzip を gzip）
        ("gzip, gzip", gzip(&gzip(&zeros))),
    ] {
        assert!(
            body.len() < 1024 * 1024,
            "{enc}: 圧縮後 {} バイト",
            body.len()
        );
        let server = serve(200, &[("content-encoding", enc)], body).await;
        let t = Instant::now();
        let err = go(
            &prepared("GET", &server.uri(), PreparedBody::None),
            &small_limit(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SendError::TooLarge { .. }), "{enc}: {err:?}");
        assert!(
            err.message().contains("大きすぎ"),
            "{enc}: {}",
            err.message()
        );
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "{enc}: {:?}",
            t.elapsed()
        );
    }
}

#[tokio::test]
async fn corrupt_or_truncated_compressed_bodies_are_reported_in_japanese() {
    let good = gzip(&b"hello world ".repeat(1000));
    let truncated = good[..good.len() / 2].to_vec();
    let mut flipped = good.clone();
    let mid = flipped.len() / 2;
    flipped[mid] ^= 0xff;
    for (enc, body) in [
        ("gzip", b"not gzip at all".to_vec()),
        ("gzip", truncated),
        ("gzip", flipped),
        ("br", b"\xff\xff\xff\xff garbage".to_vec()),
        ("deflate", b"\x00\x01\x02\x03".to_vec()),
    ] {
        let server = serve(200, &[("content-encoding", enc)], body).await;
        let r = go(
            &prepared("GET", &server.uri(), PreparedBody::None),
            &Transport::default(),
        )
        .await;
        match r {
            Err(SendError::Other { message }) => {
                assert!(message.contains("展開できません"), "{enc}: {message}");
            }
            // 生の deflate として読めてしまう・途中までの内容が読めるものは、エラーにならなくてもよい（落ちない）
            Ok(_) => {}
            Err(e) => panic!("{enc}: {e:?}"),
        }
    }
}

#[tokio::test]
async fn unknown_content_encodings_return_the_bytes_as_they_are() {
    let payload = b"\x28\xb5\x2f\xfd zstd-ish bytes".to_vec();
    for enc in ["zstd", "compress", "x-unknown"] {
        let server = serve(
            200,
            &[
                ("content-encoding", enc),
                ("content-type", "application/json"),
            ],
            payload.clone(),
        )
        .await;
        let sent = go(
            &prepared("GET", &server.uri(), PreparedBody::None),
            &Transport::default(),
        )
        .await
        .unwrap();
        assert_eq!(&sent.body[..], &payload[..], "{enc}");
        // ヘッダーはそのまま返る
        assert!(sent
            .data
            .headers
            .iter()
            .any(|h| h.name == "content-encoding" && h.value == enc));
    }
    // gzip のあとに zstd をかけたもの（"gzip, zstd"）は、zstd で止まる（gzip までは外せない）
    let server = serve(200, &[("content-encoding", "gzip, zstd")], payload.clone()).await;
    let sent = go(
        &prepared("GET", &server.uri(), PreparedBody::None),
        &Transport::default(),
    )
    .await
    .unwrap();
    assert_eq!(&sent.body[..], &payload[..]);
}

#[tokio::test]
async fn empty_bodies_with_a_content_encoding_do_not_fail() {
    // HEAD・204・304 は本文がないのに Content-Encoding を付けてくるサーバーがある
    for (m, status) in [("HEAD", 200u16), ("GET", 204), ("GET", 304)] {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(status).append_header("content-encoding", "gzip"))
            .mount(&server)
            .await;
        let sent = go(
            &prepared(m, &server.uri(), PreparedBody::None),
            &Transport::default(),
        )
        .await
        .unwrap_or_else(|e| panic!("{m} {status}: {e:?}"));
        assert_eq!(sent.data.status, status);
        assert_eq!(sent.data.body.size, 0);
    }
}

// ---- 文字コード ----

#[tokio::test]
async fn charsets_are_converted_and_odd_labels_do_not_panic() {
    let text = "日本語のテスト";
    let (euc, _, _) = encoding_rs::EUC_JP.encode(text);
    let server = serve(
        200,
        &[("content-type", "text/plain; charset=EUC-JP")],
        euc.to_vec(),
    )
    .await;
    let sent = go(
        &prepared("GET", &server.uri(), PreparedBody::None),
        &Transport::default(),
    )
    .await
    .unwrap();
    assert_eq!(sent.data.body.content, text);

    // UTF-16LE（BOM つき）
    let mut utf16 = vec![0xff, 0xfe];
    for u in text.encode_utf16() {
        utf16.extend_from_slice(&u.to_le_bytes());
    }
    let server = serve(
        200,
        &[("content-type", "text/plain; charset=utf-16le")],
        utf16,
    )
    .await;
    let sent = go(
        &prepared("GET", &server.uri(), PreparedBody::None),
        &Transport::default(),
    )
    .await
    .unwrap();
    // 読めても読めなくてもよい（落ちない）。読めたなら元の文字列を含む
    if sent.data.body.encoding == BodyEncoding::Text && sent.data.body.content.contains("日本語")
    {
        assert!(sent.data.body.content.contains(text));
    }

    // 不正なラベル・宣言は UTF-8 だが中身が壊れている
    for ct in [
        "text/plain; charset=bogus-9000",
        "text/plain; charset=",
        "text/plain; charset=\"utf-8",
        "text/plain; charset=utf-8; charset=shift_jis",
    ] {
        let server = serve(200, &[("content-type", ct)], b"ok \xff\xfe bytes".to_vec()).await;
        let sent = go(
            &prepared("GET", &server.uri(), PreparedBody::None),
            &Transport::default(),
        )
        .await
        .unwrap_or_else(|e| panic!("{ct}: {e:?}"));
        assert_eq!(sent.data.status, 200, "{ct}");
    }
}

// ---- リダイレクト ----

#[tokio::test]
async fn the_tenth_redirect_is_followed_and_the_eleventh_is_refused() {
    // /r0 → /r1 → … → /r10（200）
    let ten = MockServer::start().await;
    for i in 0..10 {
        Mock::given(path(format!("/r{i}")))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("/r{}", i + 1).as_str()),
            )
            .mount(&ten)
            .await;
    }
    Mock::given(path("/r10"))
        .respond_with(ResponseTemplate::new(200).set_body_string("end"))
        .mount(&ten)
        .await;
    let sent = go(
        &prepared("GET", &format!("{}/r0", ten.uri()), PreparedBody::None),
        &Transport::default(),
    )
    .await
    .unwrap();
    assert_eq!(sent.data.body.content, "end");
    assert_eq!(sent.data.redirect_count, 10);

    // 11 回目は断る。メッセージに URL は出さない
    let eleven = MockServer::start().await;
    for i in 0..11 {
        Mock::given(path(format!("/r{i}")))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("/r{}?secret=TOKEN-XYZ", i + 1).as_str()),
            )
            .mount(&eleven)
            .await;
    }
    Mock::given(path("/r11"))
        .respond_with(ResponseTemplate::new(200).set_body_string("end"))
        .mount(&eleven)
        .await;
    let err = go(
        &prepared("GET", &format!("{}/r0", eleven.uri()), PreparedBody::None),
        &Transport::default(),
    )
    .await
    .unwrap_err();
    assert!(err.message().contains("10 回"), "{}", err.message());
    assert!(!err.message().contains("TOKEN-XYZ"), "{}", err.message());
}

#[tokio::test]
async fn method_and_body_follow_the_status_of_the_redirect() {
    // (元の method, ステータス, 追従後の method, ボディが残る)
    let cases = [
        ("PUT", 302u16, "PUT", true),
        ("DELETE", 302, "DELETE", false),
        ("PUT", 303, "GET", false),
        ("PUT", 307, "PUT", true),
        ("POST", 308, "POST", true),
        ("POST", 301, "GET", false),
        ("PATCH", 301, "PATCH", true),
        ("HEAD", 303, "HEAD", false),
    ];
    for (m, status, want_method, body_kept) in cases {
        let server = MockServer::start().await;
        Mock::given(path("/start"))
            .respond_with(ResponseTemplate::new(status).insert_header("location", "/end"))
            .mount(&server)
            .await;
        Mock::given(path("/end"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let body = if m == "DELETE" || m == "HEAD" {
            PreparedBody::None
        } else {
            PreparedBody::Bytes(b"payload".to_vec())
        };
        let sent = go(
            &prepared(m, &format!("{}/start", server.uri()), body),
            &Transport::default(),
        )
        .await
        .unwrap();
        assert_eq!(sent.data.status, 200, "{m} {status}");
        let reqs = server.received_requests().await.unwrap();
        let last = reqs.last().unwrap();
        assert_eq!(last.method.as_str(), want_method, "{m} {status}");
        let kept = last.body == b"payload";
        assert_eq!(kept, body_kept, "{m} {status}: ボディ {:?}", last.body);
    }
}

// ---- 同じ名前のヘッダー ----

#[tokio::test]
async fn repeated_response_headers_keep_every_value_in_order() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("set-cookie", "SESSION=a; Path=/")
                .append_header("x-dup", "1")
                .append_header("set-cookie", "XSRF-TOKEN=b; Path=/")
                .append_header("x-dup", "2")
                .append_header("set-cookie", "SESSION=c; Path=/"),
        )
        .mount(&server)
        .await;
    let sent = go(
        &prepared("GET", &server.uri(), PreparedBody::None),
        &Transport::default(),
    )
    .await
    .unwrap();
    let cookies: Vec<&str> = sent
        .data
        .headers
        .iter()
        .filter(|h| h.name == "set-cookie")
        .map(|h| h.value.as_str())
        .collect();
    assert_eq!(
        cookies,
        [
            "SESSION=a; Path=/",
            "XSRF-TOKEN=b; Path=/",
            "SESSION=c; Path=/"
        ]
    );
    let dups: Vec<&str> = sent
        .data
        .headers
        .iter()
        .filter(|h| h.name == "x-dup")
        .map(|h| h.value.as_str())
        .collect();
    assert_eq!(dups, ["1", "2"]);
}
