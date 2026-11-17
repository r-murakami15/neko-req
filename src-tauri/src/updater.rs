//! 自動更新。GitHub Releases の `latest.json`（`tauri.conf.json` の `plugins.updater.endpoints`）を確認する。
//!
//! - 起動の数秒後に 1 回だけ確認する（release ビルドのみ）。更新があれば `update://available` で知らせる。
//!   確認に失敗しても画面にはエラーを出さず、標準エラーに記録するだけ（オフライン・設定が置き換え値のまま、など）。
//! - 画面からは `update_check`（今すぐ確認）、`update_pending`（起動時の確認で見つかった更新の取り出し）、
//!   `update_install`（ダウンロードして入れ替える）、`update_restart`（再起動）を呼ぶ。勝手に入れ替えない。
//! - 入れ替えの進捗は `update://progress`。入れ替え後に再起動するかは画面が決める（送信中のリクエストを失わないため）。
//! - 外へ出る通信は、この確認とダウンロード（どちらも updater プラグインが行い、更新ファイルは Tauri 専用の鍵で検証する）だけ。
//! - 設定の手動プロキシ（URL・ユーザー名・パスワード）は、この確認とダウンロードにも使う（「使わない」なら環境変数も無視）。
//!   CA 証明書・クライアント証明書は updater プラグインに渡す口がないので効かない。
//! - コマンドは `R: Runtime` で総称にしてある（モックのランタイムでテストできる）。bindings には `Wry` で出す。
//! - エラー文からは、URL のユーザー情報・クエリ・フラグメントを落とす（将来クエリにトークンを入れても画面とログに出ない）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager, Runtime, State};
use tauri_plugin_updater::{Update, UpdaterExt};
use tauri_specta::Event;

use crate::error::AppError;
use crate::settings::UpdaterProxy;
use crate::state::AppState;

/// 起動してから最初の確認までの待ち（起動直後の処理と、画面のイベント購読を待つ）。
const STARTUP_CHECK_DELAY: Duration = Duration::from_secs(5);
/// 進捗イベントを出す間隔（ダウンロードしたバイト数）。
const PROGRESS_STEP: u64 = 128 * 1024;

/// 見つかった新しい版の情報。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// 新しい版（例: `0.2.0`）。
    pub version: String,
    /// いま動いている版。
    pub current_version: String,
    /// リリースノート（GitHub のリリースの説明）。
    pub notes: Option<String>,
    /// 公開日時（RFC 3339）。
    pub date: Option<String>,
}

impl From<&Update> for UpdateInfo {
    fn from(u: &Update) -> Self {
        UpdateInfo {
            version: u.version.clone(),
            current_version: u.current_version.clone(),
            notes: u.body.clone(),
            date: u.date.map(|d| d.to_string()),
        }
    }
}

/// 更新が見つかった（`update://available`）。起動時の確認でだけ届く（`update_check` は戻り値で返す）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAvailable {
    pub info: UpdateInfo,
}

impl Event for UpdateAvailable {
    const NAME: &'static str = "update://available";
}

/// 入れ替えの段階。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum UpdatePhase {
    /// ダウンロード中。
    Downloading,
    /// ダウンロードが終わり、入れ替えている。
    Installing,
    /// 入れ替え完了。再起動すると新しい版になる。
    Installed,
}

/// 入れ替えの進捗（`update://progress`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgress {
    pub phase: UpdatePhase,
    /// ここまでにダウンロードしたバイト数。
    pub downloaded: u32,
    /// 全体のバイト数（サーバーが教えないときは `None`）。
    pub total: Option<u32>,
}

impl Event for UpdateProgress {
    const NAME: &'static str = "update://progress";
}

/// 見つかっていて、まだ入れ替えていない更新と、入れ替え中かどうか。
#[derive(Default)]
pub struct UpdaterState {
    pending: Mutex<Option<Update>>,
    installing: AtomicBool,
}

impl UpdaterState {
    fn set_pending(&self, update: Option<Update>) {
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = update;
    }

    fn pending_info(&self) -> Option<UpdateInfo> {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(UpdateInfo::from)
    }

