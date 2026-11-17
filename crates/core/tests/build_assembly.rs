//! リクエストの組み立て（`build_request` / `preview`）を公開 API だけで確かめる結合テスト。
//! 既存の単体テスト（crates/core/src/build/tests.rs）と重ならない観点を中心にする。

use base64::Engine;
use nekoreq_core::{
    build_request, preview, referenced_variables, ApiKeyIn, AuthOverride, Body, BuildError,
    DynamicSource, KeyValue, MultipartPart, MultipartValue, ParamIn, PreparedBody,
    PreparedPartValue, PreparedRequest, RequestSpec, RowPart, UnresolvedVar, VarLayer, VarLocation,
    VarScope,
};

struct NoDyn;
impl DynamicSource for NoDyn {
    fn eval(&self, _: &str) -> Option<String> {
        None
    }
}

fn scope(pairs: &[(&str, &str, bool)]) -> VarScope {
    let mut l = VarLayer::new();
    for (n, v, s) in pairs {
        l.insert(*n, *v, *s);
    }
    let mut s = VarScope::new();
    s.push_layer(l);
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

fn build(spec: &RequestSpec, scope: &VarScope) -> PreparedRequest {
    build_request(spec, scope, &NoDyn).unwrap()
}

fn header<'a>(p: &'a PreparedRequest, name: &str) -> Option<&'a str> {
    p.headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}

fn headers_named<'a>(p: &'a PreparedRequest, name: &str) -> Vec<&'a str> {
    p.headers
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
        .collect()
}

fn unresolved_of(spec: &RequestSpec, scope: &VarScope) -> Vec<UnresolvedVar> {
    match build_request(spec, scope, &NoDyn) {
        Err(BuildError::Unresolved(v)) => v,
        other => panic!("未解決のはず: {other:?}"),
    }
}

// ---- メソッド ----

#[test]
fn method_is_trimmed_and_uppercased_including_mixed_case() {
    for (input, want) in [
        ("get", "GET"),
        ("Post", "POST"),
        (" delete ", "DELETE"),
        ("pAtCh", "PATCH"),
        ("HEAD", "HEAD"),
    ] {
        let p = build(&spec(input, "http://h/"), &VarScope::new());
        assert_eq!(p.method, want);
    }
}

// ---- パスパラメータ ----

#[test]
fn path_param_values_are_encoded_for_slash_space_and_japanese() {
    let mut s = spec("GET", "http://h/files/{name}/x");
    s.params = vec![kv(ParamIn::Path, "name", "請求書 2026/11?a=b#c%d", true)];
    let p = build(&s, &VarScope::new());
    assert_eq!(
        p.url,
        "http://h/files/%E8%AB%8B%E6%B1%82%E6%9B%B8%202026%2F11%3Fa%3Db%23c%25d/x"
    );
}

#[test]
fn path_param_value_from_a_variable_is_encoded_and_a_secret_is_recorded() {
    let sc = scope(&[("tok", "a/b c", true)]);
    let mut s = spec("GET", "http://h/{p}");
    s.params = vec![kv(ParamIn::Path, "p", "{{tok}}", true)];
    let p = build(&s, &sc);
    assert_eq!(p.url, "http://h/a%2Fb%20c");
    assert_eq!(p.secrets, ["a/b c"]);
}

#[test]
fn the_same_path_placeholder_twice_and_names_with_spaces_around() {
    let mut s = spec("GET", "http://h/{id}/sub/{id}");
    s.params = vec![kv(ParamIn::Path, "  id  ", "7", true)];
    let p = build(&s, &VarScope::new());
    assert_eq!(p.url, "http://h/7/sub/7");
}

#[test]
fn a_disabled_path_param_is_not_used_and_a_placeholder_with_no_row_is_reported_missing() {
    let mut s = spec("GET", "http://h/c/{id}");
    s.params = vec![kv(ParamIn::Path, "id", "5", false)];
    let a = preview(&s, &VarScope::new(), &NoDyn);
    // 無効の行は使わない。`{id}` は URL に残る（プレビューでは `{id}` のまま見せる）
    assert_eq!(a.url, "http://h/c/{id}");
    // 行がない・無効の `{id}` も missing に載る
    assert_eq!(a.missing_path_params, ["id"]);
}

#[test]
fn an_empty_path_value_is_reported_missing_once_even_if_used_twice() {
    let mut s = spec("GET", "http://h/{a}/{a}/{b}");
    s.params = vec![
        kv(ParamIn::Path, "a", "", true),
        kv(ParamIn::Path, "b", "ok", true),
    ];
    let a = preview(&s, &VarScope::new(), &NoDyn);
    assert_eq!(a.missing_path_params, ["a"]);
    assert!(a.unresolved.is_empty());
    assert!(a.url_error.is_none());
    // 送信はできる
    assert!(build_request(&s, &VarScope::new(), &NoDyn).is_ok());
}

#[test]
fn a_path_param_value_that_looks_like_a_placeholder_is_not_substituted_again() {
    let mut s = spec("GET", "http://h/{a}/{b}");
    s.params = vec![
        kv(ParamIn::Path, "a", "{b}", true),
        kv(ParamIn::Path, "b", "B", true),
    ];
    let p = build(&s, &VarScope::new());
    assert_eq!(p.url, "http://h/%7Bb%7D/B");
}

