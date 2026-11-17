import type {
  DefaultAuth,
  ErrorCodeEntry,
  JsonValue,
  MediaDoc,
  OpKey,
  OperationDetail,
  ParamDoc,
  ParamIn,
  ResponseDoc,
  SchemaRow,
} from "../bindings";

/**
 * ブラウザ確認用の小さな OpenAPI 仕様（モック）。タグ 4 つ・11 件。cookieAuth・`security: []`・
 * Markdown の表を含む description・循環参照のスキーマ・（同期で）削除される API を含む。
 * 版 1 が最初に取り込まれる内容、版 2 は「yaml を更新したあと」の内容。
 */

export interface Schema {
  type?: string;
  format?: string;
  nullable?: boolean;
  enum?: string[];
  description?: string;
  example?: JsonValue;
  default?: JsonValue;
  minLength?: number;
  maxLength?: number;
  minimum?: number;
  maximum?: number;
  properties?: Record<string, Schema>;
  required?: string[];
  items?: Schema;
  "x-nekoreq-ref"?: string;
  "x-nekoreq-circular"?: boolean;
}

export interface MockParam {
  name: string;
  in: ParamIn;
  required: boolean;
  description?: string;
  example?: JsonValue;
  default?: JsonValue;
  schema: Schema;
}

export interface MockResponse {
  status: string;
  description: string;
  contentType?: string;
  schema?: Schema;
  example?: JsonValue;
  /** description の「返すエラー」の表から読んだ code。 */
  codes?: { code: string; note: string }[];
}

export interface MockOp {
  operationId: string | null;
  method: string;
  path: string;
  tag: string | null;
  summary: string;
  description: string | null;
  parameters: MockParam[];
  body?: { required: boolean; description?: string; contentType: string; schema: Schema; example?: JsonValue; examples?: { name: string; summary?: string; value: JsonValue }[] };
  responses: MockResponse[];
  /** `none` は `security: []`、`default` は全体（cookieAuth）に従う。 */
  security: "default" | "none";
  file: string;
  line: number;
}

export const mockOpKey = (op: Pick<MockOp, "operationId" | "method" | "path">): OpKey =>
  op.operationId ? { kind: "id", value: op.operationId } : { kind: "methodPath", value: { method: op.method, path: op.path } };

/** 差分を出すための、書かれた場所を含まない内容のハッシュ（の代わりの文字列）。 */
export function mockHash(op: MockOp): string {
  return JSON.stringify([op.operationId, op.summary, op.description, op.parameters, op.body, op.responses, op.security, op.tag]);
}

// ---- スキーマの部品 ----

const str = (o: Schema = {}): Schema => ({ type: "string", ...o });
const int = (o: Schema = {}): Schema => ({ type: "integer", format: "int32", ...o });
const obj = (properties: Record<string, Schema>, required: string[] = [], o: Schema = {}): Schema => ({
  type: "object",
  properties,
  required,
  ...o,
});
const arr = (items: Schema, o: Schema = {}): Schema => ({ type: "array", items, ...o });
const ref = (name: string, schema: Schema): Schema => ({ ...schema, "x-nekoreq-ref": name });
const circular = (name: string, type: "object" | "array" = "object"): Schema =>
  type === "array"
    ? { type: "array", items: { type: "object", "x-nekoreq-ref": name, "x-nekoreq-circular": true } }
    : { type: "object", "x-nekoreq-ref": name, "x-nekoreq-circular": true };

const PROBLEM = ref(
  "ApiProblem",
  obj(
    {
      title: str({ description: "エラーの概要（利用者向け。code ごとに固定）" }),
      status: int({ description: "HTTP ステータス" }),
      detail: str({ nullable: true, description: "エラーの詳細（利用者向け）。ないときは null" }),
      code: ref("ErrorCode", str({ enum: ["VALIDATION_FAILED", "LOGIN_FAILED", "UNAUTHENTICATED", "FORBIDDEN", "NOT_FOUND", "INTERNAL_ERROR"], description: "機械判定用のコード" })),
      traceId: str({ description: "ログと突き合わせる ID（32 桁の 16 進数）" }),
    },
    ["title", "status", "code", "traceId"],
    { description: "エラーレスポンス（RFC 9457 Problem Details）" },
  ),
);

