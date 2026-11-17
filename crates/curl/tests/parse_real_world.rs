//! 実際に出回っている形（ブラウザの「Copy as cURL」、Postman、設計書の画面例）の取り込み。

mod common;

use common::*;
use nekoreq_core::{AuthOverride, Body, MultipartPart, MultipartValue, ParamIn};
use nekoreq_curl::{parse_as, Shell, WarningKind};

/// docs/screens/NewRequest.dc.html の「cURL コマンドから作成」の例。
#[test]
fn new_request_screen_example() {
    let c = p(
        r#"curl -X POST http://localhost:8080/api/v1/auth/password-login \
  -H 'Content-Type: application/json' \
  -d '{"email":"user@example.com","password":"********"}'"#,
    );
    assert_eq!(c.method, "POST");
    assert_eq!(c.base, "http://localhost:8080");
    assert_eq!(c.path, "/api/v1/auth/password-login");
    assert!(c.query.is_empty());
    // Content-Type は JSON ボディから core が付けるので、ヘッダーには残さない
    assert!(c.headers.is_empty(), "{:?}", c.headers);
    assert_eq!(
        compact(json_text(&c)),
        r#"{"email":"user@example.com","password":"********"}"#
    );
    assert!(c.warnings.is_empty());
    // 画面の要約: POST /api/v1/auth/password-login · JSON ボディ
    let spec = c.to_request_spec(true);
    assert_eq!(spec.url, "{{baseUrl}}/api/v1/auth/password-login");
    assert_eq!(spec.method, "POST");
}

