//! cURL コマンドの書き出し。core の `RequestSpec`（変数を展開済みの値）から、貼り付けて動くコマンドを作る。
//!
//! 組み立ての規則は core の `build_request` に合わせてある（無効の行は出さない、手入力の `Authorization`・
//! `Content-Type` があれば自動のものは付けない、Cookie は 1 つにまとめる、API キーの置き場所ごとに出し分ける、
//! multipart の `Content-Type` は curl に任せる）。

use nekoreq_core::{
    is_sensitive_header, redact_header_value, redact_text, ApiKeyIn, AuthOverride, Body, KeyValue,
    MultipartValue, ParamIn, RequestSpec, MASK,
};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::Shell;

/// フォームの値をどう出すか。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum FormStyle {
    /// `--data-urlencode 'name=value'`（値は読みやすいまま。curl がエンコードする）。
    #[default]
    UrlEncode,
    /// `-d 'name=value'`（エンコード済みの文字で出す）。
    Data,
}

/// 書き出しの選択肢。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct ExportOptions {
    /// 引用符と行継続の書き方。既定は bash。
    pub shell: Shell,
    /// `\`（cmd は `^`）で改行して 1 オプション 1 行にする。既定は真。偽なら 1 行。
    pub multiline: bool,
    /// 伏せる値の一覧（シークレットの実際の値）。URL・ヘッダー・ボディ・認証のどこに現れても `••••` にする
    /// （URL に入ったときのパーセントエンコードの形も対象。core の `redact_text` と同じ規則）。
    pub redact: Vec<String>,
    /// `Authorization`・`Cookie`・`X-XSRF-TOKEN` など、秘密を運ぶヘッダー・Basic のパスワード・API キーの値を、
    /// 値の一覧にかかわらず `••••` にする（Bearer のトークンも伏せる）。既定は偽。
    pub mask_sensitive: bool,
    /// フォームボディの出し方。
    pub form_style: FormStyle,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            shell: Shell::Bash,
            multiline: true,
            redact: Vec::new(),
            mask_sensitive: false,
            form_style: FormStyle::UrlEncode,
        }
    }
}

const QUERY_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');
const FORM_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'*')
    .remove(b'-')
    .remove(b'.')
    .remove(b'_');

struct Out<'a> {
    o: &'a ExportOptions,
}

impl Out<'_> {
    /// 値の中のシークレットを伏せる。
    fn red(&self, s: &str) -> String {
        redact_text(s, &self.o.redact)
    }

    fn q(&self, s: &str) -> String {
        match self.o.shell {
            Shell::Bash => quote_bash(s),
            Shell::Cmd => quote_cmd(s),
        }
    }
}

