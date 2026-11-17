//! リクエストの送信まわり。変数の層の組み立て・プレビュー・送信・中止・ボディの保持。
//!
//! 送信の流れ（`request_send`）:
//! 1. 同期で、プロジェクト・環境を確かめ、変数の層（上書き › 選んでいる環境 › グローバル）を作る。
//!    ここでの失敗（プロジェクトがない・キーチェーンを読めない）はコマンドのエラーとして返る。
//! 2. 送信 ID をすぐ返し、組み立て・送信は非同期のタスクで行う。
//! 3. 終わったら（成功・失敗・中止のすべて）`RequestFinished`（`request://finished`）を 1 回だけ届ける。
//!    展開できない変数があるときは、送信せずに `unresolved` の失敗として届ける。
//!
//! cookieAuth と CSRF の自動化（M5。プロジェクトの設定 `ProjectSettings` でオン・オフ）:
//! - 送るとき: 選んでいる環境の `session` を Cookie `SESSION` に、`xsrfToken` を Cookie `XSRF-TOKEN`（と、
//!   GET・HEAD・OPTIONS 以外では `X-XSRF-TOKEN` ヘッダー）に付ける。送り先が環境の `baseUrl` と同じときだけ。
//! - 受け取ったら: レスポンスの `Set-Cookie: SESSION=…` / `XSRF-TOKEN=…` で、送信に使った環境の `session` /
//!   `xsrfToken`（シークレット。なければ作る）を更新する。消されたら値を消す。更新した変数は
//!   `RequestFinished.autoUpdates`（名前と環境。値は出さない）で知らせる。
//! - 環境が選ばれていないとき（グローバルだけ）は、送るのも更新するのもしない（グローバルには保存しない）。

use std::collections::HashSet;
use std::path::Path;

use bytes::Bytes;
use nekoreq_core::{
    build_request_with, cookie_changes, is_sensitive_header, origin_of, origin_of_base_url,
    preview_with, redact_header_value, redact_text, referenced_variables, AppSettings, AutoAuth,
    AutoCookieChanges, AutoUpdate, AutoUpdateAction, AutoUpdateSkip, BuildError, CookieChange,
    Environment, HeaderPair, OpKey, Project, RequestPreview, RequestSpec, SendError, SendOutcome,
    SendSettings, SystemDynamics, VarLayer, VarScope, VarValue, BASE_URL_VAR, SESSION_VAR,
    XSRF_VAR,
};
use nekoreq_http::{CancellationToken, RedirectCookies};
use nekoreq_secrets::SecretKey;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::AppError;
use crate::history::{received_secrets, HistoryCtx, Outcome};
use crate::state::{AppState, KEPT_BODIES};

/// リクエスト内の変数の上書き。環境やグローバルの同名の変数より優先される。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct VarOverride {
    pub name: String,
    pub value: String,
}

/// 送信・プレビューの入力（画面で編集している内容と、変数の出どころ）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RequestInput {
    pub project_id: String,
    /// 使う環境。`None` ならプロジェクトの「選んでいる環境」（未選択なら環境なし）。
    /// 「グローバル」はどの場合も使われる。
    pub environment_id: Option<String>,
    pub request: RequestSpec,
    /// リクエスト内の上書き（保存はしない）。
    pub overrides: Vec<VarOverride>,
    /// yaml の Operation に紐づくリクエストのキー。`request.auth` が `None`（= プロジェクトの既定を使う）のとき、
    /// この Operation の security に従った既定の認証を使う（`security: []` は認証なし）。
    /// 既定の認証の変数（例: cookieAuth の `session`）が展開できないときは、認証なしで送る
    /// （cookieAuth の自動化は M5）。`None` なら既定の認証は使わない。
    #[serde(default)]
    pub op_key: Option<OpKey>,
}

/// 送信が終わった（`request://finished`）。成功・失敗・中止のどれでも 1 回届く。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RequestFinished {
    /// `request_send` が返した送信 ID。
    pub send_id: String,
    pub result: SendOutcome,
    /// このレスポンスの `Set-Cookie` で自動で更新した環境変数（名前と環境。値は出さない）。
    /// 画面で「session を更新しました」と出す。更新がなければ空。
    pub auto_updates: Vec<AutoUpdate>,
    /// 自動で更新できなかったもの（環境が選ばれていない・キーチェーンに書けないなど）。理由は画面にそのまま出せる。
    pub auto_update_skipped: Vec<AutoUpdateSkip>,
}