#[test]
fn unresolved_variable_in_a_path_value_has_the_path_row_location() {
    let mut s = spec("GET", "http://h/{a}");
    s.params = vec![
        kv(ParamIn::Header, "X", "1", true),
        kv(ParamIn::Path, "a", "{{missing}}", true),
    ];
    let u = unresolved_of(&s, &VarScope::new());
    assert_eq!(
        u,
        [UnresolvedVar {
            name: "missing".into(),
            location: VarLocation::Param {
                location: ParamIn::Path,
                row: 1,
                part: RowPart::Value
            }
        }]
    );
}

// ---- クエリ ----

#[test]
fn query_params_keep_order_duplicates_and_encode_names_and_values() {
    let mut s = spec("GET", "http://h/search");
    s.params = vec![
        kv(ParamIn::Query, "tag", "a", true),
        kv(ParamIn::Query, "tag", "b", true),
        kv(ParamIn::Query, "q", "ネコ & 犬 = ok?", true),
        kv(ParamIn::Query, "キー", "値", true),
        kv(ParamIn::Query, "a b", "c+d", true),
        kv(ParamIn::Query, "empty", "", true),
        kv(ParamIn::Query, "", "ignored", true),
    ];
    let p = build(&s, &VarScope::new());
    assert_eq!(
        p.url,
        "http://h/search?tag=a&tag=b&q=%E3%83%8D%E3%82%B3%20%26%20%E7%8A%AC%20%3D%20ok%3F\
         &%E3%82%AD%E3%83%BC=%E5%80%A4&a%20b=c%2Bd&empty="
    );
}

#[test]
fn rows_are_appended_after_an_existing_query_and_the_fragment_stays_last() {
    let mut s = spec("GET", "http://h/x?a=1&b=2#frag");
    s.params = vec![kv(ParamIn::Query, "c", "3", true)];
    let p = build(&s, &VarScope::new());
    assert_eq!(p.url, "http://h/x?a=1&b=2&c=3#frag");
}

#[test]
fn an_existing_japanese_query_in_the_url_is_encoded_and_a_trailing_question_mark_is_harmless() {
    let p = build(&spec("GET", "http://h/x?q=日本語"), &VarScope::new());
    assert_eq!(p.url, "http://h/x?q=%E6%97%A5%E6%9C%AC%E8%AA%9E");
    let mut s = spec("GET", "http://h/x?");
    s.params = vec![kv(ParamIn::Query, "k", "v", true)];
    assert_eq!(build(&s, &VarScope::new()).url, "http://h/x?k=v");
}

#[test]
fn a_query_variable_value_containing_ampersand_does_not_split_the_pair() {
    let sc = scope(&[("v", "a&b=c#d", false)]);
    let mut s = spec("GET", "http://h/x");
    s.params = vec![kv(ParamIn::Query, "k", "{{v}}", true)];
    let p = build(&s, &sc);
    assert_eq!(p.url, "http://h/x?k=a%26b%3Dc%23d");
}

#[test]
fn a_trailing_ampersand_in_the_template_query_does_not_produce_double_ampersand() {
    let mut s = spec("GET", "http://h/x?a=1&");
    s.params = vec![kv(ParamIn::Query, "b", "2", true)];
    let p = build(&s, &VarScope::new());
    assert_eq!(p.url, "http://h/x?a=1&b=2", "空のペア（&&）を作らない");
}

#[test]
fn a_query_in_the_variable_expanded_base_url_is_merged_too() {
    let sc = scope(&[("base", "http://h/api?v=1", false)]);
    let mut s = spec("GET", "{{base}}");
    s.params = vec![kv(ParamIn::Query, "k", "v", true)];
    assert_eq!(build(&s, &sc).url, "http://h/api?v=1&k=v");
}

#[test]
fn query_value_with_only_unreserved_characters_is_untouched_and_tilde_stays() {
    let mut s = spec("GET", "http://h/x");
    s.params = vec![kv(ParamIn::Query, "a-b_c.d~e", "A-Z_0.9~", true)];
    assert_eq!(
        build(&s, &VarScope::new()).url,
        "http://h/x?a-b_c.d~e=A-Z_0.9~"
    );
}

// ---- 手入力ヘッダーと自動ヘッダー ----

#[test]
fn hand_written_authorization_wins_regardless_of_case_and_is_not_listed_as_auto() {
    let mut s = spec("GET", "http://h/");
    s.params = vec![kv(ParamIn::Header, "authorization", "Token manual", true)];
    s.auth = Some(AuthOverride::Bearer {
        token: "auto".into(),
    });
    let p = build(&s, &VarScope::new());
    assert_eq!(headers_named(&p, "Authorization"), ["Token manual"]);
    assert_eq!(p.headers[0].name, "authorization", "手入力の名前のまま");
    let pv = preview(&s, &VarScope::new(), &NoDyn);
    assert!(!pv
        .auto_headers
        .iter()
        .any(|h| h.name.eq_ignore_ascii_case("Authorization")));
}

