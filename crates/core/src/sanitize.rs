//! リクエストの入力（`RequestSpec`）から、手で書かれたシークレットを取り除く。
//!
//! シークレットは環境変数（キーチェーン）に置いて `{{変数}}` で参照するのが決まりだが、認証欄やヘッダー・Cookie に
//! 値を直接書くこともできる。そうした値を履歴（SQLite）やタブの復元（SQLite）に平文で残さないために使う。
//!
//! - 対象: 認証の Bearer のトークン・Basic のパスワード・API キーの値、`Authorization`・`Proxy-Authorization`・
//!   `Cookie`・`X-XSRF-TOKEN` ヘッダーの値、Cookie パラメータの値。
//! - `{{変数}}` だけでできた値（`{{token}}`・`Bearer {{token}}`）はテンプレートなのでそのまま残す。
//! - 置き換え先は `Fill::Mask`（`••••`。履歴の表示用）か `Fill::Empty`（空。タブの復元用）。
//! - 取り除いた値（`literals`）は、履歴のレスポンスのボディにエコーされたときに伏せるのに使う。

use base64::Engine;

use crate::build::RequestSpec;
use crate::redact::MASK;
use crate::request::{ApiKeyIn, AuthOverride, ParamIn};

/// 取り除いた値の代わりに入れるもの。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fill {
    /// `••••`。
    Mask,
    /// 空文字。
    Empty,
}

impl Fill {
    fn text(self) -> &'static str {
        match self {
            Fill::Mask => MASK,
            Fill::Empty => "",
        }
    }
}

/// 整えた入力。
#[derive(Debug, Clone)]
pub struct Sanitized {
    pub spec: RequestSpec,
    /// 置き換えたもの（画面に出す日本語。「Authorization ヘッダー」など）。
    pub fields: Vec<String>,
    /// 取り除いた値そのもの（伏せ字の対象に加える）。
    pub literals: Vec<String>,
}

/// `{{...}}` だけでできた値か（空白は無視する）。空文字は `true`（取り除くものがない）。
pub fn is_template_only(value: &str) -> bool {
    let mut rest = value.trim();
    while !rest.is_empty() {
        let Some(after) = rest.strip_prefix("{{") else {
            return false;
        };
        let Some(end) = after.find("}}") else {
            return false;
        };
        rest = after[end + 2..].trim_start();
    }
    true
}

/// 手で書かれたシークレットを `fill` に置き換えた入力を返す。
pub fn sanitize_spec(spec: &RequestSpec, fill: Fill) -> Sanitized {
    let mut out = spec.clone();
    let mut fields: Vec<String> = Vec::new();
    let mut literals: Vec<String> = Vec::new();
    let note = |label: String, fields: &mut Vec<String>| {
        if !fields.contains(&label) {
            fields.push(label);
        }
    };

    for p in &mut out.params {
        match p.location {
            ParamIn::Header if is_credential_header_name(&p.name) => {
                if let Some(new) = clean_header(&p.name, &p.value, fill, &mut literals) {
                    p.value = new;
                    note(format!("{} ヘッダー", p.name.trim()), &mut fields);
                }
            }
            ParamIn::Cookie => {
                if !is_template_only(&p.value) {
                    literals.push(p.value.trim().to_string());
                    p.value = fill.text().to_string();
                    note(format!("Cookie（{}）", p.name.trim()), &mut fields);
                }
            }
            _ => {}
        }
    }

    let mut literal = |value: &mut String, label: String, fields: &mut Vec<String>| {
        if !is_template_only(value) {
            literals.push(value.trim().to_string());
            *value = fill.text().to_string();
            note(label, fields);
        }
    };
    match &mut out.auth {
        Some(AuthOverride::Bearer { token }) => {
            literal(token, "認証（Bearer のトークン）".into(), &mut fields);
        }
        Some(AuthOverride::Basic { password, .. }) => {
            literal(password, "認証（Basic のパスワード）".into(), &mut fields);
        }
        Some(AuthOverride::ApiKey {
            location,
            name,
            value,
        }) => {
            let place = match location {
                ApiKeyIn::Header => "ヘッダー",
                ApiKeyIn::Query => "クエリ",
                ApiKeyIn::Cookie => "Cookie",
            };
            literal(
                value,
                format!("認証（API キー {place}: {}）", name.trim()),
                &mut fields,
            );
        }
        Some(AuthOverride::None) | None => {}
    }

    literals.retain(|l| !l.is_empty());
    literals.sort();
    literals.dedup();
    Sanitized {
        spec: out,
        fields,
        literals,
    }
}

