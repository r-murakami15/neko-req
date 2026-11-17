//! cookieAuth と CSRF の自動化（M5）。
//!
//! - cookieAuth: 送信するときは環境変数 `session` を Cookie `SESSION` として付け、レスポンスの
//!   `Set-Cookie: SESSION=…` を受けたら環境の `session`（シークレット）を更新する。
//! - CSRF: レスポンスの `Set-Cookie: XSRF-TOKEN=…` を環境の `xsrfToken`（シークレット）に保持し、
//!   送るときは Cookie に `XSRF-TOKEN=…` を入れ、GET・HEAD・OPTIONS 以外では `X-XSRF-TOKEN` ヘッダーも付ける
//!   （Spring の Cookie 方式は Cookie の値とヘッダーの値を照合するため、両方そろえる）。
//! - ここは組み立て用の値（`AutoAuth`）と `Set-Cookie` の解釈だけを持つ。保存（環境・キーチェーン）は app の仕事。
//! - 自動で付けるのは、送り先の origin が環境の `baseUrl` と同じときだけ（ブラウザの Cookie がドメインで
//!   分かれるのと同じ。別のサイトにセッションを送らず、別のサイトの `Set-Cookie` でセッションを上書きしない）。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::history::HeaderPair;
use crate::redact::MASK;

/// セッションの Cookie の名前。
pub const SESSION_COOKIE: &str = "SESSION";
/// セッションを置く環境変数（既定の認証の変数名と同じ）。
pub const SESSION_VAR: &str = "session";
/// CSRF トークンの Cookie の名前。
pub const XSRF_COOKIE: &str = "XSRF-TOKEN";
/// CSRF トークンを送るヘッダーの名前。
pub const XSRF_HEADER: &str = "X-XSRF-TOKEN";
/// CSRF トークンを置く環境変数。
pub const XSRF_VAR: &str = "xsrfToken";
/// 送り先の origin を決める環境変数（servers から作った環境にある）。
pub const BASE_URL_VAR: &str = "baseUrl";

/// 自動付与の出どころ（プレビューの `source`）。
pub const SOURCE_SESSION: &str = "認証タブ（cookieAuth）";
pub const SOURCE_CSRF: &str = "CSRF の自動付与";

/// CSRF のヘッダーを付けないメソッド（Cookie の XSRF-TOKEN は付ける）。
pub fn is_csrf_safe_method(method: &str) -> bool {
    ["GET", "HEAD", "OPTIONS"]
        .iter()
        .any(|m| method.trim().eq_ignore_ascii_case(m))
}

/// 組み立てるときに使う、自動化の値。値は呼び出し側が（設定・環境・キーチェーンから）決めて渡す。
#[derive(Clone, Default, PartialEq, Eq)]
pub struct AutoAuth {
    /// 自動で付けてよい送り先の origin（環境の `baseUrl` から）。`None` なら何も付けない。
    pub origin: Option<String>,
    /// Cookie `SESSION` に入れる値（自動化がオンで、値があるとき）。
    pub session: Option<String>,
    /// CSRF の自動付与がオン。
    pub csrf_enabled: bool,
    /// `XSRF-TOKEN` の値（オンで、値があるとき）。
    pub xsrf_token: Option<String>,
    /// プレビューに添える、自動化についての説明（付かない理由など）。
    pub notes: Vec<String>,
}

// 値を持つので、Debug に出さない
impl std::fmt::Debug for AutoAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutoAuth")
            .field("origin", &self.origin)
            .field("session", &self.session.as_ref().map(|_| MASK))
            .field("csrf_enabled", &self.csrf_enabled)
            .field("xsrf_token", &self.xsrf_token.as_ref().map(|_| MASK))
            .field("notes", &self.notes)
            .finish()
    }
}

/// URL の origin（`http://host:8080`。スキーム・ホストは小文字、既定のポートは省く）。http / https 以外は `None`。
pub fn origin_of(url: &str) -> Option<String> {
    let u = url::Url::parse(url.trim()).ok()?;
    if !matches!(u.scheme(), "http" | "https") || u.host().is_none() {
        return None;
    }
    Some(u.origin().ascii_serialization())
}

/// `baseUrl` のようにスキームが省かれた値も読む（`localhost:8080` は http として）。
pub fn origin_of_base_url(base_url: &str) -> Option<String> {
    let t = base_url.trim();
    if t.is_empty() {
        return None;
    }
    origin_of(t).or_else(|| origin_of(&format!("http://{t}")))
}