    fn take_pending(&self) -> Option<Update> {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
}

fn to_u32(n: u64) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// 進捗イベントを間引く（チャンクごとには出さない）。`downloaded` が前回の通知から `PROGRESS_STEP` 以上進んだとき、
/// または最後まで届いたときに true。
fn should_report(downloaded: u64, last_reported: u64, total: Option<u64>) -> bool {
    downloaded.saturating_sub(last_reported) >= PROGRESS_STEP || total == Some(downloaded)
}

/// プラグインのエラーを、画面に出せる文にする。URL のユーザー情報・クエリ・フラグメントは落とす。
fn clean(error: &dyn std::fmt::Display) -> String {
    nekoreq_http::strip_url_secrets(&error.to_string())
}

/// 確認の失敗の文（画面とログに出す）。
pub fn check_error_message(error: &dyn std::fmt::Display) -> String {
    format!("更新を確認できませんでした: {}", clean(error))
}

/// 設定（手動プロキシ）を反映した updater。
fn updater_for<R: Runtime>(app: &AppHandle<R>) -> Result<tauri_plugin_updater::Updater, AppError> {
    let mut builder = app.updater_builder();
    // AppState がない（テストなど）ときは、プラグインの既定のまま
    if let Some(state) = app.try_state::<AppState>() {
        match state
            .updater_proxy()
            .map_err(|e| AppError::Update(format!("プロキシの設定を読めませんでした: {e}")))?
        {
            UpdaterProxy::Default => {}
            UpdaterProxy::Disabled => builder = builder.no_proxy(),
            UpdaterProxy::Url(url) => builder = builder.proxy(url),
        }
    }
    builder
        .build()
        .map_err(|e| AppError::Update(format!("更新の設定を読めませんでした: {}", clean(&e))))
}

/// 更新を確認して、見つかったら `pending` に保持する。
async fn check<R: Runtime>(app: &AppHandle<R>) -> Result<Option<UpdateInfo>, AppError> {
    let updater = updater_for(app)?;
    let found = updater
        .check()
        .await
        .map_err(|e| AppError::Update(check_error_message(&e)))?;
    let info = found.as_ref().map(UpdateInfo::from);
    app.state::<UpdaterState>().set_pending(found);
    Ok(info)
}

/// 起動の数秒後に 1 回だけ確認する。失敗は画面に出さず、標準エラーに記録するだけ。
/// debug ビルド（開発中）では確認しない（`false` を返す）。確認を始めたら `true`。
pub fn spawn_startup_check<R: Runtime>(app: AppHandle<R>) -> bool {
    if cfg!(debug_assertions) {
        return false;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(STARTUP_CHECK_DELAY).await;
        match check(&app).await {
            Ok(Some(info)) => {
                eprintln!("[updater] 新しい版があります: {}", info.version);
                if let Err(e) = (UpdateAvailable { info }).emit(&app) {
                    eprintln!("[updater] イベントを届けられませんでした: {e}");
                }
            }
            Ok(None) => {}
            Err(e) => eprintln!("[updater] {e}"),
        }
    });
    true
}

/// 更新を今すぐ確認する。新しい版があればその情報、なければ `None`。
#[tauri::command]
#[specta::specta]
pub async fn update_check<R: Runtime>(app: AppHandle<R>) -> Result<Option<UpdateInfo>, AppError> {
    check(&app).await
}

/// 起動時の確認で見つかっていて、まだ入れ替えていない更新（イベントを取りこぼしたときの取り出し用）。
#[tauri::command]
#[specta::specta]
pub fn update_pending(state: State<'_, UpdaterState>) -> Option<UpdateInfo> {
    state.pending_info()
}

/// 見つかっている更新をダウンロードして入れ替える。進捗は `update://progress`。
/// 入れ替えたあと、再起動するまでは今の版のまま動く（再起動は `update_restart`）。
#[tauri::command]
#[specta::specta]
pub async fn update_install<R: Runtime>(app: AppHandle<R>) -> Result<(), AppError> {
    let state = app.state::<UpdaterState>();
    if state.installing.swap(true, Ordering::SeqCst) {
        return Err(AppError::Update("すでに更新を入れ替えています".into()));
    }
    let result = install(&app, &state).await;
    state.installing.store(false, Ordering::SeqCst);
    result
}

async fn install<R: Runtime>(app: &AppHandle<R>, state: &UpdaterState) -> Result<(), AppError> {
    // 見つかっていなければ確認し直す（画面を開いたまま時間が経った場合など）
    let update = match state.take_pending() {
        Some(u) => u,
        None => {
            check(app).await?;
            state
                .take_pending()
                .ok_or_else(|| AppError::Update("入れ替える新しい版はありません".into()))?
        }
    };

    let emit = |phase, downloaded: u64, total: Option<u64>| {
        let _ = UpdateProgress {
            phase,
            downloaded: to_u32(downloaded),
            total: total.map(to_u32),
        }
        .emit(app);
    };

    let mut downloaded: u64 = 0;
    let mut last_reported: u64 = 0;
    emit(UpdatePhase::Downloading, 0, None);
    let result = update
        .download_and_install(
            |chunk, total| {
                downloaded += chunk as u64;
                if should_report(downloaded, last_reported, total) {
                    last_reported = downloaded;
                    emit(UpdatePhase::Downloading, downloaded, total);
                }
            },
            || emit(UpdatePhase::Installing, 0, None),
        )
        .await;
    match result {
        Ok(()) => {
            emit(UpdatePhase::Installed, 0, None);
            Ok(())
        }
        Err(e) => {
            // 失敗したときは、もう一度試せるように更新を戻す
            state.set_pending(Some(update));
            Err(AppError::Update(format!(
                "更新を入れ替えられませんでした: {}",
                clean(&e)
            )))
        }
    }
}

