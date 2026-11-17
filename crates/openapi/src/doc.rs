//! 読んだファイルの集まり（`DocSet`）と、`$ref` の解決。
//!
//! - ファイルは絶対パス（URL）をキーにして 1 回だけ読む。
//! - 相対パスの `$ref` は、`$ref` を書いたファイルの場所が基準。`#/...` だけの参照は書いたファイル自身。

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;
use url::Url;

use crate::pointer;

/// ファイルの場所（ローカルのパスまたは URL）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Loc {
    File(PathBuf),
    Url(Url),
}

/// `.` と `..` を字面で畳む（シンボリックリンクは辿らない）。
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                // ルートより上へは行けない
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn percent_decoded(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

impl Loc {
    /// 一意なキー（読んだファイルの管理に使う）。
    pub fn key(&self) -> String {
        match self {
            Loc::File(p) => p.to_string_lossy().into_owned(),
            Loc::Url(u) => u.to_string(),
        }
    }

    /// `$ref` のファイル部分（`#` の前。空でないもの）を、このファイルを基準にして解決する。
    pub fn join(&self, file_part: &str) -> Result<Loc, String> {
        // 絶対 URL
        if file_part.starts_with("http://") || file_part.starts_with("https://") {
            let mut u = Url::parse(file_part).map_err(|e| format!("URL を読めません: {e}"))?;
            u.set_fragment(None);
            return Ok(Loc::Url(u));
        }
        if let Some(rest) = file_part.strip_prefix("file://") {
            return match self {
                Loc::File(_) => Ok(Loc::File(normalize_path(Path::new(&percent_decoded(rest))))),
                Loc::Url(_) => Err("URL の入口から file:// は読めません".into()),
            };
        }
        match self {
            Loc::Url(base) => {
                let mut u = base
                    .join(file_part)
                    .map_err(|e| format!("参照先の URL を作れません: {e}"))?;
                u.set_fragment(None);
                if u.scheme() != "http" && u.scheme() != "https" {
                    return Err(format!("{} は読めません", u.scheme()));
                }
                Ok(Loc::Url(u))
            }
            Loc::File(base) => {
                let decoded = percent_decoded(file_part);
                let p = Path::new(&decoded);
                let joined = if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    base.parent().unwrap_or(Path::new("")).join(p)
                };
                Ok(Loc::File(normalize_path(&joined)))
            }
        }
    }

    /// 入口 `entry` からの相対表示（`video/client.yaml` など）。
    pub fn display_from(&self, entry: &Loc) -> String {
        match (self, entry) {
            (Loc::File(p), Loc::File(e)) => {
                let base = e.parent().unwrap_or(Path::new(""));
                relative_path(base, p)
            }
            (Loc::Url(u), Loc::Url(e)) => {
                let same_origin = u.scheme() == e.scheme()
                    && u.host_str() == e.host_str()
                    && u.port_or_known_default() == e.port_or_known_default();
                if same_origin {
                    let mut dir = e.clone();
                    dir.set_query(None);
                    let dir_path = dir.path().rsplit_once('/').map(|x| x.0).unwrap_or("");
                    if let Some(rest) = u.path().strip_prefix(&format!("{dir_path}/")) {
                        return rest.to_string();
                    }
                }
                strip_secrets(u)
            }
            (Loc::Url(u), _) => strip_secrets(u),
            (Loc::File(p), _) => p.to_string_lossy().into_owned(),
        }
    }

    pub fn is_url(&self) -> bool {
        matches!(self, Loc::Url(_))
    }
}

/// 表示用に URL からクエリとユーザー情報を除く。
pub fn strip_secrets(u: &Url) -> String {
    let mut u = u.clone();
    u.set_query(None);
    u.set_fragment(None);
    let _ = u.set_username("");
    let _ = u.set_password(None);
    u.to_string()
}

/// `base`（ディレクトリ）から `target` への相対パス（`/` 区切り）。
pub fn relative_path(base: &Path, target: &Path) -> String {
    let base: Vec<_> = base.components().collect();
    let target_c: Vec<_> = target.components().collect();
    let common = base
        .iter()
        .zip(target_c.iter())
        .take_while(|(a, b)| a == b)
        .count();
    if common == 0 {
        return target.to_string_lossy().replace('\\', "/");
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in common..base.len() {
        parts.push("..".to_string());
    }
    for c in &target_c[common..] {
        parts.push(c.as_os_str().to_string_lossy().into_owned());
    }
    parts.join("/")
}

/// 読んだファイル 1 つ。
#[derive(Debug, Clone)]
pub struct FileDoc {
    pub loc: Loc,
    /// 入口からの相対パス（URL なら相対 URL か URL）。
    pub display: String,
    pub value: Value,
    pub lines: HashMap<String, u32>,
    pub modified_at: Option<String>,
    pub hash: String,
    pub bytes: usize,
}

impl FileDoc {
    /// ポインターの行番号（なければポインターを遡って近い行）。
    pub fn line_of(&self, ptr: &str) -> Option<u32> {
        let mut p = ptr;
        loop {
            if let Some(l) = self.lines.get(p) {
                return Some(*l);
            }
            match p.rfind('/') {
                Some(i) => p = &p[..i],
                None => return self.lines.get("").copied(),
            }
        }
    }
}

/// 読んだファイルの集まり。`files[0]` が入口。
#[derive(Debug, Clone, Default)]
pub struct DocSet {
    pub files: Vec<FileDoc>,
    index: HashMap<String, usize>,
}

/// `$ref` が指していたもの。
#[derive(Debug, Clone)]
pub struct Target<'a> {
    pub file: usize,
    pub pointer: String,
    pub value: &'a Value,
}

