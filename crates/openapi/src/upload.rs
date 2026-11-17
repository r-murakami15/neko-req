//! アップロード（コピー方式）。選んだフォルダ・複数ファイル・zip を、元の構成のまま `dest` に置く。
//!
//! - フォルダ 1 つだけ選んだときは、その中身を `dest` 直下に置く。複数選んだときは、全体の共通の親を基準にして
//!   相対パスを保つ（`$ref` の相対パスが壊れないように）。
//! - 置くのは yaml / yml / json だけ（`node_modules`・`.git` などは除く）。
//! - zip は `..` や絶対パスを含むエントリを拒否し、展開サイズ・ファイル数に上限を置く。
//! - 入口の候補は、トップレベルに `openapi:` を持つファイル。

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use crate::doc::{normalize_path, relative_path};

/// 置くファイルの数・大きさの上限。
pub const MAX_UPLOAD_FILES: usize = 5000;
pub const MAX_UPLOAD_BYTES: u64 = 256 * 1024 * 1024;

const SKIP_DIRS: &[&str] = &["node_modules", ".git", "target", ".svn", ".hg"];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UploadError {
    #[error("{0}")]
    Invalid(String),
    #[error("ファイルを置けませんでした: {0}")]
    Io(String),
}

impl From<std::io::Error> for UploadError {
    fn from(e: std::io::Error) -> Self {
        UploadError::Io(e.to_string())
    }
}

fn is_spec_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("yaml" | "yml" | "json")
    )
}

fn is_zip(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
}

/// `~/` を展開する。
pub fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(p)
}

struct Counter {
    files: usize,
    bytes: u64,
}

impl Counter {
    fn add(&mut self, bytes: u64) -> Result<(), UploadError> {
        self.files += 1;
        self.bytes += bytes;
        if self.files > MAX_UPLOAD_FILES {
            return Err(UploadError::Invalid(format!(
                "ファイルが多すぎます（上限 {MAX_UPLOAD_FILES} 個）。入口のあるフォルダを選んでください"
            )));
        }
        if self.bytes > MAX_UPLOAD_BYTES {
            return Err(UploadError::Invalid(format!(
                "合計が大きすぎます（上限 {} MB）",
                MAX_UPLOAD_BYTES / (1024 * 1024)
            )));
        }
        Ok(())
    }
}

/// 全部の親ディレクトリのうち共通の部分。
fn common_ancestor(paths: &[PathBuf]) -> PathBuf {
    let dirs: Vec<PathBuf> = paths
        .iter()
        .map(|p| {
            if p.is_dir() {
                p.clone()
            } else {
                p.parent().map(Path::to_path_buf).unwrap_or_default()
            }
        })
        .collect();
    let mut common: Vec<Component> = dirs[0].components().collect();
    for d in &dirs[1..] {
        let comps: Vec<Component> = d.components().collect();
        let n = common
            .iter()
            .zip(comps.iter())
            .take_while(|(a, b)| a == b)
            .count();
        common.truncate(n);
    }
    common.iter().collect()
}

fn copy_tree(
    dir: &Path,
    base: &Path,
    dest: &Path,
    counter: &mut Counter,
    copied: &mut Vec<String>,
) -> Result<(), UploadError> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let ty = e.file_type()?;
        if ty.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) || name.starts_with('.') && name != "." {
                continue;
            }
            copy_tree(&path, base, dest, counter, copied)?;
        } else if ty.is_file() && is_spec_file(&path) {
            copy_file(&path, base, dest, counter, copied)?;
        }
    }
    Ok(())
}

fn copy_file(
    src: &Path,
    base: &Path,
    dest: &Path,
    counter: &mut Counter,
    copied: &mut Vec<String>,
) -> Result<(), UploadError> {
    let rel = relative_path(base, src);
    if rel.starts_with("..") {
        return Err(UploadError::Invalid(format!(
            "基準の外のファイルです: {}",
            src.display()
        )));
    }
    let size = fs::metadata(src)?.len();
    counter.add(size)?;
    let target = dest.join(&rel);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(src, &target)?;
    copied.push(rel);
    Ok(())
}

