//! cookieAuth・CSRF の自動付与（`build_request_with` / `preview_with`）を公開 API だけで確かめる（M5）。

use nekoreq_core::{
    build_request_with, preview_with, ApiKeyIn, AuthOverride, AutoAuth, Body, DynamicSource,
    KeyValue, ParamIn, PreparedRequest, RequestSpec, VarScope, MASK,
};

struct NoDyn;
impl DynamicSource for NoDyn {
    fn eval(&self, _: &str) -> Option<String> {
        None
    }
}

const ORIGIN: &str = "http://localhost:8080";
const SESSION: &str = "SESSION-VALUE-abc123";
const TOKEN: &str = "XSRF-VALUE-xyz789";

fn auto() -> AutoAuth {
    AutoAuth {
        origin: Some(ORIGIN.into()),
        session: Some(SESSION.into()),
        csrf_enabled: true,
        xsrf_token: Some(TOKEN.into()),
        notes: vec![],
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

fn kv(location: ParamIn, name: &str, value: &str, enabled: bool) -> KeyValue {
    KeyValue {
        location,
        name: name.into(),
        value: value.into(),
        enabled,
    }
}

fn build(spec: &RequestSpec, auto: &AutoAuth) -> PreparedRequest {
    build_request_with(spec, &VarScope::new(), &NoDyn, auto).unwrap()
}

fn header<'a>(p: &'a PreparedRequest, name: &str) -> Option<&'a str> {
    p.headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}

fn count(p: &PreparedRequest, name: &str) -> usize {
    p.headers
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case(name))
        .count()
}

#[test]
fn post_gets_cookie_and_header_and_get_gets_only_the_cookie() {
    let url = format!("{ORIGIN}/api/v1/video/clients");
    let p = build(&spec("POST", &url), &auto());
    assert_eq!(
        header(&p, "Cookie"),
        Some(format!("SESSION={SESSION}; XSRF-TOKEN={TOKEN}").as_str())
    );
    assert_eq!(header(&p, "X-XSRF-TOKEN"), Some(TOKEN));

    for m in ["GET", "HEAD", "OPTIONS", "get"] {
        let p = build(&spec(m, &url), &auto());
        assert_eq!(
            header(&p, "Cookie"),
            Some(format!("SESSION={SESSION}; XSRF-TOKEN={TOKEN}").as_str()),
            "{m}"
        );
        assert_eq!(header(&p, "X-XSRF-TOKEN"), None, "{m}");
    }
    for m in ["PUT", "PATCH", "DELETE"] {
        let p = build(&spec(m, &url), &auto());
        assert_eq!(header(&p, "X-XSRF-TOKEN"), Some(TOKEN), "{m}");
    }
}

#[test]
fn each_automation_works_alone_and_off_adds_nothing() {
    let url = format!("{ORIGIN}/x");
    // CSRF だけ
    let a = AutoAuth {
        session: None,
        ..auto()
    };
    let p = build(&spec("POST", &url), &a);
    assert_eq!(
        header(&p, "Cookie"),
        Some(format!("XSRF-TOKEN={TOKEN}").as_str())
    );
    assert_eq!(header(&p, "X-XSRF-TOKEN"), Some(TOKEN));
    // セッションだけ（CSRF オフ）
    let a = AutoAuth {
        csrf_enabled: false,
        xsrf_token: None,
        ..auto()
    };
    let p = build(&spec("POST", &url), &a);
    assert_eq!(
        header(&p, "Cookie"),
        Some(format!("SESSION={SESSION}").as_str())
    );
    assert_eq!(header(&p, "X-XSRF-TOKEN"), None);
    // 何もなし（これまでと同じ）
    let p = build(&spec("POST", &url), &AutoAuth::default());
    assert!(p.headers.is_empty());
    // CSRF オンでもトークンがまだなければ付けない
    let a = AutoAuth {
        session: None,
        xsrf_token: None,
        ..auto()
    };
    let p = build(&spec("POST", &url), &a);
    assert!(p.headers.is_empty());
}

