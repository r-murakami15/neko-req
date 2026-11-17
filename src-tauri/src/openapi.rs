//! OpenAPI の取り込みと同期（M3・M4）。画面から呼ぶコマンドの中身。
//!
//! - 指定の方法は 3 つ: パス（ローカルのファイル。参照方式）、URL（参照方式。アクセストークンはキーチェーン）、
//!   アップロード（コピー方式。フォルダ・複数ファイル・zip を `<データフォルダ>/sources/<プロジェクト ID>/` に
//!   元の構成のまま置く）。
//! - 読み込みが止まる場合（入口の候補が複数・別ホストの確認待ち・足りないファイル・壊れた yaml）は、エラーではなく
//!   戻り値の `status` で返す（画面が一覧を出せるように）。
//! - 同期の反映が止まったとき（yaml が壊れているなど）は何も書き込まない（前回の Operation を使い続ける）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use nekoreq_core::{
    match_operations, ApiSource, ImportOptions, OpKey, Operation, Project, SourceFile, SourceKind,
};
use nekoreq_openapi::{
    build_preview, diff_operations, load, upload, ForeignRef, ImportPreview, LoadEntry, LoadParams,
    LoadProblem, LoadResult, Loaded, OperationDetail, OperationListItem, OperationOpen, ParamRef,
    SourceChanged, SyncDiff,
};
use nekoreq_secrets::SecretKey;
use nekoreq_storage::{ApplyReport, Selection};
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::AppError;
use crate::state::AppState;

/// 取り込み元の指定。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum SourceInput {
    /// ローカルのファイル（入口の yaml）。フォルダを指定すると、中の `openapi:` を持つファイルを探す。
    #[serde(rename_all = "camelCase")]
    Path {
        path: String,
        /// 入口が別のホストの URL を参照しているとき、確認済みのホスト。
        #[serde(default)]
        allowed_hosts: Vec<String>,
    },
    /// URL。`token` は取り込むときにキーチェーンへ保存する（確認の段階ではメモリだけ）。
    /// 同期のときはキーチェーンの値を使う。`allowedHosts` は確認済みの別ホスト。
    #[serde(rename_all = "camelCase")]
    Url {
        url: String,
        token: Option<String>,
        allowed_hosts: Vec<String>,
    },
    /// アップロード（コピー方式）。`paths` は選んだフォルダ・ファイル・zip の絶対パス。
    /// 入口の候補が複数のときは、選んだ候補を `entry` に入れて呼び直す。
    #[serde(rename_all = "camelCase")]
    Upload {
        paths: Vec<String>,
        entry: Option<String>,
        #[serde(default)]
        allowed_hosts: Vec<String>,
    },
}

/// トークンは値を出さない（ログ・panic に平文で出ないように）。
impl std::fmt::Debug for SourceInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceInput::Path {
                path,
                allowed_hosts,
            } => f
                .debug_struct("Path")
                .field("path", path)
                .field("allowed_hosts", allowed_hosts)
                .finish(),
            SourceInput::Url {
                url,
                token,
                allowed_hosts,
            } => f
                .debug_struct("Url")
                .field("url", url)
                .field("token", &token.as_ref().map(|_| "***"))
                .field("allowed_hosts", allowed_hosts)
                .finish(),
            SourceInput::Upload {
                paths,
                entry,
                allowed_hosts,
            } => f
                .debug_struct("Upload")
                .field("paths", paths)
                .field("entry", entry)
                .field("allowed_hosts", allowed_hosts)
                .finish(),
        }
    }
}

/// コピー方式の同期でもう一度選んでもらうファイル。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UploadInput {
    pub paths: Vec<String>,
    pub entry: Option<String>,
}

/// `openapi_preview` の結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum PreviewResult {
    /// 確認画面に出すもの。
    Ready { preview: Box<ImportPreview> },
    /// 入口の候補が複数。選んで呼び直す（パスなら候補の絶対パスを `Path` に、アップロードなら `entry` に）。
    NeedsEntry { candidates: Vec<String> },
    /// 入口と別のホストを指す参照がある。確認したら `allowedHosts` に入れて呼び直す。
    NeedsConfirmation { foreign: Vec<ForeignRef> },
    /// 足りないファイル・壊れた yaml など。
    Blocked { problems: Vec<LoadProblem> },
}

