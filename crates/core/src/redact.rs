//! 伏せ字処理。履歴（M6）と画面のプレビューが共通で使う。
//!
//! - `Authorization`・`Proxy-Authorization`・`Cookie`・`X-XSRF-TOKEN`・`Set-Cookie` ヘッダーの値を `••••` に置き換える。
//!   Cookie と Set-Cookie は名前と属性（`Path=/` など）を残し、値だけを置き換える
//!   （例: `SESSION=••••; XSRF-TOKEN=••••`、`XSRF-TOKEN=••••; Path=/`）。
//!   Authorization は方式を残す（例: `Bearer ••••`）。
//! - シークレット変数の値が現れた箇所は、ヘッダー・URL・ボディのどこでも `••••` にする。
//!   URL に入るときにパーセントエンコードされた形（16 進は大文字・小文字の両方）、フォームの形（`+`）、
//!   url クレートが URL を整えたあとの形（パス・クエリ・ホストなど場所ごとのエンコードと、ホストの小文字化）も対象。
//!   さらに、前後の空白を除いた形（ヘッダーの値は HTTP で前後の空白が落ちる）・JSON の文字列のエスケープ形
//!   （`\/`・`\"`・`\\`・`\uXXXX`）・base64（標準・URL セーフ、パディングあり・なし）も対象。
//!   短すぎる値（4 文字未満）からは、前後の空白と base64 の形を作らない（関係のない文字列まで伏せないため）。

use std::collections::HashSet;

use aho_corasick::AhoCorasick;
use base64::Engine as _;
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

/// 値から作る形（前後の空白を除いた形・base64）を作る最小の文字数。これより短い値は、
/// 関係のない文字列まで伏せてしまうので、そのままの形と URL・JSON の形だけを探す。
const MIN_DERIVED_CHARS: usize = 4;

/// 探す文字列（長いものが先）。空・重複は除く。
fn needles_of(secrets: &[String]) -> Vec<String> {
    let mut needles: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        for variant in secret_variants(secret) {
            if seen.insert(variant.clone()) {
                needles.push(variant);
            }
        }
    }
    // 長いものから置き換える（短い値が長い値の一部になっていても取り残さない）。
    // 安定ソートなので、同じ長さの順序は挿入順のまま
    needles.sort_by_key(|n| std::cmp::Reverse(n.len()));
    needles
}

/// `text` の中のシークレットの値をすべて `••••` にする。空の値は無視する。
pub fn redact_text(text: &str, secrets: &[String]) -> String {
    let needles = needles_of(secrets);
    if needles.is_empty() {
        return text.to_string();
    }
    // 探す文字列は数千件になるので、1 件ずつ全体を走査せず、先に元のテキストを 1 回だけ走査して
    // 現れる文字列だけに絞る（重なる出現も拾う）。`•`（MASK の文字）を含まない文字列は、置き換えで
    // 新しく現れることがない（置き換え後のテキストのうち MASK 以外の部分は元のテキストの部分文字列で、
    // MASK をまたがない出現は元のテキストにもあったため）ので、元のテキストに無ければ捨ててよい。
    // `•` を含む文字列は、置き換えのあとで新しく現れうるので、常に残す。
    let present = present_needles(text, &needles);
    let mut out = text.to_string();
    for (needle, keep) in needles.iter().zip(&present) {
        if *keep && out.contains(needle.as_str()) {
            out = out.replace(needle.as_str(), MASK);
        }
    }
    out
}

/// 各 needle を残すか。元のテキストに現れるもの、または MASK の文字を含むもの（理由は `redact_text`）。
fn present_needles(text: &str, needles: &[String]) -> Vec<bool> {
    let mask_char = MASK.chars().next().unwrap_or('•');
    let mut keep: Vec<bool> = needles.iter().map(|n| n.contains(mask_char)).collect();
    // 構築に失敗したら（巨大すぎるなど）絞り込みをやめて全部残す。結果は変わらず、遅くなるだけ
    let Ok(ac) = AhoCorasick::new(needles) else {
        return vec![true; needles.len()];
    };
    let mut remaining = keep.iter().filter(|k| !**k).count();
    if remaining == 0 {
        return keep;
    }
    for m in ac.find_overlapping_iter(text) {
        let i = m.pattern().as_usize();
        if !keep[i] {
            keep[i] = true;
            remaining -= 1;
            if remaining == 0 {
                break;
            }
        }
    }
    keep
}

/// 途中で切れたテキスト（表示上限で切ったもの）の末尾に、シークレットの先頭の断片が残らないようにする。
/// `text` は `redact_text` で伏せたあとのもの。末尾が、どれかのシークレット（の探す形）の先頭の一部と
/// 一致していたら、その部分を落とす（切り口をまたぐシークレットの前半を残さない）。
pub fn drop_partial_secret_tail(text: &str, secrets: &[String]) -> String {
    let mut cut = text.len();
    for needle in needles_of(secrets) {
        // 先頭から 1 文字ずつ長くした断片が末尾と一致するか（長い方を優先）。完全一致は伏せ済みなので `<` まで。
        let ends: Vec<usize> = needle.char_indices().map(|(i, _)| i).skip(1).collect();
        for &end in ends.iter().rev() {
            let frag = &needle[..end];
            if text.ends_with(frag) {
                cut = cut.min(text.len() - frag.len());
                break;
            }
        }
    }
    text[..cut].to_string()
}

