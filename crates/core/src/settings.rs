//! アプリ全体の設定（プロジェクトに属さないもの）。設定画面（M6）の項目。
//!
//! - 保存は storage の `app_settings`（JSON 1 件）。足りない項目は既定値で読む。
//! - パスワード（クライアント証明書の .p12・プロキシの認証）は持たない（キーチェーン。`has_password` は印だけ）。
//! - 言語の設定は持たない（v1 は日本語のみ）。

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::response::{SendSettings, DEFAULT_TIMEOUT_MS};

/// 履歴に残す件数の選択肢（設定画面の「保存する件数」）。`0` は保存しない。
pub const HISTORY_LIMITS: [u32; 3] = [1000, 5000, 0];
/// エディタの文字サイズの選択肢。
pub const EDITOR_FONT_SIZES: [u8; 3] = [13, 14, 15];
/// タイムアウト（秒）の上限（1 日）。
pub const MAX_TIMEOUT_SECS: u32 = 86_400;

/// 画面のテーマ。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    Dark,
    Light,
    /// OS の設定に合わせる。
    #[default]
    System,
}

/// 外観。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct AppearanceSettings {
    pub theme: Theme,
    /// エディタの文字サイズ（px。13・14・15）。
    pub editor_font_size: u8,
    /// 起動時に前回のタブを復元する（`session_load` の結果を使うか。保存は常にする）。
    pub restore_tabs: bool,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            editor_font_size: 13,
            restore_tabs: true,
        }
    }
}

/// 画面に表示するレスポンスのボディの上限。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ResponseLimit {
    /// 10 MB（既定）。
    #[default]
    Mb10,
    Mb50,
    /// 無制限（受け取れる上限の 512 MB まで）。
    Unlimited,
}

impl ResponseLimit {
    /// 画面に返すボディの先頭のバイト数。無制限は受け取れる上限（`usize::MAX` ではなく 512 MB）。
    pub fn preview_bytes(self) -> usize {
        match self {
            ResponseLimit::Mb10 => 10 * 1024 * 1024,
            ResponseLimit::Mb50 => 50 * 1024 * 1024,
            ResponseLimit::Unlimited => 512 * 1024 * 1024,
        }
    }
}

/// リクエストの設定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct RequestSettings {
    /// タイムアウト（秒）。`0` は無制限。既定は 30。
    pub timeout_secs: u32,
    /// リダイレクトを自動で追う。
    pub follow_redirects: bool,
    /// SSL 証明書を検証する。
    pub verify_certificates: bool,
    /// 表示するレスポンスの上限。
    pub response_limit: ResponseLimit,
}

impl Default for RequestSettings {
    fn default() -> Self {
        Self {
            timeout_secs: DEFAULT_TIMEOUT_MS / 1000,
            follow_redirects: true,
            verify_certificates: true,
            response_limit: ResponseLimit::Mb10,
        }
    }
}

/// プロキシの使い方。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ProxyMode {
    /// OS の設定（macOS・Windows のシステム設定。環境変数 `HTTP_PROXY` なども）を使う。
    #[default]
    System,
    /// 使わない。
    None,
    /// 手で指定する。
    Manual,
}

/// プロキシの設定。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct ProxySettings {
    pub mode: ProxyMode,
    /// `手動` のときのプロキシの URL（`http://host:8080`。ユーザー名・パスワードは URL に書けない）。
    pub url: String,
    /// `手動` のときの認証のユーザー名（パスワードはキーチェーン。`proxy_password_set`）。
    pub username: String,
    /// パスワードがキーチェーンにある（読み取り専用。保存のときは無視する）。
    pub has_password: bool,
    /// プロキシを通さないホスト（`localhost`・`.internal.example`・`10.0.0.0/8` など）。
    pub no_proxy: Vec<String>,
}

/// クライアント証明書 1 件（ホストのパターン → ファイル）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct ClientCertSetting {
    /// 空なら保存のときに付ける。パスワード（キーチェーン）の置き場所の名前に使う。
    pub id: String,
    /// 使うホストのパターン（`*.internal.example`・`api.example.com`・`host:8443`）。
    pub host_pattern: String,
    /// .p12 / .pfx / .pem ファイルのパス（PEM は証明書と秘密鍵を同じファイルに並べたもの）。
    pub path: String,
    /// パスワードがキーチェーンにある（読み取り専用。保存のときは無視する）。
    pub has_password: bool,
}

/// プロキシ・証明書。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct NetworkSettings {
    pub proxy: ProxySettings,
    /// OS の証明書ストアに足して信頼する CA 証明書（PEM）のパス。
    pub ca_cert_path: Option<String>,
    pub client_certs: Vec<ClientCertSetting>,
}