/// 認証情報を運ぶヘッダーか（`redact::is_sensitive_header` と同じ。`Set-Cookie` はリクエストでは使わない）。
fn is_credential_header_name(name: &str) -> bool {
    crate::redact::is_sensitive_header(name)
}

/// 認証情報のヘッダーの値を整える。置き換えたら `Some(新しい値)`、そのままでよければ `None`。
fn clean_header(name: &str, value: &str, fill: Fill, literals: &mut Vec<String>) -> Option<String> {
    let lower = name.trim().to_ascii_lowercase();
    match lower.as_str() {
        "authorization" | "proxy-authorization" => {
            let v = value.trim();
            let (scheme, token) = match v.split_once(' ') {
                Some((s, t)) if !s.is_empty() => (Some(s), t.trim()),
                _ => (None, v),
            };
            if is_template_only(v) || is_template_only(token) {
                return None;
            }
            literals.push(token.to_string());
            // Basic は base64 の中身（user:password とパスワード）も伏せる対象にする
            if scheme.is_some_and(|s| s.eq_ignore_ascii_case("basic")) {
                if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(token) {
                    if let Ok(text) = String::from_utf8(bytes) {
                        if let Some((_, pass)) = text.split_once(':') {
                            literals.push(pass.to_string());
                        }
                        literals.push(text);
                    }
                }
            }
            Some(match (fill, scheme) {
                (Fill::Mask, Some(s)) => format!("{s} {MASK}"),
                (Fill::Mask, None) => MASK.to_string(),
                (Fill::Empty, _) => String::new(),
            })
        }
        "cookie" => {
            if is_template_only(value) {
                return None;
            }
            let mut changed = false;
            let pairs: Vec<String> = value
                .split(';')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .filter_map(|pair| match pair.split_once('=') {
                    Some((n, v)) if is_template_only(v) => Some(format!("{n}={v}")),
                    Some((n, v)) => {
                        literals.push(v.trim().to_string());
                        changed = true;
                        match fill {
                            Fill::Mask => Some(format!("{n}={MASK}")),
                            Fill::Empty => None,
                        }
                    }
                    None if is_template_only(pair) => Some(pair.to_string()),
                    None => {
                        literals.push(pair.to_string());
                        changed = true;
                        match fill {
                            Fill::Mask => Some(MASK.to_string()),
                            Fill::Empty => None,
                        }
                    }
                })
                .collect();
            changed.then(|| pairs.join("; "))
        }
        _ => {
            // X-XSRF-TOKEN
            if is_template_only(value) {
                None
            } else {
                literals.push(value.trim().to_string());
                Some(fill.text().to_string())
            }
        }
    }
}

