#![allow(dead_code)]

use nekoreq_core::{
    build_request, Body, KeyValue, ParamIn, PreparedBody, PreparedPartValue, RequestSpec,
    SystemDynamics, VarScope,
};
use nekoreq_curl::{parse, ParsedCurl, WarningKind};

pub fn p(cmd: &str) -> ParsedCurl {
    parse(cmd).unwrap_or_else(|e| panic!("parse failed: {e}\n{cmd}"))
}

pub fn header<'a>(c: &'a ParsedCurl, name: &str) -> Option<&'a str> {
    c.headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}

pub fn header_names(c: &ParsedCurl) -> Vec<&str> {
    c.headers.iter().map(|h| h.name.as_str()).collect()
}

pub fn pairs(rows: &[KeyValue]) -> Vec<(&str, &str)> {
    rows.iter()
        .map(|r| (r.name.as_str(), r.value.as_str()))
        .collect()
}

pub fn kv(location: ParamIn, name: &str, value: &str) -> KeyValue {
    KeyValue {
        location,
        name: name.to_string(),
        value: value.to_string(),
        enabled: true,
    }
}

pub fn warning_options(c: &ParsedCurl, kind: WarningKind) -> Vec<&str> {
    c.warnings
        .iter()
        .filter(|w| w.kind == kind)
        .map(|w| w.option.as_str())
        .collect()
}

/// JSON の文字列から、文字列の外の空白を除く（キーの順は書かれたまま）。
pub fn compact(text: &str) -> String {
    let mut out = String::new();
    let mut in_str = false;
    let mut esc = false;
    for c in text.chars() {
        if in_str {
            out.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if !c.is_whitespace() {
            out.push(c);
        }
    }
    out
}

pub fn json_text(c: &ParsedCurl) -> &str {
    match &c.body {
        Body::Json(t) => t,
        other => panic!("JSON ボディではありません: {other:?}"),
    }
}

/// core の組み立て結果を、順序とランダムな境界に左右されない形にする（往復の同値の確認用）。
#[derive(Debug, PartialEq, Eq)]
pub struct Canon {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

pub fn canon(spec: &RequestSpec) -> Canon {
    let prepared = build_request(spec, &VarScope::new(), &SystemDynamics)
        .unwrap_or_else(|e| panic!("build failed: {e}\n{spec:?}"));
    let mut headers: Vec<(String, String)> = prepared
        .headers
        .iter()
        .map(|h| {
            let value = if h.name.eq_ignore_ascii_case("content-type")
                && h.value.starts_with("multipart/form-data")
            {
                "multipart/form-data".to_string()
            } else {
                h.value.clone()
            };
            (h.name.to_ascii_lowercase(), value)
        })
        .collect();
    headers.sort();
    let body = match &prepared.body {
        PreparedBody::None => "none".to_string(),
        PreparedBody::Bytes(b) => format!("bytes:{}", String::from_utf8_lossy(b)),
        PreparedBody::File(f) => format!("file:{f}"),
        PreparedBody::Multipart { parts, .. } => parts
            .iter()
            .map(|p| match &p.value {
                PreparedPartValue::Text(t) => format!("{}=text:{t}", p.name),
                PreparedPartValue::File(f) => format!("{}=file:{f}", p.name),
            })
            .collect::<Vec<_>>()
            .join("|"),
    };
    Canon {
        method: prepared.method,
        url: prepared.url,
        headers,
        body,
    }
}