fn extract_zip(
    zip_path: &Path,
    dest: &Path,
    counter: &mut Counter,
    copied: &mut Vec<String>,
) -> Result<(), UploadError> {
    let file = fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| UploadError::Invalid(format!("zip を開けません: {e}")))?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| UploadError::Invalid(format!("zip を読めません: {e}")))?;
        if entry.is_dir() {
            continue;
        }
        // 「..」や絶対パスのエントリは拒否する（zip slip）
        let Some(rel) = entry.enclosed_name() else {
            return Err(UploadError::Invalid(format!(
                "zip に安全でないパスのファイルがあります: {}",
                entry.name()
            )));
        };
        if rel
            .components()
            .any(|c| matches!(c, Component::Normal(n) if SKIP_DIRS.contains(&n.to_string_lossy().as_ref())))
            || rel.components().any(|c| matches!(c, Component::Normal(n) if n.to_string_lossy().starts_with("__MACOSX")))
            || !is_spec_file(&rel)
        {
            continue;
        }
        if entry.size() > MAX_UPLOAD_BYTES {
            return Err(UploadError::Invalid(
                "zip の中のファイルが大きすぎます".into(),
            ));
        }
        counter.add(entry.size())?;
        let target = dest.join(&rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        // 宣言サイズを信用せず、読む量にも上限を置く
        let mut buf = Vec::new();
        entry
            .by_ref()
            .take(MAX_UPLOAD_BYTES + 1)
            .read_to_end(&mut buf)?;
        if buf.len() as u64 > MAX_UPLOAD_BYTES {
            return Err(UploadError::Invalid(
                "zip の中のファイルが大きすぎます".into(),
            ));
        }
        fs::write(&target, &buf)?;
        copied.push(rel.to_string_lossy().replace('\\', "/"));
    }
    Ok(())
}

/// 選んだものを `dest` に置く。置いたファイルの相対パス（`/` 区切り）を返す。
pub fn stage(selected: &[PathBuf], dest: &Path) -> Result<Vec<String>, UploadError> {
    if selected.is_empty() {
        return Err(UploadError::Invalid("ファイルを選んでください".into()));
    }
    for p in selected {
        if !p.exists() {
            return Err(UploadError::Invalid(format!(
                "見つかりません: {}",
                p.display()
            )));
        }
    }
    fs::create_dir_all(dest)?;
    let mut counter = Counter { files: 0, bytes: 0 };
    let mut copied = Vec::new();

    let non_zip: Vec<PathBuf> = selected.iter().filter(|p| !is_zip(p)).cloned().collect();
    for z in selected.iter().filter(|p| is_zip(p) && p.is_file()) {
        extract_zip(z, dest, &mut counter, &mut copied)?;
    }
    if !non_zip.is_empty() {
        let base = if non_zip.len() == 1 && non_zip[0].is_dir() {
            non_zip[0].clone()
        } else {
            common_ancestor(&non_zip)
        };
        for p in &non_zip {
            if p.is_dir() {
                copy_tree(p, &base, dest, &mut counter, &mut copied)?;
            } else if is_spec_file(p) {
                copy_file(p, &base, dest, &mut counter, &mut copied)?;
            } else {
                return Err(UploadError::Invalid(format!(
                    "yaml / yml / json / zip 以外は取り込めません: {}",
                    p.display()
                )));
            }
        }
    }
    copied.sort();
    copied.dedup();
    if copied.is_empty() {
        return Err(UploadError::Invalid(
            "yaml / yml / json のファイルが見つかりませんでした".into(),
        ));
    }
    Ok(copied)
}

/// トップレベルに `openapi:` を持つか（ファイル全体は解析せず、行を見る）。
fn looks_like_entry(path: &Path) -> bool {
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    let text = String::from_utf8_lossy(&bytes);
    let is_json = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("json"));
    if is_json {
        // 先頭のほうのトップレベルのキー
        return text
            .chars()
            .take(4096)
            .collect::<String>()
            .contains("\"openapi\"");
    }
    text.lines().any(|l| {
        let t = l.trim_end();
        t.strip_prefix("openapi:").is_some()
            || t.strip_prefix("\"openapi\":").is_some()
            || t.strip_prefix("'openapi':").is_some()
    })
}

/// `root`（置いた先）の中で、入口の候補（`openapi:` を持つファイル）を相対パスで返す。浅い順・名前順。
pub fn entry_candidates(root: &Path, files: &[String]) -> Vec<String> {
    let mut v: Vec<String> = files
        .iter()
        .filter(|f| looks_like_entry(&normalize_path(&root.join(f))))
        .cloned()
        .collect();
    v.sort_by(|a, b| {
        let da = a.matches('/').count();
        let db = b.matches('/').count();
        da.cmp(&db).then_with(|| a.cmp(b))
    });
    v
}

