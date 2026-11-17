//! オプションの読み取り。単語の列を、種類ごとに集めた `Raw` にする。
//! curl と同じく、短いオプションは `-sSL` のようにまとめられ、値は `-XPOST`・`-X POST`・`--request=POST` の形で渡せる。

use crate::error::CurlError;
use crate::model::{CurlWarning, WarningKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DataKind {
    /// `-d` / `--data` / `--data-ascii`（`@file` を読む）
    Data,
    /// `--data-raw`（`@` もそのまま）
    Raw,
    /// `--data-binary`（`@file` を読む）
    Binary,
    /// `--json`（`@file` を読む。Content-Type と Accept を JSON にする）
    Json,
}

/// `-d` 系の 1 つぶん。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DataPiece {
    Text {
        kind: DataKind,
        text: String,
    },
    File {
        kind: DataKind,
        path: String,
    },
    /// `--data-urlencode` の `name=content` / `=content` / `content`。値はエンコード前。
    UrlEncoded {
        name: Option<String>,
        value: String,
    },
    /// `--data-urlencode` の `name@file` / `@file`。
    UrlEncodedFile {
        name: Option<String>,
        path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FormValue {
    Text(String),
    File(String),
}

/// `-F` の 1 つぶん。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FormPiece {
    pub name: String,
    pub value: FormValue,
    pub content_type: Option<String>,
    pub filename: Option<String>,
    /// `name=<file`（ファイルの中身を文字として送る）。
    pub from_file_content: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Raw {
    pub method: Option<String>,
    pub headers: Vec<String>,
    pub data: Vec<DataPiece>,
    pub forms: Vec<FormPiece>,
    pub user: Option<String>,
    pub oauth_bearer: Option<String>,
    pub cookies: Vec<String>,
    pub get: bool,
    pub head: bool,
    pub urls: Vec<String>,
    pub referer: Option<String>,
    pub user_agent: Option<String>,
    pub warnings: Vec<CurlWarning>,
}

#[derive(Clone, Copy)]
enum Action {
    Method,
    Header,
    Data(DataKind),
    UrlEncode,
    Form,
    FormString,
    User,
    Cookie,
    Get,
    Head,
    Url,
    Referer,
    UserAgent,
    OAuthBearer,
    Digest,
    /// 読み捨てる。説明は画面の「無視したオプション」に出す。
    Ignore(&'static str),
    /// 取り込めない（内容が欠ける）。
    Unsupported(&'static str),
}

struct Spec {
    short: Option<char>,
    long: &'static str,
    takes_arg: bool,
    action: Action,
}

const fn s(short: Option<char>, long: &'static str, takes_arg: bool, action: Action) -> Spec {
    Spec {
        short,
        long,
        takes_arg,
        action,
    }
}

const OUTPUT: &str = "出力の指定はアプリでは使いません（レスポンスは画面に出ます）";
const TLS: &str = "証明書の検証は設定画面で切り替えます";
const REDIRECT: &str = "リダイレクトの追従は設定画面で切り替えます";
const QUIET: &str = "表示の指定はアプリでは使いません";
const PROXY: &str = "プロキシは設定画面で指定します";
const TIMEOUT: &str = "タイムアウトは設定画面で指定します";
const HTTPVER: &str = "HTTP のバージョンはアプリが決めます";

#[rustfmt::skip]
const SPECS: &[Spec] = &[
    s(Some('X'), "request", true, Action::Method),
    s(Some('H'), "header", true, Action::Header),
    s(Some('d'), "data", true, Action::Data(DataKind::Data)),
    s(None, "data-ascii", true, Action::Data(DataKind::Data)),
    s(None, "data-raw", true, Action::Data(DataKind::Raw)),
    s(None, "data-binary", true, Action::Data(DataKind::Binary)),
    s(None, "data-urlencode", true, Action::UrlEncode),
    s(None, "json", true, Action::Data(DataKind::Json)),
    s(Some('F'), "form", true, Action::Form),
    s(None, "form-string", true, Action::FormString),
    s(Some('u'), "user", true, Action::User),
    s(Some('b'), "cookie", true, Action::Cookie),
    s(Some('G'), "get", false, Action::Get),
    s(Some('I'), "head", false, Action::Head),
    s(None, "url", true, Action::Url),
    s(Some('e'), "referer", true, Action::Referer),
    s(Some('A'), "user-agent", true, Action::UserAgent),
    s(None, "oauth2-bearer", true, Action::OAuthBearer),
    s(None, "digest", false, Action::Digest),
    s(None, "basic", false, Action::Ignore("Basic 認証は -u から読みます")),
    s(None, "compressed", false, Action::Ignore("圧縮はアプリが自動で扱います")),
    s(None, "compressed-ssh", false, Action::Ignore("圧縮はアプリが自動で扱います")),
    s(Some('k'), "insecure", false, Action::Ignore(TLS)),
    s(None, "cacert", true, Action::Ignore(TLS)),
    s(None, "capath", true, Action::Ignore(TLS)),
    s(Some('E'), "cert", true, Action::Ignore("クライアント証明書は設定画面で指定します")),
    s(None, "key", true, Action::Ignore("クライアント証明書は設定画面で指定します")),
    s(None, "ssl-no-revoke", false, Action::Ignore(TLS)),
    s(None, "tlsv1", false, Action::Ignore(TLS)),
    s(None, "tlsv1.0", false, Action::Ignore(TLS)),
    s(None, "tlsv1.1", false, Action::Ignore(TLS)),
    s(None, "tlsv1.2", false, Action::Ignore(TLS)),
    s(None, "tlsv1.3", false, Action::Ignore(TLS)),
    s(None, "tls-max", true, Action::Ignore(TLS)),
    s(None, "ciphers", true, Action::Ignore(TLS)),
    s(Some('L'), "location", false, Action::Ignore(REDIRECT)),
    s(None, "location-trusted", false, Action::Ignore(REDIRECT)),
    s(None, "max-redirs", true, Action::Ignore(REDIRECT)),
    s(Some('s'), "silent", false, Action::Ignore(QUIET)),
    s(Some('S'), "show-error", false, Action::Ignore(QUIET)),
    s(Some('v'), "verbose", false, Action::Ignore(QUIET)),
    s(Some('i'), "include", false, Action::Ignore(QUIET)),
    s(None, "trace", true, Action::Ignore(QUIET)),
    s(None, "trace-ascii", true, Action::Ignore(QUIET)),
    s(None, "trace-time", false, Action::Ignore(QUIET)),
    s(Some('#'), "progress-bar", false, Action::Ignore(QUIET)),
    s(None, "no-progress-meter", false, Action::Ignore(QUIET)),
    s(Some('N'), "no-buffer", false, Action::Ignore(QUIET)),
    s(Some('f'), "fail", false, Action::Ignore("HTTP エラーでも画面にレスポンスが出ます")),
    s(None, "fail-with-body", false, Action::Ignore("HTTP エラーでも画面にレスポンスが出ます")),
    s(Some('o'), "output", true, Action::Ignore(OUTPUT)),
    s(Some('O'), "remote-name", false, Action::Ignore(OUTPUT)),
    s(Some('J'), "remote-header-name", false, Action::Ignore(OUTPUT)),
    s(Some('D'), "dump-header", true, Action::Ignore(OUTPUT)),
    s(Some('w'), "write-out", true, Action::Ignore(OUTPUT)),
    s(Some('c'), "cookie-jar", true, Action::Ignore("Cookie の保存はアプリが自動で扱います")),
    s(Some('j'), "junk-session-cookies", false, Action::Ignore("Cookie の保存はアプリが自動で扱います")),
    s(Some('m'), "max-time", true, Action::Ignore(TIMEOUT)),
    s(None, "connect-timeout", true, Action::Ignore(TIMEOUT)),
    s(None, "expect100-timeout", true, Action::Ignore(TIMEOUT)),
    s(None, "retry", true, Action::Ignore("再試行はアプリでは行いません")),
    s(None, "retry-delay", true, Action::Ignore("再試行はアプリでは行いません")),
    s(None, "retry-max-time", true, Action::Ignore("再試行はアプリでは行いません")),
    s(None, "retry-connrefused", false, Action::Ignore("再試行はアプリでは行いません")),
    s(Some('x'), "proxy", true, Action::Ignore(PROXY)),
    s(Some('U'), "proxy-user", true, Action::Ignore(PROXY)),
    s(None, "noproxy", true, Action::Ignore(PROXY)),
    s(None, "proxy-insecure", false, Action::Ignore(PROXY)),
    s(None, "http1.0", false, Action::Ignore(HTTPVER)),
    s(None, "http1.1", false, Action::Ignore(HTTPVER)),
    s(None, "http2", false, Action::Ignore(HTTPVER)),
    s(None, "http2-prior-knowledge", false, Action::Ignore(HTTPVER)),
    s(None, "http3", false, Action::Ignore(HTTPVER)),
    s(Some('0'), "http1.0-short", false, Action::Ignore(HTTPVER)),
    s(Some('4'), "ipv4", false, Action::Ignore("IP のバージョンはアプリが決めます")),
    s(Some('6'), "ipv6", false, Action::Ignore("IP のバージョンはアプリが決めます")),
    s(Some('g'), "globoff", false, Action::Ignore("URL の `[]` `{}` はそのまま読みます")),
    s(None, "path-as-is", false, Action::Ignore("パスの整形はアプリが決めます")),
    s(None, "resolve", true, Action::Ignore("名前解決の上書きはアプリでは使いません")),
    s(None, "connect-to", true, Action::Ignore("接続先の上書きはアプリでは使いません")),
    s(None, "interface", true, Action::Ignore("通信の経路の指定はアプリでは使いません")),
    s(None, "limit-rate", true, Action::Ignore("通信速度の制限はアプリでは使いません")),
    s(None, "keepalive-time", true, Action::Ignore("接続の維持はアプリが決めます")),
    s(None, "no-keepalive", false, Action::Ignore("接続の維持はアプリが決めます")),
    s(Some('n'), "netrc", false, Action::Ignore("netrc は読みません")),
    s(Some('q'), "disable", false, Action::Ignore("設定ファイルは読みません")),
    s(Some('Z'), "parallel", false, Action::Ignore("並列実行はアプリでは使いません")),
    s(Some('a'), "append", false, Action::Ignore("FTP の指定はアプリでは使いません")),
    s(Some('r'), "range", true, Action::Unsupported("範囲指定（Range ヘッダーに直してください）")),
    s(Some('T'), "upload-file", true, Action::Unsupported("ファイルのアップロード（-T）は読めません。--data-binary @ファイル を使ってください")),
    s(Some('K'), "config", true, Action::Unsupported("設定ファイルは読めません")),
    s(Some('h'), "help", false, Action::Unsupported("ヘルプの表示は取り込めません")),
    s(Some('V'), "version", false, Action::Unsupported("バージョンの表示は取り込めません")),
];

fn find_long(name: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.long == name)
}

fn find_short(c: char) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.short == Some(c))
}

impl Raw {
    fn warn(&mut self, kind: WarningKind, option: &str, message: String) {
        self.warnings.push(CurlWarning::new(kind, option, message));
    }

    fn apply(&mut self, label: &str, spec: &Spec, value: Option<String>) -> Result<(), CurlError> {
        let value = || value.clone().unwrap_or_default();
        match spec.action {
            Action::Method => self.method = Some(value()),
            Action::Header => self.headers.push(value()),
            Action::Data(kind) => self.add_data(kind, &value()),
            Action::UrlEncode => self.add_urlencode(&value()),
            Action::Form => self.add_form(label, &value(), false),
            Action::FormString => self.add_form(label, &value(), true),
            Action::User => self.user = Some(value()),
            Action::OAuthBearer => self.oauth_bearer = Some(value()),
            Action::Cookie => self.cookies.push(value()),
            Action::Get => self.get = true,
            Action::Head => self.head = true,
            Action::Url => self.urls.push(value()),
            Action::Referer => self.referer = Some(value()),
            Action::UserAgent => self.user_agent = Some(value()),
            Action::Digest => self.warn(
                WarningKind::Unsupported,
                label,
                format!("{label} は未対応です（Digest 認証は v1 では扱いません）"),
            ),
            Action::Ignore(reason) => self.warn(
                WarningKind::Ignored,
                label,
                format!("{label} は無視しました（{reason}）"),
            ),
            Action::Unsupported(reason) => self.warn(
                WarningKind::Unsupported,
                label,
                format!("{label} は取り込めません（{reason}）"),
            ),
        }
        Ok(())
    }

    fn add_data(&mut self, kind: DataKind, arg: &str) {
        if kind != DataKind::Raw {
            if let Some(path) = arg.strip_prefix('@') {
                self.data.push(DataPiece::File {
                    kind,
                    path: path.to_string(),
                });
                return;
            }
        }
        self.data.push(DataPiece::Text {
            kind,
            text: arg.to_string(),
        });
    }

    /// curl と同じ規則: `=` があれば `name=content`（先頭の `=` は名前なし）、なければ `@` で `name@file`、
    /// どちらもなければ全体が content。
    fn add_urlencode(&mut self, arg: &str) {
        let (name, rest, is_file) = if let Some(i) = arg.find('=') {
            (&arg[..i], &arg[i + 1..], false)
        } else if let Some(i) = arg.find('@') {
            (&arg[..i], &arg[i + 1..], true)
        } else {
            ("", arg, false)
        };
        let name = (!name.is_empty()).then(|| name.to_string());
        self.data.push(if is_file {
            DataPiece::UrlEncodedFile {
                name,
                path: rest.to_string(),
            }
        } else {
            DataPiece::UrlEncoded {
                name,
                value: rest.to_string(),
            }
        });
    }

    fn add_form(&mut self, label: &str, arg: &str, literal: bool) {
        match parse_form(arg, literal) {
            Some(p) => self.forms.push(p),
            None => self.warn(
                WarningKind::Unsupported,
                label,
                format!("{label} '{arg}' は name=value の形ではないので読み飛ばしました"),
            ),
        }
    }
}

/// `-F` の値（`name=value`、`name=@file;type=text/plain`、`name=<file`、`name="a;b";filename=x`）。
fn parse_form(arg: &str, literal: bool) -> Option<FormPiece> {
    let (name, rest) = arg.split_once('=')?;
    if literal {
        return Some(FormPiece {
            name: name.to_string(),
            value: FormValue::Text(rest.to_string()),
            content_type: None,
            filename: None,
            from_file_content: false,
        });
    }
    let is_file = rest.starts_with('@');
    let is_content_file = rest.starts_with('<');
    let (value, params) = if is_file || is_content_file {
        split_value_and_params(&rest[1..])
    } else {
        split_value_and_params(rest)
    };
    let mut piece = FormPiece {
        name: name.to_string(),
        value: if is_file || is_content_file {
            FormValue::File(value)
        } else {
            FormValue::Text(value)
        },
        content_type: None,
        filename: None,
        from_file_content: is_content_file,
    };
    for p in params.split(';').filter(|p| !p.is_empty()) {
        if let Some(t) = p.strip_prefix("type=") {
            piece.content_type = Some(t.trim().to_string());
        } else if let Some(f) = p.strip_prefix("filename=") {
            piece.filename = Some(unquote(f.trim()));
        }
    }
    Some(piece)
}

/// 値と `;type=...` などの部分に分ける。値が `"` で始まるときは閉じる `"` までが値（`\"` と `\\` を読む）。
fn split_value_and_params(s: &str) -> (String, String) {
    if let Some(inner) = s.strip_prefix('"') {
        let mut value = String::new();
        let mut chars = inner.char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some((_, n @ ('"' | '\\'))) => value.push(n),
                    Some((_, n)) => {
                        value.push('\\');
                        value.push(n);
                    }
                    None => value.push('\\'),
                },
                '"' => {
                    let tail = &inner[i + 1..];
                    return (value, tail.trim_start_matches(';').to_string());
                }
                _ => value.push(c),
            }
        }
        // 閉じていなければ全体を値にする
        return (s.to_string(), String::new());
    }
    // `;type=` `;filename=` `;headers=` `;encoder=` の手前までが値（`a;b` のような `;` を含む文字はそのまま値）
    let mut search_from = 0;
    while let Some(rel) = s[search_from..].find(';') {
        let at = search_from + rel;
        let tail = &s[at + 1..];
        if ["type=", "filename=", "headers=", "encoder=", "headers=@"]
            .iter()
            .any(|k| tail.starts_with(k))
        {
            return (s[..at].to_string(), tail.to_string());
        }
        search_from = at + 1;
    }
    (s.to_string(), String::new())
}

