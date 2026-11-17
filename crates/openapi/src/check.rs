//! 更新日時の確認（パス方式）。読み直さず、読んだ全ファイルの更新日時だけを見て、変わったかを返す。
//! 起動時とウィンドウに戻ったときに画面が呼ぶ。

use std::path::Path;

use nekoreq_core::SourceFile;

use crate::doc::normalize_path;
use crate::load::file_modified_at;
use crate::model::SourceChanged;

/// `entry` は入口ファイルの絶対パス。`files` は前回の取り込みで覚えたファイル（入口からの相対パス + 更新日時）。
pub fn source_changed(entry: &Path, files: &[SourceFile]) -> SourceChanged {
    let dir = entry.parent().unwrap_or(Path::new(""));
    let mut changed_files = Vec::new();
    for f in files {
        let Some(recorded) = &f.modified_at else {
            continue;
        };
        let path = normalize_path(&dir.join(&f.path));
        match file_modified_at(&path) {
            Some(now) if &now == recorded => {}
            _ => changed_files.push(f.path.clone()),
        }
    }
    SourceChanged {
        changed: !changed_files.is_empty(),
        changed_files,
    }
}
