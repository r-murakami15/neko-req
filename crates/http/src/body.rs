//! 送るボディの用意。ファイルを読み、multipart を組み立てる。ボディはメモリに載せてから送る。

use std::path::Path;

use bytes::Bytes;
use nekoreq_core::{PreparedBody, PreparedPartValue, SendError};

/// ボディを送る形（バイト列）にする。ボディがなければ `None`。
pub(crate) async fn load_body(body: &PreparedBody) -> Result<Option<Bytes>, SendError> {
    match body {
        PreparedBody::None => Ok(None),
        PreparedBody::Bytes(b) => Ok(Some(Bytes::from(b.clone()))),
        PreparedBody::File(path) => Ok(Some(Bytes::from(read_file(path).await?))),
        PreparedBody::Multipart { boundary, parts } => {
            let mut out: Vec<u8> = Vec::new();
            for part in parts {
                out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
                match &part.value {
                    PreparedPartValue::Text(text) => {
                        out.extend_from_slice(
                            format!(
                                "Content-Disposition: form-data; name=\"{}\"\r\n\r\n",
                                quote(&part.name)
                            )
                            .as_bytes(),
                        );
                        out.extend_from_slice(text.as_bytes());
                    }
                    PreparedPartValue::File(path) => {
                        let data = read_file(path).await?;
                        let file_name = Path::new(path)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("file");
                        out.extend_from_slice(
                            format!(
                                "Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
                                quote(&part.name),
                                quote(file_name),
                                guess_content_type(file_name)
                            )
                            .as_bytes(),
                        );
                        out.extend_from_slice(&data);
                    }
                }
                out.extend_from_slice(b"\r\n");
            }
            out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
            Ok(Some(Bytes::from(out)))
        }
    }
}

async fn read_file(path: &str) -> Result<Vec<u8>, SendError> {
    tokio::fs::read(path)
        .await
        .map_err(|e| SendError::BodyFile {
            message: format!("ボディのファイルを読めません: {path}（{e}）"),
        })
}

/// 引用符つきの値に入れられるようにする。
fn quote(s: &str) -> String {
    s.replace('"', "%22").replace(['\r', '\n'], " ")
}

fn guess_content_type(file_name: &str) -> &'static str {
    let ext = file_name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "json" => "application/json",
        "txt" | "md" | "log" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "zip" => "application/zip",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nekoreq_core::PreparedPart;

    #[tokio::test]
    async fn multipart_is_encoded_with_text_and_file_parts() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.json");
        std::fs::write(&file, b"{\"x\":1}").unwrap();
        let body = PreparedBody::Multipart {
            boundary: "BB".into(),
            parts: vec![
                PreparedPart {
                    name: "title".into(),
                    value: PreparedPartValue::Text("こんにちは".into()),
                },
                PreparedPart {
                    name: "f\"ile".into(),
                    value: PreparedPartValue::File(file.display().to_string()),
                },
            ],
        };
        let bytes = load_body(&body).await.unwrap().unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(
            text,
            "--BB\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nこんにちは\r\n\
             --BB\r\nContent-Disposition: form-data; name=\"f%22ile\"; filename=\"a.json\"\r\nContent-Type: application/json\r\n\r\n{\"x\":1}\r\n\
             --BB--\r\n"
        );
    }

    #[tokio::test]
    async fn missing_file_is_a_body_file_error() {
        let err = load_body(&PreparedBody::File("/nonexistent/x.bin".into()))
            .await
            .unwrap_err();
        assert!(matches!(err, SendError::BodyFile { .. }), "{err:?}");
        assert!(err.message().contains("/nonexistent/x.bin"));
    }

    #[tokio::test]
    async fn none_and_bytes() {
        assert!(load_body(&PreparedBody::None).await.unwrap().is_none());
        let b = load_body(&PreparedBody::Bytes(b"abc".to_vec()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&b[..], b"abc");
    }
}
