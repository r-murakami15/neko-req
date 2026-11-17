use super::*;
use crate::request::{ApiKeyIn, MultipartPart};
use crate::vars::VarLayer;

struct Fixed;
impl DynamicSource for Fixed {
    fn eval(&self, name: &str) -> Option<String> {
        match name {
            "$uuid" => Some("UUID".into()),
            _ => None,
        }
    }
}

fn scope() -> VarScope {
    let mut env = VarLayer::new();
    env.insert("baseUrl", "http://localhost:8080", false);
    env.insert("clientId", "1", false);
    env.insert("q", "ネコ & 犬", false);
    env.insert("token", "tok/en+1", true);
    env.insert("password", "p@ss word", true);
    env.insert("session", "SESS", true);
    let mut s = VarScope::new();
    s.push_layer(env);
    s
}

fn kv(location: ParamIn, name: &str, value: &str, enabled: bool) -> KeyValue {
    KeyValue {
        location,
        name: name.into(),
        value: value.into(),
        enabled,
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

fn build(spec: &RequestSpec) -> PreparedRequest {
    build_request(spec, &scope(), &Fixed).unwrap()
}

fn header<'a>(p: &'a PreparedRequest, name: &str) -> Option<&'a str> {
    p.headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}

#[test]
fn expands_the_url_and_uppercases_the_method() {
    let p = build(&spec("get", "{{baseUrl}}/api/v1/auth/session"));
    assert_eq!(p.method, "GET");
    assert_eq!(p.url, "http://localhost:8080/api/v1/auth/session");
    assert_eq!(p.body, PreparedBody::None);
    assert!(p.headers.is_empty());
}

#[test]
fn scheme_is_added_when_missing() {
    let p = build(&spec("GET", "localhost:8080/x"));
    assert_eq!(p.url, "http://localhost:8080/x");
}

#[test]
fn path_params_are_embedded_and_encoded() {
    let mut s = spec("GET", "{{baseUrl}}/api/v1/clients/{clientId}/files/{name}");
    s.params = vec![
        kv(ParamIn::Path, "clientId", "{{clientId}}", true),
        kv(ParamIn::Path, "name", "a/b c", true),
    ];
    let p = build(&s);
    assert_eq!(
        p.url,
        "http://localhost:8080/api/v1/clients/1/files/a%2Fb%20c"
    );
}

#[test]
fn missing_path_param_is_a_warning_not_an_error() {
    let mut s = spec("GET", "{{baseUrl}}/clients/{clientId}");
    s.params = vec![kv(ParamIn::Path, "clientId", "", true)];
    let prev = preview(&s, &scope(), &Fixed);
    assert_eq!(prev.missing_path_params, ["clientId"]);
    assert!(prev.unresolved.is_empty());
    assert_eq!(prev.url_error, None);
    // 送信はできる（波括弧は URL エンコードされる）
    assert!(build_request(&s, &scope(), &Fixed).is_ok());
    // 行がまったくなくても同じ
    let s = spec("GET", "{{baseUrl}}/clients/{clientId}");
    assert!(build_request(&s, &scope(), &Fixed).is_ok());
}

#[test]
fn query_is_appended_to_an_existing_query_and_disabled_rows_are_skipped() {
    let mut s = spec("GET", "{{baseUrl}}/search?x=1#frag");
    s.params = vec![
        kv(ParamIn::Query, "q", "{{q}}", true),
        kv(ParamIn::Query, "off", "no", false),
        kv(ParamIn::Query, "empty", "", true),
        kv(ParamIn::Query, "", "ignored", true),
    ];
    let p = build(&s);
    assert_eq!(
        p.url,
        "http://localhost:8080/search?x=1&q=%E3%83%8D%E3%82%B3%20%26%20%E7%8A%AC&empty=#frag"
    );
}

#[test]
fn headers_keep_order_and_skip_disabled_ones() {
    let mut s = spec("GET", "{{baseUrl}}/x");
    s.params = vec![
        kv(ParamIn::Header, "Accept", "application/json", true),
        kv(ParamIn::Header, "X-Off", "1", false),
        kv(ParamIn::Header, "X-Request-Id", "{{$uuid}}", true),
        kv(ParamIn::Header, "Accept", "text/plain", true),
    ];
    let p = build(&s);
    let names: Vec<_> = p.headers.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(names, ["Accept", "X-Request-Id", "Accept"]);
    assert_eq!(header(&p, "x-request-id"), Some("UUID"));
}