// ---- Set-Cookie ----

/// `Set-Cookie` の 1 件（名前・値と、消すかどうかの判定に使う属性だけ）。
#[derive(Clone, PartialEq, Eq)]
pub struct SetCookie {
    pub name: String,
    pub value: String,
    pub max_age: Option<i64>,
    pub expires: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for SetCookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetCookie")
            .field("name", &self.name)
            .field("value", &MASK)
            .field("max_age", &self.max_age)
            .field("expires", &self.expires)
            .finish()
    }
}

impl SetCookie {
    /// Cookie を消す指示か。値が空、`Max-Age` が 0 以下、（`Max-Age` がなければ）`Expires` が過去。
    pub fn is_deletion(&self, now: DateTime<Utc>) -> bool {
        if self.value.is_empty() {
            return true;
        }
        match (self.max_age, self.expires) {
            (Some(age), _) => age <= 0,
            (None, Some(at)) => at <= now,
            (None, None) => false,
        }
    }
}

/// `Set-Cookie` ヘッダーの値を読む。`name=value` がなければ `None`。
pub fn parse_set_cookie(header: &str) -> Option<SetCookie> {
    let mut parts = header.split(';');
    let (name, value) = parts.next()?.split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let mut cookie = SetCookie {
        name: name.to_string(),
        value: value.trim().to_string(),
        max_age: None,
        expires: None,
    };
    for attr in parts {
        let (key, val) = match attr.split_once('=') {
            Some((k, v)) => (k.trim(), v.trim()),
            None => (attr.trim(), ""),
        };
        if key.eq_ignore_ascii_case("max-age") {
            // 数値でなければ無視する（RFC 6265: 最初が数字か `-` でなければ属性ごと無視）
            if let Ok(n) = val.parse::<i64>() {
                cookie.max_age = Some(n);
            }
        } else if key.eq_ignore_ascii_case("expires") {
            cookie.expires = parse_http_date(val);
        }
    }
    Some(cookie)
}

/// Cookie の `Expires` の日付。RFC 6265 5.1.1 の寛容な読み方（区切り文字で分けた語から、時刻 `h:m:s`・日・月名
/// （先頭 3 文字）・年を順に拾う）。`Wed, 21 Oct 2015 07:28:00 GMT`・`Thu, 01-Jan-1970 00:00:00 GMT`・
/// asctime 形式（`Thu Jan  1 00:00:00 1970`）・タイムゾーンが `UTC` などの形も読める（タイムゾーンは見ず UTC として扱う）。
/// 年は 2 桁なら 70〜99 を 19xx、0〜69 を 20xx とする。足りない・範囲外・存在しない日付は `None`。
fn parse_http_date(text: &str) -> Option<DateTime<Utc>> {
    use chrono::{NaiveDate, NaiveTime};
    let is_delimiter = |c: char| matches!(c, '\u{09}' | '\u{20}'..='\u{2F}' | '\u{3B}'..='\u{40}' | '\u{5B}'..='\u{60}' | '\u{7B}'..='\u{7E}');
    // 先頭の数字の並び（長さ `min..=max` で、次が数字でないもの）を数値にする
    let leading_number = |token: &str, min: usize, max: usize| -> Option<u32> {
        let digits = token.chars().take_while(char::is_ascii_digit).count();
        (min..=max)
            .contains(&digits)
            .then(|| token[..digits].parse().ok())
            .flatten()
    };
    let mut time: Option<(u32, u32, u32)> = None;
    let mut day: Option<u32> = None;
    let mut month: Option<u32> = None;
    let mut year: Option<i32> = None;
    for token in text.trim().trim_matches('"').split(is_delimiter) {
        if token.is_empty() {
            continue;
        }
        if time.is_none() {
            let mut fields = token.splitn(3, ':');
            if let (Some(h), Some(m), Some(sec)) = (fields.next(), fields.next(), fields.next()) {
                let exact = |t: &str| {
                    (t.len() <= 2 && t.bytes().all(|b| b.is_ascii_digit()))
                        .then(|| leading_number(t, 1, 2))
                        .flatten()
                };
                if let (Some(h), Some(m), Some(sec)) =
                    (exact(h), exact(m), leading_number(sec, 1, 2))
                {
                    time = Some((h, m, sec));
                    continue;
                }
            }
        }
        if day.is_none() {
            if let Some(d) = leading_number(token, 1, 2) {
                day = Some(d);
                continue;
            }
        }
        if month.is_none() {
            if let Some(m) = token.get(..3).and_then(|head| {
                const NAMES: [&str; 12] = [
                    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov",
                    "dec",
                ];
                NAMES
                    .iter()
                    .position(|n| head.eq_ignore_ascii_case(n))
                    .and_then(|i| u32::try_from(i + 1).ok())
            }) {
                month = Some(m);
                continue;
            }
        }
        if year.is_none() {
            if let Some(y) = leading_number(token, 2, 4) {
                let y = i32::try_from(y).ok()?;
                let two_digits = !token.as_bytes().get(2).is_some_and(u8::is_ascii_digit);
                year = Some(match y {
                    70..=99 if two_digits => y + 1900,
                    0..=69 if two_digits => y + 2000,
                    _ => y,
                });
            }
        }
    }
    let ((h, m, s), day, month, year) = (time?, day?, month?, year?);
    if year < 1601 || !(1..=31).contains(&day) {
        return None;
    }
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    let time = NaiveTime::from_hms_opt(h, m, s)?;
    Some(date.and_time(time).and_utc())
}

