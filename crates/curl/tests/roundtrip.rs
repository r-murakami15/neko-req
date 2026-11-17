//! 往復: `parse(export(x))` が `x` と同じ送信内容になること。
//!
//! 比べるのは、core の `build_request` で組み立てた結果（method・URL・ヘッダー・ボディ）。
//! 書き出しでは Cookie ヘッダーを `-b` にまとめる、フォームのエンコードの違い（`+` と `%20`）など、
//! 入力値の持ち方が変わっても送る内容が同じなら同じとみなす。

mod common;

use common::*;
use nekoreq_core::{
    ApiKeyIn, AuthOverride, Body, MultipartPart, MultipartValue, ParamIn, RequestSpec,
};
use nekoreq_curl::{export, parse, ExportOptions, FormStyle, Shell};

fn spec(method: &str, url: &str) -> RequestSpec {
    RequestSpec {
        method: method.into(),
        url: url.into(),
        params: vec![],
        body: Body::None,
        auth: None,
    }
}

fn row(loc: ParamIn, n: &str, v: &str) -> nekoreq_core::KeyValue {
    kv(loc, n, v)
}

fn text_part(n: &str, v: &str) -> MultipartPart {
    MultipartPart {
        name: n.into(),
        value: MultipartValue::Text(v.into()),
        enabled: true,
        content_type: None,
    }
}

fn file_part(n: &str, v: &str) -> MultipartPart {
    MultipartPart {
        name: n.into(),
        value: MultipartValue::File(v.into()),
        enabled: true,
        content_type: None,
    }
}

