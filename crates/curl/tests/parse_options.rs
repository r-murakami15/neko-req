//! オプションごとの読み方（method の決め方、-d の連結、-G、-F、-u、-b、ヘッダーの形など）。

mod common;

use common::*;
use nekoreq_core::{AuthOverride, Body, MultipartPart, MultipartValue, ParamIn};
use nekoreq_curl::WarningKind;

// ---- method ----

#[test]
fn method_rules_follow_curl() {
    assert_eq!(p("curl http://x/a").method, "GET");
    assert_eq!(p("curl http://x/a -d a=1").method, "POST");
    assert_eq!(p("curl http://x/a --json '{}'").method, "POST");
    assert_eq!(p("curl http://x/a -F a=1").method, "POST");
    assert_eq!(p("curl http://x/a -X put -d a=1").method, "PUT");
    assert_eq!(p("curl http://x/a -d a=1 -G").method, "GET");
    assert_eq!(p("curl -I http://x/a").method, "HEAD");
    assert_eq!(p("curl -XDELETE http://x/a").method, "DELETE");
    assert_eq!(p("curl --request=PATCH http://x/a").method, "PATCH");
    // -X GET でもボディは残す（curl と同じ）
    let c = p("curl -X GET http://x/a -d a=1");
    assert_eq!(c.method, "GET");
    assert!(matches!(c.body, Body::Form(_)));
}

#[test]
fn curl_prefix_is_optional() {
    assert_eq!(p("http://x/a").path, "/a");
    assert_eq!(p("-X POST http://x/a").method, "POST");
    assert_eq!(p("$ curl http://x/a").path, "/a");
    assert_eq!(p("TOKEN=1 curl http://x/a").path, "/a");
    assert_eq!(p("curl.exe http://x/a").path, "/a");
}

// ---- URL ----

#[test]
fn url_is_split_into_base_path_and_query() {
    let c = p("curl 'https://api.example.com:8443/v1/items/42?a=1&b=x%20y&c=&d#frag'");
    assert_eq!(c.base, "https://api.example.com:8443");
    assert_eq!(c.path, "/v1/items/42");
    assert_eq!(
        pairs(&c.query),
        [("a", "1"), ("b", "x y"), ("c", ""), ("d", "")]
    );
    assert_eq!(c.url(), "https://api.example.com:8443/v1/items/42");
}

#[test]
fn url_forms() {
    let c = p("curl --url http://x/a");
    assert_eq!(c.base, "http://x");
    // 位置引数と --url の両方があれば、`://` のあるものを優先し、残りは警告
    let c = p("curl http://first/a --url http://second/b");
    assert_eq!(c.base, "http://first");
    assert_eq!(c.warnings.len(), 1);
    // スキームなしは http:// を補う
    let c = p("curl localhost:8080/api/v1/x");
    assert_eq!(c.base, "http://localhost:8080");
    assert!(c.warnings.iter().any(|w| w.kind == WarningKind::Adjusted));
    // ホストだけ
    let c = p("curl http://localhost:8080");
    assert_eq!(
        (c.base.as_str(), c.path.as_str()),
        ("http://localhost:8080", "")
    );
    // IPv6
    assert_eq!(p("curl 'http://[::1]:8080/a'").base, "http://[::1]:8080");
    // 変数で始まる URL
    let c = p("curl '{{baseUrl}}/api/v1/clients/{{clientId}}?q={{q}}'");
    assert_eq!(c.base, "{{baseUrl}}");
    assert_eq!(c.path, "/api/v1/clients/{{clientId}}");
    assert_eq!(pairs(&c.query), [("q", "{{q}}")]);
    // クエリに `?` と `://` を含む
    let c = p("curl 'http://x/a?next=http://y/b?c=d'");
    assert_eq!(c.path, "/a");
    assert_eq!(pairs(&c.query), [("next", "http://y/b?c=d")]);
}

#[test]
fn url_userinfo_becomes_basic_auth() {
    let c = p("curl 'http://neko%40x:p%3Ass@localhost:8080/a'");
    assert_eq!(c.base, "http://localhost:8080");
    assert_eq!(
        c.auth,
        Some(AuthOverride::Basic {
            username: "neko@x".into(),
            password: "p:ss".into()
        })
    );
}

#[test]
fn non_http_scheme_is_warned() {
    let c = p("curl ftp://x/a");
    assert!(c
        .warnings
        .iter()
        .any(|w| w.kind == WarningKind::Unsupported && w.message.contains("ftp")));
}

// ---- -H ----