// イベント名は設計書の `request://finished`（derive だと構造体名のケバブケースに固定されるので手で実装する）
impl tauri_specta::Event for RequestFinished {
    const NAME: &'static str = "request://finished";
}

/// 送信のあとに自動で更新する先（プロジェクトの設定と、送信に使った環境）。
struct AutoTarget {
    project_id: String,
    /// 送信に使った環境（選ばれていなければ `None`。グローバルは使わない）。
    environment: Option<(String, String)>,
    /// 自動で更新してよい送り元の origin（環境の `baseUrl`）。
    origin: Option<String>,
    /// SESSION の自動化がオン。
    session: bool,
    /// CSRF の自動化がオン。
    csrf: bool,
}

/// 送信の準備（組み立てに使うものと、送信後に自動更新する先）。
pub(crate) struct SendPlan {
    pub(crate) spec: RequestSpec,
    pub(crate) scope: VarScope,
    pub(crate) auto: AutoAuth,
    target: Option<AutoTarget>,
    /// 送信に使う環境（id と名前。履歴に残す）。選ばれていなければ `None`。
    environment: Option<(String, String)>,
}

/// 環境の `session` / `xsrfToken` を自動で作るときの説明。
const AUTO_VAR_DESCRIPTION: &str = "自動で更新されます（Set-Cookie）";

impl AppState {
    /// 送る環境（プロジェクトの「選んでいる環境」か、指定された環境）。「グローバル」は環境として使わない（`None`）。
    fn resolve_environment(
        &self,
        input: &RequestInput,
        project: &Project,
    ) -> Result<Option<Environment>, AppError> {
        let env_id = input
            .environment_id
            .clone()
            .or_else(|| project.default_environment_id.clone());
        Ok(match env_id {
            Some(id) => {
                let env = self
                    .db()
                    .env_get(&id)?
                    .filter(|e| e.project_id == input.project_id)
                    .ok_or_else(|| AppError::NotFound(format!("環境が見つかりません: {id}")))?;
                // 「グローバル」を環境として指定されても、グローバルの層に任せる
                (!env.is_global).then_some(env)
            }
            None => None,
        })
    }

