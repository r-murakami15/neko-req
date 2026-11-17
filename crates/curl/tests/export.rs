//! 書き出し（bash・cmd、伏せ字、フォームの出し方）。

mod common;

use common::*;
use nekoreq_core::{
    ApiKeyIn, AuthOverride, Body, MultipartPart, MultipartValue, ParamIn, RequestSpec,
};
use nekoreq_curl::{export, ExportOptions, FormStyle, Shell};

fn spec(method: &str, url: &str) -> RequestSpec {
    RequestSpec {
        method: method.into(),
        url: url.into(),
        params: vec![],
        body: Body::None,
        auth: None,
    }
}

fn bash() -> ExportOptions {
    ExportOptions::default()
}

#[test]
fn simple_get_is_one_line() {
    let s = spec("GET", "http://localhost:8080/api/v1/auth/session");
    assert_eq!(
        export(&s, &bash()),
        "curl 'http://localhost:8080/api/v1/auth/session'"
    );
}

#[test]
fn post_json_matches_screen_example_layout() {
    let mut s = spec("POST", "http://localhost:8080/api/v1/auth/password-login");
    s.body = Body::Json(r#"{"email":"user@example.com","password":"********"}"#.into());
    assert_eq!(
        export(&s, &bash()),
        "curl -X POST 'http://localhost:8080/api/v1/auth/password-login' \\\n  \
         -H 'Content-Type: application/json' \\\n  \
         --data-raw '{\"email\":\"user@example.com\",\"password\":\"********\"}'"
    );
}

#[test]
fn single_line_when_multiline_is_off() {
    let mut s = spec("POST", "http://x/a");
    s.params.push(kv(ParamIn::Header, "X-A", "1"));
    s.body = Body::Json("{}".into());
    let o = ExportOptions {
        multiline: false,
        ..bash()
    };
    assert_eq!(
        export(&s, &o),
        "curl -X POST 'http://x/a' -H 'X-A: 1' -H 'Content-Type: application/json' --data-raw '{}'"
    );
}

#[test]
fn single_quotes_in_values_are_escaped() {
    let mut s = spec("POST", "http://x/a");
    s.params.push(kv(ParamIn::Header, "X-Name", "it's"));
    s.body = Body::Json(r#"{"a":"it's"}"#.into());
    let out = export(&s, &bash());
    assert!(out.contains(r"-H 'X-Name: it'\''s'"), "{out}");
    assert!(out.contains(r#"--data-raw '{"a":"it'\''s"}'"#), "{out}");
}

#[test]
fn control_characters_use_ansi_c_quoting() {
    let mut s = spec("POST", "http://x/a");
    s.body = Body::Text("a\r\nb\tc'd".into());
    let out = export(&s, &bash());
    assert!(out.contains(r"--data-raw $'a\r\nb\tc\'d'"), "{out}");
}

#[test]
fn query_rows_path_params_and_non_ascii_are_encoded_like_core() {
    let mut s = spec("GET", "http://x/api/v1/clients/{clientId}/日本語");
    s.params.push(kv(ParamIn::Path, "clientId", "a/b c"));
    s.params.push(kv(ParamIn::Query, "q", "猫 & 犬=1"));
    let mut disabled = kv(ParamIn::Query, "off", "1");
    disabled.enabled = false;
    s.params.push(disabled);
    let out = export(&s, &bash());
    assert_eq!(
        out,
        "curl 'http://x/api/v1/clients/a%2Fb%20c/%E6%97%A5%E6%9C%AC%E8%AA%9E?q=%E7%8C%AB%20%26%20%E7%8A%AC%3D1'"
    );
}

#[test]
fn scheme_is_added_and_existing_query_is_kept() {
    let mut s = spec("GET", "localhost:8080/a?x=1");
    s.params.push(kv(ParamIn::Query, "y", "2"));
    assert_eq!(
        export(&s, &bash()),
        "curl 'http://localhost:8080/a?x=1&y=2'"
    );
}

#[test]
fn methods() {
    assert_eq!(
        export(&spec("DELETE", "http://x/a"), &bash()),
        "curl -X DELETE 'http://x/a'"
    );
    assert_eq!(
        export(&spec("head", "http://x/a"), &bash()),
        "curl -I 'http://x/a'"
    );
    let mut s = spec("GET", "http://x/a");
    s.body = Body::Text("x".into());
    assert!(export(&s, &bash()).starts_with("curl -X GET "));
}

#[test]
fn auth_headers_and_cookies() {
    let mut s = spec("GET", "http://x/a");
    s.auth = Some(AuthOverride::Basic {
        username: "neko".into(),
        password: "p'w".into(),
    });
    assert_eq!(
        export(&s, &bash()),
        "curl 'http://x/a' \\\n  -u 'neko:p'\\''w'"
    );

    s.auth = Some(AuthOverride::Bearer {
        token: " tok ".into(),
    });
    assert_eq!(
        export(&s, &bash()),
        "curl 'http://x/a' \\\n  -H 'Authorization: Bearer tok'"
    );

    s.auth = Some(AuthOverride::ApiKey {
        location: ApiKeyIn::Query,
        name: "key".into(),
        value: "v 1".into(),
    });
    assert_eq!(export(&s, &bash()), "curl 'http://x/a?key=v%201'");

    s.auth = Some(AuthOverride::ApiKey {
        location: ApiKeyIn::Cookie,
        name: "SESSION".into(),
        value: "abc".into(),
    });
    s.params.push(kv(ParamIn::Cookie, "theme", "dark"));
    s.params.push(kv(ParamIn::Header, "Cookie", "manual=1"));
    assert_eq!(
        export(&s, &bash()),
        "curl 'http://x/a' \\\n  -b 'manual=1; theme=dark; SESSION=abc'"
    );

    // 手入力の Authorization があれば、認証の設定は使わない（core と同じ）
    s.auth = Some(AuthOverride::Bearer { token: "t".into() });
    s.params
        .push(kv(ParamIn::Header, "Authorization", "Token z"));
    assert!(export(&s, &bash()).contains("-H 'Authorization: Token z'"));
    assert!(!export(&s, &bash()).contains("Bearer"));
}

#[test]
fn empty_header_value_uses_semicolon_form() {
    let mut s = spec("GET", "http://x/a");
    s.params.push(kv(ParamIn::Header, "X-Empty", ""));
    assert_eq!(export(&s, &bash()), "curl 'http://x/a' \\\n  -H 'X-Empty;'");
}

#[test]
fn form_styles() {
    let mut s = spec("POST", "http://x/a");
    s.body = Body::Form(vec![
        kv(ParamIn::Query, "name", "日本 語&x"),
        kv(ParamIn::Query, "a=b", "c"),
        {
            let mut r = kv(ParamIn::Query, "off", "1");
            r.enabled = false;
            r
        },
    ]);
    assert_eq!(
        export(&s, &bash()),
        "curl -X POST 'http://x/a' \\\n  --data-urlencode 'name=日本 語&x' \\\n  -d 'a%3Db=c'"
    );
    let o = ExportOptions {
        form_style: FormStyle::Data,
        ..bash()
    };
    assert_eq!(
        export(&s, &o),
        "curl -X POST 'http://x/a' \\\n  -d 'name=%E6%97%A5%E6%9C%AC+%E8%AA%9E%26x' \\\n  -d 'a%3Db=c'"
    );
}

#[test]
fn multipart_parts_keep_their_content_type() {
    let mut s = spec("POST", "http://x/a");
    s.body = Body::Multipart(vec![
        MultipartPart {
            name: "file".into(),
            value: MultipartValue::File("/tmp/a.png".into()),
            enabled: true,
            content_type: Some("image/png".into()),
        },
        MultipartPart {
            name: "meta".into(),
            value: MultipartValue::Text(r#"{"a":1}"#.into()),
            enabled: true,
            content_type: Some("application/json".into()),
        },
        MultipartPart {
            name: "semi".into(),
            value: MultipartValue::Text("a;b".into()),
            enabled: true,
            content_type: Some("text/plain".into()),
        },
        MultipartPart {
            name: "blank".into(),
            value: MultipartValue::Text("x".into()),
            enabled: true,
            // 空白だけの指定は付けない
            content_type: Some("  ".into()),
        },
    ]);
    assert_eq!(
        export(&s, &bash()),
        "curl -X POST 'http://x/a' \\\n  -F 'file=@/tmp/a.png;type=image/png' \\\n  -F 'meta=\"{\\\"a\\\":1}\";type=application/json' \\\n  -F 'semi=\"a;b\";type=text/plain' \\\n  -F 'blank=x'"
    );
}

#[test]
fn multipart_and_binary() {
    let mut s = spec("POST", "http://x/a");
    s.params
        .push(kv(ParamIn::Header, "Content-Type", "multipart/form-data"));
    s.body = Body::Multipart(vec![
        MultipartPart {
            name: "title".into(),
            value: MultipartValue::Text("契約書".into()),
            enabled: true,
            content_type: None,
        },
        MultipartPart {
            name: "note".into(),
            value: MultipartValue::Text("@x;y".into()),
            enabled: true,
            content_type: None,
        },
        MultipartPart {
            name: "file".into(),
            value: MultipartValue::File("/tmp/a b.png".into()),
            enabled: true,
            content_type: None,
        },
        MultipartPart {
            name: "odd".into(),
            value: MultipartValue::File("/tmp/we;ird\"n.txt".into()),
            enabled: true,
            content_type: None,
        },
        MultipartPart {
            name: "off".into(),
            value: MultipartValue::Text("x".into()),
            enabled: false,
            content_type: None,
        },
    ]);
    // 手入力の Content-Type は境界が合わなくなるので出さない
    assert_eq!(
        export(&s, &bash()),
        "curl -X POST 'http://x/a' \\\n  -F 'title=契約書' \\\n  --form-string 'note=@x;y' \\\n  -F 'file=@/tmp/a b.png' \\\n  -F 'odd=@\"/tmp/we;ird\\\"n.txt\"'"
    );

    let mut s = spec("PUT", "http://x/a");
    s.body = Body::Binary("/tmp/a.bin".into());
    assert_eq!(
        export(&s, &bash()),
        "curl -X PUT 'http://x/a' \\\n  -H 'Content-Type: application/octet-stream' \\\n  --data-binary '@/tmp/a.bin'"
    );
}

#[test]
fn redact_list_masks_values_everywhere() {
    let mut s = spec("POST", "http://x/a?token=s3cr3t%20x");
    s.params.push(kv(ParamIn::Header, "X-Echo", "s3cr3t x"));
    s.params.push(kv(ParamIn::Query, "k", "s3cr3t x"));
    s.auth = Some(AuthOverride::Basic {
        username: "neko".into(),
        password: "pw-1".into(),
    });
    s.body = Body::Json(r#"{"password":"pw-1","s":"s3cr3t x"}"#.into());
    let o = ExportOptions {
        redact: vec!["s3cr3t x".into(), "pw-1".into(), "".into()],
        ..bash()
    };
    let out = export(&s, &o);
    assert!(!out.contains("s3cr3t"), "{out}");
    assert!(!out.contains("pw-1"), "{out}");
    assert!(out.contains("-u 'neko:••••'"), "{out}");
    assert!(out.contains("-H 'X-Echo: ••••'"), "{out}");
    assert!(out.contains("token=••••"), "{out}");
    assert!(
        out.contains(r#"--data-raw '{"password":"••••","s":"••••"}'"#),
        "{out}"
    );
}

#[test]
fn mask_sensitive_hides_auth_headers_and_cookies() {
    let mut s = spec("GET", "http://x/a");
    s.params.push(kv(ParamIn::Header, "X-XSRF-TOKEN", "tok"));
    s.params.push(kv(ParamIn::Header, "X-Other", "keep"));
    s.params.push(kv(ParamIn::Cookie, "SESSION", "abc"));
    s.auth = Some(AuthOverride::Bearer {
        token: "secret".into(),
    });
    let o = ExportOptions {
        mask_sensitive: true,
        ..bash()
    };
    let out = export(&s, &o);
    assert!(out.contains("-H 'X-XSRF-TOKEN: ••••'"), "{out}");
    assert!(out.contains("-H 'X-Other: keep'"), "{out}");
    assert!(out.contains("-H 'Authorization: Bearer ••••'"), "{out}");
    assert!(out.contains("-b 'SESSION=••••'"), "{out}");
    assert!(!out.contains("secret") && !out.contains("abc"));
}

#[test]
fn cmd_output_uses_caret_quoting_and_continuation() {
    let mut s = spec("POST", "http://localhost:8080/api/v1/x?a=1&b=2");
    s.params.push(kv(ParamIn::Header, "X-A", "100%"));
    s.body = Body::Json(r#"{"a":"b\\c","n":"x&y"}"#.into());
    let o = ExportOptions {
        shell: Shell::Cmd,
        ..bash()
    };
    let out = export(&s, &o);
    assert_eq!(
        out,
        "curl -X POST ^\"http://localhost:8080/api/v1/x?a=1^&b=2^\" ^\n  \
         -H ^\"X-A: 100^%^\" ^\n  \
         -H ^\"Content-Type: application/json^\" ^\n  \
         --data-raw ^\"{\\^\"a\\^\":\\^\"b\\\\c\\^\",\\^\"n\\^\":\\^\"x^&y\\^\"}^\""
    );
    // 書き出したものを読み戻せる
    let back = p(&out);
    assert_eq!(json_text(&back), r#"{"a":"b\\c","n":"x&y"}"#);
    assert_eq!(header(&back, "X-A"), Some("100%"));
}

#[test]
fn cmd_single_line() {
    let mut s = spec("GET", "http://x/a");
    s.params.push(kv(ParamIn::Header, "X-A", "1"));
    let o = ExportOptions {
        shell: Shell::Cmd,
        multiline: false,
        ..bash()
    };
    assert_eq!(export(&s, &o), "curl ^\"http://x/a^\" -H ^\"X-A: 1^\"");
}