/// 取り込みの指定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportRequest {
    pub source: SourceInput,
    /// `None` なら新しいプロジェクトを作る。あれば既存プロジェクトへ差分を反映する。
    pub project_id: Option<String>,
    /// 新しいプロジェクトの名前（なければ yaml の title）。
    pub project_name: Option<String>,
    /// 取り込む Operation（確認画面でチェックしたもの）。`None` は全部。
    pub selected: Option<Vec<OpKey>>,
    pub options: ImportOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub added: u32,
    pub changed: u32,
    pub removed: u32,
    /// 入力値が残っていて「削除された API」に移したもの。
    pub kept_as_deleted: u32,
    /// operationId の付け替えで入力値を引き継いだ数。
    pub renamed: u32,
}

impl From<ApplyReport> for ImportReport {
    fn from(r: ApplyReport) -> Self {
        Self {
            added: r.added,
            changed: r.changed,
            removed: r.removed,
            kept_as_deleted: r.kept_as_deleted,
            renamed: r.renamed,
        }
    }
}

/// `openapi_import` の結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum ImportResult {
    #[serde(rename_all = "camelCase")]
    Imported {
        project: Box<Project>,
        /// 取り込み済みの Operation の数。
        operation_count: u32,
        report: ImportReport,
        /// この取り込みで作った環境の名前（servers から）。
        created_environments: Vec<String>,
        /// 既存プロジェクトへの統合のときの差分。
        diff: Option<SyncDiff>,
    },
    NeedsEntry {
        candidates: Vec<String>,
    },
    NeedsConfirmation {
        foreign: Vec<ForeignRef>,
    },
    Blocked {
        problems: Vec<LoadProblem>,
    },
}

/// `openapi_sync_check` の結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum SyncCheckResult {
    /// 差分（空なら「変更はありません」）。`changedFiles` は中身（ハッシュ）が変わったファイル。
    #[serde(rename_all = "camelCase")]
    Ready {
        diff: SyncDiff,
        changed_files: Vec<String>,
    },
    /// コピー方式。もう一度ファイルを選んでもらう（`upload` に入れて呼び直す）。
    NeedsUpload {
        entry: String,
    },
    NeedsEntry {
        candidates: Vec<String>,
    },
    NeedsConfirmation {
        foreign: Vec<ForeignRef>,
    },
    /// yaml が壊れている・ファイルが足りない。同期は中止し、前回の Operation を使い続ける。
    Blocked {
        problems: Vec<LoadProblem>,
    },
}

/// `openapi_sync_apply` の結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum SyncApplyResult {
    #[serde(rename_all = "camelCase")]
    Applied {
        diff: SyncDiff,
        report: ImportReport,
        project: Box<Project>,
    },
    NeedsUpload {
        entry: String,
    },
    NeedsEntry {
        candidates: Vec<String>,
    },
    NeedsConfirmation {
        foreign: Vec<ForeignRef>,
    },
    Blocked {
        problems: Vec<LoadProblem>,
    },
}

// ---- 内部 ----

/// 取り込み（反映）の書き込みの指定。
struct ImportPlan {
    origin: Origin,
    /// 確認済みの別ホスト。
    hosts: Vec<String>,
    selection: Selection,
    options: ImportOptions,
    /// servers から環境を作る（取り込みのときだけ。同期では作らない）。
    create_environments: bool,
}

/// 差分に出す「新しく取り込まれる API」の範囲。
enum DiffScope<'a> {
    /// 確認画面。取り込んでいない API も、選べるので「追加」に出す。
    All,
    /// 同期。まだ保存にない API だけが「追加」（取り込んでいない API は出さない）。
    Sync,
    /// 既存プロジェクトへの取り込み。選んだ API だけが「追加」。
    Only(&'a HashSet<OpKey>),
}

/// 読み込めなかった理由（画面にそのまま返す）。
enum Gate {
    NeedsEntry(Vec<String>),
    NeedsConfirmation(Vec<ForeignRef>),
    Blocked(Vec<LoadProblem>),
}

/// どこから読んだか（取り込みで保存する形）。
enum Origin {
    Path {
        entry: PathBuf,
    },
    /// `token` は入力で渡されたもの（保存済みのトークンは持たない）。
    Url {
        entry: String,
        token: Option<String>,
    },
    /// 一時フォルダに置いたコピー。取り込みで `sources/<プロジェクト ID>/` へ移す。
    Upload {
        staged: TempDir,
        entry_rel: String,
    },
}

/// 一時フォルダ（使い終わったら消す）。
struct TempDir(Option<PathBuf>);

impl TempDir {
    fn path(&self) -> &Path {
        self.0.as_deref().unwrap_or(Path::new(""))
    }