/// 履歴の設定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct HistorySettings {
    /// 保存する件数（プロジェクトごと）。`1000`・`5000`・`0`（保存しない）。
    pub limit: u32,
    /// 1 MB を超えるボディを先頭だけ保存する（オフなら全体を保存する）。
    pub omit_large_bodies: bool,
}

impl Default for HistorySettings {
    fn default() -> Self {
        Self {
            limit: 1000,
            omit_large_bodies: true,
        }
    }
}

/// アプリの設定。保存は storage の `app_settings`。足りない項目は既定値で読む。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    /// yaml を開くエディタのコマンド（空・未設定なら、VS Code・Cursor・Sublime・JetBrains の順に見つかったもの）。
    /// シェルを通さず、空白で区切って（`"` `'` で囲める）起動する。`{file}` は絶対パス、`{line}` は行番号
    /// （行番号がなければ 1）に置き換える。例: `code -g {file}:{line}`
    pub editor_command: Option<String>,
    pub appearance: AppearanceSettings,
    pub request: RequestSettings,
    pub network: NetworkSettings,
    pub history: HistorySettings,
}

impl AppSettings {
    /// 送信の設定（タイムアウト・リダイレクト・証明書の検証）。`request_send` の `settings` が `null` のときに使う。
    pub fn send_settings(&self) -> SendSettings {
        SendSettings {
            timeout_ms: self.request.timeout_secs.saturating_mul(1000),
            follow_redirects: self.request.follow_redirects,
            verify_certificates: self.request.verify_certificates,
        }
    }

    /// 保存する前に値を整える。選択肢にない値・読めない値は `Err`（画面にそのまま出せる日本語）。
    ///
    /// - エディタのコマンド・パス・ホストのパターンは前後の空白を除く（空のコマンドは未設定）。
    /// - プロキシの URL は `http://` か `https://` で、ユーザー情報（`user:pass@`）を含まない。
    /// - `has_password` は読み取り専用なので `false` に戻す（app が実際の状態で埋め直す）。
    /// - クライアント証明書の `id` が空なら `new_id()` で付ける。
    pub fn normalize(&mut self) -> Result<(), String> {
        self.editor_command = self
            .editor_command
            .take()
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty());
        if !EDITOR_FONT_SIZES.contains(&self.appearance.editor_font_size) {
            return Err("エディタの文字サイズは 13・14・15 のどれかを選んでください".into());
        }
        if self.request.timeout_secs > MAX_TIMEOUT_SECS {
            return Err(format!(
                "タイムアウトは {MAX_TIMEOUT_SECS} 秒（1 日）以内にしてください（0 で無制限）"
            ));
        }
        if !HISTORY_LIMITS.contains(&self.history.limit) {
            return Err("履歴の件数は 1,000・5,000・保存しない のどれかを選んでください".into());
        }

