import { ApiError, type Backend, type PickKind } from "../api/backend";
import type {
  AdHocRequest,
  AppError,
  AppInfo,
  AutoUpdate,
  AutoUpdateSkip,
  CurlImport,
  CurlWarning,
  Environment,
  HeaderPair,
  KeyValue,
  OpKey,
  Project,
  ProjectSettings,
  RequestFinished,
  RequestDraft,
  RequestInput,
  RequestSpec,
  ResponseData,
  SendOutcome,
  Timing,
} from "../bindings";
import { assemble, type Assembled, type Lookup, type MockAutoAuth } from "./build";
import { applyCommon, parseCurl, renderCurl, splitUrl } from "./curl";
import { createHistoryMock, type HistoryControls } from "./history";
import { createOpenApiMock, type MockControls } from "./openapiBackend";
import { isSecretHeader, MASK, redactText } from "./scrub";
import { createSettingsMock, type SettingsControls } from "./settings";

/** Rust 側（nekoreq-storage）と同じ上限。 */
const MAX_PROJECT_NAME_CHARS = 100;
const MAX_NAME_CHARS = 100;

function validationError(message: string): ApiError {
  const error: AppError = { kind: "validation", message };
  return new ApiError(error);
}

function notFound(what: string, id: string): ApiError {
  return new ApiError({ kind: "notFound", message: `${what}が見つかりません: ${id}` });
}

/** Rust 側と同じ検証（前後の空白を除く・空と 100 文字超はエラー）。 */
function normalizeName(name: string): string {
  const trimmed = name.trim();
  if (trimmed === "") throw validationError("プロジェクト名を入力してください");
  if ([...trimmed].length > MAX_PROJECT_NAME_CHARS) {
    throw validationError(`プロジェクト名は ${MAX_PROJECT_NAME_CHARS} 文字以内にしてください`);
  }
  return trimmed;
}

function normalizeEnvName(name: string): string {
  const trimmed = name.trim();
  if (trimmed === "") throw validationError("環境名を入力してください");
  if ([...trimmed].length > MAX_NAME_CHARS) throw validationError(`環境名は ${MAX_NAME_CHARS} 文字以内にしてください`);
  return trimmed;
}

function normalizeVarName(name: string): string {
  const n = name.trim();
  if (n === "") throw validationError("変数名を入力してください");
  if (n.startsWith("$")) throw validationError(`変数名は $ から始められません（動的変数の名前です）: ${n}`);
  if (/[\s{}]/.test(n)) throw validationError(`変数名に空白と波括弧は使えません: ${n}`);
  return n;
}

const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;

export interface MockOptions {
  /** 送信してから結果が届くまでの時間（ミリ秒）。 */
  sendDelayMs?: number;
}

const SAMPLE_ITEMS = {
  data: {
    items: [
      {
        clientId: 1,
        clientName: "Netflix（Netflix Services Japan合同会社）",
        contractType: "DIRECT",
        status: "ACTIVE",
        works: [
          {
            contractWorkId: 11,
            contractId: 101,
            workId: 1001,
            workTitle: "ちびまる子ちゃん",
            mediaName: "Netflix（配信）",
            unitPrice: 10000,
            totalEpisodes: 260,
            startOn: "2025-04-01",
            endOn: "2026-03-31",
            contractState: "ACTIVE",
          },
        ],
      },
    ],
    total: 1,
  },
};

const REASONS: Record<number, string> = { 200: "OK", 400: "Bad Request", 500: "Internal Server Error" };

/**
 * ブラウザ（Tauri の外）で画面だけを確認するためのモック。データはメモリ上にだけあり、
 * 再読み込みで消える。テストでも使う。
 *
 * 送信は少し待ってから JSON の 200 を返す。URL に次の語を含めると結果が変わる:
 * `/error`（400 の problem+json）、`/server-error`（500）、`/timeout`・`/refused`（失敗）、
 * `/large`（先頭だけの長いボディ）、`/text`（テキスト）、`/binary`（バイナリ）。
 */
export type MockBackend = Backend & { mock: MockControls & HistoryControls & SettingsControls };