#[test]
fn chrome_copy_as_curl_bash() {
    let c = p(
        r#"curl 'http://localhost:8080/api/v1/video/clients?page=1&size=20&keyword=%E6%97%A5%E6%9C%AC' \
  -H 'Accept: application/json, text/plain, */*' \
  -H 'Accept-Language: ja,en-US;q=0.9,en;q=0.8' \
  -H 'Connection: keep-alive' \
  -b 'SESSION=abc123; XSRF-TOKEN=tok-1' \
  -H 'Referer: http://localhost:3000/' \
  -H 'User-Agent: Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36' \
  -H 'sec-ch-ua: "Chromium";v="130", "Not?A_Brand";v="99"' \
  -H 'sec-ch-ua-mobile: ?0' \
  --compressed"#,
    );
    assert_eq!(c.method, "GET");
    assert_eq!(c.base, "http://localhost:8080");
    assert_eq!(c.path, "/api/v1/video/clients");
    assert_eq!(
        pairs(&c.query),
        [("page", "1"), ("size", "20"), ("keyword", "日本")]
    );
    assert!(c
        .query
        .iter()
        .all(|r| r.location == ParamIn::Query && r.enabled));
    assert_eq!(
        pairs(&c.cookies),
        [("SESSION", "abc123"), ("XSRF-TOKEN", "tok-1")]
    );
    assert!(c.cookies.iter().all(|r| r.location == ParamIn::Cookie));
    assert_eq!(
        header(&c, "sec-ch-ua"),
        Some(r#""Chromium";v="130", "Not?A_Brand";v="99""#)
    );
    assert_eq!(
        header(&c, "Accept-Language"),
        Some("ja,en-US;q=0.9,en;q=0.8")
    );
    assert_eq!(c.headers.len(), 7);
    assert_eq!(warning_options(&c, WarningKind::Ignored), ["--compressed"]);
}

/// Chrome は JSON に非 ASCII があると `$'...'` と `あ`（UTF-16 のサロゲートの組を含む）で出す。
#[test]
fn chrome_copy_as_curl_bash_ansi_c_unicode() {
    let c = p(r#"curl 'http://localhost:8080/api/v1/notes' \
  -H 'content-type: application/json' \
  --data-raw $'{"title":"日本語","memo":"it\'s 😀","line":"a\\nb"}'"#);
    assert_eq!(c.method, "POST");
    assert_eq!(
        json_text(&c),
        r#"{"title":"日本語","memo":"it's 😀","line":"a\nb"}"#
    );
    assert!(c.headers.is_empty());
}

#[test]
fn chrome_copy_as_curl_cmd_with_carets() {
    let cmd = "curl ^\"http://localhost:8080/api/v1/auth/password-login^\" ^\r\n  -H ^\"Accept: application/json^\" ^\r\n  -H ^\"Content-Type: application/json^\" ^\r\n  -b ^\"SESSION=abc123^\" ^\r\n  --data-raw ^\"^{^\\^\"email^\\^\":^\\^\"user@example.com^\\^\",^\\^\"password^\\^\":^\\^\"p@ss^%^&^\\^\"^}^\" ^\r\n  --compressed";
    let c = p(cmd);
    assert_eq!(c.method, "POST");
    assert_eq!(c.path, "/api/v1/auth/password-login");
    assert_eq!(
        json_text(&c),
        r#"{"email":"user@example.com","password":"p@ss%&"}"#
    );
    assert_eq!(pairs(&c.cookies), [("SESSION", "abc123")]);
    assert_eq!(header(&c, "Accept"), Some("application/json"));
    assert_eq!(warning_options(&c, WarningKind::Ignored), ["--compressed"]);
}

/// 古い Chrome・Firefox の cmd 用: `"` を `""` にし、行末は `^`。
#[test]
fn firefox_copy_as_curl_cmd_double_quotes() {
    let cmd = "curl \"http://localhost:8080/api/v1/auth/password-login\" ^\r\n  -X POST ^\r\n  -H \"Content-Type: application/json\" ^\r\n  -H \"Accept: */*\" ^\r\n  --data-raw \"{\"\"email\"\":\"\"user@example.com\"\",\"\"memo\"\":\"\"\"\"}\"";
    let c = p(cmd);
    assert_eq!(c.method, "POST");
    assert_eq!(json_text(&c), r#"{"email":"user@example.com","memo":""}"#);
    assert_eq!(header(&c, "Accept"), Some("*/*"));
}

/// 1 行だけの cmd（行継続の `^` がない）でも、`""` の使い方で cmd と見分ける。
#[test]
fn firefox_cmd_single_line_is_detected() {
    let c = p("curl \"http://localhost:8080/x\" -X POST -H \"Content-Type: application/json\" --data-raw \"{\"\"a\"\":1}\"");
    assert_eq!(json_text(&c), r#"{"a":1}"#);
}

#[test]
fn bash_json_with_empty_string_is_not_mistaken_for_cmd() {
    let c = p(
        r#"curl http://localhost:8080/x -H 'Content-Type: application/json' -d '{"a":"","b":""}'"#,
    );
    assert_eq!(json_text(&c), r#"{"a":"","b":""}"#);
}

#[test]
fn explicit_shell_selection() {
    let c = parse_as("curl \"http://x/a\" -d \"a=1\"", Shell::Cmd).unwrap();
    assert!(matches!(c.body, Body::Form(_)));
}

#[test]
fn firefox_copy_as_curl_bash() {
    let c = p(
        r#"curl 'http://localhost:8080/api/v1/video/clients' -X POST -H 'User-Agent: Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0' -H 'Accept: application/json' -H 'Accept-Encoding: gzip, deflate, br, zstd' -H 'Content-Type: application/json' -H 'Cookie: SESSION=abc; theme=dark' -H 'Sec-Fetch-Mode: cors' --data-raw '{"name":"株式会社ねこ"}'"#,
    );
    assert_eq!(c.method, "POST");
    assert_eq!(pairs(&c.cookies), [("SESSION", "abc"), ("theme", "dark")]);
    assert!(header(&c, "Cookie").is_none());
    assert_eq!(json_text(&c), r#"{"name":"株式会社ねこ"}"#);
    assert_eq!(
        header(&c, "Accept-Encoding"),
        Some("gzip, deflate, br, zstd")
    );
}

#[test]
fn postman_raw_json() {
    let c = p(
        r#"curl --location 'http://localhost:8080/api/v1/video/clients' \
--header 'Content-Type: application/json' \
--header 'X-XSRF-TOKEN: {{xsrfToken}}' \
--data '{
    "name": "ねこ商事",
    "tags": ["a", "b"]
}'"#,
    );
    assert_eq!(c.method, "POST");
    assert_eq!(header(&c, "X-XSRF-TOKEN"), Some("{{xsrfToken}}"));
    // 改行と字下げはそのまま残す
    assert_eq!(
        json_text(&c),
        "{\n    \"name\": \"ねこ商事\",\n    \"tags\": [\"a\", \"b\"]\n}"
    );
    assert_eq!(warning_options(&c, WarningKind::Ignored), ["--location"]);
}

#[test]
fn postman_form_data_with_file() {
    let c = p(r#"curl --location 'http://localhost:8080/api/v1/upload' \
--header 'Authorization: Bearer abc.def.ghi' \
--form 'title="契約書"' \
--form 'file=@"/Users/neko/Documents/契約 書.pdf"' \
--form 'note="a;b"'"#);
    assert_eq!(c.method, "POST");
    assert_eq!(
        c.auth,
        Some(AuthOverride::Bearer {
            token: "abc.def.ghi".into()
        })
    );
    assert!(c.headers.is_empty());
    assert_eq!(
        c.body,
        Body::Multipart(vec![
            MultipartPart {
                name: "title".into(),
                value: MultipartValue::Text("契約書".into()),
                enabled: true,
                content_type: None,
            },
            MultipartPart {
                name: "file".into(),
                value: MultipartValue::File("/Users/neko/Documents/契約 書.pdf".into()),
                enabled: true,
                content_type: None,
            },
            MultipartPart {
                name: "note".into(),
                value: MultipartValue::Text("a;b".into()),
                enabled: true,
                content_type: None,
            },
        ])
    );
}

#[test]
fn postman_urlencoded() {
    let c = p(r#"curl --location 'http://localhost:8080/oauth/token' \
--header 'Content-Type: application/x-www-form-urlencoded' \
--data-urlencode 'grant_type=password' \
--data-urlencode 'username=neko@example.com' \
--data-urlencode 'password=p w&d'"#);
    assert_eq!(
        c.body,
        Body::Form(vec![
            kv(ParamIn::Query, "grant_type", "password"),
            kv(ParamIn::Query, "username", "neko@example.com"),
            kv(ParamIn::Query, "password", "p w&d"),
        ])
    );
    // core は Form のとき同じ Content-Type を付ける
    assert!(c.headers.is_empty());
}

/// Chrome が、ファイルなしの multipart を `--data-raw` に展開した形。
#[test]
fn chrome_multipart_expanded_as_data_raw() {
    let c = p(r#"curl 'http://localhost:8080/api/v1/upload' \
  -H 'Content-Type: multipart/form-data; boundary=----WebKitFormBoundaryAbC123' \
  --data-raw $'------WebKitFormBoundaryAbC123\r\nContent-Disposition: form-data; name="title"\r\n\r\n契約書\r\n------WebKitFormBoundaryAbC123\r\nContent-Disposition: form-data; name="file"; filename="a.png"\r\nContent-Type: image/png\r\n\r\n\r\n------WebKitFormBoundaryAbC123--\r\n'"#);
    let Body::Multipart(parts) = &c.body else {
        panic!("{:?}", c.body)
    };
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].name, "title");
    assert_eq!(parts[0].value, MultipartValue::Text("契約書".into()));
    assert!(parts[0].enabled);
    assert_eq!(parts[1].value, MultipartValue::File("a.png".into()));
    assert!(
        !parts[1].enabled,
        "中身がないファイルのパートは無効の行にする"
    );
    assert!(c.headers.is_empty());
    assert_eq!(
        warning_options(&c, WarningKind::Unsupported),
        ["--data-raw"]
    );
}

#[test]
fn japanese_and_emoji_values() {
    let c = p(
        r#"curl 'http://localhost:8080/api/v1/検索?q=猫%F0%9F%90%88&tag=犬' -H 'X-Memo: 🐱 ねこ' -d '{"name":"🐈 みけ"}' -H 'Content-Type: application/json; charset=utf-8'"#,
    );
    assert_eq!(c.path, "/api/v1/検索");
    assert_eq!(pairs(&c.query), [("q", "猫🐈"), ("tag", "犬")]);
    assert_eq!(header(&c, "X-Memo"), Some("🐱 ねこ"));
    // charset 付きの Content-Type は core の自動の値と違うので残す
    assert_eq!(
        header(&c, "Content-Type"),
        Some("application/json; charset=utf-8")
    );
    assert_eq!(json_text(&c), r#"{"name":"🐈 みけ"}"#);
}

#[test]
fn nested_quotes() {
    // 'in "double"'、"in 'single'"、'it'\''s'、"\"escaped\""
    let c = p(
        r#"curl http://x/a -H 'X-A: say "hi"' -H "X-B: it's" -H 'X-C: it'\''s' -H "X-D: \"q\" \\ \$" -H 'X-E: a'"b"c"#,
    );
    assert_eq!(header(&c, "X-A"), Some(r#"say "hi""#));
    assert_eq!(header(&c, "X-B"), Some("it's"));
    assert_eq!(header(&c, "X-C"), Some("it's"));
    assert_eq!(header(&c, "X-D"), Some(r#""q" \ $"#));
    assert_eq!(header(&c, "X-E"), Some("abc"));
}

#[test]
fn json_in_double_quotes_with_escapes() {
    let c = p(
        r#"curl http://x/a -H "Content-Type: application/json" -d "{\"a\": \"it's\", \"b\": \"x\\\\y\"}""#,
    );
    assert_eq!(json_text(&c), r#"{"a": "it's", "b": "x\\y"}"#);
}