    /// 消さずに取り出す。
    fn take(mut self) -> PathBuf {
        self.0.take().unwrap_or_default()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_dir_all(p);
        }
    }
}

struct Ready {
    loaded: Box<Loaded>,
    origin: Origin,
    /// 確認済みの別ホスト（入力と保存済みの合計）。
    allowed_hosts: Vec<String>,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn to_app(e: upload::UploadError) -> AppError {
    match e {
        upload::UploadError::Invalid(m) => AppError::Validation(m),
        upload::UploadError::Io(m) => AppError::Io(m),
    }
}

/// キーチェーンの「取り込み元のトークン」の置き場所（環境ではないので、環境 ID の場所に固定の名前を使う）。
fn token_key(project_id: &str) -> SecretKey {
    SecretKey::new(project_id, "source", "token")
}

fn merge_hosts(a: &[String], b: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for h in a.iter().chain(b) {
        if !out.iter().any(|x| x.eq_ignore_ascii_case(h)) {
            out.push(h.clone());
        }
    }
    out
}

impl AppState {
    fn sources_dir(&self) -> PathBuf {
        self.inner.data_dir.join("sources")
    }

    pub(crate) fn project_sources_dir(&self, project_id: &str) -> PathBuf {
        self.sources_dir().join(project_id)
    }

    fn new_temp_dir(&self) -> Result<TempDir, AppError> {
        let p = self
            .inner
            .data_dir
            .join("tmp")
            .join(format!("upload-{}", nekoreq_core::new_id()));
        std::fs::create_dir_all(&p)
            .map_err(|e| AppError::Io(format!("一時フォルダを作れませんでした: {e}")))?;
        Ok(TempDir(Some(p)))
    }

    /// 起動時に、前回の異常終了で残った一時フォルダを消す。
    pub(crate) fn clean_temp_dirs(&self) {
        let _ = std::fs::remove_dir_all(self.inner.data_dir.join("tmp"));
    }

    fn project_required(&self, id: &str) -> Result<Project, AppError> {
        self.db()
            .project_get(id)?
            .ok_or_else(|| AppError::NotFound(format!("プロジェクトが見つかりません: {id}")))
    }

    /// 指定を読み込む。止まる場合は `Gate`。
    async fn prepare(
        &self,
        input: &SourceInput,
        existing: Option<&Project>,
    ) -> Result<Result<Ready, Gate>, AppError> {
        let stored = existing.and_then(|p| p.source.as_ref());
        let input_hosts: &[String] = match input {
            SourceInput::Path { allowed_hosts, .. }
            | SourceInput::Url { allowed_hosts, .. }
            | SourceInput::Upload { allowed_hosts, .. } => allowed_hosts,
        };
        let hosts = merge_hosts(
            input_hosts,
            stored.map(|s| s.allowed_hosts.as_slice()).unwrap_or(&[]),
        );
        let (params, origin) = match input {
            SourceInput::Path { path, .. } => {
                let p = upload::expand_home(path.trim());
                let entry = if p.is_dir() {
                    let found = upload::scan_dir_for_entries(&p);
                    match found.len() {
                        0 => {
                            return Ok(Err(Gate::Blocked(vec![LoadProblem::NotOpenApi {
                                file: p.display().to_string(),
                                reason: "フォルダの中に `openapi:` を持つファイルがありません"
                                    .into(),
                            }])))
                        }
                        1 => found[0].clone(),
                        _ => {
                            return Ok(Err(Gate::NeedsEntry(
                                found.iter().map(|f| f.display().to_string()).collect(),
                            )))
                        }
                    }
                } else {
                    p
                };
                (
                    LoadParams {
                        entry: LoadEntry::Path(entry.clone()),
                        token: None,
                        allowed_hosts: hosts.clone(),
                        root: None,
                    },
                    Origin::Path { entry },
                )
            }
            SourceInput::Url { url, token, .. } => {
                // `user:pass@` は保存しない（トークンはトークン欄で）。クエリはそのまま使う
                nekoreq_openapi::load::check_entry_url(url).map_err(AppError::Validation)?;
                // 貼り付けで付いた前後の空白・改行は除く
                let input_token = token
                    .as_deref()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(str::to_string);
                // 入力のトークンがなければ、保存済みのもの（同期のとき）
                let used = match (&input_token, existing) {
                    (Some(t), _) => Some(t.clone()),
                    (None, Some(p)) if stored.is_some_and(|s| s.has_token) => {
                        self.secrets().get(&token_key(&p.id))?
                    }
                    _ => None,
                };
                (
                    LoadParams {
                        entry: LoadEntry::Url(url.trim().to_string()),
                        token: used,
                        allowed_hosts: hosts.clone(),
                        root: None,
                    },
                    Origin::Url {
                        entry: url.trim().to_string(),
                        token: input_token,
                    },
                )
            }
            SourceInput::Upload { paths, entry, .. } => {
                let tmp = self.new_temp_dir()?;
                let selected: Vec<PathBuf> = paths
                    .iter()
                    .map(|p| upload::expand_home(p.trim()))
                    .collect();
                let files = upload::stage(&selected, tmp.path()).map_err(to_app)?;
                let candidates = upload::entry_candidates(tmp.path(), &files);
                let entry_rel = match entry {
                    Some(e) => {
                        let e = e.replace('\\', "/");
                        if !files.contains(&e) {
                            return Err(AppError::Validation(format!(
                                "選んだファイルの中にありません: {e}"
                            )));
                        }
                        e
                    }
                    None => match candidates.len() {
                        0 => {
                            return Ok(Err(Gate::Blocked(vec![LoadProblem::NotOpenApi {
                                file: files.first().cloned().unwrap_or_default(),
                                reason: "`openapi:` を持つファイルがありません".into(),
                            }])))
                        }
                        1 => candidates[0].clone(),
                        _ => return Ok(Err(Gate::NeedsEntry(candidates))),
                    },
                };
                (
                    LoadParams {
                        entry: LoadEntry::Path(tmp.path().join(&entry_rel)),
                        token: None,
                        allowed_hosts: hosts.clone(),
                        // 置いたコピーの外（絶対パス・../・file://）のファイルは読ませない
                        root: Some(tmp.path().to_path_buf()),
                    },
                    Origin::Upload {
                        staged: tmp,
                        entry_rel,
                    },
                )
            }
        };
        match load(&params).await {
            LoadResult::Loaded(loaded) => Ok(Ok(Ready {
                loaded,
                origin,
                allowed_hosts: hosts,
            })),
            LoadResult::NeedsConfirmation(f) => Ok(Err(Gate::NeedsConfirmation(f))),
            LoadResult::Blocked(p) => Ok(Err(Gate::Blocked(p))),
        }
    }