    /// 送信の準備をする。
    ///
    /// - Operation の security に従った既定の認証を、`request.auth` が `None` のリクエストに付ける。
    ///   使う変数をすべて展開できるときだけ付ける（値がなければ認証なし）。付けるかどうかの判定のため、
    ///   変数の層は認証つきのリクエストで組み立てる（シークレットを読む対象に認証の変数が入る）。
    /// - cookieAuth・CSRF の自動化（プロジェクトの設定がオンで、環境が選ばれているとき）の値を決める。
    pub(crate) fn plan(&self, input: &RequestInput) -> Result<SendPlan, AppError> {
        let project = self.db().project_get(&input.project_id)?.ok_or_else(|| {
            AppError::NotFound(format!(
                "プロジェクトが見つかりません: {}",
                input.project_id
            ))
        })?;
        let environment = self.resolve_environment(input, &project)?;

        let mut spec = input.request.clone();
        let mut default_vars: Vec<String> = Vec::new();
        if spec.auth.is_none() {
            if let Some(key) = &input.op_key {
                if let Some(a) = self.default_auth_for(&input.project_id, key)? {
                    default_vars = a.variables().into_iter().map(String::from).collect();
                    spec.auth = Some(a.to_override());
                }
            }
        }

        let settings = project.settings;
        let has_env = environment.is_some();
        // 自動化で読む変数（シークレットのキーチェーンの読み出しを、使うものだけにする）
        let mut extra: Vec<&str> = Vec::new();
        if has_env && (settings.auto_session || settings.auto_csrf) {
            extra.push(BASE_URL_VAR);
        }
        if has_env && settings.auto_session {
            extra.push(SESSION_VAR);
        }
        if has_env && settings.auto_csrf {
            extra.push(XSRF_VAR);
        }
        // 自動化のためのキーチェーンの読み出しに失敗しても、リクエスト自体は送れるようにする
        // （自動化の値だけ諦めて、理由をプレビューに出す）。リクエストが使うシークレットの失敗は、これまでどおりエラー
        let mut keychain_note: Option<String> = None;
        let scope = match self.build_scope(input, &spec, environment.as_ref(), &extra) {
            Ok(scope) => scope,
            Err(AppError::Secret(_)) if !extra.is_empty() => {
                keychain_note = Some(format!(
                    "キーチェーンを読めなかったため、{SESSION_VAR}・{XSRF_VAR} は自動では付けません"
                ));
                self.build_scope(input, &spec, environment.as_ref(), &[])?
            }
            Err(e) => return Err(e),
        };
        if !default_vars.is_empty() && default_vars.iter().any(|v| scope.lookup(v).is_none()) {
            spec.auth = None;
        }

        let origin = scope
            .lookup(BASE_URL_VAR)
            .and_then(|v| origin_of_base_url(&v.value));
        let mut auto = AutoAuth {
            origin: origin.clone(),
            ..AutoAuth::default()
        };
        if has_env && (settings.auto_session || settings.auto_csrf) {
            if origin.is_none() {
                auto.notes.push(format!(
                    "環境に {BASE_URL_VAR} がないため、cookieAuth・CSRF の自動付与と自動更新は使いません"
                ));
            }
            // 認証を自分で選んでいるリクエスト（なし・API キー・Bearer など）には SESSION を付けない
            if settings.auto_session && input.request.auth.is_none() {
                auto.session = scope.lookup(SESSION_VAR).map(|v| v.value.clone());
            }
            if settings.auto_csrf {
                auto.csrf_enabled = true;
                auto.xsrf_token = scope.lookup(XSRF_VAR).map(|v| v.value.clone());
            }
            if let Some(note) = keychain_note {
                auto.notes.push(note);
            }
        }
        let env_pair = environment.map(|e| (e.id, e.name));
        let target = (settings.auto_session || settings.auto_csrf).then(|| AutoTarget {
            project_id: input.project_id.clone(),
            environment: env_pair.clone(),
            origin,
            session: settings.auto_session,
            csrf: settings.auto_csrf,
        });
        Ok(SendPlan {
            spec,
            scope,
            auto,
            target,
            environment: env_pair,
        })
    }

    /// Operation の security に従った既定の認証（ない・対応していない方式なら `None`）。
    fn default_auth_for(
        &self,
        project_id: &str,
        key: &OpKey,
    ) -> Result<Option<nekoreq_openapi::DefaultAuth>, AppError> {
        let Some(stored) = self.db().operation_get(project_id, key)? else {
            return Ok(None);
        };
        let Some(project) = self.db().project_get(project_id)? else {
            return Ok(None);
        };
        let Some(source) = project.source else {
            return Ok(None);
        };
        Ok(nekoreq_openapi::default_auth(
            &stored.operation,
            &source.meta,
            &source.options,
        ))
    }

    /// 変数の層を組み立てる。シークレットは、リクエストと `extra`（自動化で読む変数）が使っているものだけを
    /// キーチェーンから読む。
    fn build_scope(
        &self,
        input: &RequestInput,
        spec: &RequestSpec,
        environment: Option<&Environment>,
        extra: &[&str],
    ) -> Result<VarScope, AppError> {
        let global = self.db().env_global(&input.project_id)?;

        let mut used: HashSet<String> = referenced_variables(spec).into_iter().collect();
        used.extend(extra.iter().map(|s| s.to_string()));

        let mut overrides = VarLayer::new();
        for o in &input.overrides {
            let name = o.name.trim();
            if !name.is_empty() {
                overrides.insert(name, o.value.clone(), false);
            }
        }
        let env_layer = match environment {
            Some(env) => self.env_layer(&input.project_id, env, &used)?,
            None => VarLayer::new(),
        };
        let global_layer = self.env_layer(&input.project_id, &global, &used)?;
        Ok(VarScope::from_layers(overrides, env_layer, global_layer))
    }

    /// 環境の有効な変数を層にする。シークレットは `used` にあるものだけ読み、値がなければ入れない。
    fn env_layer(
        &self,
        project_id: &str,
        env: &Environment,
        used: &HashSet<String>,
    ) -> Result<VarLayer, AppError> {
        let mut layer = VarLayer::new();
        for v in env.vars.iter().filter(|v| v.enabled) {
            match &v.value {
                VarValue::Plain(s) => layer.insert(&v.name, s.clone(), false),
                VarValue::Secret => {
                    if used.contains(&v.name) {
                        let key = SecretKey::new(project_id, &env.id, &v.name);
                        if let Some(value) = self.secrets().get(&key)? {
                            layer.insert(&v.name, value, true);
                        }
                    }
                }
            }
        }
        Ok(layer)
    }