const problem = (status: string, description: string, codes: { code: string; note: string }[] = []): MockResponse => ({
  status,
  description,
  contentType: "application/problem+json",
  schema: PROBLEM,
  example: undefined,
  codes,
});

const CLIENT_ITEM = ref(
  "ClientListItem",
  obj(
    {
      clientId: int({ description: "取引先 ID", example: 1 }),
      clientName: str({ description: "取引先名", maxLength: 200, example: "Netflix（Netflix Services Japan合同会社）" }),
      contractType: ref("ContractType", str({ enum: ["DIRECT", "AGENCY"], description: "契約種別" })),
      status: ref("ClientStatus", str({ enum: ["ACTIVE", "INACTIVE"], description: "取引先の状態", default: "ACTIVE" })),
    },
    ["clientId", "clientName", "contractType", "status"],
  ),
);

const WORK = ref(
  "Work",
  obj(
    {
      workId: int({ description: "作品 ID", example: 1001 }),
      workTitle: str({ description: "作品名", example: "ちびまる子ちゃん" }),
      parent: { ...circular("Work"), description: "シリーズの親作品" },
      children: { ...circular("Work", "array"), description: "シリーズの子作品" },
    },
    ["workId", "workTitle"],
  ),
);

const TABLE_NOTE = (rows: string) => `| status | code | 状況 |\n|---|---|---|\n${rows}`;

// ---- 版 1 の API ----