/// `RequestSpec` を cURL コマンドにする。値は展開済みであること（`{{変数}}` はそのまま出る）。
///
/// 無効の行は出さない。パスパラメータ（`{id}`）は URL に埋める。
pub fn export(spec: &RequestSpec, options: &ExportOptions) -> String {
    let out = Out { o: options };
    let mut chunks: Vec<String> = Vec::new();

    let method = {
        let m = spec.method.trim().to_ascii_uppercase();
        if m.is_empty() {
            "GET".to_string()
        } else {
            m
        }
    };
    let rows = |loc: ParamIn| {
        spec.params
            .iter()
            .filter(move |p| p.location == loc && p.enabled)
    };

    // ヘッダー（手入力）
    let mut headers: Vec<(String, String)> = rows(ParamIn::Header)
        .filter(|p| !p.name.trim().is_empty())
        .map(|p| (p.name.trim().to_string(), p.value.clone()))
        .collect();
    let has = |headers: &[(String, String)], name: &str| {
        headers.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
    };

    // Cookie
    let mut cookies: Vec<String> = Vec::new();
    for p in rows(ParamIn::Cookie).filter(|p| !p.name.trim().is_empty()) {
        cookies.push(format!("{}={}", p.name.trim(), p.value));
    }

    // クエリ
    let mut query: Vec<(String, String)> = rows(ParamIn::Query)
        .filter(|p| !p.name.is_empty())
        .map(|p| (p.name.clone(), p.value.clone()))
        .collect();

    // 認証
    let mut basic: Option<(String, String)> = None;
    let manual_authorization = has(&headers, "Authorization");
    match &spec.auth {
        None | Some(AuthOverride::None) => {}
        Some(AuthOverride::Bearer { .. } | AuthOverride::Basic { .. }) if manual_authorization => {}
        Some(AuthOverride::Bearer { token }) => {
            headers.push((
                "Authorization".to_string(),
                format!("Bearer {}", token.trim()),
            ));
        }
        Some(AuthOverride::Basic { username, password }) => {
            basic = Some((username.clone(), password.clone()));
        }
        Some(AuthOverride::ApiKey {
            location,
            name,
            value,
        }) => {
            let name = name.trim().to_string();
            let overridden = matches!(location, ApiKeyIn::Header) && has(&headers, &name);
            if !name.is_empty() && !overridden {
                match location {
                    ApiKeyIn::Header => headers.push((name, value.clone())),
                    ApiKeyIn::Query => query.push((name, value.clone())),
                    ApiKeyIn::Cookie => cookies.push(format!("{name}={value}")),
                }
            }
        }
    }
    let mask_all_auth_values =
        options.mask_sensitive && matches!(&spec.auth, Some(AuthOverride::ApiKey { .. }));

    // 手入力の Cookie ヘッダーは -b にまとめる（core と同じく手入力が先）
    let mut manual_cookie: Vec<String> = Vec::new();
    headers.retain(|(n, v)| {
        if n.eq_ignore_ascii_case("Cookie") {
            if !v.trim().is_empty() {
                manual_cookie.push(v.trim().trim_end_matches(';').trim_end().to_string());
            }
            false
        } else {
            true
        }
    });
    let cookie_joined = manual_cookie
        .into_iter()
        .chain(cookies)
        .collect::<Vec<_>>()
        .join("; ");

    // ボディ
    let (body_chunks, content_type) = body_chunks(&spec.body, options, &out);
    if let Some(ct) = &content_type {
        if ct.starts_with("multipart/") {
            headers.retain(|(n, _)| !n.eq_ignore_ascii_case("Content-Type"));
        } else if !has(&headers, "Content-Type") {
            headers.push(("Content-Type".to_string(), ct.clone()));
        }
    }
    // `multipart` 以外のボディがないときも、手入力の Content-Type はそのまま出る

    // 先頭の行: curl [-X METHOD] URL
    let url = build_url(spec, &query, &out);
    let has_body = !body_chunks.is_empty();
    let method_part = match method.as_str() {
        "GET" if !has_body => String::new(),
        "HEAD" => "-I ".to_string(),
        m => format!("-X {m} "),
    };
    chunks.push(format!("curl {method_part}{}", out.q(&out.red(&url))));

    // ヘッダー
    for (name, value) in &headers {
        let shown = if options.mask_sensitive && is_sensitive_header(name) {
            redact_header_value(name, value)
        } else if mask_all_auth_values && name_is_api_key_header(&spec.auth, name) {
            MASK.to_string()
        } else {
            value.clone()
        };
        let line = if shown.is_empty() {
            format!("{name};")
        } else {
            format!("{name}: {shown}")
        };
        chunks.push(format!("-H {}", out.q(&out.red(&line))));
    }
    if let Some((user, pass)) = &basic {
        let pass = if options.mask_sensitive {
            MASK.to_string()
        } else {
            pass.clone()
        };
        chunks.push(format!("-u {}", out.q(&out.red(&format!("{user}:{pass}")))));
    }
    if !cookie_joined.is_empty() {
        let shown = if options.mask_sensitive {
            redact_header_value("Cookie", &cookie_joined)
        } else {
            cookie_joined
        };
        chunks.push(format!("-b {}", out.q(&out.red(&shown))));
    }
    chunks.extend(body_chunks);

    let sep = match (options.shell, options.multiline) {
        (_, false) => " ",
        (Shell::Bash, true) => " \\\n  ",
        (Shell::Cmd, true) => " ^\n  ",
    };
    chunks.join(sep)
}

fn name_is_api_key_header(auth: &Option<AuthOverride>, name: &str) -> bool {
    matches!(
        auth,
        Some(AuthOverride::ApiKey { location: ApiKeyIn::Header, name: n, .. })
            if n.trim().eq_ignore_ascii_case(name)
    )
}