/// 保持している値に対する変更。
#[derive(Clone, PartialEq, Eq)]
pub enum CookieChange {
    /// この値に更新する。
    Set(String),
    /// 消された（`Max-Age=0`・期限切れ・空の値）ので、値を消す。
    Clear,
}

impl std::fmt::Debug for CookieChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CookieChange::Set(_) => f.write_str("Set(••••)"),
            CookieChange::Clear => f.write_str("Clear"),
        }
    }
}

/// レスポンスから読み取った、`SESSION` と `XSRF-TOKEN` の変更（なければ `None`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutoCookieChanges {
    pub session: Option<CookieChange>,
    pub xsrf: Option<CookieChange>,
}

impl AutoCookieChanges {
    pub fn is_empty(&self) -> bool {
        self.session.is_none() && self.xsrf.is_none()
    }
}

/// レスポンスのヘッダーの `Set-Cookie` から、`SESSION` と `XSRF-TOKEN` の変更を読む。
/// 同じ名前が何度も出てきたら、あとのものが勝つ（ブラウザと同じ）。`now` は `Expires` との比較に使う。
pub fn cookie_changes(headers: &[HeaderPair], now: DateTime<Utc>) -> AutoCookieChanges {
    let mut out = AutoCookieChanges::default();
    for h in headers
        .iter()
        .filter(|h| h.name.trim().eq_ignore_ascii_case("set-cookie"))
    {
        let Some(c) = parse_set_cookie(&h.value) else {
            continue;
        };
        let change = if c.is_deletion(now) {
            CookieChange::Clear
        } else {
            CookieChange::Set(c.value.clone())
        };
        if c.name == SESSION_COOKIE {
            out.session = Some(change);
        } else if c.name == XSRF_COOKIE {
            out.xsrf = Some(change);
        }
    }
    out
}

// ---- 送信結果に付ける「自動で更新した変数」 ----

/// 自動更新の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum AutoUpdateAction {
    /// 値を更新した（なければシークレットとして作った）。
    Updated,
    /// サーバーに消されたので値を消した。
    Cleared,
}

/// 送信のあとに自動で更新した環境変数 1 件（値は出さない）。画面で「session を更新しました」と出す。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AutoUpdate {
    /// 変数名（`session` / `xsrfToken`）。
    pub name: String,
    pub environment_id: String,
    pub environment_name: String,
    pub action: AutoUpdateAction,
}

