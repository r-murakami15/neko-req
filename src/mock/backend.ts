import { ApiError, type Backend, type PickKind } from "../api/backend";
import type {
  AdHocRequest,
  AppError,
  AppInfo,
  Environment,
  HeaderPair,
  Project,
  RequestFinished,
  RequestDraft,
  RequestInput,
  RequestSpec,
  ResponseData,
  SendOutcome,
  Timing,
} from "../bindings";
import { assemble, type Lookup } from "./build";
import { createOpenApiMock, type MockControls } from "./openapiBackend";

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
export type MockBackend = Backend & { mock: MockControls };

export function createMockBackend(initial: Project[] = [], options: MockOptions = {}): MockBackend {
  const sendDelayMs = options.sendDelayMs ?? 400;
  let projects: Project[] = [...initial];
  let seq = projects.length;
  let envs: Environment[] = [];
  let adhocs: AdHocRequest[] = [];
  const secrets = new Map<string, string>();
  const secretKey = (envId: string, name: string) => `${envId}\u0000${name}`;
  const listeners = new Set<(e: RequestFinished) => void>();
  const pending = new Map<string, { timer: ReturnType<typeof setTimeout> }>();
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
      { name: "date", value: new Date().toUTCString() },
      { name: "vary", value: "Origin, Access-Control-Request-Method, Access-Control-Request-Headers" },
      { name: "x-content-type-options", value: "nosniff" },
      { name: "cache-control", value: "no-cache, no-store, max-age=0, must-revalidate" },
      { name: "x-frame-options", value: "DENY" },
      { name: "set-cookie", value: "XSRF-TOKEN=••••; Path=/" },
    ];
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
      const a = assemble(effectiveSpec(input, lookup), lookup);
      const sendId = newId("mock-send");
      if (a.preview.unresolved.length > 0) {
        const names = a.preview.unresolved.map((u) => `{{${u.name}}}`).join("、");
        emit({
          sendId,
          result: {
            status: "failure",
            error: { kind: "unresolved", message: `展開できない変数があります: ${names}`, vars: a.preview.unresolved },
          },
        });
        return sendId;
      }
      if (a.preview.urlError) {
        emit({ sendId, result: { status: "failure", error: { kind: "invalidRequest", message: a.preview.urlError } } });
        return sendId;
      }
      const timer = setTimeout(() => {
        pending.delete(sendId);
        const result = buildResponse(input, a.rawUrl, a.hasBody);
        if (result.status === "success") {
          bodies.set(sendId, result.response.body.content);
          if (bodies.size > 8) bodies.delete(bodies.keys().next().value as string);
        }
        emit({ sendId, result });
      }, sendDelayMs);
      pending.set(sendId, { timer });
      return sendId;
    },

    async requestCancel(sendId) {
      const p = pending.get(sendId);
      if (!p) return false;
      clearTimeout(p.timer);
      pending.delete(sendId);
      emit({ sendId, result: { status: "failure", error: { kind: "cancelled", message: "送信を中止しました" } } });
      return true;
    },

    async requestPreview(input) {
      const lookup = lookupFor(input.projectId, input.environmentId);
      return assemble(effectiveSpec(input, lookup), lookup).preview;
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

    ...openapi.methods,
  };
  return Object.assign(backend, { mock: openapi.controls });
}
