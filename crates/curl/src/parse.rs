//! 解析の本体。オプションを集めた `Raw` から `ParsedCurl` を作る。

use base64::Engine;
use nekoreq_core::{AuthOverride, Body, KeyValue, MultipartPart, MultipartValue, ParamIn};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

use crate::error::CurlError;
use crate::lexer::{detect_shell, tokenize};
use crate::model::{row, CurlWarning, ParsedCurl, WarningKind};
use crate::options::{read_options, DataKind, DataPiece, FormPiece, FormValue, Raw};
use crate::url_parts::{decode_component, decode_form, split_pairs, split_url};
use crate::Shell;

/// core が `Body::Text` のときに自動で付ける Content-Type（同じ値の手入力は取り除く）。
const AUTO_TEXT_TYPE: &str = "text/plain; charset=utf-8";

const URLENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// cURL コマンドを解析する。bash 系か Windows cmd 系かは自動で見分ける（`parse_as` で指定もできる）。
///
/// 先頭の `curl` は省略できる。取り込めない入力は `CurlError`、取り込めたが注意が要ること（無視したオプションなど）は
/// 結果の `warnings` に入る。
pub fn parse(cmd: &str) -> Result<ParsedCurl, CurlError> {
    parse_as(cmd, detect_shell(cmd))
}

/// シェルの書き方を指定して解析する。
pub fn parse_as(cmd: &str, shell: Shell) -> Result<ParsedCurl, CurlError> {
    if cmd.trim().is_empty() {
        return Err(CurlError::Empty);
    }
    let tokens = tokenize(cmd, shell)?;
    if tokens.is_empty() {
        return Err(CurlError::Empty);
    }
    let raw = read_options(tokens)?;
    build(raw)
}

fn push_warning(w: &mut Vec<CurlWarning>, kind: WarningKind, option: &str, message: String) {
    w.push(CurlWarning::new(kind, option, message));
}

