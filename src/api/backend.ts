import type {
  AdHocRequest,
  AppError,
  AppInfo,
  AppSettings,
  CurlExportMode,
  CurlImport,
  Environment,
  HistoryDiff,
  HistoryEntry,
  HistoryOpen,
  HistoryPage,
  HistoryQuery,
  ImportRequest,
  ImportResult,
  OpKey,
  OperationDetail,
  OperationListItem,
  OperationOpen,
  PreviewResult,
  Project,
  ProjectSettings,
  RequestDraft,
  RequestFinished,
  RequestInput,
  RequestPreview,
  Result,
  SavedInput,
  SendSettings,
  SessionState,
  SourceChanged,
  SourceInput,
  SyncApplyResult,
  SyncCheckResult,
  UpdateInfo,
  UpdateProgress,
  UploadInput,
} from "../bindings";

/** ウィンドウへのファイルのドラッグ＆ドロップ。`drop` のときだけ絶対パスが届く。 */
export type FileDropEvent = { type: "over" } | { type: "leave" } | { type: "drop"; paths: string[] };

/** 取り込みダイアログで選ぶものの種類。 */
export type PickKind = "openapi-file" | "folder" | "files";

/** 設定画面で選ぶ証明書ファイルの種類。 */
export type CertKind = "ca" | "client";

/**
 * 画面が Rust（または Tauri の外ではモック）を呼ぶための窓口。
 * 画面のコードは bindings.ts を直接呼ばず、必ずこの型を通す。
 */
export interface Backend {
  appInfo(): Promise<AppInfo>;
  projectList(): Promise<Project[]>;
  projectCreate(name: string): Promise<Project>;
  projectRename(id: string, name: string): Promise<Project>;
  projectDelete(id: string): Promise<void>;

  /**
   * 送信を始めて送信 ID を返す。結果は `onRequestFinished` で届く（戻り値より先に届くことがある）。
   * `settings` が `null` なら、設定画面で保存した値（タイムアウト・リダイレクト・証明書の検証）を使う。
   */
  requestSend(input: RequestInput, settings: SendSettings | null): Promise<string>;
  requestCancel(sendId: string): Promise<boolean>;
  requestPreview(input: RequestInput): Promise<RequestPreview>;
  /** `request://finished` の購読。解除する関数を返す。送信の前に登録しておくこと。 */
  onRequestFinished(handler: (event: RequestFinished) => void): Promise<() => void>;
  responseSaveBody(sendId: string, path: string): Promise<void>;

  /** 保存ダイアログ。選ばなければ `null`。 */
  pickSavePath(defaultName: string): Promise<string | null>;
  /** ファイルを選ぶダイアログ。選ばなければ `null`。 */
  pickFile(): Promise<string | null>;

  envList(projectId: string): Promise<Environment[]>;
  envSave(environment: Environment): Promise<Environment>;
  envCreate(projectId: string, name: string): Promise<Environment>;
  envDuplicate(id: string): Promise<Environment>;
  envDelete(id: string): Promise<void>;
  envSelect(projectId: string, environmentId: string | null): Promise<Project>;
  secretSet(projectId: string, environmentId: string, name: string, value: string): Promise<void>;
  secretDelete(projectId: string, environmentId: string, name: string): Promise<void>;
  secretReveal(projectId: string, environmentId: string, name: string): Promise<string | null>;

  adhocList(projectId: string): Promise<AdHocRequest[]>;
  adhocSave(request: AdHocRequest): Promise<AdHocRequest>;
  adhocDelete(id: string): Promise<void>;

  // ---- OpenAPI の取り込みと同期（M3・M4） ----
  openapiPreview(source: SourceInput, projectId: string | null): Promise<PreviewResult>;
  openapiImport(request: ImportRequest): Promise<ImportResult>;
  /** `allowedHosts`: 別のホストの参照を読んでよいと確認した origin（`needsConfirmation` の `foreign[].host`）。 */
  openapiSyncCheck(projectId: string, upload: UploadInput | null, allowedHosts?: string[] | null): Promise<SyncCheckResult>;
  openapiSyncApply(projectId: string, upload: UploadInput | null, allowedHosts?: string[] | null): Promise<SyncApplyResult>;
  /** パス方式: 全ファイルの更新日時だけを見る（読み直さない）。 */
  openapiSourceChanged(projectId: string): Promise<SourceChanged>;
  operationList(projectId: string): Promise<OperationListItem[]>;
  operationGet(projectId: string, opKey: OpKey): Promise<OperationDetail>;
  operationOpen(projectId: string, opKey: OpKey): Promise<OperationOpen>;
  operationForget(projectId: string, opKey: OpKey): Promise<void>;
  sourceTokenSet(projectId: string, token: string): Promise<Project>;
  editorOpen(projectId: string, file: string, line: number | null): Promise<void>;
  draftSave(draft: RequestDraft): Promise<RequestDraft>;
  draftGet(projectId: string, opKey: OpKey): Promise<RequestDraft | null>;