#[test]
fn header_forms() {
    let c = p(
        r#"curl http://x/a -H 'X-Time: 12:30:00' -H 'X-Url:http://y:80/z' -H 'X-Empty;' -H 'X-Spaces:    padded   ' -H 'x-lower: v'"#,
    );
    assert_eq!(header(&c, "X-Time"), Some("12:30:00"));
    assert_eq!(header(&c, "X-Url"), Some("http://y:80/z"));
    assert_eq!(header(&c, "X-Empty"), Some(""));
    assert_eq!(header(&c, "X-Spaces"), Some("padded"));
    assert_eq!(
        header_names(&c),
        ["X-Time", "X-Url", "X-Empty", "X-Spaces", "x-lower"]
    );
    assert!(c
        .headers
        .iter()
        .all(|h| h.location == ParamIn::Header && h.enabled));
    assert!(c.warnings.is_empty());
}

#[test]
fn header_edge_cases_warn() {
    // `Name:`（値なし）は curl ではヘッダーを消す指定。「名前: 値」でないものは捨てる
    let c = p("curl http://x/a -H 'Accept:' -H 'garbage' -H ':novalue' -H 'Content-Length: 5'");
    assert!(c.headers.is_empty());
    assert_eq!(warning_options(&c, WarningKind::Ignored).len(), 4);
}

#[test]
fn duplicate_headers_are_kept_in_order() {
    let c = p("curl http://x/a -H 'X-A: 1' -H 'X-A: 2'");
    assert_eq!(pairs(&c.headers), [("X-A", "1"), ("X-A", "2")]);
}

#[test]
fn referer_and_user_agent_options_become_headers() {
    let c = p("curl http://x/a -e http://ref/ -A 'neko/1.0'");
    assert_eq!(header(&c, "Referer"), Some("http://ref/"));
    assert_eq!(header(&c, "User-Agent"), Some("neko/1.0"));
}

// ---- Cookie ----

#[test]
fn cookie_header_and_b_together() {
    // curl は Cookie ヘッダーを手で指定すると -b の値を送らない
    let c = p("curl http://x/a -H 'Cookie: a=1' -b 'b=2'");
    assert_eq!(pairs(&c.cookies), [("a", "1")]);
    assert!(c
        .warnings
        .iter()
        .any(|w| w.option == "-b" && w.kind == WarningKind::Adjusted));
    // 逆順でも同じ
    let c = p("curl http://x/a -b 'b=2' -H 'Cookie: a=1'");
    assert_eq!(pairs(&c.cookies), [("a", "1")]);
}

#[test]
fn cookie_b_forms() {
    let c = p("curl http://x/a -b 'a=1; b=x=y;c=' --cookie 'd=4'");
    assert_eq!(
        pairs(&c.cookies),
        [("a", "1"), ("b", "x=y"), ("c", ""), ("d", "4")]
    );
    // ファイル名（`=` なし）は読めない
    let c = p("curl http://x/a -b cookies.txt");
    assert!(c.cookies.is_empty());
    assert_eq!(warning_options(&c, WarningKind::Unsupported), ["-b"]);
}

#[test]
fn cookie_header_that_is_not_name_value_stays_a_header() {
    let c = p("curl http://x/a -H 'Cookie: weird'");
    assert!(c.cookies.is_empty());
    assert_eq!(header(&c, "Cookie"), Some("weird"));
}

// ---- 認証 ----

#[test]
fn user_option_is_basic() {
    let c = p("curl -u neko:p@ss:word http://x/a");
    assert_eq!(
        c.auth,
        Some(AuthOverride::Basic {
            username: "neko".into(),
            password: "p@ss:word".into()
        })
    );
    let c = p("curl --user=neko http://x/a");
    assert_eq!(
        c.auth,
        Some(AuthOverride::Basic {
            username: "neko".into(),
            password: String::new()
        })
    );
    assert_eq!(warning_options(&c, WarningKind::Adjusted), ["-u"]);
}

#[test]
fn authorization_header_bearer_and_basic_become_auth() {
    let c = p("curl http://x/a -H 'Authorization: Bearer abc'");
    assert_eq!(
        c.auth,
        Some(AuthOverride::Bearer {
            token: "abc".into()
        })
    );
    assert!(c.headers.is_empty());
    let c = p("curl http://x/a -H 'authorization: bearer {{token}}'");
    assert_eq!(
        c.auth,
        Some(AuthOverride::Bearer {
            token: "{{token}}".into()
        })
    );
    // Basic は user:pass に戻せるときだけ（bmVrbzpzZWNyZXQ= は neko:secret）
    let c = p("curl http://x/a -H 'Authorization: Basic bmVrbzpzZWNyZXQ='");
    assert_eq!(
        c.auth,
        Some(AuthOverride::Basic {
            username: "neko".into(),
            password: "secret".into()
        })
    );
    // それ以外の方式、読めない Basic はヘッダーのまま
    for h in [
        "Authorization: Digest username=\"a\"",
        "Authorization: Token abc",
        "Authorization: Basic !!!",
        "Authorization: Bearer",
        "Authorization: abc",
    ] {
        let c = p(&format!("curl http://x/a -H '{h}'"));
        assert_eq!(c.auth, None, "{h}");
        assert_eq!(c.headers.len(), 1, "{h}");
    }
}