/// ボディのオプションと、自動で付く Content-Type。
fn body_chunks(body: &Body, o: &ExportOptions, out: &Out<'_>) -> (Vec<String>, Option<String>) {
    let mask_body = |s: &str| out.red(s);
    match body {
        Body::None => (vec![], None),
        Body::Json(t) => {
            if t.trim().is_empty() {
                return (vec![], None);
            }
            (
                vec![format!("--data-raw {}", out.q(&mask_body(t)))],
                Some("application/json".to_string()),
            )
        }
        Body::Text(t) => {
            if t.is_empty() {
                return (vec![], None);
            }
            (
                vec![format!("--data-raw {}", out.q(&mask_body(t)))],
                Some("text/plain; charset=utf-8".to_string()),
            )
        }
        Body::Binary(path) => {
            if path.trim().is_empty() {
                return (vec![], None);
            }
            (
                vec![format!(
                    "--data-binary {}",
                    out.q(&mask_body(&format!("@{path}")))
                )],
                Some("application/octet-stream".to_string()),
            )
        }
        Body::Form(rows) => {
            let chunks: Vec<String> = rows
                .iter()
                .filter(|r| r.enabled && !r.name.is_empty())
                .map(|r| form_chunk(r, o.form_style, out))
                .collect();
            (chunks, None)
        }
        Body::Multipart(parts) => {
            let mut chunks = Vec::new();
            for p in parts.iter().filter(|p| p.enabled && !p.name.is_empty()) {
                match &p.value {
                    MultipartValue::Text(t) => {
                        let ct = part_type(p);
                        // `@` `<` で始まる値や `;` を含む値を -F は別の意味に読むので、文字そのままの --form-string にする
                        // （`;type=` を付けたいときは、`;` や `"` を含む値だけ引用符で囲んで -F にする）
                        let leading = t.starts_with('@') || t.starts_with('<');
                        let risky = leading || t.contains(';');
                        if let (Some(ct), false) = (ct, leading) {
                            let value = if t.contains([';', '"', '\\']) {
                                format!("\"{}\"", t.replace('\\', "\\\\").replace('"', "\\\""))
                            } else {
                                t.clone()
                            };
                            chunks.push(format!(
                                "-F {}",
                                out.q(&mask_body(&format!("{}={value};type={ct}", p.name)))
                            ));
                            continue;
                        }
                        let flag = if risky { "--form-string" } else { "-F" };
                        chunks.push(format!(
                            "{flag} {}",
                            out.q(&mask_body(&format!("{}={t}", p.name)))
                        ));
                    }
                    MultipartValue::File(path) => {
                        let path = if path.contains([';', ',', '"']) {
                            format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
                        } else {
                            path.clone()
                        };
                        let ty = part_type(p)
                            .map(|c| format!(";type={c}"))
                            .unwrap_or_default();
                        chunks.push(format!(
                            "-F {}",
                            out.q(&mask_body(&format!("{}=@{path}{ty}", p.name)))
                        ));
                    }
                }
            }
            // Content-Type（境界つき）は curl が付ける。印として multipart/ を返す
            let ct = (!chunks.is_empty()).then(|| "multipart/form-data".to_string());
            (chunks, ct)
        }
    }
}

/// パートの Content-Type（空白だけなら付けない）。
fn part_type(p: &nekoreq_core::MultipartPart) -> Option<&str> {
    p.content_type
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
}

fn form_chunk(r: &KeyValue, style: FormStyle, out: &Out<'_>) -> String {
    // `--data-urlencode` は最初の `=` で名前と値を分ける。名前に `=` があるときは -d にする
    if style == FormStyle::UrlEncode && !r.name.contains('=') {
        return format!(
            "--data-urlencode {}",
            out.q(&out.red(&format!("{}={}", r.name, r.value)))
        );
    }
    let enc = |s: &str| {
        utf8_percent_encode(s, FORM_COMPONENT)
            .to_string()
            .replace("%20", "+")
    };
    format!(
        "-d {}",
        out.q(&out.red(&format!("{}={}", enc(&r.name), enc(&r.value))))
    )
}

/// URL を組み立てる（core の `finish_url` と同じ順: テンプレート → パスパラメータ → クエリ）。
fn build_url(spec: &RequestSpec, query: &[(String, String)], out: &Out<'_>) -> String {
    let mut path_values: Vec<(String, String)> = Vec::new();
    for p in spec
        .params
        .iter()
        .filter(|p| p.location == ParamIn::Path && p.enabled)
    {
        if !p.name.trim().is_empty() {
            path_values.push((p.name.trim().to_string(), p.value.clone()));
        }
    }
    let template = replace_path_params(spec.url.trim(), &path_values);
    let with_scheme = if has_scheme(&template) || template.is_empty() {
        template
    } else {
        format!("http://{template}")
    };
    let (before_fragment, fragment) = match with_scheme.split_once('#') {
        Some((a, b)) => (a.to_string(), Some(b.to_string())),
        None => (with_scheme, None),
    };
    let (path_part, existing_query) = match before_fragment.split_once('?') {
        Some((a, b)) => (a.to_string(), b.to_string()),
        None => (before_fragment, String::new()),
    };
    let mut q = escape_url_text(&existing_query);
    for (k, v) in query {
        if !q.is_empty() && !q.ends_with('&') {
            q.push('&');
        }
        q.push_str(&utf8_percent_encode(k, QUERY_COMPONENT).to_string());
        q.push('=');
        q.push_str(&utf8_percent_encode(v, QUERY_COMPONENT).to_string());
    }
    let mut url = escape_url_text(&path_part);
    if !q.is_empty() {
        url.push('?');
        url.push_str(&q);
    }
    if let Some(f) = fragment {
        url.push('#');
        url.push_str(&escape_url_text(&f));
    }
    let _ = out;
    url
}

