//! アプリの設定（設定画面）と、設定を送信・yaml の取得に反映する接続の設定（`Transport`）。
//!
//! - 保存は SQLite の `app_settings`。パスワード（プロキシ・クライアント証明書）はキーチェーンだけに置き、
//!   設定には `has_password`（あるかどうかの印）だけを持つ。印は `proxy_password_set` /
//!   `client_cert_password_set` が更新する（`app_settings_set` で画面から書き換えることはできない）。
//! - 送信のたびに、設定から `Transport`（プロキシ・CA 証明書・クライアント証明書・表示上限）を作る。
//!   CA 証明書・クライアント証明書のファイルとキーチェーンは、そのとき読む。クライアント証明書は、
//!   送り先のホストのパターンに当てはまるものだけ読む（無関係な証明書のためにキーチェーンを開かない）。

use nekoreq_core::{AppSettings, ProxyMode, SendError};
use nekoreq_http::{
    CertificateDer, ClientIdentity, Limits, ProxyChoice, Transport, MAX_BODY_BYTES,
};
use nekoreq_openapi::upload::expand_home;
use nekoreq_secrets::SecretKey;

use crate::error::AppError;
use crate::state::AppState;

/// 自動更新の確認・ダウンロードに使うプロキシ（設定の手動プロキシ。CA 証明書は updater には渡せない）。
pub(crate) enum UpdaterProxy {
    /// プラグインの既定（OS の設定と環境変数）。
    Default,
    /// 使わない（環境変数も無視）。
    Disabled,
    /// 手動のプロキシ（ユーザー名・パスワードは URL に入れてある）。
    Url(url::Url),
}

/// アプリ全体のシークレットの置き場所（プロジェクトに属さないもの）。
const APP_SCOPE: &str = "app";

fn proxy_password_key() -> SecretKey {
    SecretKey::new(APP_SCOPE, "proxy", "password")
}

fn client_cert_password_key(id: &str) -> SecretKey {
    SecretKey::new(APP_SCOPE, "client-cert", id)
}

impl AppState {
    /// アプリ全体の設定。保存がなければ既定値。
    pub fn app_settings_get(&self) -> Result<AppSettings, AppError> {
        Ok(self.db().app_settings_get()?)
    }

    /// アプリ全体の設定を保存する。整えた内容（`AppSettings::normalize`。エディタのコマンドの空白・パスの空白・
    /// クライアント証明書の ID など）を返す。
    ///
    /// - 選択肢にない値・読めない値は `Validation`（履歴の件数・文字サイズ・プロキシの URL など）。
    /// - `has_password` は画面から書き換えられない（保存済みの値を引き継ぐ）。
    /// - 設定から消したクライアント証明書のパスワードは、キーチェーンからも消す。
    /// - CA 証明書は、読めて PEM の証明書が入っていることを確かめる。
    pub fn app_settings_set(&self, settings: &AppSettings) -> Result<AppSettings, AppError> {
        let before = self.db().app_settings_get()?;
        let mut s = settings.clone();
        s.normalize().map_err(AppError::Validation)?;
        if let Some(c) = &s.editor_command {
            crate::editor::split_command(c).map_err(AppError::Validation)?;
        }
        if let Some(path) = &s.network.ca_cert_path {
            read_ca(path).map_err(AppError::Validation)?;
        }
        for c in &s.network.client_certs {
            if !expand_home(&c.path).is_file() {
                return Err(AppError::Validation(format!(
                    "クライアント証明書のファイルが見つかりません: {}",
                    c.path
                )));
            }
        }
        // 「パスワードあり」の印は、保存済みの値を引き継ぐ
        s.network.proxy.has_password = before.network.proxy.has_password;
        for c in &mut s.network.client_certs {
            c.has_password = before
                .network
                .client_certs
                .iter()
                .find(|b| b.id == c.id)
                .is_some_and(|b| b.has_password);
        }
        // 消えた証明書のパスワード
        for b in &before.network.client_certs {
            if !s.network.client_certs.iter().any(|c| c.id == b.id) {
                let _ = self.secrets().delete(&client_cert_password_key(&b.id));
            }
        }
        self.db().app_settings_set(&s)?;
        Ok(s)
    }

    /// 手動のプロキシのパスワードをキーチェーンに保存する（空なら消す）。更新後の設定を返す。
    pub fn proxy_password_set(&self, password: &str) -> Result<AppSettings, AppError> {
        let mut s = self.db().app_settings_get()?;
        if password.is_empty() {
            self.secrets().delete(&proxy_password_key())?;
            s.network.proxy.has_password = false;
        } else {
            self.secrets().set(&proxy_password_key(), password)?;
            s.network.proxy.has_password = true;
        }
        self.db().app_settings_set(&s)?;
        Ok(s)
    }

