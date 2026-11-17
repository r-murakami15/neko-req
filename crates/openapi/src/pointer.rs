//! JSON Pointer（RFC 6901）と `$ref` のフラグメントの扱い。

use percent_encoding::percent_decode_str;
use serde_json::Value;

/// ポインターの 1 区間をエスケープする（`~` → `~0`、`/` → `~1`）。
pub fn escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

/// `parent` に区間を足す。
pub fn child(parent: &str, segment: &str) -> String {
    let mut s = String::with_capacity(parent.len() + segment.len() + 1);
    s.push_str(parent);
    s.push('/');
    s.push_str(&escape(segment));
    s
}

/// 添字の区間を足す。
pub fn child_index(parent: &str, index: usize) -> String {
    format!("{parent}/{index}")
}

fn unescape(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

/// `$ref` の `#` のあとの部分（パーセントエンコードされていることがある）をポインターにする。
/// 空なら文書全体（空のポインター）。`/` で始まらないものは読めない。
pub fn decode_fragment(fragment: &str) -> Result<String, String> {
    if fragment.is_empty() {
        return Ok(String::new());
    }
    let decoded = percent_decode_str(fragment)
        .decode_utf8()
        .map_err(|_| format!("#{fragment} を読めません（UTF-8 ではありません）"))?;
    if !decoded.starts_with('/') {
        return Err(format!(
            "#{fragment} は JSON Pointer（/ で始まる形）ではありません"
        ));
    }
    Ok(decoded.into_owned())
}

/// ポインターが指す値。
pub fn get<'a>(value: &'a Value, pointer: &str) -> Option<&'a Value> {
    if pointer.is_empty() {
        return Some(value);
    }
    let mut cur = value;
    for seg in pointer.split('/').skip(1) {
        let seg = unescape(seg);
        cur = match cur {
            Value::Object(m) => m.get(&seg)?,
            Value::Array(a) => a.get(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// ポインターの最後の区間（デコード済み）。空なら `None`。
pub fn last_segment(pointer: &str) -> Option<String> {
    if pointer.is_empty() {
        return None;
    }
    pointer.rsplit('/').next().map(unescape)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn escape_and_get() {
        assert_eq!(escape("/api/v1/a~b"), "~1api~1v1~1a~0b");
        let v = json!({"paths": {"/a/b": {"get": {"x": [10, 20]}}}});
        let p = child(&child("", "paths"), "/a/b");
        assert_eq!(p, "/paths/~1a~1b");
        assert_eq!(get(&v, &format!("{p}/get/x/1")), Some(&json!(20)));
        assert_eq!(get(&v, "/nope"), None);
        assert_eq!(get(&v, ""), Some(&v));
    }

    #[test]
    fn fragments_are_percent_decoded() {
        assert_eq!(decode_fragment("/a%20b").unwrap(), "/a b");
        assert_eq!(decode_fragment("").unwrap(), "");
        assert!(decode_fragment("foo").is_err());
    }

    #[test]
    fn last_segment_is_unescaped() {
        assert_eq!(
            last_segment("/components/schemas/A~1B").as_deref(),
            Some("A/B")
        );
        assert_eq!(last_segment(""), None);
    }
}