function baseOps(): MockOp[] {
  return [
    {
      operationId: "getSession",
      method: "GET",
      path: "/api/v1/auth/session",
      tag: "auth",
      summary: "ログイン状態取得",
      description: "ログイン中のユーザーの情報を返す。未ログインなら 401 (UNAUTHENTICATED)。",
      parameters: [],
      responses: [
        {
          status: "200",
          description: "ログイン中",
          contentType: "application/json",
          schema: ref(
            "SessionApiResponse",
            obj({ data: ref("Session", obj({ userId: int({ example: 7 }), email: str({ format: "email", example: "user@example.com" }) }, ["userId", "email"])) }, ["data"]),
          ),
        },
        problem("401", "未ログイン・セッション切れ", [{ code: "UNAUTHENTICATED", note: "ログインが必要" }]),
      ],
      security: "default",
      file: "auth.yaml",
      line: 5,
    },
    {
      operationId: "deleteSession",
      method: "DELETE",
      path: "/api/v1/auth/session",
      tag: "auth",
      summary: "ログアウト",
      description: "セッションを破棄する。",
      parameters: [],
      responses: [{ status: "204", description: "ログアウトした" }, problem("401", "未ログイン", [{ code: "UNAUTHENTICATED", note: "" }])],
      security: "default",
      file: "auth.yaml",
      line: 28,
    },
    {
      operationId: "createPasswordLogin",
      method: "POST",
      path: "/api/v1/auth/password-login",
      tag: "auth",
      summary: "パスワードログイン (1 段階目)",
      description: [
        "メールアドレスとパスワードを確かめ、合っていれば 6 桁の認証コードをメールで送る。ログインはまだ完了しない (2 段階目は `createEmailCodeLogin`)。",
        "",
        "- コードの有効期限は 10 分、入力できるのは 5 回まで。前に送ったコードは使えなくなる",
        "- パスワードを 5 回続けて間違えると 15 分ロックする",
        "- 送ったコードはログに出さない",
        "",
        "### 画面に出すメッセージ",
        "",
        "| 状況 | メッセージ | 返し方 |",
        "|---|---|---|",
        "| ログインできない (理由は分けない) | メールアドレスまたはパスワードが正しくありません | 401 (LOGIN_FAILED) |",
        "",
        "### 返すエラー",
        "",
        TABLE_NOTE("| 401 | LOGIN_FAILED | ログインの失敗 |\n| 400 | VALIDATION_FAILED | 入力チェックエラー |"),
      ].join("\n"),
      parameters: [],
      body: {
        required: true,
        contentType: "application/json",
        schema: ref(
          "PasswordLoginRequest",
          obj(
            {
              email: str({ format: "email", minLength: 1, maxLength: 254, description: "メールアドレス" }),
              password: str({ format: "password", minLength: 1, maxLength: 72, description: "パスワード (ログに出さない)" }),
            },
            ["email", "password"],
          ),
        ),
      },
      responses: [
        {
          status: "200",
          description: "認証コードを送った",
          contentType: "application/json",
          schema: ref(
            "EmailCodeSentApiResponse",
            obj(
              {
                data: ref(
                  "EmailCodeSent",
                  obj(
                    {
                      expiresAt: str({ format: "date-time", example: "2026-11-11T10:10:00+09:00", description: "コードの有効期限" }),
                      resendAvailableAt: str({ format: "date-time", example: "2026-11-11T10:01:00+09:00", description: "再送できるようになる日時" }),
                    },
                    ["expiresAt", "resendAvailableAt"],
                  ),
                ),
              },
              ["data"],
            ),
          ),
        },
        problem("400", "入力チェックエラー (VALIDATION_FAILED)。画面に出す文言を detail に返す", [{ code: "VALIDATION_FAILED", note: "errors に項目ごとのエラーが入る" }]),
        problem("401", "ログインの失敗 (LOGIN_FAILED。理由は分けない)", [{ code: "LOGIN_FAILED", note: "画面には「メールアドレスまたはパスワードが正しくありません」" }]),
        problem("500", "想定外のエラー (INTERNAL_ERROR)", [{ code: "INTERNAL_ERROR", note: "画面には固定の文言と traceId" }]),
      ],
      security: "none",
      file: "auth.yaml",
      line: 52,
    },
    {
      operationId: "createEmailCodeLogin",
      method: "POST",
      path: "/api/v1/auth/email-code-login",
      tag: "auth",
      summary: "メール認証コードログイン (2 段階目)",
      description: "メールで届いた 6 桁のコードを確かめ、合っていればログインを完了する。`SESSION` Cookie を発行する。",
      parameters: [],
      body: {
        required: true,
        contentType: "application/json",
        schema: ref("EmailCodeLoginRequest", obj({ email: str({ format: "email" }), code: str({ minLength: 6, maxLength: 6, description: "6 桁の認証コード" }) }, ["email", "code"])),
        examples: [
          { name: "正しいコード", summary: "メールで届いた 6 桁のコード", value: { email: "user@example.com", code: "123456" } },
          { name: "期限切れのコード", summary: "LOGIN_FAILED になる", value: { email: "user@example.com", code: "000000" } },
        ],
      },
      responses: [{ status: "204", description: "ログインした (Set-Cookie: SESSION)" }, problem("401", "コードが違う・期限切れ", [{ code: "LOGIN_FAILED", note: "" }])],
      security: "none",
      file: "auth.yaml",
      line: 96,
    },
    {
      operationId: "listClients",
      method: "GET",
      path: "/api/v1/video/clients",
      tag: "video-client",
      summary: "取引先一覧取得",
      description: [
        "取引先の一覧を返す。",
        "",
        "| 状態 | 説明 |",
        "|---|---|",
        "| ACTIVE | 取引中 (省略時) |",
        "| INACTIVE | 取引停止 |",
      ].join("\n"),
      parameters: [
        { name: "q", in: "query", required: false, description: "取引先名の部分一致", example: "Netflix", schema: str() },
        { name: "contractType", in: "query", required: false, description: "契約種別で絞り込む（DIRECT / AGENCY）", example: "DIRECT", schema: str({ enum: ["DIRECT", "AGENCY"] }) },
        { name: "status", in: "query", required: false, description: "取引先の状態で絞り込む (省略時は ACTIVE)", default: "ACTIVE", schema: str({ enum: ["ACTIVE", "INACTIVE"], default: "ACTIVE" }) },
        { name: "perPage", in: "query", required: true, description: "1 ページの件数 (最大 100)", default: 50, schema: int({ minimum: 1, maximum: 100, default: 50 }) },
      ],
      responses: [
        {
          status: "200",
          description: "取引先の一覧",
          contentType: "application/json",
          schema: ref("ClientListApiResponse", obj({ data: obj({ items: arr(CLIENT_ITEM), total: int({ description: "全件数", example: 1 }) }, ["items", "total"]) }, ["data"])),
        },
        problem("401", "未ログイン", [{ code: "UNAUTHENTICATED", note: "" }]),
      ],
      security: "default",
      file: "video/client.yaml",
      line: 5,
    },
    {
      operationId: "createClient",
      method: "POST",
      path: "/api/v1/video/clients",
      tag: "video-client",
      summary: "取引先登録",
      description: "取引先を登録する。CSRF トークン (`X-XSRF-TOKEN`) が要る。",
      parameters: [],
      body: {
        required: true,
        contentType: "application/json",
        schema: ref(
          "ClientCreateRequest",
          obj(
            {
              clientName: str({ minLength: 1, maxLength: 200, description: "取引先名", example: "テスト取引先" }),
              contractType: ref("ContractType", str({ enum: ["DIRECT", "AGENCY"], description: "契約種別" })),
              contact: obj({ email: str({ format: "email", nullable: true }), phone: str({ nullable: true }) }, [], { description: "連絡先" }),
            },
            ["clientName", "contractType"],
          ),
        ),
        example: { clientName: "テスト取引先", contractType: "DIRECT", contact: { email: "test@example.com", phone: null } },
      },
      responses: [{ status: "201", description: "登録した" }, problem("400", "入力チェックエラー", [{ code: "VALIDATION_FAILED", note: "" }]), problem("403", "CSRF トークンが合わない", [{ code: "FORBIDDEN", note: "" }])],
      security: "default",
      file: "video/client.yaml",
      line: 62,
    },
    {
      operationId: "deleteClient",
      method: "DELETE",
      path: "/api/v1/video/clients/{clientId}",
      tag: "video-client",
      summary: "取引先削除",
      description: "取引先を削除する。契約のある取引先は削除できない。",
      parameters: [{ name: "clientId", in: "path", required: true, description: "取引先 ID", example: 1, schema: int({ minimum: 1 }) }],
      responses: [{ status: "204", description: "削除した" }, problem("404", "取引先が見つからない", [{ code: "NOT_FOUND", note: "" }])],
      security: "default",
      file: "video/client.yaml",
      line: 118,
    },
    {
      operationId: "getWork",
      method: "GET",
      path: "/api/v1/video/works/{workId}",
      tag: "video-work",
      summary: "作品取得",
      description: "作品を 1 件返す。シリーズの親子関係（`parent` と `children`）は循環した参照になる。",
      parameters: [{ name: "workId", in: "path", required: true, description: "作品 ID", example: 1001, schema: int() }],
      responses: [{ status: "200", description: "作品", contentType: "application/json", schema: ref("WorkApiResponse", obj({ data: WORK }, ["data"])) }, problem("404", "作品が見つからない", [{ code: "NOT_FOUND", note: "" }])],
      security: "default",
      file: "video/work.yaml",
      line: 5,
    },
    {
      operationId: "updateWork",
      method: "PUT",
      path: "/api/v1/video/works/{workId}",
      tag: "video-work",
      summary: "作品更新",
      description: null,
      parameters: [{ name: "workId", in: "path", required: true, description: "作品 ID", example: 1001, schema: int() }],
      body: { required: true, contentType: "application/json", schema: ref("WorkUpdateRequest", obj({ workTitle: str({ minLength: 1, description: "作品名" }) }, ["workTitle"])) },
      responses: [{ status: "204", description: "更新した" }],
      security: "default",
      file: "video/work.yaml",
      line: 49,
    },
    {
      operationId: null,
      method: "POST",
      path: "/api/v1/files",
      tag: null,
      summary: "ファイルアップロード",
      description: "operationId のない API の例。`POST /api/v1/files` をキーにする。",
      parameters: [{ name: "X-Request-Id", in: "header", required: false, description: "ログと突き合わせる ID", schema: str() }],
      body: {
        required: true,
        contentType: "multipart/form-data",
        schema: obj({ file: str({ format: "binary", description: "アップロードするファイル" }), note: str({ description: "メモ" }) }, ["file"]),
      },
      responses: [{ status: "201", description: "保存した" }],
      security: "default",
      file: "file.yaml",
      line: 5,
    },
    {
      operationId: "legacyPing",
      method: "GET",
      path: "/api/v1/legacy/ping",
      tag: "legacy",
      summary: "旧ヘルスチェック",
      description: "旧システムとの疎通確認（廃止予定）。",
      parameters: [],
      responses: [{ status: "200", description: "pong", contentType: "text/plain" }],
      security: "none",
      file: "legacy.yaml",
      line: 5,
    },
  ];
}

