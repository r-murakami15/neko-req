//! 伏せ字処理。履歴（M6）と画面のプレビューが共通で使う。
//!
//! - `Authorization`・`Proxy-Authorization`・`Cookie`・`X-XSRF-TOKEN`・`Set-Cookie` ヘッダーの値を `••••` に置き換える。
//!   Cookie と Set-Cookie は名前と属性（`Path=/` など）を残し、値だけを置き換える
//!   （例: `SESSION=••••; XSRF-TOKEN=••••`、`XSRF-TOKEN=••••; Path=/`）。
//!   Authorization は方式を残す（例: `Bearer ••••`）。
//! - シークレット変数の値が現れた箇所は、ヘッダー・URL・ボディのどこでも `••••` にする。
//!   URL に入るときにパーセントエンコードされた形（16 進は大文字・小文字の両方）、フォームの形（`+`）、
//!   url クレートが URL を整えたあとの形（パス・クエリ・ホストなど場所ごとのエンコードと、ホストの小文字化）も対象。

use percent_encoding::utf8_percent_encode;

use crate::history::HeaderPair;
use crate::request_encode::{FORM_COMPONENT, PATH_SEGMENT, QUERY_COMPONENT};

/// 伏せ字。
pub const MASK: &str = "••••";

/// 値を伏せる対象のヘッダーか（大文字小文字は区別しない）。
pub fn is_sensitive_header(name: &str) -> bool {
    [
        "authorization",
        "proxy-authorization",
        "cookie",
        "x-xsrf-token",
        "set-cookie",
    ]
    .iter()
    .any(|n| name.trim().eq_ignore_ascii_case(n))
}

/// 値を伏せる対象のヘッダーの値を置き換える。対象でなければそのまま返す。
pub fn redact_header_value(name: &str, value: &str) -> String {
    let name = name.trim().to_ascii_lowercase();
    match name.as_str() {
        "authorization" | "proxy-authorization" => match value.trim().split_once(' ') {
            Some((scheme, _)) if !scheme.is_empty() => format!("{scheme} {MASK}"),
            _ => MASK.to_string(),
        },
        "x-xsrf-token" => MASK.to_string(),
        "cookie" => value
            .split(';')
            .map(|pair| mask_cookie_pair(pair.trim()))
            .collect::<Vec<_>>()
            .join("; "),
        "set-cookie" => {
            // 最初の `name=value` だけが値で、以降は属性
            let mut parts = value.split(';');
            let first = mask_cookie_pair(parts.next().unwrap_or("").trim());
            let mut out = first;
            for attr in parts {
                out.push(';');
                out.push_str(attr);
            }
            out
        }
        _ => value.to_string(),
    }
}

fn mask_cookie_pair(pair: &str) -> String {
    match pair.split_once('=') {
        Some((name, _)) => format!("{name}={MASK}"),
        None if pair.is_empty() => String::new(),
        None => MASK.to_string(),
    }
}

/// `text` の中のシークレットの値をすべて `••••` にする。空の値は無視する。
pub fn redact_text(text: &str, secrets: &[String]) -> String {
    let mut needles: Vec<String> = Vec::new();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        for variant in secret_variants(secret) {
            if !needles.contains(&variant) {
                needles.push(variant);
            }
        }
    }
    // 長いものから置き換える（短い値が長い値の一部になっていても取り残さない）
    needles.sort_by_key(|n| std::cmp::Reverse(n.len()));
    let mut out = text.to_string();
    for needle in &needles {
        if out.contains(needle.as_str()) {
            out = out.replace(needle.as_str(), MASK);
        }
    }
    out
}

/// シークレットが URL・フォームに入ったときの形も含めた、探す文字列。
fn secret_variants(secret: &str) -> Vec<String> {
    let mut v = vec![
        secret.to_string(),
        utf8_percent_encode(secret, QUERY_COMPONENT).to_string(),
        utf8_percent_encode(secret, PATH_SEGMENT).to_string(),
        utf8_percent_encode(secret, FORM_COMPONENT)
            .to_string()
            .replace("%20", "+"),
    ];
    v.extend(url_normalized_forms(secret));
    // サーバーのエコーなどで 16 進が小文字になった形
    let lower: Vec<String> = v
        .iter()
        .filter(|x| x.contains('%'))
        .map(|x| lowercase_hex(x))
        .collect();
    v.extend(lower);
    v.retain(|x| !x.is_empty());
    v
}

/// url クレートが URL を整えたあとにシークレットがとる形。URL の各場所
/// （パス・クエリ・フラグメント・ユーザー名・パスワード・ホスト）に入れて実際に読ませ、前後を除いて取り出す。
/// 空白と `@` を両方含む値（空白だけ `%20` になる）や、ホストの小文字化もこれで拾える。
fn url_normalized_forms(secret: &str) -> Vec<String> {
    const CONTEXTS: [(&str, &str); 6] = [
        ("http://h/", ""),
        ("http://h/?", ""),
        ("http://h/#", ""),
        ("http://u:", "@h/"),
        ("http://", ":p@h/"),
        ("http://", ".example/"),
    ];
    let mut out = Vec::new();
    for (prefix, suffix) in CONTEXTS {
        let Ok(u) = url::Url::parse(&format!("{prefix}{secret}{suffix}")) else {
            continue;
        };
        let text = u.as_str();
        if let Some(form) = text
            .strip_prefix(prefix)
            .and_then(|t| t.strip_suffix(suffix))
        {
            if !form.is_empty() && form != secret {
                out.push(form.to_string());
            }
        }
    }
    out
}

