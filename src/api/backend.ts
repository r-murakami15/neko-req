import type {
  AdHocRequest,
  AppError,
  AppInfo,
  Environment,
  Project,
  RequestFinished,
  RequestInput,
  RequestPreview,
  Result,
  SendSettings,
} from "../bindings";

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

  /** 送信を始めて送信 ID を返す。結果は `onRequestFinished` で届く（戻り値より先に届くことがある）。 */
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