/** 版 2 = yaml を更新したあと: 一覧の説明とパラメータが変わり、取引先の詳細が増え、旧ヘルスチェックが消える。 */
function updatedOps(): MockOp[] {
  const ops = baseOps()
    .filter((o) => o.operationId !== "legacyPing")
    .map((o) => {
      if (o.operationId !== "listClients") return o;
      return {
        ...o,
        description: `${o.description}\n\n（v2）契約種別の絞り込みは廃止した。`,
        parameters: o.parameters.filter((p) => p.name !== "contractType"),
      };
    });
  const at = ops.findIndex((o) => o.operationId === "deleteClient");
  ops.splice(at, 0, {
    operationId: "getClient",
    method: "GET",
    path: "/api/v1/video/clients/{clientId}",
    tag: "video-client",
    summary: "取引先詳細取得",
    description: "取引先を 1 件返す。",
    parameters: [{ name: "clientId", in: "path", required: true, description: "取引先 ID", example: 1, schema: int({ minimum: 1 }) }],
    responses: [{ status: "200", description: "取引先", contentType: "application/json", schema: ref("ClientApiResponse", obj({ data: CLIENT_ITEM }, ["data"])) }],
    security: "default",
    file: "video/client.yaml",
    line: 96,
  });
  return ops;
}