/// `$ref` を解決できなかった理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefFail {
    /// ファイルを読んでいない（足りない）。
    MissingFile(String),
    /// ファイルはあるが、指すポインターがない。
    PointerNotFound(String),
    /// `$ref` の書き方が読めない、または `$ref` の連鎖が循環している。
    Invalid(String),
}

impl RefFail {
    pub fn reason(&self) -> String {
        match self {
            RefFail::MissingFile(f) => format!("{f} を読んでいません"),
            RefFail::PointerNotFound(r) => format!("{r} が見つかりません"),
            RefFail::Invalid(m) => m.clone(),
        }
    }
}

/// `$ref` を（ファイル部分, フラグメント）に分ける。
pub fn split_ref(reference: &str) -> (&str, &str) {
    match reference.split_once('#') {
        Some((f, frag)) => (f, frag),
        None => (reference, ""),
    }
}

impl DocSet {
    pub fn push(&mut self, doc: FileDoc) -> usize {
        let i = self.files.len();
        self.index.insert(doc.loc.key(), i);
        self.files.push(doc);
        i
    }

    pub fn find(&self, loc: &Loc) -> Option<usize> {
        self.index.get(&loc.key()).copied()
    }

    pub fn entry(&self) -> &FileDoc {
        &self.files[0]
    }

    /// `from` のファイルに書かれた `reference` を 1 段だけ解決する。
    pub fn resolve(&self, from: usize, reference: &str) -> Result<Target<'_>, RefFail> {
        let (file_part, fragment) = split_ref(reference);
        let target_file = if file_part.is_empty() {
            from
        } else {
            let loc = self.files[from]
                .loc
                .join(file_part)
                .map_err(RefFail::Invalid)?;
            self.find(&loc)
                .ok_or_else(|| RefFail::MissingFile(file_part.to_string()))?
        };
        let ptr = pointer::decode_fragment(fragment).map_err(RefFail::Invalid)?;
        let value = pointer::get(&self.files[target_file].value, &ptr)
            .ok_or_else(|| RefFail::PointerNotFound(reference.to_string()))?;
        Ok(Target {
            file: target_file,
            pointer: ptr,
            value,
        })
    }

    /// `$ref` がさらに `$ref` を指していれば、行き着く先まで辿る（循環は `Invalid`）。
    pub fn resolve_chain(&self, from: usize, reference: &str) -> Result<Target<'_>, RefFail> {
        let mut cur_from = from;
        let mut cur_ref = reference.to_string();
        let mut seen: Vec<(usize, String)> = Vec::new();
        loop {
            let t = self.resolve(cur_from, &cur_ref)?;
            let key = (t.file, t.pointer.clone());
            if seen.contains(&key) {
                return Err(RefFail::Invalid(format!("{reference} は循環しています")));
            }
            seen.push(key);
            match t.value.get("$ref").and_then(Value::as_str) {
                Some(next) => {
                    cur_from = t.file;
                    cur_ref = next.to_string();
                }
                None => return Ok(t),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_resolves_relative_to_the_referencing_file() {
        let client = Loc::File(PathBuf::from("/a/api/video/client.yaml"));
        assert_eq!(
            client.join("../common.yaml").unwrap(),
            Loc::File(PathBuf::from("/a/api/common.yaml"))
        );
        assert_eq!(
            client.join("./payee.yaml").unwrap(),
            Loc::File(PathBuf::from("/a/api/video/payee.yaml"))
        );
        let entry = Loc::File(PathBuf::from("/a/api/openapi.yaml"));
        assert_eq!(
            entry.join("./video/client.yaml").unwrap(),
            Loc::File(PathBuf::from("/a/api/video/client.yaml"))
        );
        // 空白などのパーセントエンコード
        assert_eq!(
            entry.join("./my%20dir/x.yaml").unwrap(),
            Loc::File(PathBuf::from("/a/api/my dir/x.yaml"))
        );
    }

    #[test]
    fn join_on_urls() {
        let base = Loc::Url(Url::parse("https://h.example/o/r/main/api/openapi.yaml?x=1").unwrap());
        let got = base.join("../common.yaml").unwrap();
        assert_eq!(got.key(), "https://h.example/o/r/main/common.yaml");
        let other = base.join("https://other.example/x.yaml#/a").unwrap();
        assert_eq!(other.key(), "https://other.example/x.yaml");
    }

    #[test]
    fn display_is_relative_to_the_entry() {
        let entry = Loc::File(PathBuf::from("/a/api/openapi.yaml"));
        assert_eq!(
            Loc::File(PathBuf::from("/a/api/video/client.yaml")).display_from(&entry),
            "video/client.yaml"
        );
        assert_eq!(entry.display_from(&entry), "openapi.yaml");
        assert_eq!(
            Loc::File(PathBuf::from("/a/shared/x.yaml")).display_from(&entry),
            "../shared/x.yaml"
        );
        let e = Loc::Url(Url::parse("https://h.example/o/api/openapi.yaml").unwrap());
        let u = Loc::Url(Url::parse("https://h.example/o/api/video/c.yaml?token=s").unwrap());
        assert_eq!(u.display_from(&e), "video/c.yaml");
    }
}
