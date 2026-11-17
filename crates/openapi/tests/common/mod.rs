#![allow(dead_code)]
//! テスト共通の道具。

use std::path::{Path, PathBuf};

use nekoreq_core::Operation;
use nekoreq_openapi::{load, LoadEntry, LoadParams, LoadProblem, LoadResult, Loaded};

pub fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

pub async fn load_path(p: &Path) -> LoadResult {
    load(&LoadParams {
        entry: LoadEntry::Path(p.to_path_buf()),
        token: None,
        allowed_hosts: vec![],
    })
    .await
}

pub async fn loaded(p: &Path) -> Loaded {
    match load_path(p).await {
        LoadResult::Loaded(l) => *l,
        LoadResult::Blocked(problems) => panic!("読めるはず: {problems:#?}"),
        LoadResult::NeedsConfirmation(f) => panic!("確認待ちのはず: {f:#?}"),
    }
}

pub async fn blocked(p: &Path) -> Vec<LoadProblem> {
    match load_path(p).await {
        LoadResult::Blocked(problems) => problems,
        LoadResult::Loaded(_) => panic!("止まるはず"),
        LoadResult::NeedsConfirmation(_) => panic!("止まるはず（確認待ちではなく）"),
    }
}

pub fn op<'a>(l: &'a Loaded, key: &str) -> &'a Operation {
    l.operations
        .iter()
        .find(|o| o.op_key.to_string() == key)
        .unwrap_or_else(|| {
            panic!(
                "{key} がない: {:?}",
                l.operations
                    .iter()
                    .map(|o| o.op_key.to_string())
                    .collect::<Vec<_>>()
            )
        })
}

/// 実際のファイルの `line`（1 始まり）の文字列。
pub fn line_text(file: &Path, line: u32) -> String {
    std::fs::read_to_string(file)
        .unwrap()
        .lines()
        .nth(line as usize - 1)
        .unwrap_or_default()
        .to_string()
}

/// `dir` を `dest` に丸ごとコピーする（テスト用）。
pub fn copy_dir(dir: &Path, dest: &Path) {
    std::fs::create_dir_all(dest).unwrap();
    for e in std::fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        let to = dest.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &to);
        } else {
            std::fs::copy(e.path(), to).unwrap();
        }
    }
}