export function mockSpecOps(version: 1 | 2): MockOp[] {
  return version === 1 ? baseOps() : updatedOps();
}

export const MOCK_SPEC_INFO = {
  title: "ミニ契約管理 API",
  version: "0.1.0",
  openapiVersion: "3.0.3",
  description: "ブラウザ確認用の小さな OpenAPI 仕様（モック）",
  servers: [{ url: "http://localhost:8080", description: "ローカル (Dev Container)" }],
  defaultAuth: ["cookieAuth（Cookie: SESSION）"],
  tagDescriptions: { auth: "認証", "video-client": "取引先", "video-work": "作品", legacy: "旧システム" } as Record<string, string>,
};

export const MOCK_DEFAULT_AUTH: Extract<DefaultAuth, { type: "apiKey" }> = { type: "apiKey", scheme: "cookieAuth", location: "cookie", name: "SESSION", variable: "session" };

// ---- 表示用への変換 ----

function constraintsOf(s: Schema): string[] {
  const c: string[] = [];
  if (s.nullable) c.push("nullable");
  if (s.enum) c.push(`enum: ${s.enum.join(", ")}`);
  if (s.format) c.push(`format: ${s.format}`);
  if (s.minimum !== undefined) c.push(`min: ${s.minimum}`);
  if (s.maximum !== undefined) c.push(`max: ${s.maximum}`);
  if (s.minLength !== undefined) c.push(`minLength: ${s.minLength}`);
  if (s.maxLength !== undefined) c.push(`maxLength: ${s.maxLength}`);
  if (s.default !== undefined) c.push(`default: ${String(s.default)}`);
  return c;
}

function typeLabelOf(s: Schema): string {
  if (s.type === "array") return `${s.items ? typeLabelOf(s.items) : "object"}[]`;
  return s["x-nekoreq-ref"] ?? s.type ?? "object";
}

function pushRows(name: string, s: Schema, required: boolean, depth: number, out: SchemaRow[]) {
  const isCircular = s["x-nekoreq-circular"] === true || (s.type === "array" && s.items?.["x-nekoreq-circular"] === true);
  const constraints = constraintsOf(s);
  if (isCircular) constraints.push("循環");
  out.push({
    depth,
    name,
    typeLabel: typeLabelOf(s),
    baseType: s.type ?? "object",
    required,
    nullable: s.nullable === true,
    constraints,
    description: s.description ?? null,
    example: s.example !== undefined ? String(typeof s.example === "object" ? JSON.stringify(s.example) : s.example) : null,
    refName: s["x-nekoreq-ref"] ?? s.items?.["x-nekoreq-ref"] ?? null,
    circular: isCircular,
  });
  if (isCircular) return;
  const inner = s.type === "array" ? s.items : s;
  for (const [key, value] of Object.entries(inner?.properties ?? {})) {
    pushRows(key, value, inner?.required?.includes(key) ?? false, depth + 1, out);
  }
}

export function schemaRows(schema: Schema | undefined): SchemaRow[] {
  if (!schema) return [];
  const out: SchemaRow[] = [];
  pushRows("(ルート)", schema, false, 0, out);
  return out;
}

const FORMAT_SAMPLES: Record<string, string> = {
  email: "user@example.com",
  "date-time": "2026-11-11T10:00:00+09:00",
  date: "2026-11-11",
  password: "********",
  uuid: "00000000-0000-4000-8000-000000000000",
};