/// アプリを再起動する（更新を入れ替えたあとに、画面が呼ぶ）。
#[tauri::command]
#[specta::specta]
pub fn update_restart<R: Runtime>(app: AppHandle<R>) {
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_throttled() {
        // 小さな進みでは出さない
        assert!(!should_report(1024, 0, Some(10_000_000)));
        // 一定量進んだら出す
        assert!(should_report(PROGRESS_STEP, 0, Some(10_000_000)));
        assert!(!should_report(PROGRESS_STEP + 10, PROGRESS_STEP, None));
        // 最後まで届いたら小さくても出す
        assert!(should_report(500, 400, Some(500)));
    }

    #[test]
    fn byte_counts_saturate_in_u32() {
        assert_eq!(to_u32(10), 10);
        assert_eq!(to_u32(u64::MAX), u32::MAX);
    }

    #[test]
    fn events_use_the_documented_names() {
        assert_eq!(UpdateAvailable::NAME, "update://available");
        assert_eq!(UpdateProgress::NAME, "update://progress");
    }

    /// `tauri.conf.json` の updater の設定が、プラグインが読める形になっている
    /// （エンドポイントは https の latest.json、公開鍵が空でない、更新ファイルを作る設定）。
    #[test]
    fn updater_config_is_well_formed() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json");
        let conf: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let updater = &conf["plugins"]["updater"];
        assert!(
            !updater["pubkey"].as_str().unwrap_or("").is_empty(),
            "pubkey が空"
        );
        let endpoints = updater["endpoints"].as_array().expect("endpoints がない");
        assert!(!endpoints.is_empty());
        for e in endpoints {
            let url = e.as_str().unwrap();
            assert!(url.starts_with("https://"), "endpoint が https でない");
            assert!(url.ends_with("/latest.json"));
        }
        assert_eq!(conf["bundle"]["createUpdaterArtifacts"], true);
    }

    /// モックのランタイムで、updater プラグインを載せたアプリを作る（endpoint は `endpoint`）。
    fn mock_app(endpoint: &str) -> tauri::App<tauri::test::MockRuntime> {
        use tauri::test::{mock_builder, mock_context, noop_assets};
        let mut context = mock_context(noop_assets());
        context.config_mut().plugins.0.insert(
            "updater".into(),
            serde_json::json!({
                "pubkey": "dW50cnVzdGVkIGNvbW1lbnQ6IHRlc3Q=",
                "endpoints": [endpoint],
                "dangerousInsecureTransportProtocol": true,
            }),
        );
        let app = mock_builder()
            .plugin(tauri_plugin_updater::Builder::new().build())
            .build(context)
            .unwrap();
        app.manage(UpdaterState::default());
        app
    }

    /// 更新がないとき（サーバーが 204 を返す）の `update_install` は、入れ替えるものがないというエラー。
    /// 確認し直しても更新が見つからないので、画面に出せる日本語になる。
    #[tokio::test]
    async fn install_without_an_update_reports_that_there_is_nothing_to_install() {
        use wiremock::matchers::any;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        let app = mock_app(&format!("{}/latest.json?token=SECRET-Q", server.uri()));
        let handle = app.handle().clone();
        assert_eq!(update_check(handle.clone()).await.unwrap(), None);
        let err = update_install(handle.clone()).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            AppError::Update("入れ替える新しい版はありません".into()).to_string()
        );
        // 失敗しても「入れ替え中」のまま残らない（もう一度呼べる）
        assert!(update_install(handle).await.is_err());
    }

    /// 確認に失敗したときのエラー文に、endpoint の URL のクエリ・ユーザー情報が出ない。
    #[tokio::test]
    async fn check_errors_do_not_expose_the_query_of_the_endpoint() {
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let app = mock_app(&format!(
            "http://user:PASS-9@127.0.0.1:{closed}/latest.json?token=SECRET-Q#frag-Z"
        ));
        let err = update_check(app.handle().clone()).await.unwrap_err();
        let text = err.to_string();
        assert!(text.contains("更新を確認できませんでした"), "{text}");
        for secret in ["SECRET-Q", "PASS-9", "frag-Z"] {
            assert!(!text.contains(secret), "{secret}: {text}");
        }
    }

    /// debug ビルドでは、起動時の確認を始めない。
    #[tokio::test]
    #[cfg(debug_assertions)]
    async fn the_startup_check_is_skipped_in_debug_builds() {
        let app = mock_app("https://example.invalid/latest.json");
        assert!(!spawn_startup_check(app.handle().clone()));
        assert!(app.state::<UpdaterState>().pending_info().is_none());
    }
}
