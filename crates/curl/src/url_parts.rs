//! URL の分割、パーセントデコード、OpenAPI の path テンプレートとの照合。

use percent_encoding::percent_decode_str;

use crate::error::CurlError;

/// フォームの形（`+` は空白）でデコードする。
pub(crate) fn decode_form(s: &str) -> String {
    let plus = s.replace('+', " ");
    percent_decode_str(&plus).decode_utf8_lossy().into_owned()
}

/// `+` を空白にしないパーセントデコード。
pub(crate) fn decode_component(s: &str) -> String {
    percent_decode_str(s).decode_utf8_lossy().into_owned()
}

/// `a=1&b=2` を（名前, 値）の列にする。`=` がなければ値は空。空の区間は捨てる。
pub(crate) fn split_pairs(raw: &str) -> Vec<(String, String)> {
    raw.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (decode_form(k), decode_form(v)),
            None => (decode_form(p), String::new()),
        })
        .collect()
}

/// 分割した URL。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UrlParts {
    pub base: String,
    pub path: String,
    pub query: String,
    /// `user:pass@` の部分（デコード済み。パスワードがなければ `None`）。
    pub userinfo: Option<(String, Option<String>)>,
    /// スキームがなくて `http://` を補った。
    pub scheme_added: bool,
    /// `://` の前のスキーム（小文字）。`{{baseUrl}}/...` やパスだけのときは空。
    pub scheme: String,
}

/// URL を base（スキーム+ホスト+ポート）・パス・クエリに分ける。フラグメントは送られないので捨てる。
/// パスは書かれたまま（デコードも整形もしない）。
pub(crate) fn split_url(raw: &str) -> Result<UrlParts, CurlError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CurlError::InvalidUrl("URL が空です".to_string()));
    }
    let s = trimmed.split('#').next().unwrap_or("");
    let first_delim = s.find(['/', '?']);
    let (scheme_part, rest, scheme_added, scheme) = match s.find("://") {
        Some(i) if first_delim.is_none_or(|d| i < d) => {
            let scheme = &s[..i];
            if scheme.is_empty()
                || !scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            {
                return Err(CurlError::InvalidUrl(format!(
                    "{trimmed}（スキームが不正です）"
                )));
            }
            (&s[..i + 3], &s[i + 3..], false, scheme.to_ascii_lowercase())
        }
        _ if s.starts_with("{{") || s.starts_with('/') || s.starts_with('?') => {
            ("", s, false, String::new())
        }
        _ => ("http://", s, true, "http".to_string()),
    };
    let auth_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..auth_end];
    let after = &rest[auth_end..];
    let (userinfo, hostport) = match authority.rfind('@') {
        Some(i) if !authority.starts_with("{{") => {
            let ui = &authority[..i];
            let parsed = match ui.split_once(':') {
                Some((u, p)) => (decode_component(u), Some(decode_component(p))),
                None => (decode_component(ui), None),
            };
            (Some(parsed), &authority[i + 1..])
        }
        _ => (None, authority),
    };
    if !scheme_part.is_empty() && hostport.is_empty() {
        return Err(CurlError::InvalidUrl(format!(
            "{trimmed}（ホストがありません）"
        )));
    }
    let (path, query) = match after.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (after.to_string(), String::new()),
    };
    let base = if scheme_part.is_empty() {
        hostport.to_string()
    } else {
        format!("{scheme_part}{hostport}")
    };
    Ok(UrlParts {
        base,
        path,
        query,
        userinfo,
        scheme_added,
        scheme,
    })
}

/// 比べるための正規形（スキームとホストは小文字、既定のポートは省く）。変数で始まる base はそのまま小文字にする。
pub(crate) fn origin_key(base: &str) -> Option<String> {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return None;
    }
    let Some((scheme, hostport)) = base.split_once("://") else {
        return Some(base.to_ascii_lowercase());
    };
    let scheme = scheme.to_ascii_lowercase();
    let hostport = hostport.to_ascii_lowercase();
    let default_port = match scheme.as_str() {
        "http" => ":80",
        "https" => ":443",
        _ => "",
    };
    let hostport = if !default_port.is_empty() && hostport.ends_with(default_port) {
        // IPv6 の `[::80]` の末尾を誤って削らない（`]` の直後の `:80` だけが対象）
        let head = &hostport[..hostport.len() - default_port.len()];
        if head.contains(':') && !head.ends_with(']') {
            hostport.clone()
        } else {
            head.to_string()
        }
    } else {
        hostport
    };
    Some(format!("{scheme}://{hostport}"))
}