fn build(raw: Raw) -> Result<ParsedCurl, CurlError> {
    let Raw {
        method,
        headers: raw_headers,
        data,
        forms,
        user,
        oauth_bearer,
        cookies: raw_cookies,
        get,
        head,
        urls,
        referer,
        user_agent,
        mut warnings,
    } = raw;

    // URL
    let url_index = urls
        .iter()
        .position(|u| u.contains("://"))
        .or(if urls.is_empty() { None } else { Some(0) })
        .ok_or(CurlError::NoUrl)?;
    let raw_url = urls[url_index].clone();
    for (i, u) in urls.iter().enumerate() {
        if i != url_index {
            push_warning(
                &mut warnings,
                WarningKind::Adjusted,
                "",
                format!("URL が複数あるため、最初のもの以外（{u}）は読みませんでした"),
            );
        }
    }
    let parts = split_url(&raw_url)?;
    if parts.scheme_added {
        push_warning(
            &mut warnings,
            WarningKind::Adjusted,
            "",
            "URL にスキームがないため http:// を補いました".to_string(),
        );
    }
    if !parts.scheme.is_empty() && parts.scheme != "http" && parts.scheme != "https" {
        push_warning(
            &mut warnings,
            WarningKind::Unsupported,
            "",
            format!(
                "{}:// は送信できません（http / https だけ対応しています）",
                parts.scheme
            ),
        );
    }
    if parts.base.is_empty() {
        push_warning(
            &mut warnings,
            WarningKind::Adjusted,
            "",
            "URL にホストがありません".to_string(),
        );
    }
    let mut query: Vec<KeyValue> = split_pairs(&parts.query)
        .into_iter()
        .map(|(k, v)| row(ParamIn::Query, &k, &v))
        .collect();

    // ヘッダー
    let mut headers: Vec<(String, String)> = Vec::new();
    for h in &raw_headers {
        read_header(h, &mut headers, &mut warnings);
    }
    if let Some(r) = referer {
        if !has_header(&headers, "Referer") {
            headers.push(("Referer".to_string(), r));
        }
    }
    if let Some(ua) = user_agent {
        if !has_header(&headers, "User-Agent") {
            headers.push(("User-Agent".to_string(), ua));
        }
    }

    // ボディ
    let uses_json = data.iter().any(|p| {
        matches!(
            p,
            DataPiece::Text {
                kind: DataKind::Json,
                ..
            } | DataPiece::File {
                kind: DataKind::Json,
                ..
            }
        )
    });
    let has_data = !data.is_empty();
    let has_form = !forms.is_empty();
    let mut data = data;
    if has_form && has_data {
        push_warning(
            &mut warnings,
            WarningKind::Adjusted,
            "-F",
            "-F と -d は同時に使えないため、-d は読みませんでした".to_string(),
        );
        data.clear();
    }
    let method_default = if let Some(m) = &method {
        m.trim().to_ascii_uppercase()
    } else if get {
        "GET".to_string()
    } else if head {
        "HEAD".to_string()
    } else if has_form || (has_data && !get) {
        "POST".to_string()
    } else {
        "GET".to_string()
    };
    let method_final = if method_default.is_empty() {
        "GET".to_string()
    } else {
        method_default
    };
    if uses_json {
        if !has_header(&headers, "Content-Type") {
            headers.push(("Content-Type".to_string(), "application/json".to_string()));
        }
        if !has_header(&headers, "Accept") {
            headers.push(("Accept".to_string(), "application/json".to_string()));
        }
    }

    let body = if has_form {
        // multipart のときの Content-Type は core が境界つきで付け直す
        if take_header(&mut headers, "Content-Type").is_some() {
            push_warning(
                &mut warnings,
                WarningKind::Adjusted,
                "-H",
                "multipart のときの Content-Type は境界の文字列を合わせるためアプリが付けます（手入力の値は外しました）"
                    .to_string(),
            );
        }
        multipart_body(forms, &mut warnings)
    } else if get && has_data {
        // -G: データはクエリへ
        for p in &data {
            data_to_query(p, &mut query, &mut warnings);
        }
        Body::None
    } else {
        text_body(&data, &mut headers, &mut warnings)
    };

    // Cookie
    let mut cookies: Vec<KeyValue> = Vec::new();
    let mut keep_cookie_header: Vec<(String, String)> = Vec::new();
    let mut cookie_headers: Vec<String> = Vec::new();
    headers.retain(|(n, v)| {
        if n.eq_ignore_ascii_case("Cookie") {
            cookie_headers.push(v.clone());
            false
        } else {
            true
        }
    });
    for v in &cookie_headers {
        match parse_cookie_pairs(v) {
            Some(pairs) => cookies.extend(pairs),
            None => keep_cookie_header.push(("Cookie".to_string(), v.clone())),
        }
    }
    if !raw_cookies.is_empty() {
        if cookie_headers.is_empty() {
            for c in &raw_cookies {
                match parse_cookie_pairs(c) {
                    Some(pairs) if c.contains('=') => cookies.extend(pairs),
                    _ => push_warning(
                        &mut warnings,
                        WarningKind::Unsupported,
                        "-b",
                        format!("-b '{c}' は name=value の形ではないので読みませんでした（Cookie のファイルは読めません）"),
                    ),
                }
            }
        } else {
            // curl は Cookie ヘッダーを手で指定すると -b の値を送らない
            push_warning(
                &mut warnings,
                WarningKind::Adjusted,
                "-b",
                "Cookie ヘッダーがあるため、-b の値は使われません（curl と同じ動作です）"
                    .to_string(),
            );
        }
    }

    // 認証
    let mut auth: Option<AuthOverride> = None;
    let auth_header_pos = headers
        .iter()
        .position(|(n, _)| n.eq_ignore_ascii_case("Authorization"));
    let had_auth_header = auth_header_pos.is_some();
    if let Some(pos) = auth_header_pos {
        let value = headers[pos].1.clone();
        if let Some(a) = auth_from_header(&value) {
            headers.remove(pos);
            auth = Some(a);
        }
    }
    let explicit_auth = user
        .map(|u| (u, "-u"))
        .map(|(u, opt)| (basic_from_user(&u, &mut warnings), opt))
        .or_else(|| oauth_bearer.map(|t| (AuthOverride::Bearer { token: t }, "--oauth2-bearer")));
    if let Some((a, opt)) = explicit_auth {
        if had_auth_header {
            push_warning(
                &mut warnings,
                WarningKind::Adjusted,
                opt,
                format!(
                    "Authorization ヘッダーがあるため、{opt} は使われません（curl と同じ動作です）"
                ),
            );
        } else {
            auth = Some(a);
        }
    }
    if auth.is_none() && !had_auth_header {
        if let Some((u, p)) = &parts.userinfo {
            auth = Some(AuthOverride::Basic {
                username: u.clone(),
                password: p.clone().unwrap_or_default(),
            });
        }
    }

    let mut header_rows: Vec<KeyValue> = headers
        .into_iter()
        .map(|(n, v)| row(ParamIn::Header, &n, &v))
        .collect();
    header_rows.extend(
        keep_cookie_header
            .into_iter()
            .map(|(n, v)| row(ParamIn::Header, &n, &v)),
    );
    Ok(ParsedCurl {
        method: method_final,
        base: parts.base,
        path: parts.path,
        query,
        headers: header_rows,
        cookies,
        body,
        auth,
        warnings,
    })
}