/** スキーマから例を作る。`useExamples` が真なら example・default・enum を使う。 */
export function generateExample(s: Schema, useExamples: boolean, used: { value: boolean } = { value: false }): JsonValue {
  if (useExamples) {
    if (s.example !== undefined) {
      used.value = true;
      return s.example;
    }
    if (s.default !== undefined) {
      used.value = true;
      return s.default;
    }
    if (s.enum) {
      used.value = true;
      return s.enum[0];
    }
  }
  if (s["x-nekoreq-circular"]) return s.type === "array" ? [] : null;
  switch (s.type) {
    case "string":
      return (s.format && FORMAT_SAMPLES[s.format]) || "string";
    case "integer":
    case "number":
      return s.minimum ?? 0;
    case "boolean":
      return false;
    case "array":
      return s.items?.["x-nekoreq-circular"] ? [] : [s.items ? generateExample(s.items, useExamples, used) : null];
    default: {
      const out: Record<string, JsonValue> = {};
      for (const [k, v] of Object.entries(s.properties ?? {})) out[k] = generateExample(v, useExamples, used);
      return out;
    }
  }
}

function mediaDoc(
  contentType: string,
  schema: Schema | undefined,
  yamlExample: JsonValue | undefined,
  named: { name: string; summary?: string; value: JsonValue }[] = [],
): MediaDoc {
  const generated = schema ? generateExample(schema, false) : null;
  let example: JsonValue | null = null;
  let source: MediaDoc["exampleSource"] = null;
  // `example` があれば名前 `example` で先頭に、続いて `examples` を書かれた順に並べる（Rust 側と同じ）
  const examples = [
    ...(yamlExample !== undefined ? [{ name: "example", summary: null, description: null, value: yamlExample }] : []),
    ...named.map((n) => ({ name: n.name, summary: n.summary ?? null, description: null, value: n.value })),
  ];
  if (examples.length > 0) {
    example = examples[0].value;
    source = "yaml";
  } else if (schema) {
    const used = { value: false };
    example = generateExample(schema, true, used);
    source = used.value ? "schemaExample" : "generated";
  }
  return {
    contentType,
    schemaLabel: schema ? typeLabelOf(schema) : null,
    rows: schemaRows(schema),
    example,
    exampleSource: source,
    examples,
    generatedExample: generated,
  };
}

function paramDoc(p: MockParam): ParamDoc {
  return {
    name: p.name,
    location: p.in,
    required: p.required,
    typeLabel: typeLabelOf(p.schema),
    constraints: constraintsOf(p.schema),
    description: p.description ?? null,
    example: p.example !== undefined ? String(p.example) : null,
    default: p.default !== undefined ? String(p.default) : null,
  };
}

export function securityLabels(op: MockOp): string[] {
  return op.security === "none" ? [] : ["cookieAuth（Cookie: SESSION）"];
}

export function mockOperationDetail(op: MockOp, importedAt: string | null, deleted: boolean): OperationDetail {
  const responses: ResponseDoc[] = op.responses.map((r) => ({
    status: r.status,
    description: r.description,
    contents: r.contentType ? [mediaDoc(r.contentType, r.schema, r.example)] : [],
    errorCodes: (r.codes ?? []).map((c): ErrorCodeEntry => ({ status: r.status, code: c.code, note: c.note })),
  }));
  return {
    opKey: mockOpKey(op),
    operationId: op.operationId,
    method: op.method,
    path: op.path,
    tags: op.tag ? [op.tag] : [],
    summary: op.summary,
    description: op.description,
    security: {
      badge: op.security === "none" ? "none" : "default",
      required: op.security !== "none",
      labels: securityLabels(op),
      requirements: op.security === "none" ? [] : [{ schemes: [{ name: "cookieAuth", scopes: [] }] }],
    },
    sourceFile: op.file,
    sourceLine: op.line,
    importedAt,
    parameters: op.parameters.map(paramDoc),
    requestBody: op.body
      ? { required: op.body.required, description: op.body.description ?? null, contents: [mediaDoc(op.body.contentType, op.body.schema, op.body.example, op.body.examples)] }
      : null,
    responses,
    errorCodes: responses.flatMap((r) => r.errorCodes),
    deleted,
  };
}