/// ローカルのフォルダの中から入口の候補を探す（パス方式でフォルダを指定したとき）。
pub fn scan_dir_for_entries(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 3 {
            return;
        }
        let Ok(rd) = fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(ty) = e.file_type() else { continue };
            if ty.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                    walk(&p, depth + 1, out);
                }
            } else if ty.is_file() && is_spec_file(&p) && looks_like_entry(&p) {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, 0, &mut out);
    out.sort_by(|a, b| {
        a.components()
            .count()
            .cmp(&b.components().count())
            .then_with(|| a.cmp(b))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    #[test]
    fn single_folder_keeps_structure_and_skips_other_files() {
        let src = tempfile::tempdir().unwrap();
        write(src.path(), "openapi.yaml", "openapi: 3.0.3\n");
        write(src.path(), "video/c.yaml", "paths: {}\n");
        write(src.path(), "README.md", "# x");
        write(src.path(), "node_modules/x/y.yaml", "a: 1");
        let dest = tempfile::tempdir().unwrap();
        let files = stage(&[src.path().to_path_buf()], dest.path()).unwrap();
        assert_eq!(files, vec!["openapi.yaml", "video/c.yaml"]);
        assert!(dest.path().join("video/c.yaml").exists());
        assert!(!dest.path().join("README.md").exists());
        assert_eq!(
            entry_candidates(dest.path(), &files),
            vec!["openapi.yaml".to_string()]
        );
    }

    #[test]
    fn multiple_files_keep_relative_paths_from_the_common_parent() {
        let src = tempfile::tempdir().unwrap();
        write(src.path(), "api/openapi.yaml", "openapi: 3.0.3\n");
        write(src.path(), "api/video/c.yaml", "paths: {}\n");
        let dest = tempfile::tempdir().unwrap();
        let files = stage(
            &[
                src.path().join("api/openapi.yaml"),
                src.path().join("api/video/c.yaml"),
            ],
            dest.path(),
        )
        .unwrap();
        assert_eq!(files, vec!["openapi.yaml", "video/c.yaml"]);
    }

    #[test]
    fn zip_is_extracted_and_zip_slip_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("api.zip");
        {
            let f = fs::File::create(&zip_path).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            z.start_file("api/openapi.yaml", opts).unwrap();
            z.write_all(b"openapi: 3.1.0\n").unwrap();
            z.start_file("api/video/c.yaml", opts).unwrap();
            z.write_all(b"paths: {}\n").unwrap();
            z.finish().unwrap();
        }
        let dest = tempfile::tempdir().unwrap();
        let files = stage(&[zip_path], dest.path()).unwrap();
        assert_eq!(files, vec!["api/openapi.yaml", "api/video/c.yaml"]);

        let evil = tmp.path().join("evil.zip");
        {
            let f = fs::File::create(&evil).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            z.start_file("../escape.yaml", opts).unwrap();
            z.write_all(b"a: 1\n").unwrap();
            z.finish().unwrap();
        }
        let dest2 = tempfile::tempdir().unwrap();
        let err = stage(&[evil], dest2.path()).unwrap_err();
        assert!(matches!(err, UploadError::Invalid(_)), "{err:?}");
        assert!(!dest2.path().parent().unwrap().join("escape.yaml").exists());
    }

    #[test]
    fn several_entry_candidates_are_listed() {
        let src = tempfile::tempdir().unwrap();
        write(src.path(), "openapi.yaml", "openapi: 3.0.3\n");
        write(src.path(), "openapi.bundled.yaml", "openapi: 3.0.3\n");
        write(src.path(), "video/c.yaml", "paths: {}\n");
        let dest = tempfile::tempdir().unwrap();
        let files = stage(&[src.path().to_path_buf()], dest.path()).unwrap();
        assert_eq!(
            entry_candidates(dest.path(), &files),
            vec![
                "openapi.bundled.yaml".to_string(),
                "openapi.yaml".to_string()
            ]
        );
    }

    #[test]
    fn nothing_to_copy_is_an_error() {
        let src = tempfile::tempdir().unwrap();
        write(src.path(), "README.md", "x");
        let dest = tempfile::tempdir().unwrap();
        assert!(stage(&[src.path().to_path_buf()], dest.path()).is_err());
    }
}