#[test]
fn cookie_params_become_one_cookie_header() {
    let mut s = spec("GET", "{{baseUrl}}/x");
    s.params = vec![
        kv(ParamIn::Cookie, "SESSION", "{{session}}", true),
        kv(ParamIn::Cookie, "a", "1", true),
        kv(ParamIn::Cookie, "b", "2", false),
    ];
    let p = build(&s);
    assert_eq!(header(&p, "Cookie"), Some("SESSION=SESS; a=1"));
    assert_eq!(p.secrets, ["SESS"]);
}

#[test]
fn json_body_sets_content_type_unless_the_user_did() {
    let mut s = spec("POST", "{{baseUrl}}/login");
    s.body = Body::Json(r#"{"pw":"{{password}}"}"#.into());
    let p = build(&s);
    assert_eq!(header(&p, "Content-Type"), Some("application/json"));
    assert_eq!(
        p.body,
        PreparedBody::Bytes(br#"{"pw":"p@ss word"}"#.to_vec())
    );

    s.params = vec![kv(
        ParamIn::Header,
        "content-type",
        "application/vnd.api+json",
        true,
    )];
    let p = build(&s);
    let cts: Vec<_> = p
        .headers
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case("content-type"))
        .collect();
    assert_eq!(cts.len(), 1);
    assert_eq!(cts[0].value, "application/vnd.api+json");
}

#[test]
fn blank_json_and_text_bodies_send_nothing() {
    for body in [Body::Json("  ".into()), Body::Text(String::new())] {
        let mut s = spec("POST", "{{baseUrl}}/x");
        s.body = body;
        let p = build(&s);
        assert_eq!(p.body, PreparedBody::None);
        assert_eq!(header(&p, "Content-Type"), None);
    }
}

#[test]
fn text_form_and_binary_bodies() {
    let mut s = spec("POST", "{{baseUrl}}/x");
    s.body = Body::Text("hello {{clientId}}".into());
    let p = build(&s);
    assert_eq!(
        header(&p, "Content-Type"),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(p.body, PreparedBody::Bytes(b"hello 1".to_vec()));

    s.body = Body::Form(vec![
        kv(ParamIn::Query, "q", "{{q}}", true),
        kv(ParamIn::Query, "off", "x", false),
        kv(ParamIn::Query, "n", "1", true),
    ]);
    let p = build(&s);
    assert_eq!(
        header(&p, "Content-Type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(
        p.body,
        PreparedBody::Bytes(b"q=%E3%83%8D%E3%82%B3+%26+%E7%8A%AC&n=1".to_vec())
    );

    s.body = Body::Binary("/tmp/{{clientId}}.bin".into());
    let p = build(&s);
    assert_eq!(header(&p, "Content-Type"), Some("application/octet-stream"));
    assert_eq!(p.body, PreparedBody::File("/tmp/1.bin".into()));
}

#[test]
fn multipart_body_ignores_a_hand_written_content_type() {
    let mut s = spec("POST", "{{baseUrl}}/upload");
    s.params = vec![kv(
        ParamIn::Header,
        "Content-Type",
        "multipart/form-data",
        true,
    )];
    s.body = Body::Multipart(vec![
        MultipartPart {
            name: "title".into(),
            value: MultipartValue::Text("{{q}}".into()),
            enabled: true,
            content_type: None,
        },
        MultipartPart {
            name: "off".into(),
            value: MultipartValue::Text("x".into()),
            enabled: false,
            content_type: None,
        },
        MultipartPart {
            name: "file".into(),
            value: MultipartValue::File("/tmp/a.txt".into()),
            enabled: true,
            content_type: None,
        },
    ]);
    let p = build(&s);
    let PreparedBody::Multipart { boundary, parts } = &p.body else {
        panic!("multipart のはず");
    };
    assert_eq!(
        header(&p, "Content-Type").unwrap(),
        format!("multipart/form-data; boundary={boundary}")
    );
    assert_eq!(parts.len(), 2);
    assert_eq!(
        parts[0],
        PreparedPart {
            name: "title".into(),
            value: PreparedPartValue::Text("ネコ & 犬".into()),
            content_type: None,
        }
    );
    assert_eq!(parts[1].value, PreparedPartValue::File("/tmp/a.txt".into()));
}

#[test]
fn bearer_basic_and_api_key_auth() {
    let mut s = spec("GET", "{{baseUrl}}/x");
    s.auth = Some(AuthOverride::Bearer {
        token: "{{token}}".into(),
    });
    let p = build(&s);
    assert_eq!(header(&p, "Authorization"), Some("Bearer tok/en+1"));

    s.auth = Some(AuthOverride::Basic {
        username: "user".into(),
        password: "{{password}}".into(),
    });
    let p = build(&s);
    // base64("user:p@ss word")
    assert_eq!(
        header(&p, "Authorization"),
        Some("Basic dXNlcjpwQHNzIHdvcmQ=")
    );

    s.auth = Some(AuthOverride::ApiKey {
        location: ApiKeyIn::Header,
        name: "X-Api-Key".into(),
        value: "{{token}}".into(),
    });
    let p = build(&s);
    assert_eq!(header(&p, "X-Api-Key"), Some("tok/en+1"));

    s.auth = Some(AuthOverride::ApiKey {
        location: ApiKeyIn::Query,
        name: "api_key".into(),
        value: "{{token}}".into(),
    });
    let p = build(&s);
    assert_eq!(p.url, "http://localhost:8080/x?api_key=tok%2Fen%2B1");

    s.auth = Some(AuthOverride::ApiKey {
        location: ApiKeyIn::Cookie,
        name: "SESSION".into(),
        value: "{{session}}".into(),
    });
    let p = build(&s);
    assert_eq!(header(&p, "Cookie"), Some("SESSION=SESS"));

    s.auth = Some(AuthOverride::None);
    assert!(build(&s).headers.is_empty());
}

#[test]
fn a_hand_written_authorization_header_wins_over_auth() {
    let mut s = spec("GET", "{{baseUrl}}/x");
    s.params = vec![kv(ParamIn::Header, "authorization", "Custom abc", true)];
    s.auth = Some(AuthOverride::Bearer {
        token: "{{token}}".into(),
    });
    let p = build(&s);
    assert_eq!(p.headers.len(), 1);
    assert_eq!(header(&p, "Authorization"), Some("Custom abc"));
}

#[test]
fn cookie_auth_is_appended_to_a_hand_written_cookie_header() {
    let mut s = spec("GET", "{{baseUrl}}/x");
    s.params = vec![kv(ParamIn::Header, "Cookie", "a=1;", true)];
    s.auth = Some(AuthOverride::ApiKey {
        location: ApiKeyIn::Cookie,
        name: "SESSION".into(),
        value: "{{session}}".into(),
    });
    let p = build(&s);
    assert_eq!(header(&p, "Cookie"), Some("a=1; SESSION=SESS"));
}

#[test]
fn unresolved_variables_report_where_they_are() {
    let mut s = spec("POST", "{{host}}/x");
    s.params = vec![
        kv(ParamIn::Query, "a", "1", true),
        kv(ParamIn::Query, "b", "{{nope}}", true),
        kv(ParamIn::Header, "{{hname}}", "v", true),
        kv(ParamIn::Header, "X", "{{hval}}", false), // 無効の行は見ない
        kv(ParamIn::Path, "id", "{{pid}}", true),
        kv(ParamIn::Cookie, "c", "{{cv}}", true),
    ];
    s.body = Body::Json(r#"{"a":"{{jv}}"}"#.into());
    s.auth = Some(AuthOverride::Bearer {
        token: "{{tok}}".into(),
    });
    let err = build_request(&s, &scope(), &Fixed).unwrap_err();
    let BuildError::Unresolved(vars) = err else {
        panic!("Unresolved のはず");
    };
    let got: Vec<(String, String)> = vars
        .iter()
        .map(|v| (v.name.clone(), v.location.describe()))
        .collect();
    assert_eq!(
        got,
        [
            ("pid", "パスパラメータ 1 行目の値"),
            ("host", "URL"),
            ("nope", "クエリパラメータ 2 行目の値"),
            ("hname", "ヘッダー 1 行目のキー"),
            ("cv", "Cookie 1 行目の値"),
            ("tok", "認証（token）"),
            ("jv", "ボディ"),
        ]
        .map(|(a, b)| (a.to_string(), b.to_string()))
    );
}

#[test]
fn unresolved_in_form_and_multipart_rows_have_row_numbers() {
    let mut s = spec("POST", "{{baseUrl}}/x");
    s.body = Body::Form(vec![
        kv(ParamIn::Query, "a", "1", false),
        kv(ParamIn::Query, "b", "{{x}}", true),
    ]);
    let BuildError::Unresolved(vars) = build_request(&s, &scope(), &Fixed).unwrap_err() else {
        panic!()
    };
    assert_eq!(
        vars[0].location,
        VarLocation::BodyForm {
            row: 2,
            part: RowPart::Value
        }
    );

    s.body = Body::Multipart(vec![MultipartPart {
        name: "{{n}}".into(),
        value: MultipartValue::Text("v".into()),
        enabled: true,
        content_type: None,
    }]);
    let BuildError::Unresolved(vars) = build_request(&s, &scope(), &Fixed).unwrap_err() else {
        panic!()
    };
    assert_eq!(
        vars[0].location,
        VarLocation::BodyMultipart {
            row: 1,
            part: RowPart::Name
        }
    );
}

#[test]
fn invalid_urls_are_reported() {
    for url in ["", "   ", "ftp://example.com/x", "http://"] {
        let err = build_request(&spec("GET", url), &scope(), &Fixed).unwrap_err();
        assert!(matches!(err, BuildError::InvalidUrl(_)), "{url}: {err:?}");
    }
}

#[test]
fn preview_masks_secrets_in_the_url_and_headers() {
    let mut s = spec("GET", "{{baseUrl}}/x/{token}");
    s.params = vec![
        kv(ParamIn::Path, "token", "{{token}}", true),
        kv(ParamIn::Query, "pw", "{{password}}", true),
        kv(ParamIn::Query, "q", "{{q}}", true),
    ];
    s.auth = Some(AuthOverride::Bearer {
        token: "{{token}}".into(),
    });
    let prev = preview(&s, &scope(), &Fixed);
    assert_eq!(
        prev.url,
        "http://localhost:8080/x/••••?pw=••••&q=%E3%83%8D%E3%82%B3%20%26%20%E7%8A%AC"
    );
    assert!(prev.unresolved.is_empty());
    let auth = prev
        .auto_headers
        .iter()
        .find(|h| h.name == "Authorization")
        .unwrap();
    assert_eq!(auth.value, "Bearer ••••");
    assert_eq!(auth.source, "認証（Bearer）");
}

#[test]
fn preview_lists_unresolved_and_keeps_them_in_the_url() {
    let s = spec("GET", "{{host}}/api/{{ver}}/x");
    let prev = preview(&s, &scope(), &Fixed);
    assert_eq!(prev.url, "http://{{host}}/api/{{ver}}/x");
    let names: Vec<_> = prev.unresolved.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["host", "ver"]);
}

#[test]
fn preview_auto_headers_show_defaults_unless_overridden() {
    let mut s = spec("POST", "{{baseUrl}}/x");
    s.body = Body::Json("{}".into());
    s.params = vec![kv(ParamIn::Header, "User-Agent", "mine", true)];
    let prev = preview(&s, &scope(), &Fixed);
    let names: Vec<_> = prev.auto_headers.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(names, ["Content-Type", "Accept", "Accept-Encoding", "Host"]);
    let host = prev.auto_headers.last().unwrap();
    assert_eq!(host.value, "localhost:8080");
    assert_eq!(host.source, "URL から");
}

#[test]
fn referenced_variables_cover_enabled_rows_only() {
    let mut s = spec("POST", "{{baseUrl}}/x/{{$uuid}}");
    s.params = vec![
        kv(ParamIn::Query, "a", "{{q}}", true),
        kv(ParamIn::Query, "b", "{{off}}", false),
        kv(ParamIn::Header, "X-{{hn}}", "{{token}}", true),
    ];
    s.body = Body::Form(vec![
        kv(ParamIn::Query, "f", "{{password}}", true),
        kv(ParamIn::Query, "g", "{{off2}}", false),
    ]);
    s.auth = Some(AuthOverride::Bearer {
        token: "{{token}} {{session}}".into(),
    });
    assert_eq!(
        referenced_variables(&s),
        ["baseUrl", "q", "hn", "token", "password", "session"]
    );
}