#[test]
fn hand_written_content_type_wins_for_json_form_and_text_but_not_multipart() {
    for body in [
        Body::Json("{}".into()),
        Body::Text("t".into()),
        Body::Form(vec![kv(ParamIn::Query, "a", "1", true)]),
    ] {
        let mut s = spec("POST", "http://h/");
        s.params = vec![kv(
            ParamIn::Header,
            "content-type",
            "application/vnd.x+json",
            true,
        )];
        s.body = body;
        let p = build(&s, &VarScope::new());
        assert_eq!(
            headers_named(&p, "Content-Type"),
            ["application/vnd.x+json"]
        );
    }
    let mut s = spec("POST", "http://h/");
    s.params = vec![kv(
        ParamIn::Header,
        "Content-Type",
        "multipart/form-data",
        true,
    )];
    s.body = Body::Multipart(vec![MultipartPart {
        name: "a".into(),
        value: MultipartValue::Text("1".into()),
        enabled: true,
        content_type: None,
    }]);
    let p = build(&s, &VarScope::new());
    let cts = headers_named(&p, "Content-Type");
    assert_eq!(cts.len(), 1);
    assert!(
        cts[0].starts_with("multipart/form-data; boundary="),
        "{cts:?}"
    );
}

#[test]
fn multipart_part_content_types_are_carried_to_the_prepared_parts() {
    let mut s = spec("POST", "http://h/");
    let part = |name: &str, ct: Option<&str>| MultipartPart {
        name: name.into(),
        value: MultipartValue::Text("v".into()),
        enabled: true,
        content_type: ct.map(Into::into),
    };
    s.body = Body::Multipart(vec![
        part("a", Some(" application/json ")),
        part("b", None),
        part("c", Some("   ")),
    ]);
    let p = build(&s, &VarScope::new());
    let PreparedBody::Multipart { parts, .. } = &p.body else {
        panic!("multipart のはず")
    };
    let types: Vec<_> = parts.iter().map(|p| p.content_type.as_deref()).collect();
    assert_eq!(types, [Some("application/json"), None, None]);
}

#[test]
fn a_disabled_hand_written_header_does_not_block_the_automatic_one() {
    let mut s = spec("POST", "http://h/");
    s.params = vec![kv(ParamIn::Header, "Content-Type", "text/csv", false)];
    s.body = Body::Json("{}".into());
    let p = build(&s, &VarScope::new());
    assert_eq!(headers_named(&p, "Content-Type"), ["application/json"]);
}

#[test]
fn header_names_and_values_are_trimmed_correctly_and_blank_names_are_skipped() {
    let mut s = spec("GET", "http://h/");
    s.params = vec![
        kv(ParamIn::Header, "  X-A  ", " v with spaces ", true),
        kv(ParamIn::Header, "   ", "ignored", true),
        kv(ParamIn::Header, "X-A", "second", true),
    ];
    let p = build(&s, &VarScope::new());
    assert_eq!(p.headers.len(), 2);
    assert_eq!(p.headers[0].name, "X-A");
    // 値は前後の空白も含めてそのまま（HTTP 側で整形される）
    assert_eq!(p.headers[0].value, " v with spaces ");
    assert_eq!(p.headers[1].value, "second", "重複ヘッダーは両方残す");
}

// ---- 認証 ----

#[test]
fn basic_auth_encodes_utf8_and_a_colon_in_the_username_is_an_error() {
    let mut s = spec("GET", "http://h/");
    s.auth = Some(AuthOverride::Basic {
        username: "ネコ".into(),
        password: "パス:ワード".into(),
    });
    let p = build(&s, &VarScope::new());
    let b64 = header(&p, "Authorization")
        .unwrap()
        .strip_prefix("Basic ")
        .unwrap();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .unwrap();
    assert_eq!(String::from_utf8(decoded).unwrap(), "ネコ:パス:ワード");

    // ユーザー名に ':' は使えない（RFC 7617。受け取り側が最初の ':' で分けるため）。組み立て時にエラー
    let mut s = spec("GET", "http://h/");
    s.auth = Some(AuthOverride::Basic {
        username: "a:b".into(),
        password: "pw".into(),
    });
    let r = build_request(&s, &VarScope::new(), &NoDyn);
    assert!(matches!(r, Err(BuildError::InvalidAuth(_))), "{r:?}");
    // 変数から入ったユーザー名の ':' も同じ
    let sc = scope(&[("u", "x:y", false)]);
    s.auth = Some(AuthOverride::Basic {
        username: "{{u}}".into(),
        password: "pw".into(),
    });
    assert!(matches!(
        build_request(&s, &sc, &NoDyn),
        Err(BuildError::InvalidAuth(_))
    ));
}

