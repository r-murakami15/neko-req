//! 入口ファイルから `$ref` をたどって、参照先のファイルをすべて読む（パス・URL）。
//!
//! - ファイルは絶対パス（URL）で 1 回だけ読む。相対パスは `$ref` を書いたファイルが基準。
//! - 足りない・読めないファイルは、足りないファイル名と参照元の `file:line` の一覧にして止める。
//! - 壊れた yaml は、ファイル・行・理由を返して止める。
//! - URL の入口で別の origin（スキーム・ホスト・ポート）を指す参照は、読む前に確認を求める（読まずに
//!   `NeedsConfirmation` で返す）。確認済みの origin は `allowed_hosts` に `https://host[:port]` の形で持つ。
//!   アクセストークンは入口と同じ origin にだけ送る。
//! - 入口の URL がリダイレクトされたら、相対 `$ref` はリダイレクト後の URL を基準に解決する。リダイレクト先の
//!   origin の参照は、確認なしで読む（トークンは送らない）。
//! - `root` があるときは（アップロードのコピー方式）、その外のファイルは読まない（足りないファイル扱い）。

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

use crate::doc::{split_ref, strip_secrets, DocSet, FileDoc, Loc};
use crate::model::{ForeignRef, LoadProblem, RefSite};
use crate::pointer;
use crate::pos;

/// 読み込みの入口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadEntry {
    /// ローカルのファイル（アップロードで置いたコピーも同じ）。
    Path(PathBuf),
    Url(String),
}

#[derive(Clone)]
pub struct LoadParams {
    pub entry: LoadEntry,
    /// URL 方式のアクセストークン（入口と同じ origin にだけ送る）。
    pub token: Option<String>,
    /// 確認済みの別の origin（`https://host` `http://host:8080` の形。`ForeignRef.host` と同じ）。
    pub allowed_hosts: Vec<String>,
    /// 基準フォルダ。あれば、解決した `$ref` がこの外のファイルなら読まない（アップロードのコピー方式。
    /// 一時フォルダ・`sources/<ID>`）。`None`（パス方式）は制限なし。
    pub root: Option<PathBuf>,
}

impl std::fmt::Debug for LoadParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadParams")
            .field("entry", &self.entry)
            .field("token", &self.token.as_ref().map(|_| "***"))
            .field("allowed_hosts", &self.allowed_hosts)
            .field("root", &self.root)
            .finish()
    }
}

pub enum Crawl {
    Done(DocSet),
    NeedsConfirmation(Vec<ForeignRef>),
    Blocked(Vec<LoadProblem>),
}

/// 1 ファイルの上限と、読むファイル数の上限（暴走防止）。
const MAX_FILE_BYTES: usize = 32 * 1024 * 1024;
const MAX_FILES: usize = 2000;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// 値の名前（キー）が並ぶマップ。キーが `example` などでもキーワードとして扱わない。
const NAME_MAPS: &[&str] = &[
    "properties",
    "patternProperties",
    "$defs",
    "definitions",
    "schemas",
    "parameters",
    "responses",
    "requestBodies",
    "headers",
    "examples",
    "links",
    "callbacks",
    "pathItems",
    "content",
    "paths",
    "encoding",
    "variables",
    "mapping",
    "dependentSchemas",
    "securitySchemes",
    "webhooks",
];

/// 値そのもの（スキーマではないデータ）を持つキーワード。中の `$ref` は参照ではない。
const DATA_KEYS: &[&str] = &["example", "default", "enum", "const"];

/// 文書の中の `$ref`（`$ref` を持つノードのポインターと、参照の文字列）をすべて集める。
pub fn collect_refs(value: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut ptr = String::new();
    scan(value, &mut ptr, false, &mut out);
    out
}

