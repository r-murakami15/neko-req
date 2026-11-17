//! テスト担当による独立した検証（core の公開 API だけを使う）。
//!
//! - `Set-Cookie` の解釈（Max-Age・Expires の形・値の形・名前の大文字小文字・同じ名前が複数）
//! - origin の判定（cookieAuth・CSRF を付ける・保存する先の判定）
//! - 値を持つ型の Debug に値が出ないこと

use chrono::{TimeZone, Utc};
use nekoreq_core::{cookie_changes, origin_of, origin_of_base_url, parse_set_cookie, HeaderPair};
use nekoreq_core::{AutoCookieChanges, CookieChange};

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 17, 0, 0, 0).unwrap()
}

fn sc(lines: &[&str]) -> AutoCookieChanges {
    let headers: Vec<HeaderPair> = lines
        .iter()
        .map(|l| HeaderPair {
            name: "Set-Cookie".into(),
            value: (*l).into(),
        })
        .collect();
    cookie_changes(&headers, now())
}

#[derive(Debug, PartialEq)]
enum Exp {
    Set(&'static str),
    Clear,
    Ignore,
}

fn session_of(lines: &[&str]) -> Exp {
    match sc(lines).session {
        Some(CookieChange::Set(v)) => Exp::Set(Box::leak(v.into_boxed_str())),
        Some(CookieChange::Clear) => Exp::Clear,
        None => Exp::Ignore,
    }
}

#[test]
fn max_age_forms() {
    let cases: Vec<(&str, Exp)> = vec![
        ("SESSION=a; Max-Age=0", Exp::Clear),
        ("SESSION=a; Max-Age=-1", Exp::Clear),
        ("SESSION=a; Max-Age=+0", Exp::Clear),
        ("SESSION=a; Max-Age=00", Exp::Clear),
        ("SESSION=a; Max-Age= 0 ", Exp::Clear),
        ("SESSION=a; MAX-AGE=0", Exp::Clear),
        ("SESSION=a; max-age = 0", Exp::Clear),
        ("SESSION=a; Max-Age=-9223372036854775808", Exp::Clear),
        ("SESSION=a; Max-Age=1", Exp::Set("a")),
        ("SESSION=a; Max-Age=3600", Exp::Set("a")),
        // 数値でない Max-Age は無視する（値は保存する）
        ("SESSION=a; Max-Age=abc", Exp::Set("a")),
        ("SESSION=a; Max-Age=1e3", Exp::Set("a")),
        ("SESSION=a; Max-Age=", Exp::Set("a")),
        // i64 に入らない大きな値も、エラーにならず保存する
        ("SESSION=a; Max-Age=99999999999999999999", Exp::Set("a")),
        // Max-Age が Expires に優先する
        (
            "SESSION=a; Max-Age=60; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
            Exp::Set("a"),
        ),
        (
            "SESSION=a; Max-Age=0; Expires=Wed, 21 Oct 2037 07:28:00 GMT",
            Exp::Clear,
        ),
        // 属性の順序に関係なく
        (
            "SESSION=a; Expires=Wed, 21 Oct 2037 07:28:00 GMT; Max-Age=0",
            Exp::Clear,
        ),
    ];
    for (line, want) in cases {
        assert_eq!(session_of(&[line]), want, "{line}");
    }
}

#[test]
fn expires_forms_that_are_in_the_past_clear_and_in_the_future_keep() {
    let past = [
        "Wed, 21 Oct 2015 07:28:00 GMT",
        "Thu, 01-Jan-1970 00:00:00 GMT",
        "Thu, 01-Jan-70 00:00:00 GMT",
        "Sunday, 06-Nov-94 08:49:37 GMT",
        "Thu, 01 Jan 1970 00:00:00 +0000",
        // 「現在」の 1 秒前と、ちょうど今
        "Mon, 16 Nov 2026 23:59:59 GMT",
        "Tue, 17 Nov 2026 00:00:00 GMT",
    ];
    for d in past {
        assert_eq!(
            session_of(&[&format!("SESSION=a; Expires={d}")]),
            Exp::Clear,
            "{d}"
        );
    }
    let future = [
        "Wed, 21 Oct 2037 07:28:00 GMT",
        "Wed, 21-Oct-2037 07:28:00 GMT",
        "Tue, 17 Nov 2026 00:00:01 GMT",
    ];
    for d in future {
        assert_eq!(
            session_of(&[&format!("SESSION=a; Expires={d}")]),
            Exp::Set("a"),
            "{d}"
        );
    }
    // 読めない日付は無視して、値を保存する（セッション Cookie として）
    for d in ["", "garbage", "2037", "Wed, 31 Feb 2037 00:00:00 GMT"] {
        assert_eq!(
            session_of(&[&format!("SESSION=a; Expires={d}")]),
            Exp::Set("a"),
            "{d:?}"
        );
    }
}

/// RFC 6265 の日付の読み方は寛容（asctime 形式・UTC など）。
#[test]
fn rfc6265_tolerant_past_dates_clear_the_cookie() {
    let past = [
        "Thu Jan  1 00:00:00 1970",
        "Thu, 01 Jan 1970 00:00:00 UTC",
        "thu, 01 jan 1970 00:00:00 gmt",
        "Thu, 1 Jan 1970 0:0:0 GMT",
    ];
    let mut wrong = Vec::new();
    for d in past {
        if session_of(&[&format!("SESSION=a; Expires={d}")]) != Exp::Clear {
            wrong.push(d);
        }
    }
    assert!(wrong.is_empty(), "消えなかった形: {wrong:?}");
}

#[test]
fn value_and_name_forms() {
    let cases: Vec<(&str, Exp)> = vec![
        ("SESSION=abc", Exp::Set("abc")),
        ("SESSION=abc;", Exp::Set("abc")),
        ("SESSION=abc;;;Path=/", Exp::Set("abc")),
        ("SESSION=abc; ; Max-Age=0", Exp::Clear),
        ("SESSION = abc", Exp::Set("abc")),
        ("SESSION=a b", Exp::Set("a b")),
        ("SESSION=a=b==", Exp::Set("a=b==")),
        ("SESSION=\"q\"", Exp::Set("\"q\"")),
        ("SESSION=ねこ", Exp::Set("ねこ")),
        ("SESSION=", Exp::Clear),
        ("SESSION=;", Exp::Clear),
        ("SESSION=   ", Exp::Clear),
        ("SESSION", Exp::Ignore),
        ("=abc", Exp::Ignore),
        ("", Exp::Ignore),
        ("session=abc", Exp::Ignore),
        ("Session=abc", Exp::Ignore),
        ("SESSION2=abc", Exp::Ignore),
        ("__Host-SESSION=abc", Exp::Ignore),
        ("XSESSION=abc", Exp::Ignore),
    ];
    for (line, want) in cases {
        assert_eq!(session_of(&[line]), want, "{line:?}");
    }
}

#[test]
fn several_cookies_in_one_response_the_last_of_each_name_wins() {
    let c = sc(&[
        "SESSION=a; Path=/",
        "XSRF-TOKEN=x1",
        "other=1",
        "SESSION=b",
        "XSRF-TOKEN=; Max-Age=0",
        "XSRF-TOKEN=x2",
    ]);
    assert_eq!(c.session, Some(CookieChange::Set("b".into())));
    assert_eq!(c.xsrf, Some(CookieChange::Set("x2".into())));
    // 削除のあとの再発行 / 再発行のあとの削除
    assert_eq!(
        sc(&["SESSION=; Max-Age=0", "SESSION=new"]).session,
        Some(CookieChange::Set("new".into()))
    );
    assert_eq!(
        sc(&["SESSION=new", "SESSION=; Max-Age=0"]).session,
        Some(CookieChange::Clear)
    );
    // ヘッダー名の大文字小文字・前後の空白
    for name in ["set-cookie", "SET-COOKIE", " Set-Cookie "] {
        let h = [HeaderPair {
            name: name.into(),
            value: "SESSION=v".into(),
        }];
        assert_eq!(
            cookie_changes(&h, now()).session,
            Some(CookieChange::Set("v".into())),
            "{name:?}"
        );
    }
    // Set-Cookie 以外のヘッダーは見ない
    let h = [HeaderPair {
        name: "X-Set-Cookie".into(),
        value: "SESSION=v".into(),
    }];
    assert!(cookie_changes(&h, now()).is_empty());
    assert!(sc(&[]).is_empty());
}

#[test]
fn attributes_other_than_expiry_do_not_change_the_value() {
    let c = parse_set_cookie(
        "SESSION=v; Domain=other.example; Path=/admin; Secure; HttpOnly; SameSite=None; Partitioned; Priority=High",
    )
    .unwrap();
    assert_eq!(c.name, "SESSION");
    assert_eq!(c.value, "v");
    assert!(!c.is_deletion(now()));
}

#[test]
fn debug_output_never_contains_cookie_values() {
    let line = "SESSION=DEBUG-SECRET-1; Path=/";
    let c = parse_set_cookie(line).unwrap();
    assert!(!format!("{c:?}").contains("DEBUG-SECRET-1"));
    let changes = sc(&[line, "XSRF-TOKEN=DEBUG-SECRET-2"]);
    let d = format!("{changes:?}");
    assert!(!d.contains("DEBUG-SECRET"), "{d}");
    let pretty = format!("{changes:#?}");
    assert!(!pretty.contains("DEBUG-SECRET"), "{pretty}");
}

#[test]
fn origin_table() {
    let cases: Vec<(&str, Option<&str>)> = vec![
        ("http://localhost:8080/api", Some("http://localhost:8080")),
        ("HTTP://LOCALHOST:8080", Some("http://localhost:8080")),
        ("http://localhost:80/", Some("http://localhost")),
        ("https://x.example:443", Some("https://x.example")),
        ("https://x.example:8443", Some("https://x.example:8443")),
        ("http://[::1]:8080/p", Some("http://[::1]:8080")),
        ("http://[0:0:0:0:0:0:0:1]/", Some("http://[::1]")),
        ("http://user:pw@host/x", Some("http://host")),
        ("http://127.0.0.1:1/?q=1#f", Some("http://127.0.0.1:1")),
        ("ftp://host/", None),
        ("file:///tmp/x", None),
        ("localhost:8080", None),
        ("//host/x", None),
        ("http://", None),
        ("", None),
        ("{{baseUrl}}/x", None),
        // 末尾のドットがあるホストは、ないホストと別の origin（ブラウザと同じ）
        ("http://example.com./", Some("http://example.com.")),
        ("http://example.com/", Some("http://example.com")),
        // localhost と 127.0.0.1 も別の origin
        ("http://127.0.0.1:8080", Some("http://127.0.0.1:8080")),
    ];
    for (url, want) in cases {
        assert_eq!(origin_of(url).as_deref(), want, "{url:?}");
    }
    let base_cases: Vec<(&str, Option<&str>)> = vec![
        ("localhost:8080", Some("http://localhost:8080")),
        ("127.0.0.1:8080/api", Some("http://127.0.0.1:8080")),
        ("example.com:8080", Some("http://example.com:8080")),
        ("  http://localhost:8080/  ", Some("http://localhost:8080")),
        (
            "https://api.example.com/v1/",
            Some("https://api.example.com"),
        ),
        ("api.example.com", Some("http://api.example.com")),
        ("", None),
        ("   ", None),
        ("a b", None),
    ];
    for (base, want) in base_cases {
        assert_eq!(origin_of_base_url(base).as_deref(), want, "{base:?}");
    }
}