#[test]
fn basic_auth_secrets_are_collected_and_empty_password_is_allowed() {
    let sc = scope(&[("pw", "s3cr3t", true), ("u", "alice", false)]);
    let mut s = spec("GET", "http://h/");
    s.auth = Some(AuthOverride::Basic {
        username: "{{u}}".into(),
        password: "{{pw}}".into(),
    });
    let p = build(&s, &sc);
    // シークレットの値に加えて、Authorization の base64 と `user:password` も伏せる対象になる
    let b64 = base64::engine::general_purpose::STANDARD.encode("alice:s3cr3t");
    assert_eq!(p.secrets, ["s3cr3t", b64.as_str(), "alice:s3cr3t"]);
    let mut s = spec("GET", "http://h/");
    s.auth = Some(AuthOverride::Basic {
        username: "u".into(),
        password: String::new(),
    });
    let p = build(&s, &VarScope::new());
    let b64 = header(&p, "Authorization")
        .unwrap()
        .strip_prefix("Basic ")
        .unwrap();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .unwrap();
    assert_eq!(decoded, b"u:");
}

#[test]
fn bearer_token_is_trimmed_and_an_authorization_scheme_typed_in_the_token_is_doubled() {
    let sc = scope(&[("t", "  abc \n", true)]);
    let mut s = spec("GET", "http://h/");
    s.auth = Some(AuthOverride::Bearer {
        token: "{{t}}".into(),
    });
    let p = build(&s, &sc);
    assert_eq!(header(&p, "Authorization"), Some("Bearer abc"));
    // secrets には trim 前の値が入る（伏せ字の対象は差し込んだ値そのもの）
    assert_eq!(p.secrets, ["  abc \n"]);
}

#[test]
fn api_key_in_query_goes_after_the_rows_and_is_encoded() {
    let sc = scope(&[("k", "k e/y+=", true)]);
    let mut s = spec("GET", "http://h/x?a=1");
    s.params = vec![kv(ParamIn::Query, "b", "2", true)];
    s.auth = Some(AuthOverride::ApiKey {
        location: ApiKeyIn::Query,
        name: "api_key".into(),
        value: "{{k}}".into(),
    });
    let p = build(&s, &sc);
    assert_eq!(p.url, "http://h/x?a=1&b=2&api_key=k%20e%2Fy%2B%3D");
    assert!(p.headers.is_empty());
    assert_eq!(p.secrets, ["k e/y+="]);
}

#[test]
fn api_key_in_cookie_is_added_after_cookie_rows_and_before_nothing_else() {
    let sc = scope(&[("k", "KEY", true)]);
    let mut s = spec("GET", "http://h/");
    s.params = vec![
        kv(ParamIn::Cookie, "a", "1", true),
        kv(ParamIn::Cookie, "b", "2", true),
        kv(ParamIn::Cookie, "off", "x", false),
    ];
    s.auth = Some(AuthOverride::ApiKey {
        location: ApiKeyIn::Cookie,
        name: "SESSION".into(),
        value: "{{k}}".into(),
    });
    let p = build(&s, &sc);
    assert_eq!(headers_named(&p, "Cookie"), ["a=1; b=2; SESSION=KEY"]);
}

#[test]
fn api_key_header_with_the_same_name_as_a_hand_written_header_loses() {
    let mut s = spec("GET", "http://h/");
    s.params = vec![kv(ParamIn::Header, "X-API-Key", "manual", true)];
    s.auth = Some(AuthOverride::ApiKey {
        location: ApiKeyIn::Header,
        name: "x-api-key".into(),
        value: "auto".into(),
    });
    let p = build(&s, &VarScope::new());
    assert_eq!(headers_named(&p, "x-api-key"), ["manual"]);
}

#[test]
fn api_key_with_a_blank_name_is_ignored_and_none_auth_adds_nothing() {
    let mut s = spec("GET", "http://h/x");
    s.auth = Some(AuthOverride::ApiKey {
        location: ApiKeyIn::Query,
        name: "  ".into(),
        value: "v".into(),
    });
    let p = build(&s, &VarScope::new());
    assert_eq!(p.url, "http://h/x");
    s.auth = Some(AuthOverride::None);
    assert!(build(&s, &VarScope::new()).headers.is_empty());
}

#[test]
fn a_cookie_value_with_special_characters_is_not_encoded() {
    let mut s = spec("GET", "http://h/");
    s.params = vec![kv(ParamIn::Cookie, "SESSION", "a b=c;d", true)];
    let p = build(&s, &VarScope::new());
    // 値は書かれたまま（`;` を含むと別の Cookie に見える。エンコードしない方針）
    assert_eq!(header(&p, "Cookie"), Some("SESSION=a b=c;d"));
}

// ---- ボディ ----