fn has_header(headers: &[(String, String)], name: &str) -> bool {
    headers.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
}

/// 最後に指定された値を返し、同名のヘッダーをすべて取り除く。
fn take_header(headers: &mut Vec<(String, String)>, name: &str) -> Option<String> {
    let last = headers
        .iter()
        .rev()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone());
    headers.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
    last
}

fn find_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .rev()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// `-H` の値 1 つを読む。`Name: value`、`Name;`（空の値で送る）、`Name:`（curl では「消す」指定）。
fn read_header(h: &str, out: &mut Vec<(String, String)>, warnings: &mut Vec<CurlWarning>) {
    if h.starts_with('@') {
        push_warning(
            warnings,
            WarningKind::Unsupported,
            "-H",
            format!("-H '{h}'（ヘッダーのファイル）は読めません"),
        );
        return;
    }
    let (name, value) = match h.split_once(':') {
        Some((n, v)) => {
            let name = n.trim();
            let value = v.trim();
            if value.is_empty() {
                push_warning(
                    warnings,
                    WarningKind::Ignored,
                    "-H",
                    format!("-H '{h}' は curl ではヘッダーを消す指定なので無視しました（空の値で送るなら '{name};' と書きます）"),
                );
                return;
            }
            (name.to_string(), value.to_string())
        }
        None => match h.trim().strip_suffix(';') {
            Some(n) if !n.trim().is_empty() => (n.trim().to_string(), String::new()),
            _ => {
                push_warning(
                    warnings,
                    WarningKind::Ignored,
                    "-H",
                    format!("-H '{h}' は「名前: 値」の形ではないので無視しました"),
                );
                return;
            }
        },
    };
    if name.is_empty() {
        push_warning(
            warnings,
            WarningKind::Ignored,
            "-H",
            format!("-H '{h}' はヘッダーの名前がないので無視しました"),
        );
        return;
    }
    if name.starts_with(':') {
        push_warning(
            warnings,
            WarningKind::Ignored,
            "-H",
            format!("{name}（HTTP/2 の疑似ヘッダー）は無視しました"),
        );
        return;
    }
    if name.eq_ignore_ascii_case("Content-Length") {
        push_warning(
            warnings,
            WarningKind::Ignored,
            "-H",
            "Content-Length は送るボディから自動で付くため無視しました".to_string(),
        );
        return;
    }
    out.push((name, value));
}

/// `a=1; b=2` を Cookie の行にする。`=` のない区間があれば `None`（ヘッダーのまま残す）。
fn parse_cookie_pairs(s: &str) -> Option<Vec<KeyValue>> {
    let mut out = Vec::new();
    for piece in s.split(';') {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        let (n, v) = piece.split_once('=')?;
        if n.trim().is_empty() {
            return None;
        }
        out.push(row(ParamIn::Cookie, n.trim(), v.trim()));
    }
    Some(out)
}