/// シークレットが URL・フォーム・JSON・base64 などに入ったときの形も含めた、探す文字列。
fn secret_variants(secret: &str) -> Vec<String> {
    let mut bases = vec![secret];
    // ヘッダーの値は HTTP で前後の空白が落ちる（コピー & ペーストで付いた空白つきの値を、サーバーは空白なしでエコーする）
    let trimmed = secret.trim();
    if trimmed != secret && trimmed.chars().count() >= MIN_DERIVED_CHARS {
        bases.push(trimmed);
    }
    let mut v: Vec<String> = Vec::new();
    for base in bases {
        v.extend(url_forms(base));
        v.extend(json_forms(base));
        if base.chars().count() >= MIN_DERIVED_CHARS {
            v.extend(base64_forms(base));
        }
    }
    v.retain(|x| !x.is_empty());
    v
}

/// URL・フォームに入ったときの形。
fn url_forms(secret: &str) -> Vec<String> {
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
    v
}

/// base64（標準・URL セーフ、パディングあり・なし）にした形。
fn base64_forms(secret: &str) -> Vec<String> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
    [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD]
        .iter()
        .map(|e| e.encode(secret.as_bytes()))
        .collect()
}

/// JSON の文字列に入ったときの形（`"` と `\` のエスケープ、`/` を `\/` にする言語、非 ASCII だけ・すべてを
/// `\uXXXX`（UTF-16 の単位ごと。16 進は小文字・大文字）にする言語）。元の文字列と同じ形は返さない。
fn json_forms(secret: &str) -> Vec<String> {
    #[derive(Clone, Copy, PartialEq)]
    enum Unicode {
        Off,
        NonAscii,
        All,
    }
    let mut out: Vec<String> = Vec::new();
    for slash in [false, true] {
        for unicode in [Unicode::Off, Unicode::NonAscii, Unicode::All] {
            for upper in [false, true] {
                if upper && unicode == Unicode::Off {
                    continue;
                }
                let mut s = String::with_capacity(secret.len() + 8);
                for c in secret.chars() {
                    let standard = match c {
                        '"' => Some("\\\""),
                        '\\' => Some("\\\\"),
                        '/' if slash => Some("\\/"),
                        '\n' => Some("\\n"),
                        '\r' => Some("\\r"),
                        '\t' => Some("\\t"),
                        _ => None,
                    };
                    let as_unicode = match unicode {
                        Unicode::All => true,
                        Unicode::NonAscii => {
                            !c.is_ascii() || (c.is_control() && standard.is_none())
                        }
                        Unicode::Off => c.is_control() && standard.is_none(),
                    };
                    if as_unicode {
                        let mut buf = [0u16; 2];
                        for unit in c.encode_utf16(&mut buf) {
                            let unit = *unit;
                            if upper {
                                s.push_str(&format!("\\u{unit:04X}"));
                            } else {
                                s.push_str(&format!("\\u{unit:04x}"));
                            }
                        }
                    } else if let Some(e) = standard {
                        s.push_str(e);
                    } else {
                        s.push(c);
                    }
                }
                if s != secret && !out.contains(&s) {
                    out.push(s);
                }
            }
        }
    }
    out
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

/// ヘッダーの一覧を伏せ字にする（順番と重複は保つ）。値のほかに、名前に入ったシークレットも伏せる
/// （`X-{{token}}` のようなヘッダー名）。
pub fn redact_headers(headers: &[HeaderPair], secrets: &[String]) -> Vec<HeaderPair> {
    headers
        .iter()
        .map(|h| HeaderPair {
            name: redact_text(&h.name, secrets),
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
    fn trimmed_json_escaped_and_base64_forms_are_masked() {
        // 前後の空白つきのシークレットは、空白なしの形も伏せる
        let padded = vec!["  PAD-SECRET-1  ".to_string()];
        assert_eq!(redact_text("v=PAD-SECRET-1;", &padded), "v=••••;");
        // JSON のエスケープ形
        let secrets = vec!["a/b\"c\\d".to_string(), "ねこ-SECRET".to_string()];
        assert_eq!(
            redact_text(r#"{"x":"a\/b\"c\\d","y":"\u306d\u3053-SECRET"}"#, &secrets),
            r#"{"x":"••••","y":"••••"}"#
        );
        assert_eq!(
            redact_text(r#"{"y":"ねこ-SECRET","z":"\u306D\u3053-SECRET"}"#, &secrets),
            r#"{"y":"••••","z":"••••"}"#
        );
        // base64（標準・URL セーフ、パディングあり・なし）
        let b = vec!["abc???>>>".to_string()];
        for form in ["YWJjPz8/Pj4+", "YWJjPz8_Pj4-"] {
            assert_eq!(redact_text(&format!("t={form}"), &b), "t=••••", "{form}");
        }
        let b = vec!["abcd".to_string()];
        for form in ["YWJjZA==", "YWJjZA"] {
            assert_eq!(redact_text(&format!("t={form}"), &b), "t=••••", "{form}");
        }
        // 短い値（4 文字未満）は base64 の形を探さない
        assert_eq!(redact_text("YWI= x", &["ab".to_string()]), "YWI= x");
        // ヘッダー名も伏せる
        let out = redact_headers(&[h("X-NAMESECRET", "1")], &["NAMESECRET".to_string()]);
        assert_eq!(out[0].name, "X-••••");
    }

    #[test]
    fn a_secret_fragment_at_the_cut_is_dropped() {
        let secrets = vec!["TEN-SECRET-0123".to_string()];
        assert_eq!(drop_partial_secret_tail("abc TEN-SEC", &secrets), "abc ");
        assert_eq!(drop_partial_secret_tail("abc T", &secrets), "abc ");
        assert_eq!(drop_partial_secret_tail("abc ••••", &secrets), "abc ••••");
        assert_eq!(drop_partial_secret_tail("abc xyz", &secrets), "abc xyz");
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
