//! リクエストの送信まわり。変数の層の組み立て・プレビュー・送信・中止・ボディの保持。
//!
//! 送信の流れ（`request_send`）:
//! 1. 同期で、プロジェクト・環境を確かめ、変数の層（上書き › 選んでいる環境 › グローバル）を作る。
//!    ここでの失敗（プロジェクトがない・キーチェーンを読めない）はコマンドのエラーとして返る。
//! 2. 送信 ID をすぐ返し、組み立て・送信は非同期のタスクで行う。
//! 3. 終わったら（成功・失敗・中止のすべて）`RequestFinished`（`request://finished`）を 1 回だけ届ける。
//!    展開できない変数があるときは、送信せずに `unresolved` の失敗として届ける。

use std::collections::HashSet;
use std::path::Path;

use bytes::Bytes;
use nekoreq_core::{
    build_request, preview, referenced_variables, BuildError, Environment, OpKey, RequestPreview,
    RequestSpec, SendError, SendOutcome, SendSettings, SystemDynamics, VarLayer, VarScope,
    VarValue,
};
use nekoreq_http::CancellationToken;
use nekoreq_secrets::SecretKey;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::AppError;
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
}

// イベント名は設計書の `request://finished`（derive だと構造体名のケバブケースに固定されるので手で実装する）
impl tauri_specta::Event for RequestFinished {
    const NAME: &'static str = "request://finished";
}

impl AppState {
    /// Operation の security に従った既定の認証を、`request.auth` が `None` のリクエストに付ける。
    /// 使う変数をすべて展開できるときだけ付ける（値がなければ認証なし）。付けるかどうかの判定のため、
    /// 変数の層は認証つきのリクエストで組み立てる（シークレットを読む対象に認証の変数が入る）。
    fn with_default_auth(&self, input: &RequestInput) -> Result<(RequestSpec, VarScope), AppError> {
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
        let scope = self.build_scope(input, &spec)?;
        if !default_vars.is_empty() && default_vars.iter().any(|v| scope.lookup(v).is_none()) {
            spec.auth = None;
        }
        Ok((spec, scope))
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

    /// 変数の層を組み立てる。シークレットは、リクエストが使っているものだけをキーチェーンから読む。
    fn build_scope(&self, input: &RequestInput, spec: &RequestSpec) -> Result<VarScope, AppError> {
        let project = self.db().project_get(&input.project_id)?.ok_or_else(|| {
            AppError::NotFound(format!(
                "プロジェクトが見つかりません: {}",
                input.project_id
            ))
        })?;

        let env_id = input
            .environment_id
            .clone()
            .or(project.default_environment_id);
        let environment = match env_id {
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
        };
        let global = self.db().env_global(&input.project_id)?;

        let used: HashSet<String> = referenced_variables(spec).into_iter().collect();

        let mut overrides = VarLayer::new();
        for o in &input.overrides {
            let name = o.name.trim();
            if !name.is_empty() {
                overrides.insert(name, o.value.clone(), false);
            }
        }
        let env_layer = match &environment {
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

    /// 展開後の URL（シークレットは `••••`）、展開できない変数、自動で付くヘッダーを返す。
    pub fn request_preview(&self, input: &RequestInput) -> Result<RequestPreview, AppError> {
        let (spec, scope) = self.with_default_auth(input)?;
        Ok(preview(&spec, &scope, &SystemDynamics))
    }

    /// 送信を始めて、送信 ID をすぐ返す。結果は `on_finished` に 1 回だけ渡す。
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
        let (spec, scope) = self.with_default_auth(&input)?;
        let settings = settings.unwrap_or_default();
        let send_id = nekoreq_core::new_id();
        let token = CancellationToken::new();
        self.inner
            .in_flight()
            .insert(send_id.clone(), token.clone());

        let this = self.clone();
        let id = send_id.clone();
        tokio::spawn(async move {
            let result = this.execute(&id, &spec, &scope, &settings, &token).await;
            this.inner.in_flight().remove(&id);
            on_finished(RequestFinished {
                send_id: id,
                result,
            });
        });
        Ok(send_id)
    }

    async fn execute(
        &self,
        send_id: &str,
        spec: &RequestSpec,
        scope: &VarScope,
        settings: &SendSettings,
        cancel: &CancellationToken,
    ) -> SendOutcome {
        let prepared = match build_request(spec, scope, &SystemDynamics) {
            Ok(p) => p,
            Err(BuildError::Unresolved(vars)) => {
                return SendOutcome::Failure {
                    error: SendError::unresolved(vars),
                }
            }
            Err(BuildError::InvalidUrl(message) | BuildError::InvalidAuth(message)) => {
                return SendOutcome::Failure {
                    error: SendError::InvalidRequest { message },
                }
            }
        };
        match nekoreq_http::send(&prepared, settings, cancel).await {
            Ok(sent) => {
                self.keep_body(send_id, sent.body);
                SendOutcome::Success {
                    response: sent.data,
                }
            }
            Err(error) => SendOutcome::Failure { error },
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