fn unquote(s: &str) -> String {
    s.strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(s)
        .to_string()
}

/// `curl` より前に付く `sudo`・`$`・`FOO=bar` と、コマンド名を除く。
fn strip_command_prefix(tokens: &mut Vec<String>) {
    let pos = tokens
        .iter()
        .take(6)
        .position(|t| t == "curl" || t == "curl.exe" || t.ends_with("/curl"));
    if let Some(p) = pos {
        let prefix_ok = tokens[..p].iter().all(|t| {
            t == "$"
                || t == "sudo"
                || t == "command"
                || (t.contains('=') && !t.starts_with('-') && !t.contains("://"))
        });
        if prefix_ok {
            tokens.drain(..=p);
        }
    }
}

pub(crate) fn read_options(mut tokens: Vec<String>) -> Result<Raw, CurlError> {
    strip_command_prefix(&mut tokens);
    let mut raw = Raw::default();
    let mut iter = tokens.into_iter();
    let mut only_positional = false;
    while let Some(tok) = iter.next() {
        if only_positional {
            raw.urls.push(tok);
            continue;
        }
        // シェルの区切り（`| jq .` や `> out.json`、`&& echo`）から後ろは curl の引数ではない
        if matches!(
            tok.as_str(),
            "|" | "||" | "&&" | ";" | "&" | ">" | ">>" | "<" | "2>&1"
        ) {
            raw.warn(
                WarningKind::Ignored,
                &tok,
                format!("{tok} 以降はシェルの記述なので読みませんでした"),
            );
            break;
        }
        if tok == "--" {
            only_positional = true;
        } else if let Some(body) = tok.strip_prefix("--") {
            let (name, inline) = match body.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (body, None),
            };
            let label = format!("--{name}");
            let negated = name
                .strip_prefix("no-")
                .filter(|_| find_long(name).is_none());
            match find_long(name) {
                Some(spec) => {
                    let value = if spec.takes_arg {
                        match inline {
                            Some(v) => Some(v),
                            None => {
                                Some(iter.next().ok_or_else(|| CurlError::MissingArgument {
                                    option: label.clone(),
                                })?)
                            }
                        }
                    } else {
                        None
                    };
                    raw.apply(&label, spec, value)?;
                }
                None if negated.is_some() => raw.warn(
                    WarningKind::Ignored,
                    &label,
                    format!("{label} は無視しました（既定の動作を切る指定です）"),
                ),
                None => raw.warn(
                    WarningKind::Unknown,
                    &label,
                    format!("{label} は知らないオプションなので無視しました"),
                ),
            }
        } else if tok.len() > 1 && tok.starts_with('-') {
            let chars: Vec<char> = tok[1..].chars().collect();
            let mut i = 0;
            while i < chars.len() {
                let c = chars[i];
                let label = format!("-{c}");
                i += 1;
                match find_short(c) {
                    Some(spec) => {
                        let value = if spec.takes_arg {
                            let attached: String = chars[i..].iter().collect();
                            i = chars.len();
                            if attached.is_empty() {
                                Some(iter.next().ok_or_else(|| CurlError::MissingArgument {
                                    option: label.clone(),
                                })?)
                            } else {
                                Some(attached)
                            }
                        } else {
                            None
                        };
                        raw.apply(&label, spec, value)?;
                    }
                    None => raw.warn(
                        WarningKind::Unknown,
                        &label,
                        format!("{label} は知らないオプションなので無視しました"),
                    ),
                }
            }
        } else if tok != "-" {
            // 位置引数は URL（`--url` と出てきた順に並べる）
            raw.urls.push(tok);
        }
    }
    Ok(raw)
}