    /// クライアント証明書（`id`。先に `app_settings_set` で設定に入れておく）のパスワードをキーチェーンに保存する
    /// （空なら消す）。更新後の設定を返す。
    pub fn client_cert_password_set(
        &self,
        id: &str,
        password: &str,
    ) -> Result<AppSettings, AppError> {
        let mut s = self.db().app_settings_get()?;
        let Some(cert) = s.network.client_certs.iter_mut().find(|c| c.id == id) else {
            return Err(AppError::NotFound(format!(
                "クライアント証明書が設定にありません: {id}"
            )));
        };
        if password.is_empty() {
            self.secrets().delete(&client_cert_password_key(id))?;
            cert.has_password = false;
        } else {
            self.secrets()
                .set(&client_cert_password_key(id), password)?;
            cert.has_password = true;
        }
        self.db().app_settings_set(&s)?;
        Ok(s)
    }

    /// 送信のための接続の設定。`url` は送り先（クライアント証明書は、ホストが当てはまるものだけ読む）。
    /// 読めない CA 証明書・クライアント証明書は、送信の失敗（`SendError::Other`）にする。
    pub(crate) fn send_transport(
        &self,
        settings: &AppSettings,
        url: &str,
    ) -> Result<Transport, SendError> {
        let target = url::Url::parse(url).ok();
        self.transport(settings, target.as_ref())
            .map_err(|message| SendError::Other { message })
    }

    /// 自動更新のためのプロキシ。手動のときは、設定の URL に認証（キーチェーンのパスワード）を入れて返す。
    pub(crate) fn updater_proxy(&self) -> Result<UpdaterProxy, String> {
        let settings = self.db().app_settings_get().map_err(|e| e.to_string())?;
        let net = &settings.network.proxy;
        Ok(match net.mode {
            ProxyMode::System => UpdaterProxy::Default,
            ProxyMode::None => UpdaterProxy::Disabled,
            ProxyMode::Manual => {
                let mut url = url::Url::parse(&net.url)
                    .map_err(|_| "プロキシの URL を読めません".to_string())?;
                if !net.username.is_empty() {
                    let _ = url.set_username(&net.username);
                    if net.has_password {
                        let password = self
                            .secrets()
                            .get(&proxy_password_key())
                            .map_err(|e| format!("プロキシのパスワードを読めません: {e}"))?;
                        let _ = url.set_password(password.as_deref());
                    }
                }
                UpdaterProxy::Url(url)
            }
        })
    }

    /// yaml の URL の取得のための接続の設定（クライアント証明書はすべて読む）。
    pub(crate) fn fetch_transport(&self) -> Result<Transport, AppError> {
        let settings = self.db().app_settings_get()?;
        self.transport(&settings, None)
            .map_err(AppError::Validation)
    }

    /// `target` があれば、そのホストに当てはまるクライアント証明書だけを読む。なければすべて。
    fn transport(
        &self,
        settings: &AppSettings,
        target: Option<&url::Url>,
    ) -> Result<Transport, String> {
        let net = &settings.network;
        let proxy = match net.proxy.mode {
            ProxyMode::System => ProxyChoice::System,
            ProxyMode::None => ProxyChoice::None,
            ProxyMode::Manual => {
                let username = Some(net.proxy.username.clone()).filter(|u| !u.is_empty());
                let password = if username.is_some() && net.proxy.has_password {
                    self.secrets()
                        .get(&proxy_password_key())
                        .map_err(|e| format!("プロキシのパスワードを読めません: {e}"))?
                } else {
                    None
                };
                ProxyChoice::Manual {
                    url: net.proxy.url.clone(),
                    username,
                    password,
                    no_proxy: net.proxy.no_proxy.clone(),
                }
            }
        };

        let ca_certs = match &net.ca_cert_path {
            Some(path) => read_ca(path)?,
            None => Vec::new(),
        };

        let mut client_identities = Vec::new();
        for c in &net.client_certs {
            if let Some(t) = target {
                let host = t.host_str().unwrap_or("");
                if !nekoreq_http::host_matches(&c.host_pattern, host, t.port_or_known_default()) {
                    continue;
                }
            }
            let path = expand_home(&c.path);
            let bytes = std::fs::read(&path).map_err(|e| {
                format!(
                    "クライアント証明書（{}）を読めません: {}（{e}）",
                    c.host_pattern, c.path
                )
            })?;
            let password = if c.has_password {
                self.secrets()
                    .get(&client_cert_password_key(&c.id))
                    .map_err(|e| format!("クライアント証明書のパスワードを読めません: {e}"))?
                    .unwrap_or_default()
            } else {
                String::new()
            };
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let identity =
                ClientIdentity::from_file_bytes(&c.host_pattern, name, &bytes, &password)
                    .map_err(|e| format!("クライアント証明書（{}）: {e}", c.host_pattern))?;
            client_identities.push(identity);
        }

        Ok(Transport {
            redirect_cookies: None,
            proxy,
            ca_certs,
            client_identities,
            limits: Limits {
                preview_bytes: settings.request.response_limit.preview_bytes(),
                max_body_bytes: MAX_BODY_BYTES,
            },
        })
    }
}

/// CA 証明書（PEM）のファイルを読む。
fn read_ca(path: &str) -> Result<Vec<CertificateDer<'static>>, String> {
    let bytes = std::fs::read(expand_home(path))
        .map_err(|e| format!("CA 証明書を読めません: {path}（{e}）"))?;
    nekoreq_http::parse_ca_pem(&bytes).map_err(|e| format!("CA 証明書（{path}）: {e}"))
}
