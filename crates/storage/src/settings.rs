//! アプリ全体の設定（`app_settings`）。キー `app` に `AppSettings` の JSON を 1 件だけ持つ。

use nekoreq_core::AppSettings;
use rusqlite::{params, OptionalExtension};
use serde_json::Value;

use crate::error::{Result, StorageError};
use crate::Database;

const APP_KEY: &str = "app";

impl Database {
    /// アプリの設定を返す。まだ保存していない・読めないときは既定値。
    pub fn app_settings_get(&self) -> Result<AppSettings> {
        let json: Option<String> = self
            .conn()
            .query_row(
                "SELECT value FROM app_settings WHERE key = ?1",
                [APP_KEY],
                |row| row.get(0),
            )
            .optional()?;
        Ok(json.map(|j| lenient_settings(&j)).unwrap_or_default())
    }

    /// アプリの設定を保存する。
    pub fn app_settings_set(&self, settings: &AppSettings) -> Result<()> {
        let json = serde_json::to_string(settings)
            .map_err(|e| StorageError::Corrupt(format!("app_settings: {e}")))?;
        self.conn().execute(
            "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![APP_KEY, json],
        )?;
        Ok(())
    }
}

/// 保存された JSON から設定を作る。全体として読めればそのまま。一部の項目が読めない（型が違う・選択肢にない値）ときは、
/// その項目だけ既定値に戻し、ほかの正しい項目（プロキシ・証明書の設定など）は残す。JSON として読めなければ既定値。
fn lenient_settings(json: &str) -> AppSettings {
    if let Ok(settings) = serde_json::from_str::<AppSettings>(json) {
        return settings;
    }
    let Ok(stored) = serde_json::from_str::<Value>(json) else {
        return AppSettings::default();
    };
    let Ok(mut current) = serde_json::to_value(AppSettings::default()) else {
        return AppSettings::default();
    };
    merge_valid(&mut current, &mut Vec::new(), &stored);
    serde_json::from_value(current).unwrap_or_default()
}

/// `stored` の項目を、読める（`AppSettings` として成り立つ）ものだけ `root` に入れる。`path` は今いる場所。
fn merge_valid(root: &mut Value, path: &mut Vec<String>, stored: &Value) {
    let pointer = |path: &[String]| -> String {
        path.iter()
            .map(|k| format!("/{}", k.replace('~', "~0").replace('/', "~1")))
            .collect()
    };
    let Some(object) = stored.as_object() else {
        return;
    };
    for (key, value) in object {
        path.push(key.clone());
        let here = pointer(path);
        let nested = value.is_object() && root.pointer(&here).is_some_and(Value::is_object);
        if nested {
            merge_valid(root, path, value);
        } else {
            let parent = pointer(&path[..path.len() - 1]);
            let candidates: Vec<Value> = match value {
                Value::Array(items) => {
                    // 配列は、全体が読めなければ、読める要素だけ残す
                    let mut all = vec![value.clone()];
                    let each: Vec<Value> = items
                        .iter()
                        .filter(|item| {
                            try_set(root, &parent, key, &Value::Array(vec![(*item).clone()]))
                                .is_some()
                        })
                        .cloned()
                        .collect();
                    all.push(Value::Array(each));
                    all
                }
                other => vec![other.clone()],
            };
            for candidate in candidates {
                if let Some(next) = try_set(root, &parent, key, &candidate) {
                    *root = next;
                    break;
                }
            }
        }
        path.pop();
    }
}

/// `parent`（JSON ポインター）のオブジェクトの `key` を `value` にした全体が `AppSettings` として読めれば、その全体を返す。
fn try_set(root: &Value, parent: &str, key: &str, value: &Value) -> Option<Value> {
    let mut next = root.clone();
    next.pointer_mut(parent)?
        .as_object_mut()?
        .insert(key.to_string(), value.clone());
    serde_json::from_value::<AppSettings>(next.clone())
        .ok()
        .map(|_| next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nekoreq_core::ProxyMode;

    fn with_stored(json: &str) -> AppSettings {
        let db = Database::open_in_memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO app_settings (key, value) VALUES ('app', ?1)",
                [json],
            )
            .unwrap();
        db.app_settings_get().unwrap()
    }

    #[test]
    fn a_bad_field_resets_only_that_field() {
        let s = with_stored(
            r#"{"request":{"timeoutSecs":-5,"followRedirects":false},
                "network":{"proxy":{"mode":"manual","url":"http://p:3128","noProxy":["a","b"]},
                           "clientCerts":[{"id":"ok","hostPattern":"h","path":"/x"},{"id":5}]},
                "history":{"limit":"many","omitLargeBodies":false},
                "appearance":{"theme":"neon"},
                "unknownKey":1}"#,
        );
        assert_eq!(s.network.proxy.mode, ProxyMode::Manual);
        assert_eq!(s.network.proxy.url, "http://p:3128");
        assert_eq!(s.network.proxy.no_proxy, ["a", "b"]);
        assert_eq!(s.network.client_certs.len(), 1, "読める要素だけ残る");
        assert!(!s.request.follow_redirects);
        assert_eq!(
            s.request.timeout_secs,
            AppSettings::default().request.timeout_secs
        );
        assert_eq!(s.history.limit, AppSettings::default().history.limit);
        assert!(!s.history.omit_large_bodies);
        assert_eq!(s.appearance.theme, AppSettings::default().appearance.theme);
    }

    #[test]
    fn unreadable_json_gives_defaults() {
        for j in ["", "not json", "[]", "null", "5"] {
            assert_eq!(with_stored(j), AppSettings::default(), "{j:?}");
        }
    }
}