fn cases() -> Vec<(&'static str, RequestSpec)> {
    let mut v: Vec<(&'static str, RequestSpec)> = Vec::new();

    let mut s = spec("GET", "http://localhost:8080/api/v1/video/clients");
    s.params.push(row(ParamIn::Query, "page", "1"));
    s.params
        .push(row(ParamIn::Query, "keyword", "日本 語&x=y+z%"));
    v.push(("get with query", s));

    let mut s = spec(
        "GET",
        "https://api.example.com:8443/v1/clients/{clientId}/notes/{noteId}",
    );
    s.params.push(row(ParamIn::Path, "clientId", "a/b c"));
    s.params.push(row(ParamIn::Path, "noteId", "日本"));
    s.params.push(row(ParamIn::Header, "X-Trace", "abc:def"));
    v.push(("path params and header", s));

    let mut s = spec("POST", "http://localhost:8080/api/v1/auth/password-login");
    s.body = Body::Json(r#"{"email":"user@example.com","password":"it's \"x\" 🐱 日本語"}"#.into());
    v.push(("json with quotes and emoji", s));

    let mut s = spec("PUT", "http://localhost:8080/api/v1/x");
    s.body = Body::Json(
        "{\n  \"a\": \"line1\\nline2\",\n  \"b\": \"back\\\\slash\",\n  \"c\": \"tab\\t\"\n}"
            .into(),
    );
    v.push(("json with newlines and backslashes", s));

    let mut s = spec("POST", "http://x/a");
    s.body = Body::Form(vec![
        row(ParamIn::Query, "name", "日本 語"),
        row(ParamIn::Query, "q", "a&b=c+d%e"),
        row(ParamIn::Query, "empty", ""),
        row(ParamIn::Query, "a=b", "c"),
        row(ParamIn::Query, "{ [x]", "x"),
    ]);
    v.push(("form", s));

    let mut s = spec("POST", "http://x/a");
    s.body = Body::Multipart(vec![
        text_part("title", "契約書 \"案\""),
        text_part("note", "@leading;semi"),
        text_part("lt", "<file"),
        file_part("file", "/tmp/a b.png"),
        file_part("odd", "/tmp/we;ird,\"n\".txt"),
    ]);
    v.push(("multipart", s));

    // パートの Content-Type（`;type=`）も往復する
    let mut s = spec("POST", "http://localhost:8080/api/v1/upload");
    s.body = Body::Multipart(vec![
        MultipartPart {
            content_type: Some("image/png".into()),
            ..file_part("file", "/tmp/a.png")
        },
        MultipartPart {
            content_type: Some("application/json".into()),
            ..text_part("meta", r#"{"a":1}"#)
        },
        MultipartPart {
            content_type: Some("text/plain".into()),
            ..text_part("semi", "a;b\"c\\d")
        },
    ]);
    v.push(("multipart_with_part_types", s));

    let mut s = spec("POST", "http://x/a");
    s.body = Body::Text("plain a=b&c=d\nsecond line 日本語".into());
    v.push(("text body", s));

    let mut s = spec("PUT", "http://x/upload");
    s.body = Body::Binary("/tmp/data file.bin".into());
    v.push(("binary body", s));

    let mut s = spec("GET", "http://x/a");
    s.auth = Some(AuthOverride::Basic {
        username: "neko".into(),
        password: "p@ss:wo'rd ".into(),
    });
    v.push(("basic auth", s));

    let mut s = spec("GET", "http://x/a");
    s.auth = Some(AuthOverride::Bearer {
        token: "eyJ.abc-_.def".into(),
    });
    v.push(("bearer auth", s));

    for (label, loc) in [
        ("api key header", ApiKeyIn::Header),
        ("api key query", ApiKeyIn::Query),
        ("api key cookie", ApiKeyIn::Cookie),
    ] {
        let mut s = spec("GET", "http://x/a?z=1");
        s.params.push(row(ParamIn::Query, "q", "1"));
        s.auth = Some(AuthOverride::ApiKey {
            location: loc,
            name: "X-Api-Key".into(),
            value: "k e/y".into(),
        });
        v.push((label, s));
    }

    let mut s = spec("GET", "http://x/a");
    s.params.push(row(ParamIn::Cookie, "SESSION", "abc123"));
    s.params.push(row(ParamIn::Cookie, "XSRF-TOKEN", "t=1"));
    s.params.push(row(ParamIn::Header, "Cookie", "manual=1;"));
    v.push(("cookies", s));

    let mut s = spec("GET", "http://x/a");
    s.auth = Some(AuthOverride::Bearer {
        token: "ignored".into(),
    });
    s.params
        .push(row(ParamIn::Header, "Authorization", "Digest user=\"a\""));
    v.push(("manual authorization wins", s));

    let mut s = spec("GET", "http://x/a");
    s.params.push(row(ParamIn::Header, "X-Empty", ""));
    s.params.push(row(ParamIn::Header, "X-Dup", "1"));
    s.params.push(row(ParamIn::Header, "X-Dup", "2"));
    let mut off = row(ParamIn::Header, "X-Off", "x");
    off.enabled = false;
    s.params.push(off);
    v.push(("empty, duplicate and disabled headers", s));

    let mut s = spec("DELETE", "http://x/a/1");
    s.params.push(row(ParamIn::Header, "X-XSRF-TOKEN", "tok"));
    v.push(("delete", s));

    let mut s = spec("PATCH", "http://x/a");
    s.body = Body::Json(r#"{"a":1}"#.into());
    s.params.push(row(
        ParamIn::Header,
        "Content-Type",
        "application/merge-patch+json",
    ));
    v.push(("manual json content type", s));

    let mut s = spec("GET", "http://x/a");
    s.body = Body::Text("get with body".into());
    v.push(("get with body", s));

    let mut s = spec("POST", "http://x/a");
    s.body = Body::Json("{\"cr\":\"a\r\nb\"}".into());
    v.push(("control characters", s));

    let mut s = spec("GET", "localhost:8080/日本語/{x}?a=1");
    s.params.push(row(ParamIn::Query, "b", "2"));
    v.push(("no scheme, non-ascii path, existing query", s));

    let mut s = spec("POST", "http://x/a");
    s.body = Body::Form(vec![row(ParamIn::Query, "a", "1")]);
    s.auth = Some(AuthOverride::Basic {
        username: "u".into(),
        password: "".into(),
    });
    v.push(("form with empty password", s));

    v
}

#[test]
fn round_trip_keeps_the_request_semantics() {
    let all = cases();
    assert!(all.len() >= 15);
    for (label, original) in &all {
        for shell in [Shell::Bash, Shell::Cmd] {
            for multiline in [true, false] {
                for form_style in [FormStyle::UrlEncode, FormStyle::Data] {
                    let options = ExportOptions {
                        shell,
                        multiline,
                        form_style,
                        ..ExportOptions::default()
                    };
                    let text = export(original, &options);
                    let parsed = parse(&text).unwrap_or_else(|e| {
                        panic!(
                            "[{label}] {shell:?} multiline={multiline}: parse failed: {e}\n{text}"
                        )
                    });
                    let back = parsed.to_request_spec(false);
                    assert_eq!(
                        canon(&back),
                        canon(original),
                        "[{label}] {shell:?} multiline={multiline} {form_style:?}\n{text}\nparsed: {parsed:#?}"
                    );
                }
            }
        }
    }
}

/// 書き出しで出る警告は、意図したものだけ（例: Cookie ヘッダーの読み替えなど）。
#[test]
fn exported_commands_parse_without_warnings() {
    for (label, original) in &cases() {
        let text = export(original, &ExportOptions::default());
        let parsed = parse(&text).unwrap();
        assert!(
            parsed.warnings.is_empty(),
            "[{label}] {:?}\n{text}",
            parsed.warnings
        );
    }
}

/// 書き出しは展開済みの値が前提。展開前の `{{baseUrl}}` を渡したときは、core が送る URL と同じにエンコードされる
/// （変数のまま出す機能はない）。`{{baseUrl}}` で始まる cURL は読める。
#[test]
fn base_url_variable_round_trip() {
    let mut s = spec("GET", "{{baseUrl}}/api/v1/clients/{{clientId}}");
    s.params.push(row(ParamIn::Query, "q", "{{q}}"));
    let text = export(&s, &ExportOptions::default());
    let parsed = parse(&text).unwrap();
    // `{` `}` は curl の展開を避けるためエンコードされる（core が送る URL と同じ）
    assert_eq!(parsed.base, "http://%7B%7BbaseUrl%7D%7D");
    // 変数のまま出したいときは、呼び出し側で展開してから export する
    let text = "curl '{{baseUrl}}/api/v1/clients/1?q=2'";
    let parsed = parse(text).unwrap();
    assert_eq!(parsed.base, "{{baseUrl}}");
    assert_eq!(
        parsed.to_request_spec(false).url,
        "{{baseUrl}}/api/v1/clients/1"
    );
}
