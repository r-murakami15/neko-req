//! レスポンスのボディの展開（gzip・deflate・br）。reqwest の自動展開は使わず、
//! 受け取ったヘッダー（`Content-Encoding`・`Content-Length`）をそのまま画面に返せるようにしている。

use std::io::Read;

use nekoreq_core::HeaderPair;

/// 展開に失敗した理由。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DecodeError {
    /// 壊れている（形式が合わない）。値は方式の名前。
    Corrupt(String),
    /// 展開後が上限を超える。
    TooLarge,
}

/// `Content-Encoding` の方式を、かけられた順（先頭が最初）に返す。`identity` と空は除く。
pub(crate) fn content_encodings(headers: &[HeaderPair]) -> Vec<String> {
    headers
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case("content-encoding"))
        .flat_map(|h| h.value.split(','))
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty() && e != "identity")
        .collect()
}

/// 方式を知っているか（知らない方式のボディは展開せず、そのまま返す）。
fn supported(encoding: &str) -> bool {
    matches!(encoding, "gzip" | "x-gzip" | "deflate" | "br")
}

/// ボディを展開する。複数の方式があれば、かけられたのと逆の順に外す。知らない方式に当たったら、
/// そこまで外した状態で止める（バイナリとして返る）。展開後が `max` バイトを超えたら `TooLarge`。
/// 重い処理なので、呼び出し側は `spawn_blocking` で呼ぶこと。
pub(crate) fn decode_body(
    raw: Vec<u8>,
    encodings: &[String],
    max: usize,
) -> Result<Vec<u8>, DecodeError> {
    let mut data = raw;
    for encoding in encodings.iter().rev() {
        if !supported(encoding) {
            break;
        }
        if data.is_empty() {
            // HEAD・204 などの空のボディ
            continue;
        }
        data = decode_one(&data, encoding, max)?;
    }
    Ok(data)
}

fn decode_one(data: &[u8], encoding: &str, max: usize) -> Result<Vec<u8>, DecodeError> {
    let corrupt = || DecodeError::Corrupt(encoding.to_string());
    match encoding {
        "gzip" | "x-gzip" => read_limited(flate2::read::MultiGzDecoder::new(data), max, &corrupt),
        "br" => read_limited(brotli::Decompressor::new(data, 4096), max, &corrupt),
        // HTTP の deflate は zlib 形式。生の deflate を返すサーバーもあるので、だめなら生として読み直す
        _ => match read_limited(flate2::read::ZlibDecoder::new(data), max, &corrupt) {
            Err(DecodeError::Corrupt(_)) => {
                read_limited(flate2::read::DeflateDecoder::new(data), max, &corrupt)
            }
            other => other,
        },
    }
}

fn read_limited(
    reader: impl Read,
    max: usize,
    corrupt: &dyn Fn() -> DecodeError,
) -> Result<Vec<u8>, DecodeError> {
    let mut out = Vec::new();
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    reader
        .take(limit)
        .read_to_end(&mut out)
        .map_err(|_| corrupt())?;
    if out.len() > max {
        return Err(DecodeError::TooLarge);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn raw_deflate(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn brotli(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut w = brotli::CompressorWriter::new(&mut out, 4096, 5, 22);
            w.write_all(data).unwrap();
        }
        out
    }

    fn enc(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn each_encoding_round_trips() {
        let plain = b"hello hello hello hello";
        for (name, data) in [
            ("gzip", gzip(plain)),
            ("x-gzip", gzip(plain)),
            ("deflate", zlib(plain)),
            ("deflate", raw_deflate(plain)),
            ("br", brotli(plain)),
        ] {
            assert_eq!(
                decode_body(data, &enc(&[name]), 1024).unwrap(),
                plain,
                "{name}"
            );
        }
    }

    #[test]
    fn stacked_encodings_are_removed_in_reverse_order() {
        let plain = b"layered body";
        let data = brotli(&gzip(plain)); // gzip のあとに br
        assert_eq!(
            decode_body(data, &enc(&["gzip", "br"]), 1024).unwrap(),
            plain
        );
    }

    #[test]
    fn unknown_encoding_stops_and_returns_what_it_has() {
        let data = b"zstd-bytes".to_vec();
        assert_eq!(
            decode_body(data.clone(), &enc(&["zstd"]), 1024).unwrap(),
            data
        );
        // 内側が知らない方式なら、外側の gzip だけ外す
        let inner = gzip(b"x");
        assert_eq!(
            decode_body(inner.clone(), &enc(&["zstd", "gzip"]), 1024).unwrap(),
            b"x"
        );
    }

    #[test]
    fn empty_input_is_fine_and_garbage_is_corrupt() {
        assert!(decode_body(vec![], &enc(&["gzip"]), 10).unwrap().is_empty());
        assert_eq!(
            decode_body(b"not gzip".to_vec(), &enc(&["gzip"]), 10),
            Err(DecodeError::Corrupt("gzip".into()))
        );
        assert!(matches!(
            decode_body(b"not brotli at all".to_vec(), &enc(&["br"]), 100),
            Err(DecodeError::Corrupt(_))
        ));
    }

    #[test]
    fn output_over_the_limit_is_too_large() {
        let data = gzip(&vec![b'a'; 10_000]);
        assert_eq!(
            decode_body(data.clone(), &enc(&["gzip"]), 9_999),
            Err(DecodeError::TooLarge)
        );
        assert!(decode_body(data, &enc(&["gzip"]), 10_000).is_ok());
    }

    #[test]
    fn content_encodings_reads_every_header_and_skips_identity() {
        let h = |n: &str, v: &str| HeaderPair {
            name: n.into(),
            value: v.into(),
        };
        let headers = [
            h("Content-Encoding", "gzip, BR"),
            h("X-Other", "deflate"),
            h("content-encoding", "identity"),
        ];
        assert_eq!(content_encodings(&headers), ["gzip", "br"]);
    }
}