fn scan(v: &Value, ptr: &mut String, in_name_map: bool, out: &mut Vec<(String, String)>) {
    match v {
        Value::Object(m) => {
            // 名前のマップの中では、キーはただの名前（`$ref` や `example` という名前の項目もありえる）
            if !in_name_map {
                if let Some(Value::String(r)) = m.get("$ref") {
                    out.push((ptr.clone(), r.clone()));
                }
            }
            for (k, child) in m {
                if !in_name_map {
                    // 値そのもの（スキーマではないデータ）の中は見ない。`examples` は配列なら値、
                    // オブジェクトなら Example Object の名前のマップ
                    let is_data = k == "$ref"
                        || DATA_KEYS.contains(&k.as_str())
                        || (k == "examples" && child.is_array());
                    if is_data {
                        continue;
                    }
                }
                let child_is_name_map = !in_name_map && NAME_MAPS.contains(&k.as_str());
                let len = ptr.len();
                ptr.push('/');
                ptr.push_str(&pointer::escape(k));
                scan(child, ptr, child_is_name_map, out);
                ptr.truncate(len);
            }
        }
        Value::Array(a) => {
            for (i, child) in a.iter().enumerate() {
                let len = ptr.len();
                ptr.push('/');
                ptr.push_str(&i.to_string());
                scan(child, ptr, false, out);
                ptr.truncate(len);
            }
        }
        _ => {}
    }
}

/// origin の表記（`https://host` `http://host:8080`。スキーム・ホスト・ポートで 1 つ。小文字）。
/// 既定のポート（http 80・https 443）は省く。
pub fn origin_key(u: &Url) -> String {
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();
    match u.port() {
        Some(p) => format!("{}://{host}:{p}", u.scheme()),
        None => format!("{}://{host}", u.scheme()),
    }
}