    /// 展開後の URL（シークレットは `••••`）、展開できない変数、自動で付くヘッダー（出どころつき・伏せ字）を返す。
    pub fn request_preview(&self, input: &RequestInput) -> Result<RequestPreview, AppError> {
        let plan = self.plan(input)?;
        Ok(preview_with(
            &plan.spec,
            &plan.scope,
            &SystemDynamics,
            &plan.auto,
        ))
    }

    /// 送信を始めて、送信 ID をすぐ返す。結果は `on_finished` に 1 回だけ渡す。
    /// `settings` が `None` のときは、保存された設定（タイムアウト・リダイレクト・証明書の検証）を使う。
    /// プロキシ・CA 証明書・クライアント証明書・表示上限は、いつも保存された設定を使う。
    /// 送信したものは履歴に残す（`history.rs`）。
    /// tokio のランタイムの中で呼ぶこと（Tauri の async コマンドの中ならよい）。
    pub fn request_send<F>(
        &self,
        input: RequestInput,
        settings: Option<SendSettings>,
        on_finished: F,
    ) -> Result<String, AppError>
    where
        F: FnOnce(RequestFinished) + Send + 'static,
    {
        let plan = self.plan(&input)?;
        let app = self.db().app_settings_get()?;
        let settings = settings.unwrap_or_else(|| app.send_settings());
        let ctx = self.history_ctx(&input, plan.environment.clone(), app.history.clone());
        let send_id = nekoreq_core::new_id();
        let token = CancellationToken::new();
        self.inner
            .in_flight()
            .insert(send_id.clone(), token.clone());

        let this = self.clone();
        let id = send_id.clone();
        tokio::spawn(async move {
            let (result, auto_updates, auto_update_skipped) = this
                .execute(&id, &plan, &settings, &app, &ctx, &token)
                .await;
            this.inner.in_flight().remove(&id);
            on_finished(RequestFinished {
                send_id: id,
                result,
                auto_updates,
                auto_update_skipped,
            });
        });
        Ok(send_id)
    }