/// `%E5%90` → `%e5%90`（`%` のあとの 16 進だけ小文字にする）。
fn lowercase_hex(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && b[i + 1].is_ascii_hexdigit()
            && b[i + 2].is_ascii_hexdigit()
        {
            out.push('%');
            out.push(b[i + 1].to_ascii_lowercase() as char);
            out.push(b[i + 2].to_ascii_lowercase() as char);
            i += 3;
        } else {
            // `%` と 16 進は ASCII なので、残りは UTF-8 の境界でそのまま写す
            let ch_len = s[i..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&s[i..i + ch_len]);
            i += ch_len;
        }
    }
    out
}

/// ヘッダーの一覧を伏せ字にする（順番と重複は保つ）。
pub fn redact_headers(headers: &[HeaderPair], secrets: &[String]) -> Vec<HeaderPair> {
    headers
        .iter()
        .map(|h| HeaderPair {
            name: h.name.clone(),
            value: redact_text(&redact_header_value(&h.name, &h.value), secrets),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(name: &str, value: &str) -> HeaderPair {
        HeaderPair {
            name: name.into(),
            value: value.into(),
        }
    }

    #[test]
    fn the_four_headers_are_masked() {
        let out = redact_headers(
            &[
                h("Authorization", "Bearer abc.def"),
                h("authorization", "Basic dXNlcjpwYXNz"),
                h("COOKIE", "SESSION=abc123; XSRF-TOKEN=tok"),
                h("X-XSRF-TOKEN", "tok"),
                h("Set-Cookie", "XSRF-TOKEN=tok; Path=/; HttpOnly"),
                h("Content-Type", "application/json"),
            ],
            &[],
        );
        let values: Vec<_> = out.iter().map(|x| x.value.as_str()).collect();
        assert_eq!(
            values,
            [
                "Bearer ••••",
                "Basic ••••",
                "SESSION=••••; XSRF-TOKEN=••••",
                "••••",
                "XSRF-TOKEN=••••; Path=/; HttpOnly",
                "application/json",
            ]
        );
        assert_eq!(out[0].name, "Authorization", "名前はそのまま");
    }

    #[test]
    fn proxy_authorization_is_masked_like_authorization() {
        assert!(is_sensitive_header("proxy-authorization"));
        assert_eq!(
            redact_header_value("Proxy-Authorization", "Basic dXNlcjpwYXNz"),
            "Basic ••••"
        );
    }

    #[test]
    fn lowercase_hex_and_url_normalized_forms_are_masked() {
        let secrets = vec!["合言葉".to_string(), "p@ss word".to_string()];
        assert_eq!(
            redact_text(
                "a=%e5%90%88%e8%a8%80%e8%91%89 b=%E5%90%88%E8%A8%80%E8%91%89",
                &secrets
            ),
            "a=•••• b=••••"
        );
        // url クレートは空白だけ `%20` にして `@` は残す
        assert_eq!(
            redact_text("http://h/p@ss%20word", &secrets),
            "http://h/••••"
        );
        // ホストに入ると小文字になる
        let host = vec!["MixedHost".to_string()];
        assert_eq!(
            redact_text("http://mixedhost.example/", &host),
            "http://••••.example/"
        );
    }

    #[test]
    fn authorization_without_scheme_is_fully_masked() {
        assert_eq!(redact_header_value("Authorization", "rawtoken"), "••••");
    }

    #[test]
    fn duplicate_headers_keep_order() {
        let out = redact_headers(
            &[
                h("Set-Cookie", "a=1; Path=/"),
                h("X", "1"),
                h("Set-Cookie", "b=2; Path=/"),
            ],
            &[],
        );
        assert_eq!(out[0].value, "a=••••; Path=/");
        assert_eq!(out[2].value, "b=••••; Path=/");
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn secret_values_are_masked_anywhere() {
        let secrets = vec!["s3cr3t".to_string(), "p@ss word".to_string()];
        assert_eq!(redact_text("a s3cr3t b s3cr3t", &secrets), "a •••• b ••••");
        // URL に入ったときのパーセントエンコード・フォームの形
        assert_eq!(
            redact_text("?pw=p%40ss%20word&x=1", &secrets),
            "?pw=••••&x=1"
        );
        assert_eq!(redact_text("pw=p%40ss+word", &secrets), "pw=••••");
        // 別のヘッダーにシークレットが入っていても伏せる
        let out = redact_headers(&[h("X-Custom", "token=s3cr3t")], &secrets);
        assert_eq!(out[0].value, "token=••••");
    }

    #[test]
    fn longer_secret_wins_over_its_prefix() {
        let secrets = vec!["abc".to_string(), "abcdef".to_string()];
        assert_eq!(redact_text("x abcdef y", &secrets), "x •••• y");
    }

    #[test]
    fn empty_secrets_are_ignored() {
        assert_eq!(redact_text("abc", &["".to_string()]), "abc");
    }

    #[test]
    fn sensitive_header_names_are_case_insensitive() {
        assert!(is_sensitive_header("x-xsrf-token"));
        assert!(is_sensitive_header(" Set-Cookie "));
        assert!(!is_sensitive_header("Accept"));
    }
}