    /// 保存済みの Operation と今回の Operation の差分。ツリーに出ている（取り込み済み・消えていない）API だけが
    /// 比べる対象。取り込んでいない API の変更・削除は出さず、`scope` に応じて新しく取り込まれるものだけを「追加」にする。
    fn diff_against_stored(
        &self,
        project_id: &str,
        new: &[Operation],
        scope: DiffScope<'_>,
    ) -> Result<SyncDiff, AppError> {
        let stored = self.db().operations_list(project_id)?;
        let drafts = self.db().draft_keys(project_id)?;
        let all_old: Vec<Operation> = stored.iter().map(|s| s.operation.clone()).collect();
        let m = match_operations(&all_old, new);
        let wanted = |key: &OpKey| match &scope {
            DiffScope::All | DiffScope::Sync => true,
            DiffScope::Only(set) => set.contains(key),
        };
        let mut keep = vec![false; new.len()];
        for &(i, j) in &m.pairs {
            // すでに取り込んでいるものは比べる。取り込んでいないものは、選ばれたときだけ（追加になる）
            keep[j] = stored[i].imported
                || matches!(scope, DiffScope::All)
                || matches!(&scope, DiffScope::Only(set) if set.contains(&new[j].op_key));
        }
        for &j in &m.added {
            keep[j] = wanted(&new[j].op_key);
        }
        let filtered: Vec<Operation> = new
            .iter()
            .zip(&keep)
            .filter(|(_, k)| **k)
            .map(|(o, _)| o.clone())
            .collect();
        let old: Vec<Operation> = stored
            .into_iter()
            .filter(|s| s.imported && s.deleted_at.is_none())
            .map(|s| s.operation)
            .collect();
        Ok(diff_operations(&old, &filtered, &drafts))
    }

    // ---- 確認 ----