    async fn execute(
        &self,
        send_id: &str,
        plan: &SendPlan,
        settings: &SendSettings,
        app: &AppSettings,
        ctx: &HistoryCtx,
        cancel: &CancellationToken,
    ) -> (SendOutcome, Vec<AutoUpdate>, Vec<AutoUpdateSkip>) {
        let prepared =
            match build_request_with(&plan.spec, &plan.scope, &SystemDynamics, &plan.auto) {
                Ok(p) => p,
                Err(BuildError::Unresolved(vars)) => {
                    return failure(SendError::unresolved(vars));
                }
                Err(BuildError::InvalidUrl(message) | BuildError::InvalidAuth(message)) => {
                    return failure(SendError::InvalidRequest { message });
                }
            };
        // プロキシ・CA 証明書・クライアント証明書は、送るときに設定とキーチェーンから読む
        let mut transport = {
            let this = self.clone();
            let app = app.clone();
            let url = prepared.url.clone();
            match tokio::task::spawn_blocking(move || this.send_transport(&app, &url)).await {
                Ok(Ok(t)) => t,
                Ok(Err(e)) => return failure(e),
                Err(e) => {
                    return failure(SendError::Other {
                        message: format!("接続の設定を読めませんでした: {e}"),
                    })
                }
            }
        };
        // リダイレクトの途中の Set-Cookie（ログイン後の 302 など）を、同じ origin の追従先に反映する
        transport.redirect_cookies = plan.target.as_ref().and_then(|t| {
            t.origin.clone().map(|origin| RedirectCookies {
                origin,
                session: t.session,
                csrf: t.csrf,
            })
        });
        match nekoreq_http::send_with_transport(&prepared, settings, cancel, &transport).await {
            Ok(sent) => {
                self.keep_body(send_id, sent.body);
                let hops = sent.redirect_hops;
                let mut response = sent.data;
                // 履歴（伏せ字にしてから保存する。ヘッダーは値が入ったままのものから作る）
                self.history_record(
                    ctx,
                    &prepared,
                    Outcome::Response(&response),
                    &received_secrets(&response, &hops),
                );
                // Set-Cookie から SESSION・XSRF-TOKEN を保持する（画面へ返す前の、値が入ったヘッダーで読む）
                let (updates, skipped) = match &plan.target {
                    Some(target) => {
                        // リダイレクトの途中のレスポンス → 最後のレスポンスの順に読む（あとのものが勝つ）
                        let now = chrono::Utc::now();
                        let mut all_updates: Vec<AutoUpdate> = Vec::new();
                        let mut all_skipped: Vec<AutoUpdateSkip> = Vec::new();
                        let steps = hops
                            .iter()
                            .map(|h| (cookie_changes(&h.set_cookies, now), h.url.as_str()))
                            .chain([(
                                cookie_changes(&response.headers, now),
                                response.final_url.as_str(),
                            )]);
                        for (changes, url) in steps {
                            let (u, s) = self.apply_cookie_changes(target, changes, url);
                            merge_by_name(&mut all_updates, u, |x| x.name.clone());
                            merge_by_name(&mut all_skipped, s, |x| x.name.clone());
                        }
                        // 更新できた変数は、別の段で「保存しなかった」と出た理由を残さない
                        all_skipped.retain(|k| !all_updates.iter().any(|u| u.name == k.name));
                        (all_updates, all_skipped)
                    }
                    None => (Vec::new(), Vec::new()),
                };
                // 画面へ返すヘッダーは、Set-Cookie など値を伏せる対象のものを `••••` にする
                response.headers = mask_sensitive_headers(response.headers);
                (SendOutcome::Success { response }, updates, skipped)
            }
            Err(error) => {
                // 送っていないもの（URL・ヘッダーが正しくない・ボディのファイルを読めない）は残さない
                if !matches!(
                    error,
                    SendError::Unresolved { .. }
                        | SendError::InvalidRequest { .. }
                        | SendError::BodyFile { .. }
                ) {
                    self.history_record(ctx, &prepared, Outcome::Error(&error), &[]);
                }
                failure(error)
            }
        }
    }

    /// レスポンスの `Set-Cookie` を、送信に使った環境の `session` / `xsrfToken`（シークレット）に反映する。
    ///
    /// - 値が変わったときだけ書く（変わらなければ何もせず、報告にも出さない）。変数がなければシークレットとして作る
    ///   （通常の変数だったときはシークレットに変える）。消す指示（`Max-Age=0`・期限切れ・空の値）なら値を消す。
    /// - 環境が選ばれていない・送り元が環境の `baseUrl` と違う・キーチェーンに書けないときは、書かずに理由を返す。
    /// - 変数の有効・無効は変えない。
    fn apply_cookie_changes(
        &self,
        target: &AutoTarget,
        changes: AutoCookieChanges,
        final_url: &str,
    ) -> (Vec<AutoUpdate>, Vec<AutoUpdateSkip>) {
        let mut updates = Vec::new();
        let mut skipped = Vec::new();
        let wanted = [
            (SESSION_VAR, changes.session, target.session),
            (XSRF_VAR, changes.xsrf, target.csrf),
        ];
        for (name, change, enabled) in wanted {
            let Some(change) = change.filter(|_| enabled) else {
                continue;
            };
            let skip = |reason: String| AutoUpdateSkip {
                name: name.to_string(),
                reason,
            };
            let Some((env_id, env_name)) = &target.environment else {
                skipped.push(skip(format!(
                    "環境が選ばれていないため、{name} は保存しませんでした（グローバルには保存しません）"
                )));
                continue;
            };
            let same_origin = match (&target.origin, origin_of(final_url)) {
                (Some(base), Some(dest)) => *base == dest,
                _ => false,
            };
            if !same_origin {
                skipped.push(skip(format!(
                    "送り元が環境の {BASE_URL_VAR} と違うため、{name} は保存しませんでした"
                )));
                continue;
            }
            match self.store_auto_value(&target.project_id, env_id, name, &change) {
                Ok(true) => updates.push(AutoUpdate {
                    name: name.to_string(),
                    environment_id: env_id.clone(),
                    environment_name: env_name.clone(),
                    action: match change {
                        CookieChange::Set(_) => AutoUpdateAction::Updated,
                        CookieChange::Clear => AutoUpdateAction::Cleared,
                    },
                }),
                Ok(false) => {}
                Err(e) => {
                    // メッセージに値が入っていても伏せる
                    let secrets = match &change {
                        CookieChange::Set(v) => vec![v.clone()],
                        CookieChange::Clear => vec![],
                    };
                    skipped.push(skip(format!(
                        "{name} を保存できませんでした: {}",
                        redact_text(&e.to_string(), &secrets)
                    )));
                }
            }
        }
        (updates, skipped)
    }