export function createMockBackend(initial: Project[] = [], options: MockOptions = {}): MockBackend {
  const sendDelayMs = options.sendDelayMs ?? 400;
  let projects: Project[] = [...initial];
  let seq = projects.length;
  let envs: Environment[] = [];
  let adhocs: AdHocRequest[] = [];
  const secrets = new Map<string, string>();
  const secretKey = (envId: string, name: string) => `${envId}\u0000${name}`;
  const listeners = new Set<(e: RequestFinished) => void>();
  const pending = new Map<string, { timer: ReturnType<typeof setTimeout>; input: RequestInput; assembled: Assembled }>();
  const bodies = new Map<string, string>();
  let nextId = 0;
  const newId = (prefix: string) => `${prefix}-${(nextId += 1)}`;

  const now = () => new Date().toISOString();
  const drafts = new Map<string, RequestDraft>();

  const projectOf = (id: string): Project => {
    const p = projects.find((x) => x.id === id);
    if (!p) throw notFound("プロジェクト", id);
    return p;
  };

  function ensureGlobal(projectId: string): Environment {
    projectOf(projectId);
    let g = envs.find((e) => e.projectId === projectId && e.isGlobal);
    if (!g) {
      const ts = now();
      g = { id: newId("mock-env"), projectId, name: "グローバル", isGlobal: true, vars: [], createdAt: ts, updatedAt: ts };
      envs.push(g);
    }
    return g;
  }

  const envOf = (id: string): Environment => {
    const e = envs.find((x) => x.id === id);
    if (!e) throw notFound("環境", id);
    return e;
  };

  const setDefault = (projectId: string, environmentId: string | null) => {
    const i = projects.findIndex((p) => p.id === projectId);
    projects[i] = { ...projects[i], defaultEnvironmentId: environmentId, updatedAt: now() };
    return clone(projects[i]);
  };

  function lookupFor(projectId: string, environmentId: string | null): Lookup {
    const project = projectOf(projectId);
    const global = ensureGlobal(projectId);
    const selected = environmentId ?? project.defaultEnvironmentId;
    const layers = [selected ? envs.find((e) => e.id === selected) : undefined, global];
    return (name) => {
      for (const layer of layers) {
        const v = layer?.vars.find((x) => x.enabled && x.name === name);
        if (!v) continue;
        if (v.value.kind === "plain") return { value: v.value.value, secret: false };
        const s = secrets.get(secretKey(layer!.id, v.name));
        if (s !== undefined) return { value: s, secret: true };
      }
      return undefined;
    };
  }

  function createProject(name: string): Project {
    const normalized = normalizeName(name);
    seq += 1;
    const ts = now();
    const project: Project = {
      id: `mock-project-${seq}`,
      name: normalized,
      source: null,
      defaultEnvironmentId: null,
      createdAt: ts,
      updatedAt: ts,
    };
    projects.push(project);
    ensureGlobal(project.id);
    return { ...project };
  }

  function addEnvironment(projectId: string, name: string, baseUrl: string): boolean {
    ensureGlobal(projectId);
    if (envs.some((e) => e.projectId === projectId && !e.isGlobal && e.name === name)) return false;
    const ts = now();
    const env: Environment = {
      id: newId("mock-env"),
      projectId,
      name,
      isGlobal: false,
      vars: [{ name: "baseUrl", value: { kind: "plain", value: baseUrl }, description: "", enabled: true }],
      createdAt: ts,
      updatedAt: ts,
    };
    envs.push(env);
    if (projectOf(projectId).defaultEnvironmentId === null) setDefault(projectId, env.id);
    return true;
  }

  const openapi = createOpenApiMock(
    {
      projectOf: (id) => clone(projectOf(id)),
      createProject,
      updateProject: (p) => {
        const i = projects.findIndex((x) => x.id === p.id);
        if (i >= 0) projects[i] = clone(p);
      },
      addEnvironment,
      newId,
      now,
    },
    drafts,
  );

  const settingsMock = createSettingsMock({ now, newId });
  const DEFAULT_PROJECT_SETTINGS: ProjectSettings = { autoSession: true, autoCsrf: true };
  const projectSettingsOf = (projectId: string): ProjectSettings => projectOf(projectId).settings ?? DEFAULT_PROJECT_SETTINGS;

  /** cookieAuth・CSRF の自動化の材料（選んでいる環境の session・xsrfToken・baseUrl）。 */
  function autoAuthFor(projectId: string, environmentId: string | null, lookup: Lookup): MockAutoAuth | null {
    const project = projectOf(projectId);
    const settings = projectSettingsOf(projectId);
    const selected = environmentId ?? project.defaultEnvironmentId;
    if (!selected) return null;
    const baseUrl = lookup("baseUrl")?.value ?? null;
    let origin: string | null = null;
    try {
      origin = baseUrl ? new URL(baseUrl).origin : null;
    } catch {
      origin = null;
    }
    const notes: string[] = [];
    if (origin === null && (settings.autoSession || settings.autoCsrf)) notes.push("環境に baseUrl がないため、session・xsrfToken は自動では付けません");
    return {
      session: settings.autoSession ? (lookup("session")?.value ?? null) : null,
      csrfEnabled: settings.autoCsrf,
      xsrfToken: settings.autoCsrf ? (lookup("xsrfToken")?.value ?? null) : null,
      origin,
      notes,
    };
  }

  /** モックの Set-Cookie: `/auth/password-login`・`/auth/email-code-login` が SESSION、GET `/auth/session` が XSRF-TOKEN、DELETE `/auth/session` が SESSION の削除。 */
  function cookiesSetBy(method: string, path: string): { name: "session" | "xsrfToken"; action: "updated" | "cleared" }[] {
    if (method === "POST" && /\/auth\/(password-login|email-code-login)$/.test(path)) return [{ name: "session", action: "updated" }];
    if (method === "DELETE" && /\/auth\/session$/.test(path)) return [{ name: "session", action: "cleared" }];
    if (method === "GET" && /\/auth\/session$/.test(path)) return [{ name: "xsrfToken", action: "updated" }];
    return [];
  }

  let sessionSeq = 0;
  function applyAutoUpdates(input: RequestInput, rawUrl: string, outcome: SendOutcome): { updates: AutoUpdate[]; skipped: AutoUpdateSkip[] } {
    const updates: AutoUpdate[] = [];
    const skipped: AutoUpdateSkip[] = [];
    if (outcome.status !== "success") return { updates, skipped };
    const settings = projectSettingsOf(input.projectId);
    let path = "";
    try {
      path = new URL(rawUrl).pathname;
    } catch {
      return { updates, skipped };
    }
    const project = projectOf(input.projectId);
    const envId = input.environmentId ?? project.defaultEnvironmentId;
    for (const c of cookiesSetBy(input.request.method.toUpperCase(), path)) {
      if (c.name === "session" ? !settings.autoSession : !settings.autoCsrf) continue;
      if (!envId) {
        skipped.push({ name: c.name, reason: `環境が選ばれていないため、${c.name} は保存しませんでした` });
        continue;
      }
      const env = envOf(envId);
      const base = env.vars.find((v) => v.name === "baseUrl" && v.value.kind === "plain");
      let origin: string | null = null;
      try {
        origin = base && base.value.kind === "plain" ? new URL(base.value.value).origin : null;
      } catch {
        origin = null;
      }
      if (origin === null) {
        skipped.push({ name: c.name, reason: `環境に baseUrl がないため、${c.name} は保存しませんでした` });
        continue;
      }
      if (new URL(rawUrl).origin !== origin) {
        skipped.push({ name: c.name, reason: `送り元が環境の baseUrl と違うため、${c.name} は保存しませんでした` });
        continue;
      }
      const existing = env.vars.find((v) => v.name === c.name);
      const key = secretKey(env.id, c.name);
      if (c.action === "cleared") {
        if (secrets.has(key)) {
          secrets.delete(key);
          updates.push({ name: c.name, environmentId: env.id, environmentName: env.name, action: "cleared" });
        }
        continue;
      }
      const value = c.name === "session" ? `mock-session-${(sessionSeq += 1)}` : "mock-xsrf-token";
      if (secrets.get(key) === value) continue;
      secrets.set(key, value);
      if (!existing) env.vars.push({ name: c.name, value: { kind: "secret" }, description: "自動で更新されます（Set-Cookie）", enabled: true });
      else if (existing.value.kind !== "secret") existing.value = { kind: "secret" };
      env.updatedAt = now();
      updates.push({ name: c.name, environmentId: env.id, environmentName: env.name, action: "updated" });
    }
    return { updates, skipped };
  }

  const historyMock = createHistoryMock({
    now,
    newId,
    limit: () => settingsMock.settings().history.limit,
    omitLargeBodies: () => settingsMock.settings().history.omitLargeBodies,
    projectExists: (id) => projects.some((p) => p.id === id),
    saveDraft: (projectId, opKey, spec) =>
      openapi.methods.draftSave({ id: "", projectId, opKey, params: spec.params, body: spec.body, auth: spec.auth, updatedAt: "" }),
    saveAdhoc: (projectId, name, spec) =>
      backend.adhocSave({ id: "", projectId, name, description: "", method: spec.method, url: spec.url, params: spec.params, body: spec.body, auth: spec.auth, createdAt: "", updatedAt: "" }),
    send: (input) => backend.requestSend(input, null),
  });

  /** `request.auth` が `None` で `opKey` があれば、yaml の security に従った既定の認証にする（モック）。 */
  function effectiveSpec(input: RequestInput, lookup: Lookup): RequestSpec {
    const spec = input.request;
    if (spec.auth !== null || !input.opKey) return spec;
    const variable = openapi.defaultAuthName(input.projectId, input.opKey);
    // 使う変数が展開できないときは認証なしで送る
    if (!variable || !lookup(variable)) return spec;
    return { ...spec, auth: { type: "apiKey", location: "cookie", name: "SESSION", value: `{{${variable}}}` } };
  }

  const emit = (event: RequestFinished) => {
    setTimeout(() => {
      for (const l of [...listeners]) l(event);
    }, 0);
  };

  function buildResponse(input: RequestInput, rawUrl: string, hasBody: boolean): SendOutcome {
    let u: URL;
    try {
      u = new URL(rawUrl);
    } catch {
      return { status: "failure", error: { kind: "invalidRequest", message: "URL を読めません" } };
    }
    const path = u.pathname;
    if (path.includes("/timeout")) {
      return { status: "failure", error: { kind: "timeout", message: "30 秒以内に応答がありませんでした" } };
    }
    if (path.includes("/refused")) {
      return {
        status: "failure",
        error: { kind: "connect", message: `接続できません（${u.host} に接続を拒否されました）` },
      };
    }

    let status = 200;
    let contentType = "application/json";
    let content = JSON.stringify(SAMPLE_ITEMS);
    let encoding: "text" | "base64" = "text";
    let size = new TextEncoder().encode(content).length;
    let truncated = false;
    if (path.includes("/error") || path.includes("/server-error")) {
      status = path.includes("/server-error") ? 500 : 400;
      contentType = "application/problem+json";
      content = JSON.stringify({
        type: "about:blank",
        title: status === 400 ? "Bad Request" : "Internal Server Error",
        status,
        code: status === 400 ? "VALIDATION_ERROR" : "INTERNAL_ERROR",
        detail: status === 400 ? "入力内容に誤りがあります（email: 必須です）" : "サーバー内部でエラーが起きました",
      });
      size = new TextEncoder().encode(content).length;
    } else if (path.includes("/large")) {
      const row = JSON.stringify({ id: 1, name: "あ".repeat(20) });
      content = `[${Array.from({ length: 200 }, () => row).join(",")}`;
      size = 25_165_824;
      truncated = true;
    } else if (path.includes("/text")) {
      contentType = "text/plain; charset=utf-8";
      content = "pong\n";
      size = 5;
    } else if (path.includes("/binary")) {
      contentType = "application/pdf";
      encoding = "base64";
      content = "JVBERi0xLjQK";
      size = 9;
    }

    const https = u.protocol === "https:";
    const timing: Timing = {
      dnsMs: 1,
      connectMs: 1,
      tlsMs: https ? 8 : null,
      sendMs: hasBody ? 1 : null,
      waitMs: 33,
      downloadMs: 3,
      totalMs: 1 + 1 + (https ? 8 : 0) + (hasBody ? 1 : 0) + 33 + 3,
    };
    const headers: HeaderPair[] = [
      { name: "content-type", value: contentType },
      { name: "transfer-encoding", value: "chunked" },
      // 毎回変わらないようにする（履歴の比較で、同じレスポンスどうしが「同じ」になる）
      { name: "date", value: "Tue, 17 Nov 2026 01:24:12 GMT" },
      { name: "vary", value: "Origin, Access-Control-Request-Method, Access-Control-Request-Headers" },
      { name: "x-content-type-options", value: "nosniff" },
      { name: "cache-control", value: "no-cache, no-store, max-age=0, must-revalidate" },
      { name: "x-frame-options", value: "DENY" },
      { name: "set-cookie", value: "XSRF-TOKEN=••••; Path=/" },
    ];
    if (/\/auth\/(password-login|email-code-login)$/.test(path)) headers.push({ name: "set-cookie", value: "SESSION=••••; Path=/; HttpOnly" });
    const response: ResponseData = {
      status,
      statusText: REASONS[status] ?? "",
      headers,
      body: { contentType, encoding, content, size, truncated },
      timing,
      connection: {
        remoteAddr: u.hostname === "localhost" ? "127.0.0.1:8080" : "203.0.113.10:443",
        httpVersion: "HTTP/1.1",
        tls: https,
      },
      finalUrl: rawUrl,
      redirectCount: 0,
      sentBytes: 318,
      receivedBytes: size + 400,
    };
    void input;
    return { status: "success", response };
  }

  function recordHistory(input: RequestInput, a: Assembled, outcome: SendOutcome) {
    const project = projectOf(input.projectId);
    const envId = input.environmentId ?? project.defaultEnvironmentId;
    const env = envId ? envs.find((e) => e.id === envId) : undefined;
    historyMock.record({
      input,
      environmentId: env?.id ?? null,
      environmentName: env?.name ?? null,
      opSummary: input.opKey ? openapi.summaryOf(input.projectId, input.opKey) : null,
      assembled: a,
      outcome,
    });
  }

  /** cURL の解析（確認用の簡易版）。 */
  async function parseCurlText(projectId: string, text: string, environmentId: string | null): Promise<CurlImport> {
    const parsed = parseCurl(text);
    if ("error" in parsed) throw validationError(parsed.error);
    const url = splitUrl(parsed.url);
    if (!url) throw validationError(`URL を読めません: ${parsed.url}`);
    const project = projectOf(projectId);
    const order = [
      environmentId ?? project.defaultEnvironmentId,
      ...envs.filter((e) => e.projectId === projectId && !e.isGlobal).map((e) => e.id),
      envs.find((e) => e.projectId === projectId && e.isGlobal)?.id ?? null,
    ].filter((id, i, all): id is string => id !== null && all.indexOf(id) === i);
    let matchedEnv: string | null = null;
    let prefix = "";
    for (const id of order) {
      const base = envs.find((e) => e.id === id)?.vars.find((v) => v.enabled && v.name === "baseUrl" && v.value.kind === "plain");
      if (!base || base.value.kind !== "plain") continue;
      try {
        const b = new URL(base.value.value);
        if (b.origin === url.origin && url.pathname.startsWith(b.pathname.replace(/\/$/, ""))) {
          matchedEnv = id;
          prefix = b.pathname.replace(/\/$/, "");
          break;
        }
      } catch {
        /* baseUrl が URL でなければ使わない */
      }
    }
    const baseReplaced = matchedEnv !== null;
    const warnings: CurlWarning[] = [...parsed.warnings];
    const query: [string, string][] = [...url.query];
    if (parsed.getMode && parsed.data) for (const pair of parsed.data.split("&")) query.push(pair.split("=") as [string, string]);
    const method = parsed.method;
    const rest = url.pathname.slice(prefix.length);
    const match = baseReplaced ? openapi.matchOperation(projectId, method, rest) : null;
    if (!baseReplaced && openapi.matchOperation(projectId, method, url.pathname)) {
      warnings.push({ kind: "adjusted", option: "url", message: "ホストが環境の baseUrl と違うため、OpenAPI の API とは照合しませんでした" });
    }
    if (match) {
      const opened = await openapi.methods.operationOpen(projectId, match.opKey);
      let params: KeyValue[] = opened.request.params.map((p) => (p.location === "path" && match.params[p.name] !== undefined ? { ...p, value: decodeURIComponent(match.params[p.name]), enabled: true } : p));
      for (const [k, v] of query) {
        const i = params.findIndex((p) => p.location === "query" && p.name === k);
        params = i >= 0 ? params.map((p, j) => (j === i ? { ...p, value: v, enabled: true } : p)) : [...params, { location: "query", name: k, value: v, enabled: true }];
      }
      const request = applyCommon({ ...opened.request, method, params }, { ...parsed, method });
      return {
        target: "operation",
        opKey: match.opKey as OpKey,
        summary: match.op.summary,
        matchedPath: match.op.path,
        request,
        baseReplaced,
        environmentId: matchedEnv,
        suggestedName: `${method} ${rest}`,
        warnings,
      };
    }
    const qs = query.length > 0 ? `?${query.map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`).join("&")}` : "";
    const urlText = baseReplaced ? `{{baseUrl}}${rest}${qs}` : `${url.origin}${url.pathname}${qs}`;
    const request = applyCommon({ method, url: urlText, params: [], body: { kind: "none" }, auth: null }, { ...parsed, method });
    return { target: "adhoc", opKey: null, summary: null, matchedPath: null, request, baseReplaced, environmentId: matchedEnv, suggestedName: `${method} ${url.pathname}`, warnings };
  }

  async function exportCurl(input: RequestInput, mode: "redacted" | "raw"): Promise<string> {
    const lookup = lookupFor(input.projectId, input.environmentId);
    const a = assemble(effectiveSpec(input, lookup), lookup, autoAuthFor(input.projectId, input.environmentId, lookup));
    if (a.preview.unresolved.length > 0) {
      throw validationError(`展開できない変数があります: ${a.preview.unresolved.map((u) => `{{${u.name}}}`).join("、")}`);
    }
    if (a.preview.urlError) throw validationError(a.preview.urlError);
    const red = (t: string) => (mode === "redacted" ? redactText(t, a.secrets) : t);
    return renderCurl({
      method: input.request.method,
      url: red(a.rawUrl),
      headers: a.headers.map((h) => ({ name: h.name, value: mode === "redacted" && isSecretHeader(h.name) ? MASK : red(h.value) })),
      bodyText: a.bodyText === null ? null : red(a.bodyText),
      parts: a.parts.map((p) => ({ ...p, value: red(p.value) })),
    });
  }

  const backend: Backend = {
    async appInfo(): Promise<AppInfo> {
      return {
        name: "NekoReq",
        version: "0.1.0",
        identifier: "com.nekoreq.desktop",
        dataDir: "（ブラウザ上のモック）",
        dbPath: "（ブラウザ上のモック: メモリ）",
        schemaVersion: 2,
      };
    },

    async projectList() {
      return projects.map((p) => ({ ...p }));
    },

    async projectCreate(name) {
      return createProject(name);
    },

    async projectRename(id, name) {
      const normalized = normalizeName(name);
      const index = projects.findIndex((p) => p.id === id);
      if (index < 0) throw notFound("プロジェクト", id);
      const renamed = { ...projects[index], name: normalized, updatedAt: now() };
      projects[index] = renamed;
      return { ...renamed };
    },

    async projectDelete(id) {
      if (!projects.some((p) => p.id === id)) throw notFound("プロジェクト", id);
      projects = projects.filter((p) => p.id !== id);
      envs = envs.filter((e) => e.projectId !== id);
      adhocs = adhocs.filter((a) => a.projectId !== id);
    },

    // ---- 送信 ----

    async requestSend(input) {
      const lookup = lookupFor(input.projectId, input.environmentId);
      const a = assemble(effectiveSpec(input, lookup), lookup, autoAuthFor(input.projectId, input.environmentId, lookup));
      const sendId = newId("mock-send");
      if (a.preview.unresolved.length > 0) {
        const names = a.preview.unresolved.map((u) => `{{${u.name}}}`).join("、");
        emit({
          sendId,
          result: {
            status: "failure",
            error: { kind: "unresolved", message: `展開できない変数があります: ${names}`, vars: a.preview.unresolved },
          },
          autoUpdates: [],
          autoUpdateSkipped: [],
        });
        return sendId;
      }
      if (a.preview.urlError) {
        emit({
          sendId,
          result: { status: "failure", error: { kind: "invalidRequest", message: a.preview.urlError } },
          autoUpdates: [],
          autoUpdateSkipped: [],
        });
        return sendId;
      }
      const timer = setTimeout(() => {
        pending.delete(sendId);
        const result = buildResponse(input, a.rawUrl, a.hasBody);
        if (result.status === "success") {
          bodies.set(sendId, result.response.body.content);
          if (bodies.size > 8) bodies.delete(bodies.keys().next().value as string);
        }
        const { updates, skipped } = applyAutoUpdates(input, a.rawUrl, result);
        recordHistory(input, a, result);
        emit({ sendId, result, autoUpdates: updates, autoUpdateSkipped: skipped });
      }, sendDelayMs);
      pending.set(sendId, { timer, input, assembled: a });
      return sendId;
    },

    async requestCancel(sendId) {
      const p = pending.get(sendId);
      if (!p) return false;
      clearTimeout(p.timer);
      pending.delete(sendId);
      const result: SendOutcome = { status: "failure", error: { kind: "cancelled", message: "送信を中止しました" } };
      recordHistory(p.input, p.assembled, result);
      emit({ sendId, result, autoUpdates: [], autoUpdateSkipped: [] });
      return true;
    },

    async requestPreview(input) {
      const lookup = lookupFor(input.projectId, input.environmentId);
      return assemble(effectiveSpec(input, lookup), lookup, autoAuthFor(input.projectId, input.environmentId, lookup)).preview;
    },

    async onRequestFinished(handler) {
      listeners.add(handler);
      return () => {
        listeners.delete(handler);
      };
    },

    async responseSaveBody(sendId) {
      if (!bodies.has(sendId)) throw notFound("レスポンス", sendId);
    },

    async pickSavePath(defaultName) {
      return `/mock/downloads/${defaultName}`;
    },

    async pickFile() {
      return "/mock/files/sample.pdf";
    },

    async pickPaths(kind: PickKind) {
      if (kind === "openapi-file") return ["/mock/openapi.yaml"];
      if (kind === "folder") return ["/mock/api-docs"];
      return ["/mock/api/openapi.yaml", "/mock/api/common.yaml"];
    },

    /** ブラウザでは、ページへのドロップのファイル名から `/mock/dropped/<名前>` を作って届ける（本物の絶対パスは取れない）。 */
    async onFileDrop(handler) {
      if (typeof window === "undefined") return () => {};
      const over = (e: DragEvent) => {
        e.preventDefault();
        handler({ type: "over" });
      };
      const leave = () => handler({ type: "leave" });
      const drop = (e: DragEvent) => {
        e.preventDefault();
        const dt = e.dataTransfer;
        if (!dt) return;
        const names = new Set<string>();
        for (const item of Array.from(dt.items ?? [])) {
          const entry = (item as DataTransferItem & { webkitGetAsEntry?: () => { name: string } | null }).webkitGetAsEntry?.();
          if (entry) names.add(entry.name);
        }
        if (names.size === 0) for (const f of Array.from(dt.files)) names.add(f.name);
        if (names.size > 0) handler({ type: "drop", paths: [...names].map((n) => `/mock/dropped/${n}`) });
      };
      window.addEventListener("dragover", over);
      window.addEventListener("dragleave", leave);
      window.addEventListener("drop", drop);
      return () => {
        window.removeEventListener("dragover", over);
        window.removeEventListener("dragleave", leave);
        window.removeEventListener("drop", drop);
      };
    },

    async openExternal(url) {
      if (typeof window !== "undefined" && /^https?:/.test(url)) window.open(url, "_blank", "noopener,noreferrer");
    },

    // ---- 環境 ----

    async envList(projectId) {
      const global = ensureGlobal(projectId);
      return [global, ...envs.filter((e) => e.projectId === projectId && !e.isGlobal)].map(clone);
    },

    async envCreate(projectId, name) {
      const n = normalizeEnvName(name);
      ensureGlobal(projectId);
      const ts = now();
      const env: Environment = { id: newId("mock-env"), projectId, name: n, isGlobal: false, vars: [], createdAt: ts, updatedAt: ts };
      envs.push(env);
      if (projectOf(projectId).defaultEnvironmentId === null) setDefault(projectId, env.id);
      return clone(env);
    },

    async envSave(environment) {
      const current = envOf(environment.id);
      let name = current.name;
      if (current.isGlobal) {
        if (environment.name.trim() !== current.name) throw validationError("グローバルの名前は変えられません");
      } else {
        name = normalizeEnvName(environment.name);
      }
      const vars = environment.vars.map((v) => ({ ...clone(v), name: normalizeVarName(v.name) }));
      const seen = new Set<string>();
      for (const v of vars) {
        if (seen.has(v.name)) throw validationError(`変数名が重複しています: ${v.name}`);
        seen.add(v.name);
      }
      // 秘密でなくなった・消えた変数の値はキーチェーンからも消す
      for (const v of current.vars) {
        if (v.value.kind === "secret" && !vars.some((x) => x.name === v.name && x.value.kind === "secret")) {
          secrets.delete(secretKey(current.id, v.name));
        }
      }
      current.name = name;
      current.vars = vars;
      current.updatedAt = now();
      return clone(current);
    },

    async envDuplicate(id) {
      const src = envOf(id);
      if (src.isGlobal) throw validationError("グローバルは複製できません");
      const ts = now();
      const copy: Environment = {
        ...clone(src),
        id: newId("mock-env"),
        name: `${src.name} のコピー`,
        createdAt: ts,
        updatedAt: ts,
      };
      envs.push(copy);
      for (const v of src.vars) {
        const s = secrets.get(secretKey(src.id, v.name));
        if (v.value.kind === "secret" && s !== undefined) secrets.set(secretKey(copy.id, v.name), s);
      }
      return clone(copy);
    },

    async envDelete(id) {
      const env = envOf(id);
      if (env.isGlobal) throw validationError("グローバルは削除できません");
      envs = envs.filter((e) => e.id !== id);
      for (const v of env.vars) secrets.delete(secretKey(id, v.name));
      const project = projectOf(env.projectId);
      if (project.defaultEnvironmentId === id) {
        const next = envs.find((e) => e.projectId === env.projectId && !e.isGlobal);
        setDefault(env.projectId, next?.id ?? null);
      }
    },

    async envSelect(projectId, environmentId) {
      projectOf(projectId);
      if (environmentId !== null) {
        const env = envOf(environmentId);
        if (env.projectId !== projectId) throw notFound("環境", environmentId);
        if (env.isGlobal) throw validationError("グローバルは選べません（どの環境を選んでいても使われます）");
      }
      return setDefault(projectId, environmentId);
    },

    async secretSet(_projectId, environmentId, name, value) {
      envOf(environmentId);
      secrets.set(secretKey(environmentId, name), value);
    },

    async secretDelete(_projectId, environmentId, name) {
      secrets.delete(secretKey(environmentId, name));
    },

    async secretReveal(_projectId, environmentId, name) {
      return secrets.get(secretKey(environmentId, name)) ?? null;
    },

    // ---- 手で作ったリクエスト ----

    async adhocList(projectId) {
      return adhocs.filter((a) => a.projectId === projectId).map(clone);
    },

    async adhocSave(request) {
      const name = request.name.trim();
      if (name === "") throw validationError("リクエスト名を入力してください");
      if ([...name].length > 200) throw validationError("リクエスト名は 200 文字以内にしてください");
      const method = request.method.trim().toUpperCase();
      if (method === "") throw validationError("method を指定してください");
      projectOf(request.projectId);
      const ts = now();
      const existing = request.id ? adhocs.find((a) => a.id === request.id) : undefined;
      const saved: AdHocRequest = {
        ...clone(request),
        id: existing ? existing.id : newId("mock-adhoc"),
        name,
        description: request.description ?? "",
        method,
        createdAt: existing ? existing.createdAt : ts,
        updatedAt: ts,
      };
      adhocs = existing ? adhocs.map((a) => (a.id === saved.id ? saved : a)) : [...adhocs, saved];
      return clone(saved);
    },

    async adhocDelete(id) {
      if (!adhocs.some((a) => a.id === id)) throw notFound("リクエスト", id);
      adhocs = adhocs.filter((a) => a.id !== id);
    },

    async projectSettingsSet(projectId, settings) {
      projectOf(projectId);
      const i = projects.findIndex((p) => p.id === projectId);
      projects[i] = { ...projects[i], settings: { autoSession: settings.autoSession, autoCsrf: settings.autoCsrf }, updatedAt: now() };
      return clone(projects[i]);
    },

    async curlParse(projectId, text, environmentId) {
      return parseCurlText(projectId, text, environmentId);
    },

    async curlExport(input, mode) {
      return exportCurl(input, mode);
    },

    ...settingsMock.methods,
    ...historyMock.methods,
    ...openapi.methods,
  };
  return Object.assign(backend, { mock: { ...openapi.controls, ...historyMock.controls, ...settingsMock.controls } });
}