#[test]
fn authorization_header_wins_over_user_option() {
    let c = p("curl -u a:b http://x/a -H 'Authorization: Bearer t'");
    assert_eq!(c.auth, Some(AuthOverride::Bearer { token: "t".into() }));
    assert!(c.warnings.iter().any(|w| w.option == "-u"));
}

#[test]
fn oauth2_bearer_option() {
    let c = p("curl --oauth2-bearer tok http://x/a");
    assert_eq!(
        c.auth,
        Some(AuthOverride::Bearer {
            token: "tok".into()
        })
    );
}

// ---- ボディ: -d 系 ----

#[test]
fn multiple_d_are_joined_with_ampersand_into_form_rows() {
    let c = p("curl http://x/a -d 'a=1' -d 'b=2&c=%E3%81%82' --data 'd=x+y'");
    assert_eq!(
        c.body,
        Body::Form(vec![
            kv(ParamIn::Query, "a", "1"),
            kv(ParamIn::Query, "b", "2"),
            kv(ParamIn::Query, "c", "あ"),
            kv(ParamIn::Query, "d", "x y"),
        ])
    );
    assert!(c.headers.is_empty());
}

#[test]
fn d_with_get_goes_to_query() {
    let c = p("curl -G http://x/a?z=0 -d 'a=1' -d 'b=2&c=3' --data-urlencode 'q=日本 語'");
    assert_eq!(c.method, "GET");
    assert_eq!(c.body, Body::None);
    assert_eq!(
        pairs(&c.query),
        [
            ("z", "0"),
            ("a", "1"),
            ("b", "2"),
            ("c", "3"),
            ("q", "日本 語")
        ]
    );
}

