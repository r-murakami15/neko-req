//! cURL の取り込み（`curl_parse`）とコピー（`curl_export`・履歴からのコピー）。
//!
//! 取り込み（設計書「cURL 取り込み」）:
//! 1. `nekoreq-curl` で解析する（無視したオプションなどは警告で返す。Chrome の `Accept-Encoding` の `zstd` は除く）。
//! 2. ホスト部分が環境の `baseUrl`（指定された環境、なければ選んでいる環境、それでもなければほかの環境）と同じなら、
//!    URL の先頭を `{{baseUrl}}` に置き換える。
//! 3. 置き換えられたときだけ、method と path を yaml の Operation の path テンプレートと照合する（固定の区間が多い
//!    ものを優先。method も一致）。一致すればその API の入力値（path パラメータは当てはまった値）として開ける形で、
//!    一致しなければ AdHocRequest（「その他」）として開ける形で返す。何も保存しない。
//!
//! コピー: 今の入力を展開した内容（`curl_export`）か、履歴の送った内容（伏せ字済み）を cURL にする。

use nekoreq_core::PreparedPart;
use nekoreq_core::{
    build_request_with, Body, HistoryBodyKind, HistoryEntry, KeyValue, MultipartPart,
    MultipartValue, OpKey, ParamIn, PreparedBody, PreparedPartValue, PreparedRequest, RequestDraft,
    RequestSpec, SystemDynamics, VarValue, BASE_URL_VAR, DEFAULT_ACCEPT, DEFAULT_ACCEPT_ENCODING,
    DEFAULT_USER_AGENT,
};
use nekoreq_curl::{CurlWarning, ExportOptions, WarningKind};
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::AppError;
use crate::history::header_rows;
use crate::send::RequestInput;
use crate::state::AppState;

/// 取り込んだ cURL の行き先。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum CurlTarget {
    /// yaml の API（`op_key`）の入力値として開く。
    Operation,
    /// 手で作ったリクエスト（「その他」）として開く。
    Adhoc,
}

/// `curl_parse` の結果。何も保存しない（画面が新しいタブで開く）。
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CurlImport {
    pub target: CurlTarget,
    /// `target` が `operation` のときの API のキー。
    pub op_key: Option<OpKey>,
    /// API の summary（タブの見出し）。
    pub summary: Option<String>,
    /// 当てはまった API の path テンプレート（`/api/v1/video/clients/{clientId}`）。
    pub matched_path: Option<String>,
    /// 開くリクエスト。`operation` のときは、`url` が `{{baseUrl}}` + path テンプレートで、path パラメータは
    /// `params`（`location: path`）に当てはまった値が入っている（yaml のほかのパラメータは無効の行で並ぶ）。
    pub request: RequestSpec,
    /// `{{baseUrl}}` に置き換えたか。
    pub base_replaced: bool,
    /// ホストが当てはまった環境（`baseUrl` が同じ環境。画面はこの環境を選ぶよう案内できる）。
    pub environment_id: Option<String>,
    /// `adhoc` のときの名前の案（`POST /api/v1/auth/password-login`）。
    pub suggested_name: String,
    /// 無視したオプションなど（取り込めなかった指定）。
    pub warnings: Vec<CurlWarning>,
}

/// `curl_export` の伏せ方。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum CurlExportMode {
    /// シークレットを伏せる（既定）。`Authorization`・`Cookie`・`X-XSRF-TOKEN` などの値と、
    /// シークレット変数の値が現れた箇所を `••••` にする。
    #[default]
    Redacted,
    /// そのまま（実際の値が入る。貼り付けて動く）。
    Raw,
}