/// 自動更新できなかったもの（キーチェーンに書けないなど）。送信自体は成功している。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AutoUpdateSkip {
    /// 変数名（`session` / `xsrfToken`）。
    pub name: String,
    /// 画面にそのまま出せる理由。値は含まない。
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 17, 12, 0, 0).unwrap()
    }

    fn h(name: &str, value: &str) -> HeaderPair {
        HeaderPair {
            name: name.into(),
            value: value.into(),
        }
    }

    #[test]
    fn parses_spring_style_cookies() {
        let c = parse_set_cookie("SESSION=abc123; Path=/; HttpOnly; SameSite=Lax").unwrap();
        assert_eq!((c.name.as_str(), c.value.as_str()), ("SESSION", "abc123"));
        assert!(!c.is_deletion(now()));
        let c = parse_set_cookie("XSRF-TOKEN=tok==; Path=/; Max-Age=3600").unwrap();
        assert_eq!(c.value, "tok==");
        assert_eq!(c.max_age, Some(3600));
        assert!(!c.is_deletion(now()));
    }

    #[test]
    fn deletion_by_max_age_expires_or_empty_value() {
        let del = |s: &str| parse_set_cookie(s).unwrap().is_deletion(now());
        assert!(del("SESSION=; Path=/"));
        assert!(del("SESSION=x; Max-Age=0"));
        assert!(del("SESSION=x; max-age=-1"));
        assert!(del("SESSION=x; Expires=Thu, 01 Jan 1970 00:00:00 GMT"));
        assert!(del("SESSION=x; Expires=Thu, 01-Jan-1970 00:00:00 GMT"));
        assert!(del("SESSION=x; Expires=Mon, 16 Nov 2026 12:00:00 GMT"));
        // 未来の期限は消さない
        assert!(!del("SESSION=x; Expires=Wed, 21 Oct 2037 07:28:00 GMT"));
        // Max-Age があれば Expires より優先する
        assert!(!del(
            "SESSION=x; Max-Age=60; Expires=Thu, 01 Jan 1970 00:00:00 GMT"
        ));
        assert!(del(
            "SESSION=x; Max-Age=0; Expires=Wed, 21 Oct 2037 07:28:00 GMT"
        ));
        // 読めない属性は無視する
        assert!(!del("SESSION=x; Max-Age=abc; Expires=いつか"));
    }

    #[test]
    fn rejects_cookies_without_name_or_equals() {
        assert!(parse_set_cookie("novalue; Path=/").is_none());
        assert!(parse_set_cookie("=x").is_none());
        assert!(parse_set_cookie("").is_none());
    }

    #[test]
    fn changes_pick_session_and_xsrf_and_the_last_one_wins() {
        let headers = vec![
            h("Content-Type", "application/json"),
            h("Set-Cookie", "SESSION=first; Path=/"),
            h("set-cookie", "OTHER=1"),
            h("set-cookie", "XSRF-TOKEN=t1; Path=/"),
            h("Set-Cookie", "SESSION=second; Path=/"),
        ];
        let c = cookie_changes(&headers, now());
        assert_eq!(c.session, Some(CookieChange::Set("second".into())));
        assert_eq!(c.xsrf, Some(CookieChange::Set("t1".into())));
        // 消された
        let c = cookie_changes(
            &[
                h("Set-Cookie", "SESSION=; Max-Age=0; Path=/"),
                h(
                    "Set-Cookie",
                    "XSRF-TOKEN=x; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
                ),
            ],
            now(),
        );
        assert_eq!(c.session, Some(CookieChange::Clear));
        assert_eq!(c.xsrf, Some(CookieChange::Clear));
        assert!(cookie_changes(&[h("Set-Cookie", "other=1")], now()).is_empty());
        // 名前は大文字小文字を区別する
        assert!(cookie_changes(&[h("Set-Cookie", "session=x")], now()).is_empty());
    }

    #[test]
    fn debug_never_prints_values() {
        let a = AutoAuth {
            origin: Some("http://h".into()),
            session: Some("SESS-SECRET".into()),
            csrf_enabled: true,
            xsrf_token: Some("TOK-SECRET".into()),
            notes: vec![],
        };
        let d = format!("{a:?}");
        assert!(
            !d.contains("SESS-SECRET") && !d.contains("TOK-SECRET"),
            "{d}"
        );
        let c = parse_set_cookie("SESSION=SESS-SECRET").unwrap();
        assert!(!format!("{c:?}").contains("SESS-SECRET"));
        assert!(!format!("{:?}", CookieChange::Set("SESS-SECRET".into())).contains("SESS-SECRET"));
    }

    #[test]
    fn origins_are_normalized() {
        assert_eq!(
            origin_of("HTTP://Example.com:80/a/b?x=1").as_deref(),
            Some("http://example.com")
        );
        assert_eq!(
            origin_of("http://host.docker.internal:8080/api").as_deref(),
            Some("http://host.docker.internal:8080")
        );
        assert_eq!(origin_of("ftp://x/").as_deref(), None);
        assert_eq!(origin_of("{{baseUrl}}/x"), None);
        assert_eq!(
            origin_of_base_url("localhost:8080").as_deref(),
            Some("http://localhost:8080")
        );
        assert_eq!(origin_of_base_url("  "), None);
    }

    #[test]
    fn safe_methods() {
        for m in ["GET", "head", " Options "] {
            assert!(is_csrf_safe_method(m), "{m}");
        }
        for m in ["POST", "PUT", "PATCH", "DELETE"] {
            assert!(!is_csrf_safe_method(m), "{m}");
        }
    }
}