/// 履歴に残った伏せ字（`••••`）を、入力から取り除く（再送信・編集して開く・入力値として保存に使う）。
///
/// 伏せ字を含む認証・ヘッダーの行・Cookie の行は、`••••` を送ってしまわないように外す。
/// 外したものの名前（画面に出す日本語）を一緒に返す。
pub fn drop_masked(spec: &RequestSpec) -> (RequestSpec, Vec<String>) {
    let mut out = spec.clone();
    let mut dropped: Vec<String> = Vec::new();
    let has_mask = |s: &str| s.contains(MASK);

    out.params.retain_mut(|p| match p.location {
        ParamIn::Header if is_credential_header_name(&p.name) && has_mask(&p.value) => {
            if p.name.trim().eq_ignore_ascii_case("cookie") {
                // 伏せていない Cookie だけ残す
                let kept: Vec<&str> = p
                    .value
                    .split(';')
                    .map(str::trim)
                    .filter(|c| !c.is_empty() && !has_mask(c))
                    .collect();
                dropped.push(format!("{} ヘッダー", p.name.trim()));
                if kept.is_empty() {
                    return false;
                }
                p.value = kept.join("; ");
                return true;
            }
            dropped.push(format!("{} ヘッダー", p.name.trim()));
            false
        }
        ParamIn::Cookie if has_mask(&p.value) => {
            dropped.push(format!("Cookie（{}）", p.name.trim()));
            false
        }
        _ => true,
    });

    let drop_auth = match &out.auth {
        Some(AuthOverride::Bearer { token }) if has_mask(token) => {
            Some("認証（Bearer のトークン）".to_string())
        }
        Some(AuthOverride::Basic { password, .. }) if has_mask(password) => {
            Some("認証（Basic のパスワード）".to_string())
        }
        Some(AuthOverride::ApiKey { name, value, .. }) if has_mask(value) => {
            Some(format!("認証（API キー {}）", name.trim()))
        }
        _ => None,
    };
    if let Some(label) = drop_auth {
        out.auth = None;
        dropped.push(label);
    }
    dropped.dedup();
    (out, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::{Body, KeyValue};

    fn spec() -> RequestSpec {
        RequestSpec {
            method: "GET".into(),
            url: "{{baseUrl}}/a".into(),
            params: vec![],
            body: Body::None,
            auth: None,
        }
    }

    fn kv(location: ParamIn, name: &str, value: &str) -> KeyValue {
        KeyValue {
            location,
            name: name.into(),
            value: value.into(),
            enabled: true,
        }
    }

    #[test]
    fn template_only_values() {
        assert!(is_template_only("{{token}}"));
        assert!(is_template_only(" {{a}}{{b}} "));
        assert!(is_template_only(""));
        assert!(is_template_only("{{$uuid}}"));
        assert!(!is_template_only("abc"));
        assert!(!is_template_only("pre{{a}}"));
        assert!(!is_template_only("{{a}}post"));
        assert!(!is_template_only("{{a"));
    }

    #[test]
    fn literal_auth_values_are_masked_and_variables_are_kept() {
        let mut s = spec();
        s.auth = Some(AuthOverride::Bearer {
            token: "LITERAL-TOKEN".into(),
        });
        let r = sanitize_spec(&s, Fill::Mask);
        assert_eq!(
            r.spec.auth,
            Some(AuthOverride::Bearer { token: MASK.into() })
        );
        assert_eq!(r.fields, vec!["認証（Bearer のトークン）"]);
        assert_eq!(r.literals, vec!["LITERAL-TOKEN"]);

        s.auth = Some(AuthOverride::Bearer {
            token: "{{token}}".into(),
        });
        let r = sanitize_spec(&s, Fill::Mask);
        assert_eq!(r.spec, s);
        assert!(r.fields.is_empty() && r.literals.is_empty());

        s.auth = Some(AuthOverride::Basic {
            username: "neko".into(),
            password: "pw-literal".into(),
        });
        let r = sanitize_spec(&s, Fill::Empty);
        assert_eq!(
            r.spec.auth,
            Some(AuthOverride::Basic {
                username: "neko".into(),
                password: String::new()
            })
        );
        s.auth = Some(AuthOverride::ApiKey {
            location: ApiKeyIn::Query,
            name: "api_key".into(),
            value: "KEY123".into(),
        });
        let r = sanitize_spec(&s, Fill::Mask);
        assert!(r.fields[0].contains("api_key"));
        assert_eq!(r.literals, vec!["KEY123"]);
    }

    #[test]
    fn credential_headers_and_cookies() {
        let mut s = spec();
        s.params = vec![
            kv(ParamIn::Header, "Authorization", "Bearer abc.def"),
            kv(ParamIn::Header, "authorization", "Bearer {{token}}"),
            kv(ParamIn::Header, "X-XSRF-TOKEN", "xsrf-literal"),
            kv(
                ParamIn::Header,
                "Cookie",
                "SESSION=sess1; theme={{t}}; flag",
            ),
            kv(ParamIn::Header, "Content-Type", "application/json"),
            kv(ParamIn::Cookie, "SESSION", "sess2"),
            kv(ParamIn::Cookie, "ok", "{{v}}"),
            kv(ParamIn::Query, "q", "plain"),
        ];
        let r = sanitize_spec(&s, Fill::Mask);
        let v = |i: usize| r.spec.params[i].value.as_str();
        assert_eq!(v(0), "Bearer ••••");
        assert_eq!(v(1), "Bearer {{token}}");
        assert_eq!(v(2), "••••");
        assert_eq!(v(3), "SESSION=••••; theme={{t}}; ••••");
        assert_eq!(v(4), "application/json");
        assert_eq!(v(5), "••••");
        assert_eq!(v(6), "{{v}}");
        assert_eq!(v(7), "plain");
        for lit in ["abc.def", "xsrf-literal", "sess1", "flag", "sess2"] {
            assert!(r.literals.contains(&lit.to_string()), "{lit}");
        }
        assert!(!r.literals.iter().any(|l| l.contains("{{")));
        assert!(r.fields.iter().any(|f| f == "Authorization ヘッダー"));

        let r = sanitize_spec(&s, Fill::Empty);
        assert_eq!(r.spec.params[0].value, "");
        assert_eq!(r.spec.params[3].value, "theme={{t}}");
        assert_eq!(r.spec.params[5].value, "");
    }

    #[test]
    fn basic_header_adds_decoded_pieces_to_the_literals() {
        let mut s = spec();
        // neko:pw-literal
        s.params = vec![kv(
            ParamIn::Header,
            "Authorization",
            "Basic bmVrbzpwdy1saXRlcmFs",
        )];
        let r = sanitize_spec(&s, Fill::Mask);
        assert_eq!(r.spec.params[0].value, "Basic ••••");
        for lit in ["bmVrbzpwdy1saXRlcmFs", "neko:pw-literal", "pw-literal"] {
            assert!(r.literals.contains(&lit.to_string()), "{lit}");
        }
    }

    #[test]
    fn sanitizing_twice_changes_nothing_more() {
        let mut s = spec();
        s.auth = Some(AuthOverride::Bearer { token: "T".into() });
        s.params = vec![kv(ParamIn::Header, "Cookie", "a=b")];
        let once = sanitize_spec(&s, Fill::Mask);
        let twice = sanitize_spec(&once.spec, Fill::Mask);
        assert_eq!(once.spec, twice.spec);
    }

    #[test]
    fn drop_masked_removes_what_was_masked() {
        let mut s = spec();
        s.auth = Some(AuthOverride::Bearer { token: MASK.into() });
        s.params = vec![
            kv(ParamIn::Header, "Authorization", "Bearer ••••"),
            kv(ParamIn::Header, "Authorization", "Bearer {{t}}"),
            kv(ParamIn::Header, "Cookie", "SESSION=••••; theme={{t}}"),
            kv(ParamIn::Cookie, "SESSION", "••••"),
            kv(ParamIn::Cookie, "keep", "{{v}}"),
            kv(ParamIn::Query, "q", "••••"), // クエリは触らない
        ];
        let (out, dropped) = drop_masked(&s);
        assert_eq!(out.auth, None);
        let names: Vec<_> = out.params.iter().map(|p| p.value.as_str()).collect();
        assert_eq!(names, vec!["Bearer {{t}}", "theme={{t}}", "{{v}}", "••••"]);
        assert!(dropped.contains(&"認証（Bearer のトークン）".to_string()));
        assert!(dropped.contains(&"Authorization ヘッダー".to_string()));
        assert!(dropped.contains(&"Cookie（SESSION）".to_string()));
    }
}