    /// 取り込みの確認画面のデータを作る（何も保存しない）。`project_id` があれば、そのプロジェクトとの差分も付ける。
    pub async fn openapi_preview(
        &self,
        source: SourceInput,
        project_id: Option<String>,
    ) -> Result<PreviewResult, AppError> {
        let existing = project_id
            .as_deref()
            .map(|id| self.project_required(id))
            .transpose()?;
        let ready = match self.prepare(&source, existing.as_ref()).await? {
            Ok(r) => r,
            Err(Gate::NeedsEntry(candidates)) => {
                return Ok(PreviewResult::NeedsEntry { candidates })
            }
            Err(Gate::NeedsConfirmation(foreign)) => {
                return Ok(PreviewResult::NeedsConfirmation { foreign })
            }
            Err(Gate::Blocked(problems)) => return Ok(PreviewResult::Blocked { problems }),
        };
        let diff = match &existing {
            Some(p) => {
                Some(self.diff_against_stored(&p.id, &ready.loaded.operations, DiffScope::All)?)
            }
            None => None,
        };
        Ok(PreviewResult::Ready {
            preview: Box::new(build_preview(&ready.loaded, diff)),
        })
    }

    // ---- 取り込み ----

    /// 取り込む。新しいプロジェクト、または既存プロジェクトへの差分の反映。
    pub async fn openapi_import(&self, req: ImportRequest) -> Result<ImportResult, AppError> {
        let existing = req
            .project_id
            .as_deref()
            .map(|id| self.project_required(id))
            .transpose()?;
        let ready = match self.prepare(&req.source, existing.as_ref()).await? {
            Ok(r) => r,
            Err(Gate::NeedsEntry(candidates)) => {
                return Ok(ImportResult::NeedsEntry { candidates })
            }
            Err(Gate::NeedsConfirmation(foreign)) => {
                return Ok(ImportResult::NeedsConfirmation { foreign })
            }
            Err(Gate::Blocked(problems)) => return Ok(ImportResult::Blocked { problems }),
        };
        let Ready {
            loaded,
            origin,
            allowed_hosts,
        } = ready;

        let all_keys: HashSet<OpKey> = loaded.operations.iter().map(|o| o.op_key.clone()).collect();
        let selection = match &req.selected {
            Some(sel) => {
                let set: HashSet<OpKey> = sel
                    .iter()
                    .filter(|k| all_keys.contains(k))
                    .cloned()
                    .collect();
                if set.is_empty() && existing.is_none() {
                    return Err(AppError::Validation(
                        "取り込む API を 1 つ以上選んでください".into(),
                    ));
                }
                Selection::Only(set)
            }
            None => Selection::AllNew,
        };

        let diff = match &existing {
            Some(p) => {
                let scope = match &selection {
                    Selection::Only(set) => DiffScope::Only(set),
                    Selection::AllNew => DiffScope::Sync,
                };
                Some(self.diff_against_stored(&p.id, &loaded.operations, scope)?)
            }
            None => None,
        };

        let is_new = existing.is_none();
        let project = match existing {
            Some(p) => p,
            None => {
                let name = req
                    .project_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        if loaded.meta.title.trim().is_empty() {
                            "OpenAPI".to_string()
                        } else {
                            loaded.meta.title.chars().take(100).collect()
                        }
                    });
                self.db().project_create(&name)?
            }
        };