#[test]
fn never_attached_to_another_origin_or_without_a_base_origin() {
    let a = auto();
    for url in [
        "http://evil.example.com/x",
        "https://localhost:8080/x",
        "http://localhost:8081/x",
        "http://localhost/x",
    ] {
        let p = build(&spec("POST", url), &a);
        assert!(p.headers.is_empty(), "{url}: {:?}", p.headers);
        assert!(p.secrets.is_empty(), "{url}");
    }
    // origin が決まっていなければ付けない
    let no_origin = AutoAuth {
        origin: None,
        ..auto()
    };
    assert!(build(&spec("POST", &format!("{ORIGIN}/x")), &no_origin)
        .headers
        .is_empty());
    // ポートの既定値・大文字小文字・パスは同じ origin とみなす
    let a = AutoAuth {
        origin: Some("http://example.com".into()),
        ..auto()
    };
    let p = build(&spec("POST", "HTTP://Example.COM:80/a/b?x=1"), &a);
    assert_eq!(count(&p, "X-XSRF-TOKEN"), 1);
}

#[test]
fn manual_headers_and_cookies_win() {
    let url = format!("{ORIGIN}/x");
    // 手入力の X-XSRF-TOKEN があれば、ヘッダーは足さない（Cookie は足す）
    let mut s = spec("POST", &url);
    s.params = vec![kv(ParamIn::Header, "x-xsrf-token", "MANUAL", true)];
    let p = build(&s, &auto());
    assert_eq!(count(&p, "X-XSRF-TOKEN"), 1);
    assert_eq!(header(&p, "X-XSRF-TOKEN"), Some("MANUAL"));
    assert!(header(&p, "Cookie").unwrap().contains("XSRF-TOKEN="));
    // 無効の行は手入力として数えない
    let mut s = spec("POST", &url);
    s.params = vec![kv(ParamIn::Header, "X-XSRF-TOKEN", "MANUAL", false)];
    assert_eq!(header(&build(&s, &auto()), "X-XSRF-TOKEN"), Some(TOKEN));
    // 手入力の Cookie パラメータ・Cookie ヘッダーに同じ名前があれば足さない
    let mut s = spec("POST", &url);
    s.params = vec![
        kv(ParamIn::Cookie, "SESSION", "MY-SESSION", true),
        kv(
            ParamIn::Header,
            "Cookie",
            "XSRF-TOKEN=MY-XSRF; theme=dark",
            true,
        ),
    ];
    let p = build(&s, &auto());
    assert_eq!(count(&p, "Cookie"), 1);
    let c = header(&p, "Cookie").unwrap();
    assert!(
        c.contains("SESSION=MY-SESSION") && c.contains("XSRF-TOKEN=MY-XSRF"),
        "{c}"
    );
    assert!(!c.contains(SESSION) && !c.contains(TOKEN), "{c}");
    // 手入力の Cookie ヘッダーに別の Cookie があれば、その後ろに足す（Cookie ヘッダーは 1 本のまま）
    let mut s = spec("GET", &url);
    s.params = vec![kv(ParamIn::Header, "Cookie", "theme=dark;", true)];
    let p = build(&s, &auto());
    assert_eq!(count(&p, "Cookie"), 1);
    assert_eq!(
        header(&p, "Cookie"),
        Some(format!("theme=dark; SESSION={SESSION}; XSRF-TOKEN={TOKEN}").as_str())
    );
}

#[test]
fn default_auth_cookie_named_session_is_not_duplicated() {
    // 既定の認証（cookieAuth の apiKey）が SESSION を付けているときは、自動の SESSION を重ねない
    let mut s = spec("POST", &format!("{ORIGIN}/x"));
    s.auth = Some(AuthOverride::ApiKey {
        location: ApiKeyIn::Cookie,
        name: "SESSION".into(),
        value: "FROM-AUTH".into(),
    });
    let p = build(&s, &auto());
    assert_eq!(count(&p, "Cookie"), 1);
    assert_eq!(
        header(&p, "Cookie"),
        Some(format!("SESSION=FROM-AUTH; XSRF-TOKEN={TOKEN}").as_str())
    );
}