    /// 環境のシークレット `name` に反映する。変えたら `true`。
    fn store_auto_value(
        &self,
        project_id: &str,
        env_id: &str,
        name: &str,
        change: &CookieChange,
    ) -> Result<bool, AppError> {
        // 環境の変更（変数を足す）と値の書き込みを、同時に走る送信どうしで混ぜない
        let _guard = self.inner.auto_lock();
        let key = SecretKey::new(project_id, env_id, name);
        let current = self.secrets().get(&key)?;
        match change {
            CookieChange::Set(value) => {
                if current.as_deref() == Some(value.as_str()) {
                    // 値は同じ。変数が宣言されていなければ宣言だけ足す
                    self.db()
                        .env_var_ensure_secret(env_id, name, AUTO_VAR_DESCRIPTION)?;
                    return Ok(false);
                }
                self.db()
                    .env_var_ensure_secret(env_id, name, AUTO_VAR_DESCRIPTION)?;
                self.secrets().set(&key, value)?;
                Ok(true)
            }
            CookieChange::Clear => {
                if current.is_none() {
                    return Ok(false);
                }
                self.secrets().delete(&key)?;
                Ok(true)
            }
        }
    }

    fn keep_body(&self, send_id: &str, body: Bytes) {
        let mut bodies = self.inner.bodies();
        bodies.push_back((send_id.to_string(), body));
        while bodies.len() > KEPT_BODIES {
            bodies.pop_front();
        }
    }

    /// 送信中のリクエストを止める。止めるものがあれば `true`（すでに終わっていれば `false`）。
    /// 止めると `request://finished` が `cancelled` の失敗で届く。
    pub fn request_cancel(&self, send_id: &str) -> bool {
        match self.inner.in_flight().get(send_id) {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// 受け取ったボディの全体を `path`（絶対パス）に保存する。
    /// 直近 8 件のレスポンスだけ持っているので、古い送信 ID は `NotFound`。
    pub fn response_save_body(&self, send_id: &str, path: &str) -> Result<(), AppError> {
        let target = Path::new(path);
        if !target.is_absolute() {
            return Err(AppError::Validation(format!(
                "保存先は絶対パスで指定してください: {path}"
            )));
        }
        let body = self
            .inner
            .bodies()
            .iter()
            .find(|(id, _)| id == send_id)
            .map(|(_, b)| b.clone())
            .ok_or_else(|| {
                AppError::NotFound(
                    "保存できるレスポンスがありません（古い送信か、まだ終わっていません）".into(),
                )
            })?;
        std::fs::write(target, &body)
            .map_err(|e| AppError::Io(format!("ファイルに保存できませんでした: {path}（{e}）")))
    }
}

/// `new` の各要素を `all` に足す。同じ名前のものがあれば置き換える（あとのものが勝つ）。
fn merge_by_name<T>(all: &mut Vec<T>, new: Vec<T>, name: impl Fn(&T) -> String) {
    for item in new {
        let key = name(&item);
        all.retain(|x| name(x) != key);
        all.push(item);
    }
}

fn failure(error: SendError) -> (SendOutcome, Vec<AutoUpdate>, Vec<AutoUpdateSkip>) {
    (SendOutcome::Failure { error }, Vec::new(), Vec::new())
}

/// 値を伏せる対象のヘッダー（`Set-Cookie`・`Cookie`・`Authorization`・`X-XSRF-TOKEN` など）の値を `••••` にする。
/// Cookie 系は名前と属性（`Path=/` など）を残す。
fn mask_sensitive_headers(headers: Vec<HeaderPair>) -> Vec<HeaderPair> {
    headers
        .into_iter()
        .map(|h| {
            if is_sensitive_header(&h.name) {
                HeaderPair {
                    value: redact_header_value(&h.name, &h.value),
                    name: h.name,
                }
            } else {
                h
            }
        })
        .collect()
}