/// 確認済みの一覧に `u` の origin があるか。スキームのない古い表記（`host` `host:port`）は https だけに効く。
fn origin_allowed(allowed: &[String], u: &Url) -> bool {
    let key = origin_key(u);
    allowed.iter().any(|h| {
        let h = h.trim();
        if h.contains("://") {
            h.eq_ignore_ascii_case(&key)
        } else {
            u.scheme() == "https" && format!("https://{h}").eq_ignore_ascii_case(&key)
        }
    })
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

fn modified_string(path: &Path) -> Option<String> {
    let m = std::fs::metadata(path).ok()?.modified().ok()?;
    let dt: chrono::DateTime<chrono::Utc> = m.into();
    Some(dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

/// 更新日時の文字列（`SourceFile.modified_at` と同じ形式）。
pub fn file_modified_at(path: &Path) -> Option<String> {
    modified_string(path)
}

fn sha_hex(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// `openapi:` の版を確かめる。
pub fn check_openapi_version(file: &str, root: &Value) -> Result<String, LoadProblem> {
    let obj = root.as_object();
    if let Some(sw) = obj.and_then(|o| o.get("swagger")) {
        return Err(LoadProblem::UnsupportedVersion {
            file: file.into(),
            version: format!("Swagger {}", value_text(sw)),
        });
    }
    let Some(v) = obj.and_then(|o| o.get("openapi")) else {
        return Err(LoadProblem::NotOpenApi {
            file: file.into(),
            reason: "OpenAPI の文書ではありません（`openapi:` がありません）".into(),
        });
    };
    let version = value_text(v);
    let supported = ["3.0", "3.1", "3.2"]
        .iter()
        .any(|p| version == *p || version.starts_with(&format!("{p}.")));
    if supported {
        Ok(version)
    } else {
        Err(LoadProblem::UnsupportedVersion {
            file: file.into(),
            version: format!("OpenAPI {version}"),
        })
    }
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

struct Crawler<'a> {
    params: &'a LoadParams,
    entry: Loc,
    /// 入口を URL で読んだときの、リダイレクトのあとの URL。
    entry_final: Option<Url>,
    /// 解決したパスの基準フォルダ（`LoadParams::root` を正規化したもの）。
    root: Option<PathBuf>,
    docs: DocSet,
    queue: VecDeque<Loc>,
    queued: HashSet<String>,
    /// ファイルのキー → 参照元（足りなかったときの報告用）
    sites: HashMap<String, Vec<RefSite>>,
    problems: Vec<LoadProblem>,
    foreign: Vec<ForeignRef>,
    foreign_seen: HashSet<String>,
}

impl Crawler<'_> {
    fn display(&self, loc: &Loc) -> String {
        loc.display_from(&self.entry)
    }

    fn token_for(&self, loc: &Loc) -> Option<&str> {
        let (Loc::Url(u), Loc::Url(e)) = (loc, &self.entry) else {
            return None;
        };
        if same_origin(u, e) {
            self.params.token.as_deref()
        } else {
            None
        }
    }

    async fn read(&self, loc: &Loc) -> Result<(Vec<u8>, Option<String>, Option<Url>), String> {
        match loc {
            Loc::File(p) => {
                let bytes = std::fs::read(p).map_err(|e| match e.kind() {
                    std::io::ErrorKind::NotFound => "ファイルがありません".to_string(),
                    _ => format!("読めません: {e}"),
                })?;
                if bytes.len() > MAX_FILE_BYTES {
                    return Err("ファイルが大きすぎます".into());
                }
                Ok((bytes, modified_string(p), None))
            }
            Loc::Url(u) => {
                let (bytes, final_url) = nekoreq_http::fetch_bytes_at(
                    u,
                    self.token_for(loc),
                    FETCH_TIMEOUT,
                    MAX_FILE_BYTES,
                )
                .await
                .map_err(|e| e.message)?;
                Ok((bytes, None, Some(final_url)))
            }
        }
    }

    fn inside_root(&self, p: &Path) -> bool {
        match &self.root {
            Some(root) => p.starts_with(root),
            None => true,
        }
    }

    /// 入口と同じ origin か（入口のリダイレクト先を含む）。
    fn is_entry_origin(&self, u: &Url) -> bool {
        match &self.entry {
            Loc::Url(e) => {
                same_origin(u, e) || self.entry_final.as_ref().is_some_and(|f| same_origin(u, f))
            }
            Loc::File(_) => false,
        }
    }

    fn add_site(&mut self, key: &str, site: RefSite) {
        let v = self.sites.entry(key.to_string()).or_default();
        if !v.contains(&site) {
            v.push(site);
        }
    }

    /// `loc` を読んで文書に加え、中の `$ref` から次に読むファイルを集める。
    async fn load_one(&mut self, loc: Loc, is_entry: bool) {
        let display = self.display(&loc);
        let (bytes, modified_at, final_url) = match self.read(&loc).await {
            Ok(x) => x,
            Err(reason) => {
                let problem = if is_entry {
                    LoadProblem::Unreadable {
                        file: display,
                        reason,
                    }
                } else {
                    LoadProblem::MissingFile {
                        file: display,
                        reason,
                        referenced_from: self.sites.get(&loc.key()).cloned().unwrap_or_default(),
                    }
                };
                self.problems.push(problem);
                return;
            }
        };
        let text = match String::from_utf8(bytes) {
            Ok(t) => t,
            Err(_) => {
                self.problems.push(LoadProblem::YamlError {
                    file: display,
                    line: None,
                    column: None,
                    reason: "UTF-8 として読めません".into(),
                });
                return;
            }
        };
        let parsed = match pos::parse(&text) {
            Ok(p) => p,
            Err(e) => {
                self.problems.push(LoadProblem::YamlError {
                    file: display,
                    line: e.line,
                    column: e.column,
                    reason: e.reason,
                });
                return;
            }
        };
        if is_entry {
            if let Err(p) = check_openapi_version(&display, &parsed.value) {
                self.problems.push(p);
                return;
            }
        }
        let refs = collect_refs(&parsed.value);
        let doc = FileDoc {
            loc: loc.clone(),
            display: display.clone(),
            value: parsed.value,
            lines: parsed.lines,
            modified_at,
            hash: sha_hex(text.as_bytes()),
            bytes: text.len(),
            base_url: final_url.clone(),
        };
        if is_entry {
            self.entry_final = final_url.map(|mut u| {
                u.set_fragment(None);
                u
            });
        }
        let idx = self.docs.push(doc);
        for (node_ptr, reference) in refs {
            let (file_part, _) = split_ref(&reference);
            if file_part.is_empty() {
                continue;
            }
            let line = self.docs.files[idx]
                .lines
                .get(&format!("{node_ptr}/$ref"))
                .copied();
            let site = RefSite {
                file: display.clone(),
                line,
            };
            match self.docs.files[idx].join(file_part) {
                Err(reason) => self.problems.push(LoadProblem::MissingFile {
                    file: file_part.to_string(),
                    reason,
                    referenced_from: vec![site],
                }),
                Ok(Loc::File(p)) if !self.inside_root(&p) => {
                    // 基準フォルダの外は読まない。表示は `$ref` に書かれた文字列（実際のパスは出さない）
                    self.problems.push(LoadProblem::MissingFile {
                        file: file_part.to_string(),
                        reason: "取り込んだフォルダの外のファイルは読めません".into(),
                        referenced_from: vec![site],
                    });
                }
                Ok(target) => self.enqueue(target, site),
            }
        }
    }

    fn enqueue(&mut self, target: Loc, site: RefSite) {
        let key = target.key();
        if let Loc::Url(u) = &target {
            let allowed = self.is_entry_origin(u) || origin_allowed(&self.params.allowed_hosts, u);
            if !allowed {
                if self.foreign_seen.insert(key) {
                    self.foreign.push(ForeignRef {
                        host: origin_key(u),
                        url: strip_secrets(u),
                        referenced_from: site,
                    });
                }
                return;
            }
        }
        if let (Loc::File(_), Loc::Url(_)) = (&target, &self.entry) {
            self.problems.push(LoadProblem::MissingFile {
                file: target.key(),
                reason: "URL の入口からローカルのファイルは読めません".into(),
                referenced_from: vec![site],
            });
            return;
        }
        self.add_site(&key, site);
        if self.queued.insert(key) {
            self.queue.push_back(target);
        }
    }
}

/// URL の入口にユーザー名・パスワード（`user:pass@`）があったときの案内。
pub const USERINFO_MESSAGE: &str =
    "URL にユーザー名・パスワード（user:pass@）は入れられません。アクセストークンはトークン欄に入力してください";

/// 入口の URL に `user:pass@` があればエラーの文（取り込み・同期の入口で先に弾く）。
pub fn check_entry_url(s: &str) -> Result<(), String> {
    match Url::parse(s.trim()) {
        Ok(u) if !u.username().is_empty() || u.password().is_some() => {
            Err(USERINFO_MESSAGE.to_string())
        }
        _ => Ok(()),
    }
}

fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    }
}

/// 入口から参照先まですべて読む。
pub async fn crawl(params: &LoadParams) -> Crawl {
    let entry = match &params.entry {
        LoadEntry::Path(p) => Loc::File(crate::doc::normalize_path(&absolute(p))),
        LoadEntry::Url(s) => match Url::parse(s) {
            Ok(u) if !u.username().is_empty() || u.password().is_some() => {
                return Crawl::Blocked(vec![LoadProblem::Unreadable {
                    file: strip_secrets(&u),
                    reason: USERINFO_MESSAGE.into(),
                }])
            }
            Ok(mut u) if u.scheme() == "http" || u.scheme() == "https" => {
                u.set_fragment(None);
                Loc::Url(u)
            }
            _ => {
                return Crawl::Blocked(vec![LoadProblem::Unreadable {
                    file: s.clone(),
                    reason: "http:// か https:// で始まる URL を指定してください".into(),
                }])
            }
        },
    };
    let mut c = Crawler {
        params,
        entry: entry.clone(),
        entry_final: None,
        root: params
            .root
            .as_ref()
            .map(|r| crate::doc::normalize_path(&absolute(r))),
        docs: DocSet::default(),
        queue: VecDeque::new(),
        queued: HashSet::new(),
        sites: HashMap::new(),
        problems: Vec::new(),
        foreign: Vec::new(),
        foreign_seen: HashSet::new(),
    };
    c.queued.insert(entry.key());
    c.load_one(entry, true).await;
    // 入口が読めなければ、そこで止める
    if c.docs.files.is_empty() {
        return Crawl::Blocked(c.problems);
    }
    while let Some(loc) = c.queue.pop_front() {
        if c.docs.files.len() >= MAX_FILES {
            c.problems.push(LoadProblem::Unreadable {
                file: c.display(&loc),
                reason: format!("読むファイルが {MAX_FILES} 個を超えました"),
            });
            break;
        }
        c.load_one(loc, false).await;
    }
    if !c.problems.is_empty() {
        merge_missing(&mut c.problems);
        return Crawl::Blocked(c.problems);
    }
    if !c.foreign.is_empty() {
        return Crawl::NeedsConfirmation(c.foreign);
    }
    Crawl::Done(c.docs)
}

/// 同じファイルの `MissingFile` を 1 つにまとめる（参照元は足し合わせる）。
fn merge_missing(problems: &mut Vec<LoadProblem>) {
    let mut out: Vec<LoadProblem> = Vec::new();
    for p in problems.drain(..) {
        if let LoadProblem::MissingFile {
            file,
            referenced_from,
            ..
        } = &p
        {
            if let Some(LoadProblem::MissingFile {
                referenced_from: existing,
                ..
            }) = out
                .iter_mut()
                .find(|q| matches!(q, LoadProblem::MissingFile { file: f, .. } if f == file))
            {
                for s in referenced_from {
                    if !existing.contains(s) {
                        existing.push(s.clone());
                    }
                }
                continue;
            }
        }
        out.push(p);
    }
    *problems = out;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collect_refs_skips_example_data_but_not_properties_named_example() {
        let v = json!({
            "a": {"$ref": "./x.yaml#/A"},
            "schema": {
                "example": {"$ref": "fake"},
                "properties": {
                    "example": {"$ref": "#/components/schemas/B"},
                    "default": {"$ref": "./y.yaml#/C"}
                }
            },
            "examples": ["data", {"$ref": "not-a-ref"}],
            "content": {"application/json": {"examples": {"one": {"$ref": "#/components/examples/E"}}}}
        });
        let mut refs: Vec<_> = collect_refs(&v).into_iter().map(|(_, r)| r).collect();
        refs.sort();
        assert_eq!(
            refs,
            vec![
                "#/components/examples/E",
                "#/components/schemas/B",
                "./x.yaml#/A",
                "./y.yaml#/C"
            ]
        );
    }

    #[test]
    fn version_check() {
        assert!(check_openapi_version("f", &json!({"openapi": "3.0.3"})).is_ok());
        assert!(check_openapi_version("f", &json!({"openapi": "3.1.0"})).is_ok());
        assert!(check_openapi_version("f", &json!({"openapi": "3.2.0"})).is_ok());
        assert!(matches!(
            check_openapi_version("f", &json!({"swagger": "2.0"})),
            Err(LoadProblem::UnsupportedVersion { .. })
        ));
        assert!(matches!(
            check_openapi_version("f", &json!({"openapi": "4.0.0"})),
            Err(LoadProblem::UnsupportedVersion { .. })
        ));
        assert!(matches!(
            check_openapi_version("f", &json!({"title": "x"})),
            Err(LoadProblem::NotOpenApi { .. })
        ));
    }
}