#[test]
fn preview_shows_sources_and_masks_values() {
    let url = format!("{ORIGIN}/x");
    let p = preview_with(&spec("POST", &url), &VarScope::new(), &NoDyn, &auto());
    let cookie = p.auto_headers.iter().find(|h| h.name == "Cookie").unwrap();
    assert_eq!(cookie.value, format!("SESSION={MASK}; XSRF-TOKEN={MASK}"));
    assert_eq!(cookie.source, "認証タブ（cookieAuth）、CSRF の自動付与");
    let xsrf = p
        .auto_headers
        .iter()
        .find(|h| h.name == "X-XSRF-TOKEN")
        .unwrap();
    assert_eq!(xsrf.value, MASK);
    assert_eq!(xsrf.source, "CSRF の自動付与");
    // どこにも値が出ない
    let dump = format!("{p:?}");
    assert!(!dump.contains(SESSION) && !dump.contains(TOKEN), "{dump}");
    // GET は Cookie だけ
    let p = preview_with(&spec("GET", &url), &VarScope::new(), &NoDyn, &auto());
    assert!(p.auto_headers.iter().all(|h| h.name != "X-XSRF-TOKEN"));
    assert!(p.auto_headers.iter().any(|h| h.name == "Cookie"));
    // セッションだけなら出どころはひとつ
    let a = AutoAuth {
        csrf_enabled: false,
        xsrf_token: None,
        ..auto()
    };
    let p = preview_with(&spec("GET", &url), &VarScope::new(), &NoDyn, &a);
    assert_eq!(
        p.auto_headers
            .iter()
            .find(|h| h.name == "Cookie")
            .unwrap()
            .source,
        "認証タブ（cookieAuth）"
    );
}

#[test]
fn values_become_secrets_for_redaction() {
    let p = build(&spec("POST", &format!("{ORIGIN}/x")), &auto());
    assert!(p.secrets.iter().any(|s| s == SESSION));
    assert!(p.secrets.iter().any(|s| s == TOKEN));
    // Debug にも出ない
    let d = format!("{p:?}");
    assert!(!d.contains(SESSION) && !d.contains(TOKEN), "{d}");
}

#[test]
fn notes_explain_why_nothing_was_attached() {
    let url = format!("{ORIGIN}/x");
    // トークンがない POST
    let a = AutoAuth {
        session: None,
        xsrf_token: None,
        ..auto()
    };
    let p = preview_with(&spec("POST", &url), &VarScope::new(), &NoDyn, &a);
    assert!(
        p.auto_notes.iter().any(|n| n.contains("xsrfToken")),
        "{:?}",
        p.auto_notes
    );
    // GET では出さない
    let p = preview_with(&spec("GET", &url), &VarScope::new(), &NoDyn, &a);
    assert!(p.auto_notes.is_empty(), "{:?}", p.auto_notes);
    // 送り先が baseUrl と違う
    let p = preview_with(
        &spec("POST", "http://other.example.com/x"),
        &VarScope::new(),
        &NoDyn,
        &auto(),
    );
    assert!(
        p.auto_notes.iter().any(|n| n.contains("baseUrl")),
        "{:?}",
        p.auto_notes
    );
    assert!(p.auto_headers.iter().all(|h| h.name != "Cookie"));
    // 呼び出し側の説明もそのまま届く
    let a = AutoAuth {
        notes: vec!["説明".into()],
        ..AutoAuth::default()
    };
    let p = preview_with(&spec("GET", &url), &VarScope::new(), &NoDyn, &a);
    assert_eq!(p.auto_notes, vec!["説明".to_string()]);
}