/// 環境の `baseUrl`（通常の変数）。
fn base_url_of(env: &nekoreq_core::Environment) -> Option<String> {
    env.vars
        .iter()
        .find(|v| v.enabled && v.name == BASE_URL_VAR)
        .and_then(|v| match &v.value {
            VarValue::Plain(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
            _ => None,
        })
}

/// `Accept-Encoding` から `zstd` を除く（http クレートが展開できないため）。除いたら `true`。
fn drop_zstd(rows: &mut Vec<KeyValue>) -> bool {
    let mut changed = false;
    rows.retain_mut(|r| {
        if r.location != ParamIn::Header || !r.name.eq_ignore_ascii_case("Accept-Encoding") {
            return true;
        }
        let kept: Vec<&str> = r
            .value
            .split(',')
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .filter(|e| {
                let coding = e.split(';').next().unwrap_or("").trim();
                !coding.eq_ignore_ascii_case("zstd")
            })
            .collect();
        let original = r.value.split(',').filter(|e| !e.trim().is_empty()).count();
        if kept.len() == original {
            return true;
        }
        changed = true;
        if kept.is_empty() {
            return false;
        }
        r.value = kept.join(", ");
        true
    });
    changed
}

impl AppState {
    /// cURL を取り込む。`environment_id` がなければプロジェクトの「選んでいる環境」で `baseUrl` と照合する。
    pub fn curl_parse(
        &self,
        project_id: &str,
        text: &str,
        environment_id: Option<&str>,
    ) -> Result<CurlImport, AppError> {
        let project = self.db().project_get(project_id)?.ok_or_else(|| {
            AppError::NotFound(format!("プロジェクトが見つかりません: {project_id}"))
        })?;
        let mut parsed =
            nekoreq_curl::parse(text).map_err(|e| AppError::Validation(e.to_string()))?;
        let mut warnings: Vec<CurlWarning> = parsed.warnings.clone();

        // Chrome の Accept-Encoding に入る zstd は、展開できないので外す
        let mut headers = parsed.headers.clone();
        if drop_zstd(&mut headers) {
            parsed.headers = headers;
            warnings.push(CurlWarning {
                kind: WarningKind::Adjusted,
                option: "-H".into(),
                message: "Accept-Encoding の zstd は展開できないため外しました".into(),
            });
        }

        // baseUrl と照合する環境の順: 指定 › 選んでいる › ほかの環境（作った順）› グローバル
        let envs = self.db().env_list(project_id)?;
        let wanted = environment_id
            .map(str::to_string)
            .or(project.default_environment_id.clone());
        let mut ordered: Vec<&nekoreq_core::Environment> = Vec::new();
        if let Some(id) = &wanted {
            ordered.extend(envs.iter().filter(|e| &e.id == id && !e.is_global));
        }
        ordered.extend(
            envs.iter()
                .filter(|e| !e.is_global && wanted.as_deref() != Some(e.id.as_str())),
        );
        ordered.extend(envs.iter().filter(|e| e.is_global));
        let matched_env = ordered.iter().find_map(|env| {
            let base = base_url_of(env)?;
            parsed
                .matches_base(&base)
                .then(|| (env.id.clone(), env.is_global, base))
        });

        let base_replaced = matched_env.is_some();
        let environment_id = matched_env
            .as_ref()
            .filter(|(_, global, _)| !global)
            .map(|(id, _, _)| id.clone());
        // 照合に使う path（`baseUrl` にパスがあればそれを除いたもの）
        let match_path = match &matched_env {
            Some((_, _, base)) => parsed
                .path_after_base(base)
                .unwrap_or_else(|| parsed.path.clone()),
            None => parsed.path.clone(),
        };

        // Operation との照合（`baseUrl` に置き換えられたときだけ）
        let candidate = self.match_operation(project_id, &parsed.method, &match_path)?;
        let suggested_name = format!(
            "{} {}",
            parsed.method,
            if match_path.is_empty() {
                "/"
            } else {
                &match_path
            }
        )
        .chars()
        .take(200)
        .collect::<String>();

        if let (true, Some(hit)) = (base_replaced, &candidate) {
            let source = project.source.as_ref();
            let (meta, options) = match source {
                Some(s) => (s.meta.clone(), s.options),
                None => Default::default(),
            };
            let mut params: Vec<KeyValue> = hit
                .params
                .iter()
                .map(|(name, value)| KeyValue {
                    location: ParamIn::Path,
                    name: name.clone(),
                    value: value.clone(),
                    enabled: true,
                })
                .collect();
            params.extend(parsed.params());
            let draft = RequestDraft {
                id: String::new(),
                project_id: project_id.to_string(),
                op_key: hit.operation.op_key.clone(),
                params,
                body: parsed.body.clone(),
                auth: parsed.auth.clone(),
                updated_at: String::new(),
            };
            // 入力値があるものとして開くと、yaml のほかのパラメータが無効の行で足される
            let open = nekoreq_openapi::open_operation(
                &hit.operation,
                &meta,
                &options,
                Some(&draft),
                &[],
                false,
            );
            return Ok(CurlImport {
                target: CurlTarget::Operation,
                op_key: Some(hit.operation.op_key.clone()),
                summary: hit.operation.summary.clone(),
                matched_path: Some(hit.operation.path.clone()),
                request: open.request,
                base_replaced,
                environment_id,
                suggested_name,
                warnings,
            });
        }

        if !base_replaced {
            if let Some(hit) = &candidate {
                warnings.push(CurlWarning {
                    kind: WarningKind::Adjusted,
                    option: String::new(),
                    message: format!(
                        "path は {} {}（{}）に当てはまりますが、ホスト（{}）がどの環境の baseUrl とも違うため、「その他」のリクエストとして開きます",
                        hit.operation.method,
                        hit.operation.path,
                        hit.operation.summary.clone().unwrap_or_default(),
                        parsed.base
                    ),
                });
            }
        }
        let request = match &matched_env {
            Some((_, _, base)) => parsed
                .to_request_spec_for_base(base)
                .unwrap_or_else(|| parsed.to_request_spec(false)),
            None => parsed.to_request_spec(false),
        };
        Ok(CurlImport {
            target: CurlTarget::Adhoc,
            op_key: None,
            summary: None,
            matched_path: None,
            request,
            base_replaced,
            environment_id,
            suggested_name,
            warnings,
        })
    }

    /// method と path が一致する取り込み済みの Operation。固定の区間が多いものを優先する（同数なら先のもの）。
    fn match_operation(
        &self,
        project_id: &str,
        method: &str,
        path: &str,
    ) -> Result<Option<OperationHit>, AppError> {
        let mut best: Option<(usize, OperationHit)> = None;
        for stored in self.db().operations_list(project_id)? {
            if !stored.imported || stored.deleted_at.is_some() {
                continue;
            }
            let op = stored.operation;
            if !op.method.eq_ignore_ascii_case(method) {
                continue;
            }
            let Some(m) = nekoreq_curl::match_path_template(&op.path, path) else {
                continue;
            };
            if best
                .as_ref()
                .is_none_or(|(score, _)| m.literal_segments > *score)
            {
                best = Some((
                    m.literal_segments,
                    OperationHit {
                        operation: op,
                        params: m.params,
                    },
                ));
            }
        }
        Ok(best.map(|(_, h)| h))
    }

    /// 今の入力を展開して cURL にする。展開できない変数があれば `Validation`（どの変数かを含む）。
    pub fn curl_export(
        &self,
        input: &RequestInput,
        mode: CurlExportMode,
    ) -> Result<String, AppError> {
        let plan = self.plan(input)?;
        let prepared = build_request_with(&plan.spec, &plan.scope, &SystemDynamics, &plan.auto)
            .map_err(|e| AppError::Validation(e.to_string()))?;
        Ok(prepared_to_curl(&prepared, mode))
    }
}

struct OperationHit {
    operation: nekoreq_core::Operation,
    params: Vec<(String, String)>,
}

/// 組み立て済みのリクエストを cURL にする。
pub(crate) fn prepared_to_curl(prepared: &PreparedRequest, mode: CurlExportMode) -> String {
    let body = match &prepared.body {
        PreparedBody::None => Body::None,
        PreparedBody::Bytes(b) => Body::Text(String::from_utf8_lossy(b).into_owned()),
        PreparedBody::File(path) => Body::Binary(path.clone()),
        PreparedBody::Multipart { parts, .. } => Body::Multipart(
            parts
                .iter()
                .map(|p: &PreparedPart| MultipartPart {
                    name: p.name.clone(),
                    value: match &p.value {
                        PreparedPartValue::Text(t) => MultipartValue::Text(t.clone()),
                        PreparedPartValue::File(f) => MultipartValue::File(f.clone()),
                    },
                    enabled: true,
                    content_type: p.content_type.clone(),
                })
                .collect(),
        ),
    };
    let spec = RequestSpec {
        method: prepared.method.clone(),
        url: prepared.url.clone(),
        params: header_rows(&prepared.headers),
        body,
        auth: None,
    };
    let options = match mode {
        CurlExportMode::Redacted => ExportOptions {
            redact: prepared.secrets.clone(),
            mask_sensitive: true,
            ..ExportOptions::default()
        },
        CurlExportMode::Raw => ExportOptions::default(),
    };
    nekoreq_curl::export(&spec, &options)
}

/// 履歴の送った内容（伏せ字済み）を cURL にする。
pub(crate) fn history_to_curl(e: &HistoryEntry) -> String {
    // アプリが付けた既定のヘッダー（User-Agent・Accept・Accept-Encoding）は、cURL では要らない
    let headers: Vec<_> = e
        .request_headers
        .iter()
        .filter(|h| {
            !((h.name.eq_ignore_ascii_case("User-Agent") && h.value == DEFAULT_USER_AGENT)
                || (h.name.eq_ignore_ascii_case("Accept") && h.value == DEFAULT_ACCEPT)
                || (h.name.eq_ignore_ascii_case("Accept-Encoding")
                    && h.value == DEFAULT_ACCEPT_ENCODING))
        })
        .cloned()
        .collect();
    let body = match e.request_body_kind {
        HistoryBodyKind::Text => Body::Text(e.request_body.clone().unwrap_or_default()),
        HistoryBodyKind::File => Body::Binary(e.request_file.clone().unwrap_or_default()),
        HistoryBodyKind::Multipart => Body::Multipart(
            e.request_parts
                .iter()
                .map(|p| MultipartPart {
                    name: p.name.clone(),
                    value: if p.is_file {
                        MultipartValue::File(p.value.clone())
                    } else {
                        MultipartValue::Text(p.value.clone())
                    },
                    enabled: true,
                    content_type: p.content_type.clone(),
                })
                .collect(),
        ),
        HistoryBodyKind::None | HistoryBodyKind::Binary => Body::None,
    };
    let spec = RequestSpec {
        method: e.method.clone(),
        url: e.url.clone(),
        params: header_rows(&headers),
        body,
        auth: None,
    };
    let text = nekoreq_curl::export(
        &spec,
        &ExportOptions {
            mask_sensitive: true,
            ..ExportOptions::default()
        },
    );
    // URL に入った伏せ字は、cURL の書き出しで `%E2%80%A2` になるので、読める形に戻す
    text.replace("%E2%80%A2%E2%80%A2%E2%80%A2%E2%80%A2", nekoreq_core::MASK)
}