/// `Authorization` ヘッダーの値を認証に直す。Bearer と Basic（`user:pass` に戻せるもの）だけ。
/// それ以外（Digest、Token など）や、読めないものは `None`（ヘッダーのまま残す）。
fn auth_from_header(value: &str) -> Option<AuthOverride> {
    let (scheme, rest) = value.trim().split_once(char::is_whitespace)?;
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    if scheme.eq_ignore_ascii_case("bearer") {
        return Some(AuthOverride::Bearer {
            token: rest.to_string(),
        });
    }
    if scheme.eq_ignore_ascii_case("basic") {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(rest)
            .ok()?;
        let text = String::from_utf8(decoded).ok()?;
        let (u, p) = text.split_once(':')?;
        return Some(AuthOverride::Basic {
            username: u.to_string(),
            password: p.to_string(),
        });
    }
    None
}

fn basic_from_user(u: &str, warnings: &mut Vec<CurlWarning>) -> AuthOverride {
    match u.split_once(':') {
        Some((n, p)) => AuthOverride::Basic {
            username: n.to_string(),
            password: p.to_string(),
        },
        None => {
            push_warning(
                warnings,
                WarningKind::Adjusted,
                "-u",
                "-u にパスワードがないため、空のパスワードにしました（curl は入力を求めます）"
                    .to_string(),
            );
            AuthOverride::Basic {
                username: u.to_string(),
                password: String::new(),
            }
        }
    }
}

fn urlencode(s: &str) -> String {
    utf8_percent_encode(s, URLENCODE_SET).to_string()
}

/// `-G` で、データをクエリの行にする。
fn data_to_query(p: &DataPiece, query: &mut Vec<KeyValue>, warnings: &mut Vec<CurlWarning>) {
    match p {
        DataPiece::Text { text, .. } => {
            for (k, v) in split_pairs(text) {
                query.push(row(ParamIn::Query, &k, &v));
            }
        }
        DataPiece::UrlEncoded { name, value } => match name {
            Some(n) => query.push(row(ParamIn::Query, n, value)),
            None => query.push(row(ParamIn::Query, value, "")),
        },
        DataPiece::File { path, .. } => push_warning(
            warnings,
            WarningKind::Unsupported,
            "-d",
            format!("@{path} の中身はクエリにできません（ファイルは読めません）"),
        ),
        DataPiece::UrlEncodedFile { name, path } => {
            // 中身は読めない。無効の行にして、パスを見えるようにしておく
            let mut r = row(
                ParamIn::Query,
                name.as_deref().unwrap_or(""),
                &format!("@{path}"),
            );
            r.enabled = false;
            query.push(r);
            push_warning(
                warnings,
                WarningKind::Unsupported,
                "--data-urlencode",
                format!("@{path} の中身は読めないため、値を無効の行にしました"),
            );
        }
    }
}

fn is_json_type(ct: &str) -> bool {
    let media = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    media == "application/json" || media.ends_with("+json")
}

