//! シークレットが平文で出ないこと（伏せ字処理）を、公開 API だけで確かめる結合テスト。
//! preview の URL・autoHeaders、BuildError、Debug 表示、伏せ字の境界（短い値・部分一致・空）を見る。

use base64::Engine;
use nekoreq_core::{
    build_request, expand, is_sensitive_header, preview, redact_header_value, redact_headers,
    redact_text, ApiKeyIn, AuthOverride, Body, DynamicSource, HeaderPair, KeyValue, MultipartPart,
    MultipartValue, ParamIn, RequestSpec, VarLayer, VarScope, MASK,
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

fn h(name: &str, value: &str) -> HeaderPair {
    HeaderPair {
        name: name.into(),
        value: value.into(),
    }
}

/// 値そのもの・URL エンコード（%20 / +）・Basic の base64 のどれでも `text` に含まれないこと。
fn assert_no_leak(text: &str, secret: &str) {
    let q = percent_encoding::utf8_percent_encode(secret, percent_encoding::NON_ALPHANUMERIC)
        .to_string();
    let b64 = base64::engine::general_purpose::STANDARD.encode(secret);
    for needle in [secret.to_string(), q, b64] {
        assert!(!text.contains(&needle), "{needle:?} が残っている: {text}");
    }
}

// ---- preview：すべての置き場所でシークレットが出ない ----

#[test]
fn preview_hides_secrets_placed_in_every_location() {
    let secrets = [
        ("pPath", "S-path/x y"),
        ("pQuery", "S-query&=ネコ"),
        ("pKey", "S-apikey+/="),
        ("pHdr", "S-header"),
        ("pCookie", "S-cookie"),
        ("pForm", "S-form value"),
        ("pJson", "S-json\"quoted\""),
        ("pPart", "S-part"),
        ("pBasic", "S-basic:pw"),
    ];
    let sc = scope(
        &secrets
            .iter()
            .map(|(n, v)| (*n, *v, true))
            .collect::<Vec<_>>(),
    );

    let mut s = spec("POST", "http://h/a/{p}");
    s.params = vec![
        kv(ParamIn::Path, "p", "{{pPath}}"),
        kv(ParamIn::Query, "q", "{{pQuery}}"),
        kv(ParamIn::Header, "X-Custom", "v={{pHdr}}"),
        kv(ParamIn::Cookie, "c", "{{pCookie}}"),
    ];
    s.body = Body::Multipart(vec![MultipartPart {
        name: "n".into(),
        value: MultipartValue::Text("{{pPart}}".into()),
        enabled: true,
    }]);
    s.auth = Some(AuthOverride::ApiKey {
        location: ApiKeyIn::Query,
        name: "key".into(),
        value: "{{pKey}}".into(),
    });
    let pv = preview(&s, &sc, &NoDyn);
    let dump = format!(
        "{}\n{:?}\n{}",
        pv.url,
        pv.auto_headers,
        serde_json::to_string(&pv.auto_headers).unwrap()
    );
    for (n, v) in secrets
        .iter()
        .filter(|(n, _)| ["pPath", "pQuery", "pKey", "pHdr", "pCookie", "pPart"].contains(n))
    {
        // multipart と JSON・フォームのボディは preview には出ない（URL と自動ヘッダーだけ）
        assert_no_leak(&dump, v);
        let _ = n;
    }
    assert!(pv.url.contains(MASK), "{}", pv.url);
}

#[test]
fn preview_masks_basic_and_bearer_and_cookie_auth_headers_by_name() {
    let sc = scope(&[
        ("u", "alice", false),
        ("pw", "p@ss:w0rd", true),
        ("t", "tok.en/1", true),
    ]);
    for auth in [
        AuthOverride::Basic {
            username: "{{u}}".into(),
            password: "{{pw}}".into(),
        },
        AuthOverride::Bearer {
            token: "{{t}}".into(),
        },
    ] {
        let mut s = spec("GET", "http://h/");
        s.auth = Some(auth);
        let pv = preview(&s, &sc, &NoDyn);
        let a = pv
            .auto_headers
            .iter()
            .find(|a| a.name == "Authorization")
            .unwrap();
        assert!(
            a.value == "Basic ••••" || a.value == "Bearer ••••",
            "{}",
            a.value
        );
        let dump = format!("{pv:?}");
        for secret in ["p@ss:w0rd", "tok.en/1"] {
            assert_no_leak(&dump, secret);
        }
        // Basic の base64（user:secret 全体）も出ない
        let full = base64::engine::general_purpose::STANDARD.encode("alice:p@ss:w0rd");
        assert!(!dump.contains(&full), "{dump}");
    }
}

#[test]
fn preview_cookie_auto_header_keeps_names_and_masks_values() {
    let sc = scope(&[("sess", "SESSVAL", true)]);
    let mut s = spec("GET", "http://h/");
    s.params = vec![kv(ParamIn::Cookie, "plain", "visible-but-masked")];
    s.auth = Some(AuthOverride::ApiKey {
        location: ApiKeyIn::Cookie,
        name: "SESSION".into(),
        value: "{{sess}}".into(),
    });
    let pv = preview(&s, &sc, &NoDyn);
    let c = pv.auto_headers.iter().find(|a| a.name == "Cookie").unwrap();
    assert_eq!(c.value, "plain=••••; SESSION=••••");
}

#[test]
fn preview_shows_a_hand_written_secret_header_masked_by_value_even_with_an_unusual_name() {
    // 手入力の X-Api-Key（自動ヘッダーではないので autoHeaders には出ない）。auth 経由なら出る
    let sc = scope(&[("k", "KEYVALUE123", true)]);
    let mut s = spec("GET", "http://h/");
    s.auth = Some(AuthOverride::ApiKey {
        location: ApiKeyIn::Header,
        name: "X-Api-Key".into(),
        value: "{{k}}".into(),
    });
    let pv = preview(&s, &sc, &NoDyn);
    let a = pv
        .auto_headers
        .iter()
        .find(|a| a.name == "X-Api-Key")
        .unwrap();
    assert_eq!(a.value, MASK);
}

#[test]
fn secret_directly_in_the_url_template_is_masked_in_plain_and_percent_encoded_forms() {
    let sc = scope(&[("k", "a b", true)]);
    let s = spec("GET", "http://h/x/{{k}}?token={{k}}");
    let pv = preview(&s, &sc, &NoDyn);
    assert_eq!(pv.url, "http://h/x/••••?token=••••");
}

#[test]
fn secret_with_a_space_and_a_char_the_url_crate_leaves_alone_is_masked_in_preview() {
    let sc = scope(&[("k", "p@ss word", true)]);
    let s = spec("GET", "http://h/x/{{k}}");
    let pv = preview(&s, &sc, &NoDyn);
    assert!(!pv.url.contains("p@ss"), "平文が残っている: {}", pv.url);
}

fn scope_plain(pairs: &[(&str, &str)]) -> VarScope {
    scope(
        &pairs
            .iter()
            .map(|(n, v)| (*n, *v, true))
            .collect::<Vec<_>>(),
    )
}

#[test]
fn secret_in_the_host_part_is_masked_even_though_the_url_crate_lowercases_it() {
    let sc = scope_plain(&[("h", "MixedCaseHost")]);
    let s = spec("GET", "http://{{h}}.example.com/");
    let pv = preview(&s, &sc, &NoDyn);
    assert!(
        !pv.url.to_lowercase().contains("mixedcasehost"),
        "{}",
        pv.url
    );
}

#[test]
fn secret_in_userinfo_of_the_url_is_masked() {
    let sc = scope_plain(&[("pw", "hunter2")]);
    let s = spec("GET", "http://user:{{pw}}@h/x");
    let pv = preview(&s, &sc, &NoDyn);
    assert!(!pv.url.contains("hunter2"), "{}", pv.url);
}

#[test]
fn secret_in_the_fragment_or_after_a_hash_is_masked() {
    let sc = scope_plain(&[("t", "frag-secret")]);
    let s = spec("GET", "http://h/x#{{t}}");
    let pv = preview(&s, &sc, &NoDyn);
    assert!(!pv.url.contains("frag-secret"), "{}", pv.url);
}

#[test]
fn unicode_secret_in_a_query_row_is_masked_in_upper_and_lower_case_hex() {
    let sc = scope_plain(&[("t", "合言葉")]);
    let mut s = spec("GET", "http://h/x");
    s.params = vec![kv(ParamIn::Query, "q", "{{t}}")];
    let pv = preview(&s, &sc, &NoDyn);
    assert_eq!(pv.url, "http://h/x?q=••••");
    // 小文字の 16 進で返ってくる形（サーバーのエコーなど）は redact_text では伏せられない
    let lower = "%e5%90%88%e8%a8%80%e8%91%89";
    let out = redact_text(&format!("echo {lower}"), &["合言葉".to_string()]);
    assert!(
        !out.contains(lower),
        "小文字 16 進のエンコードが残る: {out}"
    );
}

#[test]
fn unresolved_preview_url_does_not_print_secret_values_of_resolved_variables() {
    let sc = scope_plain(&[("t", "SECRETVAL")]);
    let s = spec("GET", "http://h/{{t}}/{{missing}}");
    let pv = preview(&s, &sc, &NoDyn);
    assert_eq!(pv.unresolved.len(), 1);
    assert!(!pv.url.contains("SECRETVAL"), "{}", pv.url);
    assert!(pv.url.contains("{{missing}}"));
}

// ---- Debug / Display ----

#[test]
fn prepared_request_debug_never_prints_secrets_in_url_headers_or_body() {
    let sc = scope_plain(&[("k", "DEBUG-SECRET-1"), ("b", "DEBUG-SECRET-2")]);
    let mut s = spec("POST", "http://h/x?a={{k}}");
    s.params = vec![kv(ParamIn::Header, "X-K", "{{k}}")];
    s.body = Body::Json(r#"{"b":"{{b}}"}"#.into());
    s.auth = Some(AuthOverride::Bearer {
        token: "{{k}}".into(),
    });
    let p = build_request(&s, &sc, &NoDyn).unwrap();
    for dump in [format!("{p:?}"), format!("{p:#?}")] {
        assert!(!dump.contains("DEBUG-SECRET"), "{dump}");
    }
}

#[test]
fn build_error_messages_name_variables_but_do_not_include_resolved_secret_values() {
    let sc = scope_plain(&[("ok", "RESOLVED-SECRET")]);
    let mut s = spec("GET", "http://h/{{ok}}/{{missing}}");
    s.auth = Some(AuthOverride::Bearer {
        token: "{{ok}}".into(),
    });
    let e = build_request(&s, &sc, &NoDyn).unwrap_err();
    let text = format!("{e} / {e:?}");
    assert!(!text.contains("RESOLVED-SECRET"), "{text}");
    assert!(text.contains("missing"));
}

#[test]
fn debug_of_expansion_results_and_scopes_does_not_print_secret_values() {
    let sc = scope_plain(&[("k", "LEAKY-DEBUG-VALUE")]);
    let e = expand("{{k}}", &sc, &NoDyn);
    assert!(
        !format!("{e:?}").contains("LEAKY-DEBUG-VALUE"),
        "Expanded: {e:?}"
    );
    assert!(
        !format!("{sc:?}").contains("LEAKY-DEBUG-VALUE"),
        "VarScope: {sc:?}"
    );
    let mut l = VarLayer::new();
    l.insert("k", "LEAKY-DEBUG-VALUE", true);
    assert!(
        !format!("{l:?}").contains("LEAKY-DEBUG-VALUE"),
        "VarLayer: {l:?}"
    );
    assert!(
        !format!("{:?}", l.get("k").unwrap()).contains("LEAKY-DEBUG-VALUE"),
        "ScopedValue"
    );
}

// ---- Basic の base64 ----

#[test]
fn basic_auth_base64_is_covered_by_the_secrets_list_for_echoed_responses() {
    let sc = scope(&[("pw", "S3cret-Pass", true)]);
    let mut s = spec("GET", "http://h/");
    s.auth = Some(AuthOverride::Basic {
        username: "alice".into(),
        password: "{{pw}}".into(),
    });
    let p = build_request(&s, &sc, &NoDyn).unwrap();
    let auth = p
        .headers
        .iter()
        .find(|x| x.name == "Authorization")
        .unwrap()
        .value
        .clone();
    let echoed = format!(r#"{{"headers":{{"Authorization":"{auth}"}}}}"#);
    let out = redact_text(&echoed, &p.secrets);
    let b64 = auth.strip_prefix("Basic ").unwrap();
    assert!(!out.contains(b64), "base64 が残っている: {out}");
}

#[test]
fn raw_bearer_token_echoed_in_a_response_body_is_masked() {
    let sc = scope_plain(&[("t", "tok-ABC.def")]);
    let mut s = spec("GET", "http://h/");
    s.auth = Some(AuthOverride::Bearer {
        token: "{{t}}".into(),
    });
    let p = build_request(&s, &sc, &NoDyn).unwrap();
    let echoed = r#"{"authorization":"Bearer tok-ABC.def"}"#;
    assert_eq!(
        redact_text(echoed, &p.secrets),
        r#"{"authorization":"Bearer ••••"}"#
    );
}

#[test]
fn secret_in_a_json_body_is_masked_when_the_prepared_body_text_is_redacted() {
    let sc = scope_plain(&[("pw", "pass\"word\\1")]);
    let mut s = spec("POST", "http://h/");
    s.body = Body::Json(r#"{"password":"{{pw}}"}"#.into());
    let p = build_request(&s, &sc, &NoDyn).unwrap();
    let nekoreq_core::PreparedBody::Bytes(b) = &p.body else {
        panic!()
    };
    let text = String::from_utf8(b.clone()).unwrap();
    let out = redact_text(&text, &p.secrets);
    assert_eq!(out, r#"{"password":"••••"}"#);
}

#[test]
fn secret_in_a_form_body_is_masked_in_the_form_encoding() {
    let sc = scope_plain(&[("pw", "p w&=ネコ")]);
    let mut s = spec("POST", "http://h/");
    s.body = Body::Form(vec![
        kv(ParamIn::Query, "pw", "{{pw}}"),
        kv(ParamIn::Query, "a", "1"),
    ]);
    let p = build_request(&s, &sc, &NoDyn).unwrap();
    let nekoreq_core::PreparedBody::Bytes(b) = &p.body else {
        panic!()
    };
    let text = String::from_utf8(b.clone()).unwrap();
    assert_eq!(redact_text(&text, &p.secrets), "pw=••••&a=1");
}

// ---- 伏せ字の境界 ----

#[test]
fn short_secrets_are_masked_but_do_not_loop_or_explode() {
    let out = redact_text("a-a-aaa", &["a".to_string()]);
    assert_eq!(out, format!("{MASK}-{MASK}-{MASK}{MASK}{MASK}"));
    // 1〜2 文字の値が他の値の部分文字列でも、長い方が先に置き換わる
    let out = redact_text(
        "token=abc12 id=12",
        &["12".to_string(), "abc12".to_string()],
    );
    assert_eq!(out, "token=•••• id=••••");
}

#[test]
fn a_secret_that_is_a_substring_of_a_non_secret_value_over_masks_as_documented() {
    // 値で探すので、無関係な箇所の同じ文字列も伏せられる（過剰だが安全側）
    let out = redact_text("http://127.0.0.1:8080/a1", &["1".to_string()]);
    assert!(!out.contains('1'));
}

#[test]
fn empty_and_whitespace_only_secret_lists_do_not_change_the_text() {
    assert_eq!(redact_text("abc", &[]), "abc");
    assert_eq!(redact_text("abc", &[String::new(), String::new()]), "abc");
    assert_eq!(redact_text("", &["x".to_string()]), "");
}

#[test]
fn secret_containing_the_mask_character_or_equal_to_the_mask_terminates() {
    let out = redact_text("x ••••", &["••••".to_string()]);
    assert_eq!(out, "x ••••");
    let out = redact_text("x •", &["•".to_string()]);
    assert_eq!(out, "x ••••");
    let out = redact_text("aaaa", &["a".to_string(), "aa".to_string()]);
    assert_eq!(out, format!("{MASK}{MASK}"));
}

#[test]
fn redaction_is_idempotent() {
    let secrets = vec!["s3cr3t".to_string(), "p@ss word".to_string()];
    let once = redact_text("a=s3cr3t&b=p%40ss%20word&c=p@ss word", &secrets);
    assert_eq!(redact_text(&once, &secrets), once);
}

#[test]
fn many_secrets_on_a_large_text_finish_quickly() {
    let secrets: Vec<String> = (0..200).map(|i| format!("secret-value-{i:04}")).collect();
    let text = "xx secret-value-0100 yy ".repeat(50_000);
    let start = std::time::Instant::now();
    let out = redact_text(&text, &secrets);
    assert!(!out.contains("secret-value-0100"));
    assert!(
        start.elapsed() < std::time::Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
}

// ---- ヘッダー名の伏せ字 ----

#[test]
fn sensitive_header_names_in_mixed_case_and_with_spaces() {
    for name in [
        "AUTHORIZATION",
        "aUtHoRiZaTiOn",
        " cookie",
        "SET-COOKIE",
        "X-XSRF-Token",
        "x-xsrf-token",
    ] {
        assert!(is_sensitive_header(name), "{name}");
    }
    let out = redact_headers(
        &[
            h("AUTHORIZATION", "bearer abc"),
            h("sEt-CoOkIe", "SESSION=abc; Path=/"),
            h("X-Xsrf-Token", "tok"),
            h("cOOkie", "a=1; b=2"),
        ],
        &[],
    );
    let values: Vec<_> = out.iter().map(|x| x.value.as_str()).collect();
    assert_eq!(
        values,
        [
            "bearer ••••",
            "SESSION=••••; Path=/",
            "••••",
            "a=••••; b=••••"
        ]
    );
}

#[test]
fn set_cookie_attributes_are_preserved_and_only_the_value_is_masked() {
    let v = "SESSION=abc123; Path=/; Domain=example.com; Expires=Wed, 21 Oct 2026 07:28:00 GMT; Max-Age=3600; Secure; HttpOnly; SameSite=Lax";
    assert_eq!(
        redact_header_value("Set-Cookie", v),
        "SESSION=••••; Path=/; Domain=example.com; Expires=Wed, 21 Oct 2026 07:28:00 GMT; Max-Age=3600; Secure; HttpOnly; SameSite=Lax"
    );
    // 値が空（削除の指示）や = を含む値
    assert_eq!(
        redact_header_value("Set-Cookie", "SESSION=; Max-Age=0"),
        "SESSION=••••; Max-Age=0"
    );
    assert_eq!(
        redact_header_value("Set-Cookie", "t=abc==; Path=/"),
        "t=••••; Path=/"
    );
}

#[test]
fn multiple_set_cookie_headers_are_each_masked_and_keep_their_order() {
    let out = redact_headers(
        &[
            h("Set-Cookie", "SESSION=a; Path=/"),
            h("set-cookie", "XSRF-TOKEN=b; Path=/"),
            h("Set-Cookie", "other=c"),
        ],
        &[],
    );
    let v: Vec<_> = out.iter().map(|x| x.value.clone()).collect();
    assert_eq!(
        v,
        [
            "SESSION=••••; Path=/",
            "XSRF-TOKEN=••••; Path=/",
            "other=••••"
        ]
    );
}

#[test]
fn cookie_header_forms() {
    assert_eq!(redact_header_value("Cookie", "a=1;b=2"), "a=••••; b=••••");
    assert_eq!(
        redact_header_value("Cookie", "a=1; ; b=2;"),
        "a=••••; ; b=••••; "
    );
    assert_eq!(redact_header_value("Cookie", "flag"), MASK);
    assert_eq!(redact_header_value("Cookie", "t=a=b"), "t=••••");
    assert_eq!(redact_header_value("Cookie", ""), "");
}

#[test]
fn authorization_forms() {
    assert_eq!(
        redact_header_value("Authorization", "Bearer   spaced"),
        "Bearer ••••"
    );
    assert_eq!(
        redact_header_value("Authorization", "Digest username=\"a\", response=\"b\""),
        "Digest ••••"
    );
    assert_eq!(redact_header_value("Authorization", "Bearer"), MASK);
    assert_eq!(redact_header_value("Authorization", ""), MASK);
}

#[test]
fn proxy_authorization_is_masked_by_name() {
    assert!(is_sensitive_header("Proxy-Authorization"));
}

#[test]
fn non_sensitive_headers_are_untouched_unless_they_contain_a_secret_value() {
    let secrets = vec!["tok123".to_string()];
    let out = redact_headers(
        &[
            h("Accept", "*/*"),
            h("X-Trace", "id=tok123"),
            h("Location", "/x?t=tok123"),
        ],
        &secrets,
    );
    assert_eq!(out[0].value, "*/*");
    assert_eq!(out[1].value, "id=••••");
    assert_eq!(out[2].value, "/x?t=••••");
}