/// URL に書かれた文字のうち、そのまま送れないもの（空白・非 ASCII・`{}[]` など）をエンコードする
/// （curl の `[]{}` の展開を避け、core が送る URL と同じにする）。`%` はそのまま。
fn escape_url_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii()
            && !c.is_ascii_control()
            && !matches!(
                c,
                ' ' | '"' | '<' | '>' | '\\' | '^' | '`' | '{' | '|' | '}' | '[' | ']'
            )
        {
            out.push(c);
        } else {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

fn has_scheme(url: &str) -> bool {
    match url.find("://") {
        Some(i) => url.find(['/', '?', '#']).is_none_or(|d| i < d),
        None => false,
    }
}

/// `{name}` を値（パーセントエンコード）に置き換える。`{{...}}` は触らない。該当がなければそのまま。
fn replace_path_params(url: &str, values: &[(String, String)]) -> String {
    let mut out = String::with_capacity(url.len());
    let mut rest = url;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(after) = tail.strip_prefix("{{") {
            match after.find("}}") {
                Some(end) => {
                    out.push_str(&tail[..end + 4]);
                    rest = &after[end + 2..];
                }
                None => {
                    out.push_str("{{");
                    rest = after;
                }
            }
            continue;
        }
        match tail.find('}') {
            Some(end) => {
                let name = &tail[1..end];
                match values.iter().find(|(n, _)| n == name) {
                    Some((_, v)) if !v.is_empty() && v != "." && v != ".." => {
                        out.push_str(&utf8_percent_encode(v, QUERY_COMPONENT).to_string());
                    }
                    _ => out.push_str(&tail[..end + 1]),
                }
                rest = &tail[end + 1..];
            }
            None => {
                out.push('{');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// bash 用の引用。普通は `'...'`（`'` は `'\''`）。制御文字（改行とタブ以外）があるときは `$'...'`。
fn quote_bash(s: &str) -> String {
    let has_control = s
        .chars()
        .any(|c| (c.is_control() && c != '\n' && c != '\t') || c == '\u{7f}');
    if !has_control {
        return format!("'{}'", s.replace('\'', "'\\''"));
    }
    let mut out = String::from("$'");
    for c in s.chars() {
        match c {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// cmd.exe 用の引用（Chrome の「Copy as cURL (cmd)」と同じ形）。全体を `^"` で囲み、cmd の特殊文字を `^` で
/// エスケープして、C ランタイムの規則で `"` を `\"`、`"` の直前の `\` を 2 倍にする。
fn quote_cmd(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::from("^\"");
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            let mut n = 0;
            while chars.get(i + n) == Some(&'\\') {
                n += 1;
            }
            match chars.get(i + n) {
                Some('"') => {
                    out.push_str(&"\\".repeat(n * 2 + 1));
                    out.push_str("^\"");
                    i += n + 1;
                }
                None => {
                    out.push_str(&"\\".repeat(n * 2));
                    i += n;
                }
                Some(_) => {
                    out.push_str(&"\\".repeat(n));
                    i += n;
                }
            }
            continue;
        }
        match c {
            '"' => out.push_str("\\^\""),
            '^' | '&' | '<' | '>' | '|' | '(' | ')' | '%' | '!' => {
                out.push('^');
                out.push(c);
            }
            '\n' => out.push_str("^\n\n"),
            c => out.push(c),
        }
        i += 1;
    }
    out.push_str("^\"");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_quotes() {
        assert_eq!(quote_bash("a'b"), "'a'\\''b'");
        assert_eq!(quote_bash("a\rb"), "$'a\\rb'");
        assert_eq!(quote_bash("日本語 😀"), "'日本語 😀'");
    }

    #[test]
    fn cmd_quotes() {
        assert_eq!(quote_cmd("a\"b"), "^\"a\\^\"b^\"");
        assert_eq!(quote_cmd("a&b%"), "^\"a^&b^%^\"");
        assert_eq!(quote_cmd("c:\\dir\\"), "^\"c:\\dir\\\\^\"");
    }
}