  // ---- 認証の自動化・設定（M5・M6） ----
  projectSettingsSet(projectId: string, settings: ProjectSettings): Promise<Project>;
  appSettingsGet(): Promise<AppSettings>;
  appSettingsSet(settings: AppSettings): Promise<AppSettings>;
  /** プロキシのパスワード（キーチェーン）。空で消す。返り値は `hasPassword` が変わった設定。 */
  proxyPasswordSet(password: string): Promise<AppSettings>;
  /** クライアント証明書のパスワード（キーチェーン）。先に `appSettingsSet` で証明書を入れて `id` を決めておく。 */
  clientCertPasswordSet(id: string, password: string): Promise<AppSettings>;
  /** 証明書ファイルを選ぶダイアログ。選ばなければ `null`。 */
  pickCertFile(kind: CertKind): Promise<string | null>;

  // ---- 履歴・cURL・タブの復元（M6） ----
  historyList(projectId: string, query: HistoryQuery): Promise<HistoryPage>;
  historyGet(id: string): Promise<HistoryEntry>;
  historyDelete(id: string): Promise<void>;
  /** プロジェクトの履歴をすべて消す。消した件数を返す。 */
  historyClear(projectId: string): Promise<number>;
  historyOpen(id: string): Promise<HistoryOpen>;
  /** 元の入力を今の環境で送り直す。送信 ID を返し、結果は `onRequestFinished` で届く。 */
  historyResend(id: string, settings: SendSettings | null): Promise<string>;
  historySaveAsInput(id: string, name: string | null): Promise<SavedInput>;
  historyCurl(id: string): Promise<string>;
  historyCompare(leftId: string, rightId: string): Promise<HistoryDiff>;
  curlParse(projectId: string, text: string, environmentId: string | null): Promise<CurlImport>;
  curlExport(input: RequestInput, mode: CurlExportMode): Promise<string>;
  sessionSave(projectId: string, session: SessionState): Promise<SessionState>;
  sessionLoad(projectId: string): Promise<SessionState | null>;

  // ---- 自動更新（M7） ----
  updateCheck(): Promise<UpdateInfo | null>;
  updatePending(): Promise<UpdateInfo | null>;
  updateInstall(): Promise<void>;
  updateRestart(): Promise<void>;
  /** `update://available` の購読。解除する関数を返す。 */
  onUpdateAvailable(handler: (info: UpdateInfo) => void): Promise<() => void>;
  /** `update://progress` の購読。解除する関数を返す。 */
  onUpdateProgress(handler: (progress: UpdateProgress) => void): Promise<() => void>;

  /** 取り込み元を選ぶダイアログ。選ばなければ空の配列。 */
  pickPaths(kind: PickKind): Promise<string[]>;
  /** ウィンドウへのドロップの購読。解除する関数を返す。 */
  onFileDrop(handler: (event: FileDropEvent) => void): Promise<() => void>;
  /** http(s) の URL を OS の既定のブラウザで開く。 */
  openExternal(url: string): Promise<void>;
}

/** Rust 側が返した `AppError`（`{ kind, message }`）を包んだ例外。`message` は日本語でそのまま表示できる。 */
export class ApiError extends Error {
  readonly error: AppError;

  constructor(error: AppError) {
    super(error.message);
    this.name = "ApiError";
    this.error = error;
  }

  get kind(): AppError["kind"] {
    return this.error.kind;
  }
}

/** bindings.ts の `Result` を、成功なら値、失敗なら `ApiError` を投げる形にする。 */
export async function unwrap<T>(result: Promise<Result<T, AppError>>): Promise<T> {
  const r = await result;
  if (r.status === "ok") return r.data;
  throw new ApiError(r.error);
}

/** 画面に出せるエラー文言にする。 */
export function errorMessage(e: unknown): string {
  if (e instanceof ApiError) return e.message;
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  return "予期しないエラーが起きました";
}