        let result = self.apply_import(
            &project,
            loaded.as_ref(),
            ImportPlan {
                origin,
                hosts: allowed_hosts,
                selection,
                options: req.options,
                create_environments: true,
            },
        );
        match result {
            Ok((report, created)) => {
                let count = self
                    .db()
                    .operations_list(&project.id)?
                    .iter()
                    .filter(|s| s.imported && s.deleted_at.is_none())
                    .count();
                Ok(ImportResult::Imported {
                    project: Box::new(self.project_required(&project.id)?),
                    operation_count: u32::try_from(count).unwrap_or(u32::MAX),
                    report: report.into(),
                    created_environments: created,
                    diff,
                })
            }
            Err(e) => {
                // 作りかけの新規プロジェクトは片づける
                if is_new {
                    let _ = self.project_delete(&project.id);
                }
                Err(e)
            }
        }
    }

    /// 取り込みの書き込み部分（コピー方式なら yaml 一式を置き、取り込み元・Operation・環境・トークンを保存する）。
    fn apply_import(
        &self,
        project: &Project,
        loaded: &Loaded,
        plan: ImportPlan,
    ) -> Result<(ApplyReport, Vec<String>), AppError> {
        let ImportPlan {
            origin,
            hosts,
            selection,
            options,
            create_environments,
        } = plan;
        let (kind, entry, token, staged) = match origin {
            Origin::Path { entry } => (SourceKind::Path, entry.display().to_string(), None, None),
            Origin::Url { entry, token } => (SourceKind::Url, entry, token, None),
            Origin::Upload { staged, entry_rel } => {
                (SourceKind::Copy, entry_rel, None, Some(staged))
            }
        };
        // コピー方式: 先に置き場所へ移す（DB の反映が失敗したら、新規プロジェクトごと片づく）
        if let Some(staged) = staged {
            let dest = self.project_sources_dir(&project.id);
            let _ = std::fs::remove_dir_all(&dest);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    AppError::Io(format!("sources フォルダを作れませんでした: {e}"))
                })?;
            }
            let src = staged.take();
            if std::fs::rename(&src, &dest).is_err() {
                copy_dir_all(&src, &dest)
                    .map_err(|e| AppError::Io(format!("ファイルを置けませんでした: {e}")))?;
                let _ = std::fs::remove_dir_all(&src);
            }
        }
        let had_token = project.source.as_ref().is_some_and(|s| s.has_token);
        let has_token = match (&token, kind) {
            (Some(t), SourceKind::Url) => {
                self.secrets().set(&token_key(&project.id), t)?;
                true
            }
            (None, SourceKind::Url) => had_token,
            _ => {
                if had_token {
                    let _ = self.secrets().delete(&token_key(&project.id));
                }
                false
            }
        };
        let source = ApiSource {
            id: String::new(),
            kind,
            entry,
            files: loaded.files.clone(),
            openapi_version: loaded.openapi_version.clone(),
            synced_at: Some(now()),
            has_token,
            allowed_hosts: hosts,
            options,
            meta: loaded.meta.clone(),
        };
        let report =
            self.db()
                .operations_apply(&project.id, &source, &loaded.operations, &selection)?;
        let created = if create_environments {
            self.db()
                .env_create_from_servers(&project.id, &loaded.meta.servers)?
                .into_iter()
                .map(|e| e.name)
                .collect()
        } else {
            vec![]
        };
        Ok((report, created))
    }

    // ---- 同期 ----

    /// 同期の入力を決める（コピー方式は再アップロードが要る）。
    fn sync_input(
        &self,
        project: &Project,
        upload: Option<UploadInput>,
    ) -> Result<Result<SourceInput, String>, AppError> {
        let source = project.source.as_ref().ok_or_else(|| {
            AppError::Validation("このプロジェクトには取り込み元がありません".into())
        })?;
        Ok(match source.kind {
            SourceKind::Path => Ok(SourceInput::Path {
                path: source.entry.clone(),
                allowed_hosts: source.allowed_hosts.clone(),
            }),
            SourceKind::Url => Ok(SourceInput::Url {
                url: source.entry.clone(),
                token: None,
                allowed_hosts: source.allowed_hosts.clone(),
            }),
            SourceKind::Copy => match upload {
                Some(u) => Ok(SourceInput::Upload {
                    paths: u.paths,
                    entry: u.entry,
                    allowed_hosts: source.allowed_hosts.clone(),
                }),
                None => Err(source.entry.clone()),
            },
        })
    }

    /// 同期の差分を確認する（反映はしない）。差分がなくても、更新日時・ハッシュは覚え直す。
    pub async fn openapi_sync_check(
        &self,
        project_id: &str,
        upload: Option<UploadInput>,
    ) -> Result<SyncCheckResult, AppError> {
        let project = self.project_required(project_id)?;
        let input = match self.sync_input(&project, upload)? {
            Ok(i) => i,
            Err(entry) => return Ok(SyncCheckResult::NeedsUpload { entry }),
        };
        let ready = match self.prepare(&input, Some(&project)).await? {
            Ok(r) => r,
            Err(Gate::NeedsEntry(candidates)) => {
                return Ok(SyncCheckResult::NeedsEntry { candidates })
            }
            Err(Gate::NeedsConfirmation(foreign)) => {
                return Ok(SyncCheckResult::NeedsConfirmation { foreign })
            }
            Err(Gate::Blocked(problems)) => return Ok(SyncCheckResult::Blocked { problems }),
        };
        let diff =
            self.diff_against_stored(project_id, &ready.loaded.operations, DiffScope::Sync)?;
        let before: &[SourceFile] = project
            .source
            .as_ref()
            .map(|s| s.files.as_slice())
            .unwrap_or(&[]);
        let changed_files = changed_files(before, &ready.loaded.files);
        // コピー方式は再アップロードなので覚え直さない（反映のときに置き直す）
        if diff.is_empty()
            && !matches!(ready.origin, Origin::Upload { .. })
            && before != ready.loaded.files.as_slice()
        {
            self.db()
                .source_touch_files(project_id, &ready.loaded.files)?;
        }
        Ok(SyncCheckResult::Ready {
            diff,
            changed_files,
        })
    }

    /// 同期を反映する。入力値（RequestDraft）は消さない。
    pub async fn openapi_sync_apply(
        &self,
        project_id: &str,
        upload: Option<UploadInput>,
    ) -> Result<SyncApplyResult, AppError> {
        let project = self.project_required(project_id)?;
        let input = match self.sync_input(&project, upload)? {
            Ok(i) => i,
            Err(entry) => return Ok(SyncApplyResult::NeedsUpload { entry }),
        };
        let ready = match self.prepare(&input, Some(&project)).await? {
            Ok(r) => r,
            Err(Gate::NeedsEntry(candidates)) => {
                return Ok(SyncApplyResult::NeedsEntry { candidates })
            }
            Err(Gate::NeedsConfirmation(foreign)) => {
                return Ok(SyncApplyResult::NeedsConfirmation { foreign })
            }
            Err(Gate::Blocked(problems)) => return Ok(SyncApplyResult::Blocked { problems }),
        };
        let Ready {
            loaded,
            origin,
            allowed_hosts,
        } = ready;
        let diff = self.diff_against_stored(project_id, &loaded.operations, DiffScope::Sync)?;
        let options = project
            .source
            .as_ref()
            .map(|s| s.options)
            .unwrap_or_default();
        // 同期では環境は足さない（消した環境が戻らないように）
        let (report, _) = self.apply_import(
            &project,
            &loaded,
            ImportPlan {
                origin,
                hosts: allowed_hosts,
                selection: Selection::AllNew,
                options,
                create_environments: false,
            },
        )?;
        Ok(SyncApplyResult::Applied {
            diff,
            report: report.into(),
            project: Box::new(self.project_required(project_id)?),
        })
    }

    /// 更新日時の確認（パス方式）。読み直さない。パス方式以外は常に「変わっていない」。
    pub fn openapi_source_changed(&self, project_id: &str) -> Result<SourceChanged, AppError> {
        let project = self.project_required(project_id)?;
        let Some(source) = &project.source else {
            return Ok(SourceChanged::default());
        };
        if source.kind != SourceKind::Path {
            return Ok(SourceChanged::default());
        }
        Ok(nekoreq_openapi::source_changed(
            Path::new(&source.entry),
            &source.files,
        ))
    }

    // ---- API ツリー・設計タブ・送信タブ ----

    pub fn operation_list(&self, project_id: &str) -> Result<Vec<OperationListItem>, AppError> {
        let project = self.project_required(project_id)?;
        let Some(source) = &project.source else {
            return Ok(vec![]);
        };
        let drafts = self.db().draft_keys(project_id)?;
        let stored = self.db().operations_list(project_id)?;
        let mut items: Vec<OperationListItem> = stored
            .iter()
            .filter(|s| s.imported)
            .map(|s| {
                nekoreq_openapi::list_item(
                    &s.operation,
                    &source.meta,
                    &source.options,
                    s.deleted_at.is_some(),
                    drafts.contains(&s.operation.op_key),
                )
            })
            .collect();
        // Operation の行が残っていない入力値（付け替えで残ったものなど）も「削除された API」として出す
        let known: HashSet<&OpKey> = stored.iter().map(|s| &s.operation.op_key).collect();
        for key in &drafts {
            if known.contains(key) {
                continue;
            }
            let (method, path) = match key {
                OpKey::MethodPath { method, path } => (method.clone(), path.clone()),
                OpKey::Id(id) => (String::new(), id.clone()),
            };
            items.push(OperationListItem {
                op_key: key.clone(),
                operation_id: matches!(key, OpKey::Id(_)).then(|| key.to_string()),
                method,
                path,
                summary: None,
                folder: None,
                tags: vec![],
                requires_auth: false,
                deleted: true,
                has_draft: true,
            });
        }
        Ok(items)
    }

    fn stored_operation(
        &self,
        project_id: &str,
        key: &OpKey,
    ) -> Result<(Project, nekoreq_storage::StoredOperation), AppError> {
        let project = self.project_required(project_id)?;
        let stored = self
            .db()
            .operation_get(project_id, key)?
            .ok_or_else(|| AppError::NotFound(format!("API が見つかりません: {key}")))?;
        Ok((project, stored))
    }

    /// 設計タブの内容。
    pub fn operation_get(
        &self,
        project_id: &str,
        key: &OpKey,
    ) -> Result<OperationDetail, AppError> {
        let (project, stored) = self.stored_operation(project_id, key)?;
        let source = project
            .source
            .ok_or_else(|| AppError::NotFound("取り込み元がありません".into()))?;
        Ok(nekoreq_openapi::describe_operation(
            &stored.operation,
            &source.meta,
            &source.options,
            source.synced_at.clone(),
            stored.deleted_at.is_some(),
        ))
    }

    /// 送信タブを開く。入力値（RequestDraft）があればそれを、なければ example・default から初期値を作る。
    pub fn operation_open(&self, project_id: &str, key: &OpKey) -> Result<OperationOpen, AppError> {
        let (project, stored) = self.stored_operation(project_id, key)?;
        let source = project
            .source
            .ok_or_else(|| AppError::NotFound("取り込み元がありません".into()))?;
        let draft = self.db().draft_get(project_id, key)?;
        let stale: Vec<ParamRef> = match &draft {
            Some(d) => self
                .db()
                .draft_stale_params(project_id, key)?
                .into_iter()
                // 画面で消した行は出さない
                .filter(|s| {
                    d.params
                        .iter()
                        .any(|p| p.location == s.location && p.name == s.name)
                })
                .map(|s| ParamRef {
                    location: s.location,
                    name: s.name,
                })
                .collect(),
            None => vec![],
        };
        Ok(nekoreq_openapi::open_operation(
            &stored.operation,
            &source.meta,
            &source.options,
            draft.as_ref(),
            &stale,
            stored.deleted_at.is_some(),
        ))
    }

    /// 「削除された API」を自分で消す（入力値も消える）。
    pub fn operation_forget(&self, project_id: &str, key: &OpKey) -> Result<(), AppError> {
        Ok(self.db().operation_forget(project_id, key)?)
    }

    // ---- トークン・エディタ ----

    /// URL 方式のアクセストークンをキーチェーンに保存する（空なら消す）。
    pub fn source_token_set(&self, project_id: &str, token: &str) -> Result<Project, AppError> {
        let project = self.project_required(project_id)?;
        let source = project.source.as_ref().ok_or_else(|| {
            AppError::Validation("このプロジェクトには取り込み元がありません".into())
        })?;
        if source.kind != SourceKind::Url {
            return Err(AppError::Validation(
                "アクセストークンは URL 方式の取り込み元だけに使います".into(),
            ));
        }
        let token = token.trim();
        if token.is_empty() {
            self.secrets().delete(&token_key(project_id))?;
            self.db().source_set_has_token(project_id, false)?;
        } else {
            self.secrets().set(&token_key(project_id), token)?;
            self.db().source_set_has_token(project_id, true)?;
        }
        self.project_required(project_id)
    }

    /// エディタで開くファイルの場所。取り込んだファイルの一覧にあるものだけ（画面から任意のパスは開けない）。
    pub fn editor_target(&self, project_id: &str, file: &str) -> Result<PathBuf, AppError> {
        let project = self.project_required(project_id)?;
        let source = project
            .source
            .ok_or_else(|| AppError::NotFound("取り込み元がありません".into()))?;
        if !source.files.iter().any(|f| f.path == file) {
            return Err(AppError::Validation(format!(
                "取り込んだファイルにありません: {file}"
            )));
        }
        let base = match source.kind {
            SourceKind::Path => Path::new(&source.entry)
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
            SourceKind::Copy => {
                let entry = self.project_sources_dir(project_id).join(&source.entry);
                entry.parent().map(Path::to_path_buf).unwrap_or_default()
            }
            SourceKind::Url => {
                return Err(AppError::Validation(
                    "URL 方式の yaml はエディタで開けません（ブラウザで開いてください）".into(),
                ))
            }
        };
        let path = nekoreq_openapi::doc::normalize_path(&base.join(file));
        if !path.exists() {
            return Err(AppError::NotFound(format!(
                "ファイルがありません: {}",
                path.display()
            )));
        }
        Ok(path)
    }
}

/// 中身（ハッシュ）が変わった・増えた・消えたファイル。
fn changed_files(before: &[SourceFile], after: &[SourceFile]) -> Vec<String> {
    let mut out = Vec::new();
    for a in after {
        match before.iter().find(|b| b.path == a.path) {
            Some(b) if b.hash == a.hash => {}
            _ => out.push(a.path.clone()),
        }
    }
    for b in before {
        if !after.iter().any(|a| a.path == b.path) {
            out.push(b.path.clone());
        }
    }
    out
}

fn copy_dir_all(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let to = dest.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir_all(&e.path(), &to)?;
        } else {
            std::fs::copy(e.path(), to)?;
        }
    }
    Ok(())
}