        let proxy = &mut self.network.proxy;
        proxy.has_password = false;
        proxy.url = proxy.url.trim().to_string();
        proxy.username = proxy.username.trim().to_string();
        proxy.no_proxy = proxy
            .no_proxy
            .iter()
            .flat_map(|h| h.split([',', '\n']))
            .map(|h| h.trim().to_string())
            .filter(|h| !h.is_empty())
            .collect();
        if proxy.mode == ProxyMode::Manual {
            if proxy.url.is_empty() {
                return Err("手動のプロキシは URL を入力してください".into());
            }
            let parsed = url::Url::parse(&proxy.url).map_err(|_| {
                "プロキシの URL を読めません（http://host:8080 の形で入力してください）".to_string()
            })?;
            if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                return Err(
                    "プロキシの URL は http:// か https:// で始めてください（http://host:8080）"
                        .into(),
                );
            }
            if !parsed.username().is_empty() || parsed.password().is_some() {
                return Err(
                    "プロキシの URL にユーザー名・パスワードは書けません。認証が要るときはユーザー名欄とパスワード欄に入力してください"
                        .into(),
                );
            }
        }

        self.network.ca_cert_path = self
            .network
            .ca_cert_path
            .take()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty());

        let mut seen = std::collections::HashSet::new();
        for c in &mut self.network.client_certs {
            c.has_password = false;
            c.host_pattern = c.host_pattern.trim().to_string();
            c.path = c.path.trim().to_string();
            if c.host_pattern.is_empty() {
                return Err("クライアント証明書のホストのパターンを入力してください".into());
            }
            if c.path.is_empty() {
                return Err(format!(
                    "クライアント証明書（{}）のファイルを選んでください",
                    c.host_pattern
                ));
            }
            if c.id.trim().is_empty() {
                c.id = crate::new_id();
            }
            if !seen.insert(c.id.clone()) {
                return Err("クライアント証明書の ID が重なっています".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let s: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, AppSettings::default());
        let s: AppSettings =
            serde_json::from_str(r#"{"editorCommand":"code -g {file}:{line}"}"#).unwrap();
        assert_eq!(s.editor_command.as_deref(), Some("code -g {file}:{line}"));
        assert_eq!(s.request.timeout_secs, 30);
        assert_eq!(s.history.limit, 1000);
        // 一部の項目だけ書かれていても、残りは既定値
        let s: AppSettings = serde_json::from_str(r#"{"request":{"timeoutSecs":5}}"#).unwrap();
        assert_eq!(s.request.timeout_secs, 5);
        assert!(s.request.follow_redirects && s.request.verify_certificates);
    }

    #[test]
    fn defaults_match_the_spec() {
        let s = AppSettings::default();
        assert_eq!(s.appearance.theme, Theme::System);
        assert_eq!(s.appearance.editor_font_size, 13);
        assert!(s.appearance.restore_tabs);
        assert_eq!(s.request.response_limit, ResponseLimit::Mb10);
        assert_eq!(s.network.proxy.mode, ProxyMode::System);
        assert!(s.history.omit_large_bodies);
        assert_eq!(s.send_settings(), SendSettings::default());
    }

    #[test]
    fn send_settings_follow_the_request_section() {
        let mut s = AppSettings::default();
        s.request.timeout_secs = 0;
        s.request.follow_redirects = false;
        s.request.verify_certificates = false;
        let send = s.send_settings();
        assert_eq!(send.timeout_ms, 0);
        assert!(!send.follow_redirects && !send.verify_certificates);
    }

    #[test]
    fn wire_names() {
        let v = serde_json::to_value(AppSettings::default()).unwrap();
        assert_eq!(v["appearance"]["theme"], "system");
        assert_eq!(v["request"]["responseLimit"], "mb10");
        assert_eq!(v["network"]["proxy"]["mode"], "system");
        assert_eq!(v["history"]["limit"], 1000);
    }

    #[test]
    fn normalize_validates_choices() {
        let mut s = AppSettings::default();
        s.history.limit = 2000;
        assert!(s.normalize().unwrap_err().contains("履歴"));
        let mut s = AppSettings::default();
        s.appearance.editor_font_size = 20;
        assert!(s.normalize().is_err());
        let mut s = AppSettings::default();
        s.request.timeout_secs = MAX_TIMEOUT_SECS + 1;
        assert!(s.normalize().is_err());
    }

    #[test]
    fn normalize_checks_the_manual_proxy() {
        let mut s = AppSettings::default();
        s.network.proxy.mode = ProxyMode::Manual;
        assert!(s.normalize().unwrap_err().contains("URL"));
        s.network.proxy.url = "socks5://h:1".into();
        assert!(s.normalize().unwrap_err().contains("http://"));
        s.network.proxy.url = "http://u:pw@h:8080".into();
        let e = s.normalize().unwrap_err();
        assert!(e.contains("ユーザー名"), "{e}");
        assert!(!e.contains("pw"));
        s.network.proxy.url = "  http://h:8080 ".into();
        s.network.proxy.no_proxy = vec!["localhost, .internal.example".into(), " ".into()];
        s.network.proxy.has_password = true;
        s.normalize().unwrap();
        assert_eq!(s.network.proxy.url, "http://h:8080");
        assert_eq!(
            s.network.proxy.no_proxy,
            vec!["localhost", ".internal.example"]
        );
        assert!(!s.network.proxy.has_password, "has_password は読み取り専用");
        // 手動でなければ URL は検査しない
        let mut s = AppSettings::default();
        s.network.proxy.url = "garbage".into();
        s.normalize().unwrap();
    }

    #[test]
    fn normalize_client_certs_and_editor() {
        let mut s = AppSettings {
            editor_command: Some("  ".into()),
            ..AppSettings::default()
        };
        s.network.client_certs = vec![ClientCertSetting {
            host_pattern: " *.internal.example ".into(),
            path: " /c/client.p12".into(),
            ..ClientCertSetting::default()
        }];
        s.normalize().unwrap();
        assert_eq!(s.editor_command, None);
        let c = &s.network.client_certs[0];
        assert_eq!(c.host_pattern, "*.internal.example");
        assert_eq!(c.path, "/c/client.p12");
        assert!(!c.id.is_empty(), "ID を付ける");

        s.network.client_certs[0].path.clear();
        assert!(s.normalize().is_err());
    }

    #[test]
    fn response_limits() {
        assert_eq!(ResponseLimit::Mb10.preview_bytes(), 10 * 1024 * 1024);
        assert_eq!(ResponseLimit::Mb50.preview_bytes(), 50 * 1024 * 1024);
        assert!(ResponseLimit::Unlimited.preview_bytes() > 50 * 1024 * 1024);
    }
}