/// 環境の `baseUrl` を（origin, パスの前置き）に分ける。パスの前置きは `/api` の形（末尾の `/` なし、なければ空）。
pub(crate) fn split_base_prefix(env_base_url: &str) -> Option<(String, String)> {
    let s = env_base_url.trim();
    let s = s.split(['?', '#']).next().unwrap_or("");
    if s.is_empty() {
        return None;
    }
    let with_scheme = if s.contains("://") || s.starts_with("{{") {
        s.to_string()
    } else {
        format!("http://{s}")
    };
    let (scheme_part, rest) = match with_scheme.find("://") {
        Some(i) => (
            with_scheme[..i + 3].to_string(),
            with_scheme[i + 3..].to_string(),
        ),
        None => (String::new(), with_scheme.clone()),
    };
    let (host, prefix) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].trim_end_matches('/')),
        None => (rest.as_str(), ""),
    };
    if host.is_empty() {
        return None;
    }
    Some((format!("{scheme_part}{host}"), prefix.to_string()))
}

/// path テンプレートとの照合結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMatch {
    /// `{name}` に当たった値（パーセントデコード済み。テンプレートに書かれた順）。
    pub params: Vec<(String, String)>,
    /// 固定の文字だけで一致した区間の数。複数のテンプレートが一致したときは、これが大きいほうを選ぶ
    /// （`/clients/search` が `/clients/{id}` より優先される）。
    pub literal_segments: usize,
}

/// OpenAPI の path テンプレート（`/api/v1/video/clients/{clientId}`）と、cURL の path が一致するか調べる。
///
/// - 区間（`/` で区切った 1 つ）ごとに比べる。`{name}` は空でない 1 区間に当たる（`{id}.json` のような一部だけの指定も可）。
/// - 末尾の `/` の有無は無視する。固定の文字はパーセントデコードして比べる。
/// - `path` にクエリは含めない（`ParsedCurl.path` をそのまま渡せる）。
pub fn match_path_template(template: &str, path: &str) -> Option<PathMatch> {
    let tpl: Vec<&str> = trim_one_trailing_slash(template).split('/').collect();
    let act: Vec<&str> = trim_one_trailing_slash(path).split('/').collect();
    if tpl.len() != act.len() {
        return None;
    }
    let mut params = Vec::new();
    let mut literal_segments = 0;
    for (t, a) in tpl.iter().zip(&act) {
        let pieces = segment_pieces(t);
        let has_param = pieces.iter().any(|p| matches!(p, Piece::Param(_)));
        if has_param {
            if a.is_empty() {
                return None;
            }
            let decoded = decode_component(a);
            let caps = match_segment(&pieces, &decoded)?;
            params.extend(caps);
        } else {
            if decode_component(t) != decode_component(a) {
                return None;
            }
            if !t.is_empty() {
                literal_segments += 1;
            }
        }
    }
    Some(PathMatch {
        params,
        literal_segments,
    })
}

fn trim_one_trailing_slash(s: &str) -> &str {
    if s.len() > 1 {
        s.strip_suffix('/').unwrap_or(s)
    } else {
        s
    }
}

enum Piece<'a> {
    Literal(&'a str),
    Param(&'a str),
}

fn segment_pieces(seg: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut rest = seg;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        if open > 0 {
            out.push(Piece::Literal(&rest[..open]));
        }
        out.push(Piece::Param(&rest[open + 1..open + close]));
        rest = &rest[open + close + 1..];
    }
    if !rest.is_empty() {
        out.push(Piece::Literal(rest));
    }
    out
}

fn match_segment(pieces: &[Piece<'_>], text: &str) -> Option<Vec<(String, String)>> {
    fn go(pieces: &[Piece<'_>], text: &str, out: &mut Vec<(String, String)>) -> bool {
        match pieces.split_first() {
            None => text.is_empty(),
            Some((Piece::Literal(l), rest)) => {
                text.strip_prefix(l).is_some_and(|t| go(rest, t, out))
            }
            Some((Piece::Param(name), rest)) => {
                // 最短の一致から試す（空は不可）
                let mut ends: Vec<usize> =
                    text.char_indices().map(|(i, c)| i + c.len_utf8()).collect();
                ends.retain(|&e| e > 0);
                for e in ends {
                    let mark = out.len();
                    out.push(((*name).to_string(), text[..e].to_string()));
                    if go(rest, &text[e..], out) {
                        return true;
                    }
                    out.truncate(mark);
                }
                false
            }
        }
    }
    let mut out = Vec::new();
    go(pieces, text, &mut out).then_some(out)
}