#[test]
fn form_body_uses_form_urlencoding_with_plus_for_space_and_utf8() {
    let sc = scope(&[("pw", "p@ss word&=", true)]);
    let mut s = spec("POST", "http://h/");
    s.body = Body::Form(vec![
        kv(ParamIn::Query, "name", "ネコ 犬", true),
        kv(ParamIn::Query, "pw", "{{pw}}", true),
        kv(ParamIn::Query, "off", "x", false),
        kv(ParamIn::Query, "", "no-name", true),
        kv(ParamIn::Query, "star*dot.under_dash-tilde~", "*", true),
    ]);
    let p = build(&s, &sc);
    let PreparedBody::Bytes(b) = &p.body else {
        panic!("{:?}", p.body)
    };
    assert_eq!(
        String::from_utf8(b.clone()).unwrap(),
        "name=%E3%83%8D%E3%82%B3+%E7%8A%AC&pw=p%40ss+word%26%3D&star*dot.under_dash-tilde%7E=*"
    );
    assert_eq!(
        header(&p, "Content-Type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(p.secrets, ["p@ss word&="]);
}

#[test]
fn form_with_only_disabled_or_nameless_rows_sends_nothing_and_no_content_type() {
    let mut s = spec("POST", "http://h/");
    s.body = Body::Form(vec![
        kv(ParamIn::Query, "a", "1", false),
        kv(ParamIn::Query, "", "2", true),
    ]);
    let p = build(&s, &VarScope::new());
    assert_eq!(p.body, PreparedBody::None);
    assert!(header(&p, "Content-Type").is_none());
}

#[test]
fn multipart_has_a_unique_boundary_matching_the_header_and_keeps_part_order() {
    let mk = || {
        let mut s = spec("POST", "http://h/");
        s.body = Body::Multipart(vec![
            MultipartPart {
                name: "b".into(),
                value: MultipartValue::Text("2".into()),
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
                name: "a".into(),
                value: MultipartValue::File("/tmp/x.bin".into()),
                enabled: true,
                content_type: None,
            },
            MultipartPart {
                name: "".into(),
                value: MultipartValue::Text("nameless".into()),
                enabled: true,
                content_type: None,
            },
        ]);
        build(&s, &VarScope::new())
    };
    let (p1, p2) = (mk(), mk());
    let (
        PreparedBody::Multipart {
            boundary: b1,
            parts,
        },
        PreparedBody::Multipart { boundary: b2, .. },
    ) = (&p1.body, &p2.body)
    else {
        panic!()
    };
    assert_ne!(b1, b2, "送信ごとに別の境界");
    assert_eq!(
        header(&p1, "Content-Type"),
        Some(format!("multipart/form-data; boundary={b1}").as_str())
    );
    // RFC 2046: 境界は 1〜70 文字、使える文字だけ
    assert!(b1.len() <= 70 && !b1.is_empty());
    assert!(
        b1.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "{b1}"
    );
    let names: Vec<_> = parts.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["b", "a"]);
    assert_eq!(parts[1].value, PreparedPartValue::File("/tmp/x.bin".into()));
}

#[test]
fn multipart_with_no_usable_parts_sends_nothing() {
    let mut s = spec("POST", "http://h/");
    s.body = Body::Multipart(vec![MultipartPart {
        name: "a".into(),
        value: MultipartValue::Text("1".into()),
        enabled: false,
        content_type: None,
    }]);
    let p = build(&s, &VarScope::new());
    assert_eq!(p.body, PreparedBody::None);
    assert!(header(&p, "Content-Type").is_none());
}

#[test]
fn json_body_is_not_escaped_for_variable_values_and_keeps_unicode() {
    // 決定事項: 値は書かれたまま差し込む（JSON の文字列のエスケープはしない）
    let sc = scope(&[("name", "山田\"太郎\"", false), ("n", "42", false)]);
    let mut s = spec("POST", "http://h/");
    s.body = Body::Json(r#"{"name":"{{name}}","n":{{n}}}"#.into());
    let p = build(&s, &sc);
    let PreparedBody::Bytes(b) = &p.body else {
        panic!()
    };
    assert_eq!(
        String::from_utf8(b.clone()).unwrap(),
        "{\"name\":\"山田\"太郎\"\",\"n\":42}"
    );
}

#[test]
fn whitespace_only_text_body_is_sent_but_whitespace_only_json_is_not() {
    let mut s = spec("POST", "http://h/");
    s.body = Body::Text("  ".into());
    assert!(matches!(
        build(&s, &VarScope::new()).body,
        PreparedBody::Bytes(_)
    ));
    s.body = Body::Json("  \n".into());
    assert_eq!(build(&s, &VarScope::new()).body, PreparedBody::None);
}

#[test]
fn json_body_that_expands_to_empty_sends_nothing() {
    let sc = scope(&[("e", "", false)]);
    let mut s = spec("POST", "http://h/");
    s.body = Body::Json("{{e}}".into());
    assert_eq!(build(&s, &sc).body, PreparedBody::None);
}

#[test]
fn binary_body_path_is_expanded() {
    let sc = scope(&[("dir", "/data", false)]);
    let mut s = spec("PUT", "http://h/");
    s.body = Body::Binary("{{dir}}/a.bin".into());
    let p = build(&s, &sc);
    assert_eq!(p.body, PreparedBody::File("/data/a.bin".into()));
    assert_eq!(header(&p, "Content-Type"), Some("application/octet-stream"));
}

// ---- 未解決の場所 ----

#[test]
fn row_numbers_are_one_based_per_location_and_count_disabled_rows() {
    let mut s = spec("GET", "http://h/");
    s.params = vec![
        kv(ParamIn::Query, "a", "1", true),               // query 1
        kv(ParamIn::Header, "H", "{{nope_h}}", false),    // header 1（無効: 報告しない）
        kv(ParamIn::Query, "b", "2", false),              // query 2（無効）
        kv(ParamIn::Header, "{{nope_hname}}", "v", true), // header 2
        kv(ParamIn::Query, "c", "{{nope_q}}", true),      // query 3
        kv(ParamIn::Cookie, "ck", "{{nope_c}}", true),    // cookie 1
    ];
    let u = unresolved_of(&s, &VarScope::new());
    let got: Vec<(String, VarLocation)> = u.into_iter().map(|x| (x.name, x.location)).collect();
    let loc = |location, row, part| VarLocation::Param {
        location,
        row,
        part,
    };
    assert!(
        got.contains(&("nope_hname".into(), loc(ParamIn::Header, 2, RowPart::Name))),
        "{got:?}"
    );
    assert!(
        got.contains(&("nope_q".into(), loc(ParamIn::Query, 3, RowPart::Value))),
        "{got:?}"
    );
    assert!(
        got.contains(&("nope_c".into(), loc(ParamIn::Cookie, 1, RowPart::Value))),
        "{got:?}"
    );
    assert!(
        !got.iter().any(|(n, _)| n == "nope_h"),
        "無効の行の変数は報告しない: {got:?}"
    );
    assert_eq!(got.len(), 3);
}

#[test]
fn form_and_multipart_row_numbers_count_disabled_rows() {
    let mut s = spec("POST", "http://h/");
    s.body = Body::Form(vec![
        kv(ParamIn::Query, "a", "1", false),
        kv(ParamIn::Query, "b", "{{x}}", true),
    ]);
    let u = unresolved_of(&s, &VarScope::new());
    assert_eq!(
        u[0].location,
        VarLocation::BodyForm {
            row: 2,
            part: RowPart::Value
        }
    );
    s.body = Body::Multipart(vec![
        MultipartPart {
            name: "a".into(),
            value: MultipartValue::Text("1".into()),
            enabled: false,
            content_type: None,
        },
        MultipartPart {
            name: "b".into(),
            value: MultipartValue::Text("1".into()),
            enabled: false,
            content_type: None,
        },
        MultipartPart {
            name: "{{pn}}".into(),
            value: MultipartValue::File("{{fp}}".into()),
            enabled: true,
            content_type: None,
        },
    ]);
    let u = unresolved_of(&s, &VarScope::new());
    let locs: Vec<_> = u
        .iter()
        .map(|x| (x.name.as_str(), x.location.clone()))
        .collect();
    assert!(
        locs.contains(&(
            "pn",
            VarLocation::BodyMultipart {
                row: 3,
                part: RowPart::Name
            }
        )),
        "{locs:?}"
    );
    // 名前が空になる（未解決のまま残る）ので値は展開されないことがある。値側の報告があれば行番号が 3
    for (n, l) in &locs {
        if *n == "fp" {
            assert_eq!(
                *l,
                VarLocation::BodyMultipart {
                    row: 3,
                    part: RowPart::Value
                }
            );
        }
    }
}

#[test]
fn unresolved_in_every_auth_field_names_the_field() {
    let cases: Vec<(AuthOverride, Vec<&str>)> = vec![
        (
            AuthOverride::Bearer {
                token: "{{a}}".into(),
            },
            vec!["token"],
        ),
        (
            AuthOverride::Basic {
                username: "{{a}}".into(),
                password: "{{b}}".into(),
            },
            vec!["username", "password"],
        ),
        (
            AuthOverride::ApiKey {
                location: ApiKeyIn::Header,
                name: "{{a}}".into(),
                value: "{{b}}".into(),
            },
            vec!["name", "value"],
        ),
    ];
    for (auth, fields) in cases {
        let mut s = spec("GET", "http://h/");
        s.auth = Some(auth);
        let u = unresolved_of(&s, &VarScope::new());
        let got: Vec<String> = u
            .iter()
            .map(|x| match &x.location {
                VarLocation::Auth { field } => field.clone(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(got, fields);
    }
}

#[test]
fn the_same_variable_in_different_places_is_reported_per_place_but_not_duplicated_within_one() {
    let mut s = spec("GET", "{{x}}/{{x}}");
    s.params = vec![kv(ParamIn::Query, "q", "{{x}}{{x}}", true)];
    let u = unresolved_of(&s, &VarScope::new());
    assert_eq!(u.len(), 2, "{u:?}");
    assert_eq!(u[0].location, VarLocation::Url);
}

#[test]
fn an_unresolved_variable_is_reported_even_when_a_hand_written_header_overrides_the_auth() {
    // 手入力の Authorization があるときは認証の設定を使わないので、認証側の `{{token}}` が未解決でも止めない
    let mut s = spec("GET", "http://h/");
    s.params = vec![kv(ParamIn::Header, "Authorization", "Bearer manual", true)];
    s.auth = Some(AuthOverride::Bearer {
        token: "{{token}}".into(),
    });
    let p = build_request(&s, &VarScope::new(), &NoDyn).unwrap();
    assert_eq!(header(&p, "Authorization"), Some("Bearer manual"));
    let pv = preview(&s, &VarScope::new(), &NoDyn);
    assert!(pv.unresolved.is_empty(), "{:?}", pv.unresolved);
    // 手入力の Authorization がなければ、これまでどおり未解決で止まる
    s.params.clear();
    let r = build_request(&s, &VarScope::new(), &NoDyn);
    assert!(matches!(r, Err(BuildError::Unresolved(_))), "{r:?}");
}

#[test]
fn unresolved_variables_are_reported_even_if_the_url_is_invalid_and_url_error_is_not_set_for_them()
{
    let s = spec("GET", "{{base}}/x");
    let pv = preview(&s, &VarScope::new(), &NoDyn);
    assert_eq!(pv.unresolved.len(), 1);
    assert_eq!(pv.url_error, None, "未解決のときは URL の検証をしない");
    assert_eq!(
        pv.url, "http://{{base}}/x",
        "スキームだけ補って途中の姿を見せる"
    );
}

#[test]
fn url_errors() {
    for (url, needle) in [
        ("", "URL"),
        ("   ", "URL"),
        ("ftp://h/x", "http"),
        ("http://", ""),
        ("http://h:99999/", ""),
        ("http://exa mple.com/", ""),
    ] {
        let s = spec("GET", url);
        let pv = preview(&s, &VarScope::new(), &NoDyn);
        let e = pv
            .url_error
            .unwrap_or_else(|| panic!("{url:?} はエラーのはず: {}", pv.url));
        assert!(e.contains(needle), "{url:?} → {e}");
        assert!(matches!(
            build_request(&s, &VarScope::new(), &NoDyn),
            Err(BuildError::InvalidUrl(_))
        ));
    }
}

#[test]
fn invalid_url_message_does_not_echo_secret_values() {
    let sc = scope(&[("k", "TOPSECRET", true)]);
    let s = spec("GET", "http://h:{{k}}/x");
    let e = build_request(&s, &sc, &NoDyn).unwrap_err();
    assert!(!e.to_string().contains("TOPSECRET"), "{e}");
    let pv = preview(&s, &sc, &NoDyn);
    assert!(!format!("{pv:?}").contains("TOPSECRET"));
}

#[test]
fn scheme_less_urls_get_http_and_https_is_kept() {
    assert_eq!(
        build(&spec("GET", "example.com/a"), &VarScope::new()).url,
        "http://example.com/a"
    );
    assert_eq!(
        build(&spec("GET", "https://example.com"), &VarScope::new()).url,
        "https://example.com/"
    );
    assert_eq!(
        build(&spec("GET", "  http://example.com/a  "), &VarScope::new()).url,
        "http://example.com/a"
    );
}

// ---- referenced_variables ----

#[test]
fn referenced_variables_covers_names_urls_paths_and_all_body_kinds_without_dynamic_names() {
    let mut s = spec("GET", "{{base}}/{id}?x={{inurl}}");
    s.params = vec![
        kv(ParamIn::Header, "{{hn}}", "{{hv}}", true),
        kv(ParamIn::Query, "q", "{{$uuid}}", true),
        kv(ParamIn::Cookie, "c", "{{off}}", false),
    ];
    s.body = Body::Multipart(vec![
        MultipartPart {
            name: "{{pn}}".into(),
            value: MultipartValue::File("{{fp}}".into()),
            enabled: true,
            content_type: None,
        },
        MultipartPart {
            name: "{{poff}}".into(),
            value: MultipartValue::Text("{{x}}".into()),
            enabled: false,
            content_type: None,
        },
    ]);
    s.auth = Some(AuthOverride::ApiKey {
        location: ApiKeyIn::Cookie,
        name: "{{an}}".into(),
        value: "{{av}}".into(),
    });
    assert_eq!(
        referenced_variables(&s),
        ["base", "inurl", "hn", "hv", "pn", "fp", "an", "av"]
    );
}

#[test]
fn referenced_variables_is_consistent_with_the_names_that_build_reports_as_unresolved() {
    // build が「未解決」と報告できる名前は、必ず referenced_variables に含まれる
    // （app はこの一覧にあるシークレットだけキーチェーンから読むため、漏れると未解決になってしまう）
    let mut s = spec("POST", "{{a}}/{{ b }}");
    s.params = vec![
        kv(ParamIn::Query, "{{c}}", "{{d}}", true),
        kv(ParamIn::Path, "{{e}}", "{{f}}", true),
        kv(ParamIn::Header, "{{g}}", "{{h}}", true),
        kv(ParamIn::Cookie, "{{i}}", "{{j}}", true),
    ];
    s.body = Body::Form(vec![kv(ParamIn::Query, "{{k}}", "{{l}}", true)]);
    s.auth = Some(AuthOverride::Basic {
        username: "{{m}}".into(),
        password: "{{n}}".into(),
    });
    let referenced = referenced_variables(&s);
    let pv = preview(&s, &VarScope::new(), &NoDyn);
    for u in &pv.unresolved {
        assert!(
            referenced.contains(&u.name),
            "{} が referenced_variables にない",
            u.name
        );
    }
    for name in &referenced {
        assert!(
            pv.unresolved.iter().any(|u| &u.name == name),
            "{name} は未解決として報告されない"
        );
    }
}

// ---- URL の読み取り ----

#[test]
fn a_scheme_less_url_whose_query_contains_a_url_still_gets_http() {
    let pv = preview(
        &spec("GET", "localhost:8080/a?next=http://x"),
        &VarScope::new(),
        &NoDyn,
    );
    assert_eq!(pv.url_error, None, "{}", pv.url);
    assert_eq!(pv.url, "http://localhost:8080/a?next=http://x");
}

#[test]
fn a_dot_dot_path_param_does_not_climb_out_of_the_path() {
    let mut s = spec("GET", "http://h/files/{id}");
    s.params = vec![kv(ParamIn::Path, "id", "..", true)];
    let p = build(&s, &VarScope::new());
    assert!(p.url.starts_with("http://h/files/"), "{}", p.url);
}

#[test]
fn url_oddities_that_work() {
    // 参考: 現状どう読まれるか（挙動の記録）
    assert_eq!(
        build(&spec("GET", "//host/x"), &VarScope::new()).url,
        "http://host/x"
    );
    assert_eq!(
        build(&spec("GET", "http://[::1]:8080/x"), &VarScope::new()).url,
        "http://[::1]:8080/x"
    );
    assert_eq!(
        build(&spec("GET", "http://h/x/../y"), &VarScope::new()).url,
        "http://h/y"
    );
    let mut s = spec("GET", "localhost:8080/a");
    s.params = vec![kv(ParamIn::Query, "q", "http://x", true)];
    assert_eq!(
        build(&s, &VarScope::new()).url,
        "http://localhost:8080/a?q=http%3A%2F%2Fx"
    );
}

// ---- プレビューの URL の見た目 ----

#[test]
fn preview_shows_unresolved_variables_in_query_rows_as_written() {
    let mut s = spec("GET", "http://h/x");
    s.params = vec![
        kv(ParamIn::Query, "q", "{{keyword}}", true),
        kv(ParamIn::Query, "{{ k }}", "a b{{keyword}}&", true),
    ];
    let pv = preview(&s, &VarScope::new(), &NoDyn);
    assert_eq!(
        pv.url,
        "http://h/x?q={{keyword}}&{{ k }}=a%20b{{keyword}}%26"
    );
    assert_eq!(pv.unresolved.len(), 3, "{:?}", pv.unresolved);
    // 解決できる行・変数でない波括弧は、これまでどおりエンコードする
    let sc = scope(&[("ok", "a b", false)]);
    let mut s = spec("GET", "http://h/x/{{missing}}");
    s.params = vec![
        kv(ParamIn::Query, "a", "{{ok}}", true),
        kv(ParamIn::Query, "b", "{x}{{ }}", true),
    ];
    let pv = preview(&s, &sc, &NoDyn);
    assert_eq!(
        pv.url,
        "http://h/x/{{missing}}?a=a%20b&b=%7Bx%7D%7B%7B%20%7D%7D"
    );
}

#[test]
fn preview_shows_unfilled_path_placeholders_as_written_but_the_sent_url_stays_encoded() {
    for params in [
        vec![],
        vec![kv(ParamIn::Path, "clientId", "", true)],
        vec![kv(ParamIn::Path, "clientId", "5", false)],
    ] {
        let mut s = spec("GET", "http://h/clients/{clientId}/x");
        s.params = params;
        let pv = preview(&s, &VarScope::new(), &NoDyn);
        assert_eq!(pv.url, "http://h/clients/{clientId}/x");
        assert_eq!(pv.missing_path_params, ["clientId"]);
        // 送る URL（エンコード）は変えない
        let p = build(&s, &VarScope::new());
        assert_eq!(p.url, "http://h/clients/%7BclientId%7D/x");
    }
    // 埋まっているものは普通に埋まる
    let mut s = spec("GET", "http://h/clients/{clientId}");
    s.params = vec![kv(ParamIn::Path, "clientId", "5", true)];
    let pv = preview(&s, &VarScope::new(), &NoDyn);
    assert_eq!(pv.url, "http://h/clients/5");
    assert!(pv.missing_path_params.is_empty());
}

#[test]
fn an_api_key_header_with_the_same_name_as_a_hand_written_header_is_not_used() {
    let mut s = spec("GET", "http://h/");
    s.params = vec![kv(ParamIn::Header, "X-Api-Key", "manual", true)];
    s.auth = Some(AuthOverride::ApiKey {
        location: ApiKeyIn::Header,
        name: "x-api-key".into(),
        value: "{{unset}}".into(),
    });
    let p = build_request(&s, &VarScope::new(), &NoDyn).unwrap();
    assert_eq!(headers_named(&p, "X-Api-Key"), ["manual"]);
}

#[test]
fn a_scheme_less_url_with_a_url_in_the_path_or_fragment_still_gets_http() {
    for (input, want) in [
        ("h/a/http://x", "http://h/a/http://x"),
        ("h#http://x", "http://h/#http://x"),
        ("HTTPS://h/", "https://h/"),
    ] {
        let p = build(&spec("GET", input), &VarScope::new());
        assert_eq!(p.url, want, "{input}");
    }
}