fn media_type(ct: &str) -> String {
    ct.split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// `-d` 系からボディを作る。Content-Type があれば、それに合わせて JSON・フォーム・テキストにする。
fn text_body(
    data: &[DataPiece],
    headers: &mut Vec<(String, String)>,
    warnings: &mut Vec<CurlWarning>,
) -> Body {
    if data.is_empty() {
        return Body::None;
    }
    let content_type = find_header(headers, "Content-Type").map(str::to_string);
    let json_kind = data.iter().any(|p| {
        matches!(
            p,
            DataPiece::Text {
                kind: DataKind::Json,
                ..
            } | DataPiece::File {
                kind: DataKind::Json,
                ..
            }
        )
    });

    // ファイルだけ（1 つ）なら、そのファイルを送るボディ
    if let [DataPiece::File { path, .. }] = data {
        if path == "-" {
            push_warning(
                warnings,
                WarningKind::Unsupported,
                "-d",
                "@-（標準入力）は読めないため、ボディを空にしました".to_string(),
            );
            return Body::None;
        }
        if find_header(headers, "Content-Type")
            .is_some_and(|c| c.trim().eq_ignore_ascii_case("application/octet-stream"))
        {
            // core は Binary のとき同じ Content-Type を付ける
            take_header(headers, "Content-Type");
        }
        return Body::Binary(path.clone());
    }

    // ファイルが混ざっているときは、ファイルを除く（中身を読めないため）
    let mut text_pieces: Vec<&DataPiece> = Vec::new();
    for p in data {
        match p {
            DataPiece::File { path, .. } => push_warning(
                warnings,
                WarningKind::Unsupported,
                "-d",
                format!("@{path} は他の -d と混ぜて送るため読めません（ファイルの中身は取り込めません）"),
            ),
            other => text_pieces.push(other),
        }
    }
    if text_pieces.is_empty() {
        return Body::None;
    }
    let joined = |pieces: &[&DataPiece]| -> String {
        let sep = if json_kind { "" } else { "&" };
        pieces
            .iter()
            .map(|p| match p {
                DataPiece::Text { text, .. } => text.clone(),
                DataPiece::UrlEncoded { name, value } => match name {
                    Some(n) => format!("{n}={}", urlencode(value)),
                    None => urlencode(value),
                },
                DataPiece::UrlEncodedFile { name, path } => match name {
                    Some(n) => format!("{n}=@{path}"),
                    None => format!("@{path}"),
                },
                DataPiece::File { .. } => String::new(),
            })
            .collect::<Vec<_>>()
            .join(sep)
    };

    if json_kind || content_type.as_deref().is_some_and(is_json_type) {
        if content_type
            .as_deref()
            .is_some_and(|c| media_type(c) == "application/json" && !c.contains(';'))
        {
            take_header(headers, "Content-Type");
        }
        return Body::Json(joined(&text_pieces));
    }

    let is_form_type = match content_type.as_deref() {
        None => true,
        Some(c) => media_type(c) == "application/x-www-form-urlencoded",
    };
    if is_form_type {
        if let Some(rows) = form_rows(&text_pieces, warnings) {
            if content_type.is_some() {
                // core は Form のとき同じ Content-Type を付ける
                take_header(headers, "Content-Type");
            }
            return Body::Form(rows);
        }
        // 「name=value」に分けられないもの（JSON の文字列を -d で送るなど）は、文字のまま送る。
        // curl は Content-Type を x-www-form-urlencoded にするので、それを残す
        if content_type.is_none() {
            headers.push((
                "Content-Type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ));
        }
        return Body::Text(joined(&text_pieces));
    }

    let text = joined(&text_pieces);
    if let Some(ct) = content_type.as_deref() {
        if media_type(ct) == "multipart/form-data" {
            if let Some(parts) = parse_multipart_text(ct, &text, warnings) {
                take_header(headers, "Content-Type");
                return Body::Multipart(parts);
            }
        }
        if ct.trim().eq_ignore_ascii_case(AUTO_TEXT_TYPE) {
            take_header(headers, "Content-Type");
        }
    }
    Body::Text(text)
}

/// すべてが `name=value`（`-d` の `&` 区切りと `--data-urlencode name=...`）ならフォームの行にする。
fn form_rows(pieces: &[&DataPiece], warnings: &mut Vec<CurlWarning>) -> Option<Vec<KeyValue>> {
    let mut rows = Vec::new();
    for p in pieces {
        match p {
            DataPiece::Text { text, .. } => {
                if text.is_empty() {
                    continue;
                }
                for seg in text.split('&') {
                    if seg.is_empty() {
                        continue;
                    }
                    let (k, v) = seg.split_once('=')?;
                    rows.push(row(ParamIn::Query, &decode_form(k), &decode_form(v)));
                }
            }
            DataPiece::UrlEncoded {
                name: Some(n),
                value,
            } => {
                rows.push(row(ParamIn::Query, n, value));
            }
            DataPiece::UrlEncodedFile {
                name: Some(n),
                path,
            } => {
                let mut r = row(ParamIn::Query, n, &format!("@{path}"));
                r.enabled = false;
                rows.push(r);
                push_warning(
                    warnings,
                    WarningKind::Unsupported,
                    "--data-urlencode",
                    format!("{n}@{path} の中身は読めないため、値を無効の行にしました"),
                );
            }
            _ => return None,
        }
    }
    if rows.is_empty() {
        return None;
    }
    Some(rows)
}

/// `-F` から multipart のパートを作る。`;type=` はパートの Content-Type として保持する。
/// `;filename=` は core の型に持ち場所がないので警告にする。
fn multipart_body(forms: Vec<FormPiece>, warnings: &mut Vec<CurlWarning>) -> Body {
    let mut parts = Vec::new();
    for f in forms {
        if let Some(n) = &f.filename {
            push_warning(
                warnings,
                WarningKind::Unsupported,
                "-F",
                format!(
                    "{}: ;filename={n} は保持できません（ファイル名はパスから決まります）",
                    f.name
                ),
            );
        }
        let value = match f.value {
            FormValue::Text(t) => MultipartValue::Text(t),
            FormValue::File(path) => {
                if f.from_file_content {
                    push_warning(
                        warnings,
                        WarningKind::Adjusted,
                        "-F",
                        format!("{}=<{path} はファイルの中身を文字として送る指定ですが、ファイルとして送ります", f.name),
                    );
                }
                MultipartValue::File(path)
            }
        };
        parts.push(MultipartPart {
            name: f.name,
            value,
            enabled: true,
            content_type: f.content_type.filter(|t| !t.is_empty()),
        });
    }
    Body::Multipart(parts)
}

/// Chrome の「Copy as cURL」が、ファイルなしの multipart を `--data-raw` に展開した本文を、パートの行に戻す。
/// 読めなければ `None`（テキストのまま扱う）。ファイルのパートは中身がないので、無効の行にしてファイル名を残す。
fn parse_multipart_text(
    content_type: &str,
    body: &str,
    warnings: &mut Vec<CurlWarning>,
) -> Option<Vec<MultipartPart>> {
    let boundary = content_type
        .split(';')
        .skip(1)
        .filter_map(|p| p.trim().strip_prefix("boundary="))
        .next()?
        .trim_matches('"')
        .to_string();
    if boundary.is_empty() {
        return None;
    }
    let delim = format!("--{boundary}");
    let body = body.replace("\r\n", "\n");
    if !body.trim_start().starts_with(&delim) {
        return None;
    }
    let mut parts = Vec::new();
    for chunk in body.split(&delim).skip(1) {
        let chunk = chunk.strip_prefix('\n').unwrap_or(chunk);
        if chunk.trim_start().starts_with("--") && chunk.trim().len() <= 2 {
            break; // 終端の `--boundary--`
        }
        let (head, content) = chunk.split_once("\n\n")?;
        let content = content.strip_suffix('\n').unwrap_or(content);
        let disposition = head
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("content-disposition:"))?;
        let name = disposition_param(disposition, "name")?;
        match disposition_param(disposition, "filename") {
            Some(file) => {
                push_warning(
                    warnings,
                    WarningKind::Unsupported,
                    "--data-raw",
                    format!("{name}: ファイル「{file}」の中身は取り込めないため、無効の行にしました（パスを選び直してください）"),
                );
                parts.push(MultipartPart {
                    name,
                    value: MultipartValue::File(file),
                    enabled: false,
                    content_type: None,
                });
            }
            None => parts.push(MultipartPart {
                name,
                value: MultipartValue::Text(content.to_string()),
                enabled: true,
                content_type: None,
            }),
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts)
    }
}

fn disposition_param(line: &str, key: &str) -> Option<String> {
    let pat = format!("{key}=\"");
    let mut search = 0;
    while let Some(i) = line[search..].find(&pat) {
        let at = search + i;
        // `filename=` を `name=` と取り違えない
        let boundary_ok = at == 0 || matches!(line.as_bytes()[at - 1], b' ' | b';');
        if boundary_ok {
            let rest = &line[at + pat.len()..];
            let end = rest.find('"')?;
            return Some(decode_component(&rest[..end]));
        }
        search = at + pat.len();
    }
    None
}