#[test]
fn d_that_is_not_name_value_stays_text_with_form_content_type() {
    // curl は -d の Content-Type を x-www-form-urlencoded にする。name=value に分けられないので文字のまま送る
    let c = p(r#"curl http://x/a -d '{"a":1}'"#);
    assert_eq!(c.body, Body::Text(r#"{"a":1}"#.into()));
    assert_eq!(
        header(&c, "Content-Type"),
        Some("application/x-www-form-urlencoded")
    );
}

#[test]
fn json_content_type_makes_json_body() {
    let c = p(r#"curl http://x/a -H 'content-type: Application/JSON' -d '{"a":1}' -d '{"b":2}'"#);
    // -d の連結は `&`
    assert_eq!(c.body, Body::Json(r#"{"a":1}&{"b":2}"#.into()));
    assert!(c.headers.is_empty());
    // +json の種類は Content-Type を残す
    let c = p(r#"curl http://x/a -H 'Content-Type: application/vnd.api+json' -d '{}'"#);
    assert!(matches!(c.body, Body::Json(_)));
    assert_eq!(header(&c, "Content-Type"), Some("application/vnd.api+json"));
}

#[test]
fn json_option() {
    let c = p(r#"curl http://x/a --json '{"a":1}' --json '{"b":2}'"#);
    assert_eq!(c.method, "POST");
    // --json は連結（& なし）で、Accept も JSON にする
    assert_eq!(c.body, Body::Json(r#"{"a":1}{"b":2}"#.into()));
    assert_eq!(header(&c, "Accept"), Some("application/json"));
    assert!(header(&c, "Content-Type").is_none());
    // 手で指定した Accept は上書きしない
    let c = p(r#"curl http://x/a --json '{}' -H 'Accept: text/csv'"#);
    assert_eq!(header(&c, "Accept"), Some("text/csv"));
    // ファイルは Binary にして、Content-Type を残す
    let c = p("curl http://x/a --json @body.json");
    assert_eq!(c.body, Body::Binary("body.json".into()));
    assert_eq!(header(&c, "Content-Type"), Some("application/json"));
}

#[test]
fn other_content_type_makes_text() {
    let c = p("curl http://x/a -H 'Content-Type: application/xml' -d '<a/>'");
    assert_eq!(c.body, Body::Text("<a/>".into()));
    assert_eq!(header(&c, "Content-Type"), Some("application/xml"));
    // core が付ける値と同じなら外す
    let c = p("curl http://x/a -H 'Content-Type: text/plain; charset=utf-8' --data-raw 'hello'");
    assert_eq!(c.body, Body::Text("hello".into()));
    assert!(c.headers.is_empty());
}

#[test]
fn data_raw_does_not_read_files() {
    let c = p("curl http://x/a -H 'Content-Type: text/plain' --data-raw '@not-a-file'");
    assert_eq!(c.body, Body::Text("@not-a-file".into()));
}

#[test]
fn data_at_file_is_binary_path() {
    let c = p("curl http://x/a -d @payload.json");
    assert_eq!(c.body, Body::Binary("payload.json".into()));
    let c = p(
        "curl http://x/a --data-binary '@/tmp/a b.bin' -H 'Content-Type: application/octet-stream'",
    );
    assert_eq!(c.body, Body::Binary("/tmp/a b.bin".into()));
    assert!(c.headers.is_empty());
    // 標準入力は読めない
    let c = p("curl http://x/a -d @-");
    assert_eq!(c.body, Body::None);
    assert_eq!(warning_options(&c, WarningKind::Unsupported), ["-d"]);
}

#[test]
fn data_urlencode_forms() {
    // name=content / =content / content / name@file / @file（curl の規則）
    let c = p("curl http://x/a --data-urlencode 'a=b c&d' --data-urlencode 'e=f=g'");
    assert_eq!(
        c.body,
        Body::Form(vec![
            kv(ParamIn::Query, "a", "b c&d"),
            kv(ParamIn::Query, "e", "f=g"),
        ])
    );
    // 名前なしは、エンコードした文字そのままのテキスト
    let c = p("curl http://x/a --data-urlencode 'hello world' --data-urlencode '=a&b'");
    assert_eq!(c.body, Body::Text("hello%20world&a%26b".into()));
}

#[test]
fn data_urlencode_name_at_file() {
    let c = p("curl http://x/a --data-urlencode 'comment@./comment.txt' -d 'x=1'");
    let Body::Form(rows) = &c.body else {
        panic!("{:?}", c.body)
    };
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].name, "comment");
    assert_eq!(rows[0].value, "@./comment.txt");
    assert!(!rows[0].enabled, "中身を読めないので無効の行にする");
    assert!(rows[1].enabled);
    assert_eq!(
        warning_options(&c, WarningKind::Unsupported),
        ["--data-urlencode"]
    );
}

#[test]
fn d_and_form_together_prefers_form() {
    let c = p("curl http://x/a -d a=1 -F b=2");
    assert!(matches!(c.body, Body::Multipart(_)));
    assert!(c.warnings.iter().any(|w| w.option == "-F"));
}

// ---- ボディ: -F ----

#[test]
fn form_with_file_and_type() {
    let c = p("curl http://x/a -F 'file=@a.png;type=image/png' -F 'name=neko' -F 'meta={\"a\":1};type=application/json' -F 'x=@\"we;ird,name.txt\"'");
    assert_eq!(
        c.body,
        Body::Multipart(vec![
            MultipartPart {
                name: "file".into(),
                value: MultipartValue::File("a.png".into()),
                enabled: true,
                content_type: Some("image/png".into()),
            },
            MultipartPart {
                name: "name".into(),
                value: MultipartValue::Text("neko".into()),
                enabled: true,
                content_type: None,
            },
            MultipartPart {
                name: "meta".into(),
                value: MultipartValue::Text("{\"a\":1}".into()),
                enabled: true,
                content_type: Some("application/json".into()),
            },
            MultipartPart {
                name: "x".into(),
                value: MultipartValue::File("we;ird,name.txt".into()),
                enabled: true,
                content_type: None,
            },
        ])
    );
    // `;type=` はパートの Content-Type として保持するので、警告は出ない
    assert!(
        !c.warnings
            .iter()
            .any(|w| w.kind == WarningKind::Unsupported),
        "{:?}",
        c.warnings
    );
}

#[test]
fn form_string_and_text_values() {
    let c = p(
        "curl http://x/a --form-string 'a=@literal;type=x' -F 'b=a;b' -F 'c=' -F 'd=@' -F broken",
    );
    let Body::Multipart(parts) = &c.body else {
        panic!("{:?}", c.body)
    };
    assert_eq!(
        parts[0].value,
        MultipartValue::Text("@literal;type=x".into())
    );
    assert_eq!(parts[1].value, MultipartValue::Text("a;b".into()));
    assert_eq!(parts[2].value, MultipartValue::Text(String::new()));
    assert_eq!(parts[3].value, MultipartValue::File(String::new()));
    assert_eq!(parts.len(), 4);
    assert!(c.warnings.iter().any(|w| w.message.contains("broken")));
}

#[test]
fn form_drops_manual_content_type() {
    let c = p("curl http://x/a -F a=1 -H 'Content-Type: multipart/form-data'");
    assert!(c.headers.is_empty());
}

// ---- 読み捨てるオプション ----

#[test]
fn ignored_options_are_listed() {
    let c = p("curl -sSLk -o out.json -v -i --compressed --max-time 5 --retry 3 http://x/a");
    assert_eq!(
        warning_options(&c, WarningKind::Ignored),
        [
            "-s",
            "-S",
            "-L",
            "-k",
            "-o",
            "-v",
            "-i",
            "--compressed",
            "--max-time",
            "--retry"
        ]
    );
    assert!(c.warnings.iter().all(|w| !w.message.is_empty()));
    assert_eq!(c.method, "GET");
}

#[test]
fn unknown_options_are_warned() {
    let c = p("curl --frobnicate -Q http://x/a --no-sessionid --weird=1");
    assert_eq!(
        warning_options(&c, WarningKind::Unknown),
        ["--frobnicate", "-Q", "--weird"]
    );
    assert_eq!(
        warning_options(&c, WarningKind::Ignored),
        ["--no-sessionid"]
    );
    assert_eq!(c.path, "/a");
}

#[test]
fn attached_short_options() {
    let c = p("curl -XPOST -H'X-A: 1' -d'a=1' -uneko:pw http://x/a");
    assert_eq!(c.method, "POST");
    assert_eq!(header(&c, "X-A"), Some("1"));
    assert_eq!(c.body, Body::Form(vec![kv(ParamIn::Query, "a", "1")]));
    assert!(matches!(c.auth, Some(AuthOverride::Basic { .. })));
}

#[test]
fn long_options_with_equals() {
    let c = p("curl --request=POST --header='X-A: 1' --data='a=1' --url=http://x/a");
    assert_eq!(c.method, "POST");
    assert_eq!(header(&c, "X-A"), Some("1"));
    assert_eq!(c.path, "/a");
}

#[test]
fn pipes_and_redirects_end_the_command() {
    let c = p("curl -s http://x/a | jq .");
    assert_eq!(c.path, "/a");
    assert!(c.warnings.iter().any(|w| w.option == "|"));
}

// ---- RequestSpec への変換 ----

#[test]
fn to_request_spec_replaces_base_on_request() {
    let c = p("curl 'http://localhost:8080/api/v1/clients?a=1' -H 'X-A: 1' -b 'c=2' -u u:p");
    let plain = c.to_request_spec(false);
    assert_eq!(plain.url, "http://localhost:8080/api/v1/clients");
    let replaced = c.to_request_spec(true);
    assert_eq!(replaced.url, "{{baseUrl}}/api/v1/clients");
    assert_eq!(replaced.method, "GET");
    assert_eq!(
        replaced.params,
        vec![
            kv(ParamIn::Query, "a", "1"),
            kv(ParamIn::Header, "X-A", "1"),
            kv(ParamIn::Cookie, "c", "2"),
        ]
    );
    assert_eq!(nekoreq_core::RequestSpec::from(&c), plain);
}

#[test]
fn base_matching_against_environment_base_url() {
    let c = p("curl http://LocalHost:8080/api/v1/clients/1");
    assert!(c.matches_base("http://localhost:8080"));
    assert!(c.matches_base("http://localhost:8080/"));
    assert!(!c.matches_base("http://localhost:8081"));
    assert!(!c.matches_base("https://localhost:8080"));
    assert_eq!(
        c.to_request_spec_for_base("http://localhost:8080")
            .unwrap()
            .url,
        "{{baseUrl}}/api/v1/clients/1"
    );
    // baseUrl にパスがあるときは、そのパスの分も baseUrl に含める
    assert_eq!(
        c.to_request_spec_for_base("http://localhost:8080/api")
            .unwrap()
            .url,
        "{{baseUrl}}/v1/clients/1"
    );
    assert_eq!(
        c.path_after_base("http://localhost:8080/api/").as_deref(),
        Some("/v1/clients/1")
    );
    assert!(c
        .to_request_spec_for_base("http://localhost:8080/ap")
        .is_none());
    // 既定のポートは省いても同じ
    assert!(p("curl https://x.example/a").matches_base("https://x.example:443"));
    assert!(p("curl http://x.example:80/a").matches_base("http://x.example"));
}
