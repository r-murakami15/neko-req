import { ApiError, type Backend } from "../api/backend";
import type {
  ApiSource,
  Body,
  ChangedEntry,
  DiffEntry,
  FieldChange,
  ImportOptions,
  ImportPreview,
  ImportReport,
  ImportRequest,
  ImportResult,
  KeyValue,
  LoadProblem,
  OpKey,
  OperationListItem,
  OperationOpen,
  ParamRef,
  PreviewResult,
  PreviewTag,
  Project,
  RequestDraft,
  SourceFile,
  SourceInput,
  SyncApplyResult,
  SyncCheckResult,
  SyncDiff,
  UploadInput,
} from "../bindings";
import { opKeyString } from "../lib/opkey";
import {
  generateExample,
  MOCK_DEFAULT_AUTH,
  MOCK_SPEC_INFO,
  type MockOp,
  mockHash,
  mockOpKey,
  mockOperationDetail,
  mockSpecOps,
} from "./openapi";

/** モックのバックエンドから OpenAPI 側が借りるもの（プロジェクトと環境の操作）。 */
export interface OpenApiHost {
  projectOf(id: string): Project;
  createProject(name: string): Project;
  updateProject(project: Project): void;
  /** servers から環境を作る。同名の環境があれば作らず `false`。 */
  addEnvironment(projectId: string, name: string, baseUrl: string): boolean;
  newId(prefix: string): string;
  now(): string;
}

/** ブラウザやテストから、モックの「yaml の状態」を変えるための操作。 */
export interface MockControls {
  /** 取り込み元の yaml が更新された状態にする（同期ボタンに印が付き、差分が出る）。 */
  touchSource(): void;
  /** 次の同期で yaml が壊れている（blocked）ようにする。 */
  breakSource(on?: boolean): void;
  /** 取り込んだ API の数。 */
  operationCount(projectId: string): number;
  /** ツリーの動作確認用: 合成した API を `count` 件足す（タグは 20 個に振り分ける）。 */
  inflate(projectId: string, count: number): void;
  /** 次の同期で、入口と別のホストの参照がある（確認が要る）ようにする。確認したホストは覚える。 */
  requireHostConfirm(on?: boolean): void;
}

interface ProjectApi {
  ops: MockOp[];
  version: 1 | 2;
  imported: Set<string>;
  options: ImportOptions;
  deleted: Map<string, MockOp>;
  stale: Map<string, ParamRef[]>;
  importedAt: string;
  kind: ApiSource["kind"];
  entry: string;
}

const validation = (message: string) => new ApiError({ kind: "validation", message });
const notFound = (message: string) => new ApiError({ kind: "notFound", message });

function problemsFor(text: string): LoadProblem[] | null {
  if (text.includes("missing")) {
    return [
      {
        kind: "missingFile",
        file: "video/client.yaml",
        reason: "ファイルが見つかりません",
        referencedFrom: [
          { file: "openapi.yaml", line: 38 },
          { file: "openapi.yaml", line: 57 },
        ],
      },
    ];
  }
  if (text.includes("broken")) {
    return [{ kind: "yamlError", file: "video/client.yaml", line: 42, column: 7, reason: "mapping values are not allowed in this context" }];
  }
  if (text.includes("swagger")) return [{ kind: "unsupportedVersion", file: "swagger.yaml", version: "2.0" }];
  if (text.includes("notopenapi")) return [{ kind: "notOpenApi", file: "config.yaml", reason: "openapi: がありません" }];
  if (text.includes("unreadable") || text.includes("404")) {
    return [{ kind: "unreadable", file: text, reason: "入口の yaml を読めません（404 Not Found）" }];
  }
  return null;
}

const FOREIGN_HOST = "assets.other-host.example";

function sourceText(source: SourceInput): string {
  if (source.kind === "path") return source.path;
  if (source.kind === "url") return source.url;
  return source.paths.join("\n");
}

function hasDraftKey(drafts: Map<string, RequestDraft>, projectId: string, key: OpKey): boolean {
  return drafts.has(`${projectId}|${opKeyString(key)}`);
}

export function createOpenApiMock(host: OpenApiHost, drafts: Map<string, RequestDraft>) {
  const apis = new Map<string, ProjectApi>();
  /** 「ディスクの yaml」の版。`touchSource` で 2 になる。 */
  let disk: 1 | 2 = 1;
  let broken = false;
  let needHostConfirm = false;
  const confirmedHosts = new Map<string, Set<string>>();
  /** 同期で確認が要るとき、まだ確認していない別ホストの参照（確認済みなら `null`）。 */
  const foreignFor = (projectId: string, allowed: string[] | null | undefined): SyncCheckResult | null => {
    if (!needHostConfirm) return null;
    const known = confirmedHosts.get(projectId) ?? new Set<string>();
    for (const h of allowed ?? []) known.add(h);
    confirmedHosts.set(projectId, known);
    if (known.has(FOREIGN_HOST)) return null;
    return {
      status: "needsConfirmation",
      foreign: [{ host: FOREIGN_HOST, url: `https://${FOREIGN_HOST}/common.yaml`, referencedFrom: { file: "openapi.yaml", line: 12 } }],
    };
  };

  const apiOf = (projectId: string): ProjectApi => {
    host.projectOf(projectId);
    const api = apis.get(projectId);
    if (!api) throw notFound(`取り込んだ API がありません: ${projectId}`);
    return api;
  };

  const entryOf = (op: MockOp): DiffEntry => ({
    opKey: mockOpKey(op),
    operationId: op.operationId,
    method: op.method,
    path: op.path,
    summary: op.summary,
    tag: op.tag,
    hasDraft: false,
  });

  function diffOps(projectId: string, prev: MockOp[], next: MockOp[]): SyncDiff {
    const withDraft = (op: MockOp): DiffEntry => ({ ...entryOf(op), hasDraft: hasDraftKey(drafts, projectId, mockOpKey(op)) });
    const prevMap = new Map(prev.map((o) => [opKeyString(mockOpKey(o)), o]));
    const nextMap = new Map(next.map((o) => [opKeyString(mockOpKey(o)), o]));
    const added = next.filter((o) => !prevMap.has(opKeyString(mockOpKey(o)))).map(withDraft);
    const removed = prev.filter((o) => !nextMap.has(opKeyString(mockOpKey(o)))).map(withDraft);
    const changed: ChangedEntry[] = [];
    for (const n of next) {
      const p = prevMap.get(opKeyString(mockOpKey(n)));
      if (!p || mockHash(p) === mockHash(n)) continue;
      const changes: FieldChange[] = [];
      if (p.summary !== n.summary) changes.push({ field: "summary", note: "概要が変わりました", before: p.summary, after: n.summary });
      if (p.description !== n.description) changes.push({ field: "description", note: "説明が変わりました", before: p.description, after: n.description });
      const pNames = p.parameters.map((x) => x.name);
      const nNames = n.parameters.map((x) => x.name);
      const addedParams = nNames.filter((x) => !pNames.includes(x));
      const removedParams = pNames.filter((x) => !nNames.includes(x));
      if (JSON.stringify(p.parameters) !== JSON.stringify(n.parameters)) {
        const parts = [
          addedParams.length > 0 ? `追加: ${addedParams.join("、")}` : "",
          removedParams.length > 0 ? `削除: ${removedParams.join("、")}` : "",
        ].filter((s) => s !== "");
        changes.push({ field: "parameters", note: parts.join("、") || "内容が変わりました", before: null, after: null });
      }
      if (JSON.stringify(p.body) !== JSON.stringify(n.body)) changes.push({ field: "requestBody", note: "リクエストボディが変わりました", before: null, after: null });
      if (JSON.stringify(p.responses) !== JSON.stringify(n.responses)) changes.push({ field: "responses", note: "レスポンスが変わりました", before: null, after: null });
      if (p.security !== n.security) changes.push({ field: "security", note: "認証の指定が変わりました", before: null, after: null });
      if (p.tag !== n.tag) changes.push({ field: "tags", note: "タグが変わりました", before: p.tag, after: n.tag });
      changed.push({ entry: withDraft(n), renamedFrom: null, changes, removedParams });
    }
    return { added, changed, removed };
  }

  function applyOps(projectId: string, api: ProjectApi, next: MockOp[]): ImportReport {
    const diff = diffOps(projectId, api.ops, next);
    let keptAsDeleted = 0;
    for (const r of diff.removed) {
      const key = opKeyString(r.opKey);
      const op = api.ops.find((o) => opKeyString(mockOpKey(o)) === key);
      if (op && r.hasDraft) {
        api.deleted.set(key, op);
        keptAsDeleted += 1;
      }
    }
    for (const c of diff.changed) {
      const key = opKeyString(c.entry.opKey);
      const draft = drafts.get(`${projectId}|${key}`);
      if (!draft || c.removedParams.length === 0) continue;
      const marks = draft.params.filter((p) => c.removedParams.includes(p.name)).map((p): ParamRef => ({ location: p.location, name: p.name }));
      if (marks.length > 0) api.stale.set(key, [...(api.stale.get(key) ?? []), ...marks]);
    }
    for (const o of next) api.deleted.delete(opKeyString(mockOpKey(o)));
    api.ops = next;
    api.version = disk;
    api.importedAt = host.now();
    return { added: diff.added.length, changed: diff.changed.length, removed: diff.removed.length, keptAsDeleted, renamed: 0 };
  }

  function classify(source: SourceInput): PreviewResult | null {
    const text = sourceText(source);
    const problems = problemsFor(text);
    if (problems) return { status: "blocked", problems };
    if (text.includes("multiple")) {
      if (source.kind === "path" && !/\.ya?ml$/.test(source.path)) {
        return { status: "needsEntry", candidates: [`${source.path}/openapi.yaml`, `${source.path}/legacy/openapi.yaml`] };
      }
      if (source.kind === "upload" && source.entry === null) {
        return { status: "needsEntry", candidates: ["openapi.yaml", "legacy/openapi.yaml"] };
      }
    }
    if (source.kind === "url" && text.includes("foreign") && !source.allowedHosts.includes(FOREIGN_HOST)) {
      return {
        status: "needsConfirmation",
        foreign: [{ host: FOREIGN_HOST, url: `https://${FOREIGN_HOST}/common.yaml`, referencedFrom: { file: "openapi.yaml", line: 12 } }],
      };
    }
    return null;
  }

  function entryName(source: SourceInput): string {
    if (source.kind === "path") return source.path;
    if (source.kind === "url") return source.url;
    return source.entry ?? "openapi.yaml";
  }

  function previewOf(source: SourceInput, projectId: string | null): ImportPreview {
    const ops = mockSpecOps(disk);
    const text = sourceText(source);
    const bundled = text.includes("bundled");
    const order: (string | null)[] = [];
    for (const o of ops) if (!order.includes(o.tag)) order.push(o.tag);
    order.sort((a, b) => (a === null ? 1 : b === null ? -1 : 0));
    const tags: PreviewTag[] = order.map((name) => ({
      name,
      description: name ? (MOCK_SPEC_INFO.tagDescriptions[name] ?? null) : null,
      operations: ops
        .filter((o) => o.tag === name)
        .map((o) => ({
          opKey: mockOpKey(o),
          operationId: o.operationId,
          method: o.method,
          path: o.path,
          summary: o.summary,
          security: o.security === "none" ? ("none" as const) : ("default" as const),
          sourceFile: o.file,
          sourceLine: o.line,
        })),
    }));
    const files = bundled ? ["openapi.bundled.yaml"] : ["openapi.yaml", "auth.yaml", "file.yaml", "legacy.yaml", "video/client.yaml", "video/work.yaml"];
    let diff: SyncDiff | null = null;
    if (projectId) {
      host.projectOf(projectId);
      diff = diffOps(projectId, apis.get(projectId)?.ops ?? [], ops);
    }
    const dup = text.includes("duplicate");
    return {
      title: MOCK_SPEC_INFO.title,
      version: MOCK_SPEC_INFO.version,
      openapiVersion: MOCK_SPEC_INFO.openapiVersion,
      description: MOCK_SPEC_INFO.description,
      servers: MOCK_SPEC_INFO.servers,
      defaultAuth: MOCK_SPEC_INFO.defaultAuth,
      reference: bundled ? "bundled" : "split",
      entry: bundled ? "openapi.bundled.yaml" : "openapi.yaml",
      fileCount: files.length,
      files,
      totalBytes: bundled ? 497_664 : 38_400,
      operationCount: ops.length,
      tags,
      inspection: {
        unresolvedRefs: text.includes("refwarn")
          ? [{ reference: "./common.yaml#/components/schemas/Missing", from: { file: "video/client.yaml", line: 88 }, reason: "common.yaml に /components/schemas/Missing がありません" }]
          : [],
        duplicateOperationIds: dup ? [{ operationId: "getSession", operations: ["GET /api/v1/auth/session", "GET /api/v2/auth/session"] }] : [],
        noSecurityCount: ops.filter((o) => o.security === "none").length,
        markdownTableCount: ops.filter((o) => o.description?.includes("|---")).length,
      },
      diff,
      elapsedMs: 42,
    };
  }

  const sourceFiles = (preview: ImportPreview, at: string): SourceFile[] =>
    preview.files.map((path, i) => ({ path, modifiedAt: at, hash: `mockhash${i}` }));

  const asImportResult = (r: PreviewResult): ImportResult => {
    if (r.status === "needsEntry") return { status: "needsEntry", candidates: r.candidates };
    if (r.status === "needsConfirmation") return { status: "needsConfirmation", foreign: r.foreign };
    if (r.status === "blocked") return { status: "blocked", problems: r.problems };
    throw new Error("ready は取り込みの結果ではありません");
  };

  const diffIsEmpty = (d: SyncDiff) => d.added.length + d.changed.length + d.removed.length === 0;
  const changedFilesOf = (prev: MockOp[], next: MockOp[]): string[] => {
    const prevHash = new Map(prev.map((o) => [opKeyString(mockOpKey(o)), mockHash(o)]));
    return [...new Set(next.filter((o) => prevHash.get(opKeyString(mockOpKey(o))) !== mockHash(o)).map((o) => o.file))];
  };

  const sourceBlocked = (): LoadProblem[] => [
    { kind: "yamlError", file: "video/client.yaml", line: 42, column: 7, reason: "mapping values are not allowed in this context" },
  ];

  const defaultBodyOf = (op: MockOp, options: ImportOptions): { body: Body; contentType: string | null } => {
    if (!op.body) return { body: { kind: "none" }, contentType: null };
    const ct = op.body.contentType;
    const props = op.body.schema.properties ?? {};
    if (ct.includes("json")) {
      const value = options.fillFromExamples ? JSON.stringify(op.body.example ?? generateExample(op.body.schema, true), null, 2) : "";
      return { body: { kind: "json", value }, contentType: ct };
    }
    if (ct.includes("x-www-form-urlencoded")) {
      const rows: KeyValue[] = Object.keys(props).map((name) => ({ location: "query", name, value: "", enabled: true }));
      return { body: { kind: "form", value: rows }, contentType: ct };
    }
    if (ct.includes("multipart")) {
      return {
        body: {
          kind: "multipart",
          value: Object.entries(props).map(([name, s]) =>
            s.format === "binary"
              ? { name, value: { kind: "file" as const, value: "" }, enabled: false }
              : { name, value: { kind: "text" as const, value: "" }, enabled: true },
          ),
        },
        contentType: ct,
      };
    }
    if (ct.startsWith("text/")) return { body: { kind: "text", value: "" }, contentType: ct };
    return { body: { kind: "binary", value: "" }, contentType: ct };
  };

  const findOp = (projectId: string, opKey: OpKey): { op: MockOp; deleted: boolean; api: ProjectApi } => {
    const api = apiOf(projectId);
    const key = opKeyString(opKey);
    const live = api.ops.find((o) => opKeyString(mockOpKey(o)) === key && api.imported.has(key));
    if (live) return { op: live, deleted: false, api };
    const gone = api.deleted.get(key);
    if (gone) return { op: gone, deleted: true, api };
    throw notFound(`API が見つかりません: ${key}`);
  };

  /** 送信のときの既定の認証（モック）。`security: []` や API がなければ `null`。 */
  const defaultAuthName = (projectId: string, opKey: OpKey): string | null => {
    try {
      const { op } = findOp(projectId, opKey);
      return op.security === "none" ? null : MOCK_DEFAULT_AUTH.variable;
    } catch {
      return null;
    }
  };

  /** method と path で、取り込み済みの API に当てはまるもの（literal な区間が多いものを優先）。 */
  const matchOperation = (projectId: string, method: string, pathname: string): { opKey: OpKey; op: MockOp; params: Record<string, string> } | null => {
    const api = apis.get(projectId);
    if (!api) return null;
    let best: { op: MockOp; params: Record<string, string>; literal: number } | null = null;
    for (const op of api.ops) {
      if (op.method !== method || !api.imported.has(opKeyString(mockOpKey(op)))) continue;
      const tpl = op.path.split("/");
      const act = pathname.split("/");
      if (tpl.length !== act.length) continue;
      const params: Record<string, string> = {};
      let literal = 0;
      let ok = true;
      tpl.forEach((seg, i) => {
        const m = /^\{(.+)\}$/.exec(seg);
        if (m) params[m[1]] = act[i];
        else if (seg === act[i]) literal += 1;
        else ok = false;
      });
      if (ok && (!best || literal > best.literal)) best = { op, params, literal };
    }
    return best ? { opKey: mockOpKey(best.op), op: best.op, params: best.params } : null;
  };

  const methods: Pick<
    Backend,
    | "openapiPreview"
    | "openapiImport"
    | "openapiSyncCheck"
    | "openapiSyncApply"
    | "openapiSourceChanged"
    | "operationList"
    | "operationGet"
    | "operationOpen"
    | "operationForget"
    | "sourceTokenSet"
    | "editorOpen"
    | "draftSave"
    | "draftGet"
  > = {
    async openapiPreview(source, projectId) {
      const special = classify(source);
      if (special) return special;
      return { status: "ready", preview: previewOf(source, projectId) };
    },

    async openapiImport(request: ImportRequest): Promise<ImportResult> {
      const special = classify(request.source);
      if (special) return asImportResult(special);
      if (request.selected !== null && request.selected.length === 0) throw validation("取り込む API を 1 つ以上選んでください");

      const preview = previewOf(request.source, request.projectId);
      const all = mockSpecOps(disk);
      const selectedKeys = new Set((request.selected ?? all.map(mockOpKey)).map(opKeyString));

      let project: Project;
      let diff: SyncDiff | null = null;
      let report: ImportReport = { added: all.length, changed: 0, removed: 0, keptAsDeleted: 0, renamed: 0 };
      const existing = request.projectId ? apis.get(request.projectId) : undefined;
      if (request.projectId) {
        project = host.projectOf(request.projectId);
        diff = preview.diff;
      } else {
        const name = request.projectName?.trim();
        project = host.createProject(name && name !== "" ? name : preview.title);
      }

      if (existing) {
        report = applyOps(project.id, existing, all);
        for (const k of selectedKeys) existing.imported.add(k);
        existing.options = request.options;
      } else {
        apis.set(project.id, {
          ops: all,
          version: disk,
          imported: selectedKeys,
          options: request.options,
          deleted: new Map(),
          stale: new Map(),
          importedAt: host.now(),
          kind: request.source.kind === "upload" ? "copy" : request.source.kind,
          entry: entryName(request.source),
        });
      }

      const at = host.now();
      const api = apis.get(project.id)!;
      const created: string[] = [];
      for (const s of MOCK_SPEC_INFO.servers) {
        const name = s.description ?? s.url;
        if (host.addEnvironment(project.id, name, s.url)) created.push(name);
      }
      const source: ApiSource = {
        id: host.newId("mock-source"),
        kind: api.kind,
        entry: api.entry,
        files: sourceFiles(preview, at),
        openapiVersion: preview.openapiVersion,
        syncedAt: at,
        hasToken: request.source.kind === "url" && (request.source.token ?? "") !== "",
        allowedHosts: request.source.kind === "url" ? request.source.allowedHosts : [],
        options: request.options,
        meta: {
          title: preview.title,
          version: preview.version,
          description: preview.description,
          servers: preview.servers,
          securitySchemes: [{ name: "cookieAuth", kind: "apiKey", location: "cookie", keyName: "SESSION", scheme: null, bearerFormat: null, description: null }],
          rootSecurity: [{ schemes: [{ name: "cookieAuth", scopes: [] }] }],
          tags: Object.entries(MOCK_SPEC_INFO.tagDescriptions).map(([name, description]) => ({ name, description })),
        },
      };
      project = { ...host.projectOf(project.id), source, updatedAt: at };
      host.updateProject(project);
      return { status: "imported", project, operationCount: selectedKeys.size, report, createdEnvironments: created, diff };
    },

    async openapiSyncCheck(projectId, upload: UploadInput | null, allowedHosts): Promise<SyncCheckResult> {
      const api = apiOf(projectId);
      if (api.kind === "copy" && upload === null) return { status: "needsUpload", entry: api.entry };
      const foreign = foreignFor(projectId, allowedHosts);
      if (foreign) return foreign;
      if (broken) return { status: "blocked", problems: sourceBlocked() };
      const next = mockSpecOps(disk);
      const diff = diffOps(projectId, api.ops, next);
      return { status: "ready", diff, changedFiles: diffIsEmpty(diff) ? [] : changedFilesOf(api.ops, next) };
    },

    async openapiSyncApply(projectId, upload: UploadInput | null, allowedHosts): Promise<SyncApplyResult> {
      const api = apiOf(projectId);
      if (api.kind === "copy" && upload === null) return { status: "needsUpload", entry: api.entry };
      const foreign = foreignFor(projectId, allowedHosts);
      if (foreign && foreign.status === "needsConfirmation") return foreign;
      if (broken) return { status: "blocked", problems: sourceBlocked() };
      const next = mockSpecOps(disk);
      const diff = diffOps(projectId, api.ops, next);
      const report = applyOps(projectId, api, next);
      for (const o of next) if (!api.imported.has(opKeyString(mockOpKey(o))) && diff.added.some((a) => opKeyString(a.opKey) === opKeyString(mockOpKey(o)))) {
        api.imported.add(opKeyString(mockOpKey(o)));
      }
      const project = host.projectOf(projectId);
      const updated: Project = {
        ...project,
        source: project.source ? { ...project.source, syncedAt: host.now() } : project.source,
        updatedAt: host.now(),
      };
      host.updateProject(updated);
      return { status: "applied", diff, report, project: updated };
    },

    async openapiSourceChanged(projectId) {
      const api = apiOf(projectId);
      const changed = api.kind === "path" && api.version !== disk;
      return { changed, changedFiles: changed ? changedFilesOf(api.ops, mockSpecOps(disk)) : [] };
    },

    async operationList(projectId) {
      const api = apis.get(projectId);
      host.projectOf(projectId);
      if (!api) return [];
      const item = (op: MockOp, deleted: boolean): OperationListItem => ({
        opKey: mockOpKey(op),
        operationId: op.operationId,
        method: op.method,
        path: op.path,
        summary: op.summary,
        folder: api.options.tagFolders ? op.tag : null,
        tags: op.tag ? [op.tag] : [],
        requiresAuth: op.security !== "none",
        deleted,
        hasDraft: hasDraftKey(drafts, projectId, mockOpKey(op)),
      });
      return [
        ...api.ops.filter((o) => api.imported.has(opKeyString(mockOpKey(o)))).map((o) => item(o, false)),
        ...[...api.deleted.values()].map((o) => item(o, true)),
      ];
    },

    async operationGet(projectId, opKey) {
      const { op, deleted, api } = findOp(projectId, opKey);
      const detail = mockOperationDetail(op, api.importedAt, deleted);
      return api.options.descriptionAsDoc ? detail : { ...detail, description: null };
    },

    async operationOpen(projectId, opKey): Promise<OperationOpen> {
      const { op, deleted, api } = findOp(projectId, opKey);
      const key = opKeyString(opKey);
      const draft = drafts.get(`${projectId}|${key}`);
      const fill = api.options.fillFromExamples;
      const params: KeyValue[] = draft
        ? draft.params
        : op.parameters.map((p) => ({
            location: p.in,
            name: p.name,
            value: fill ? String(p.example ?? p.default ?? "") : "",
            enabled: p.in === "path",
          }));
      const { body, contentType } = draft ? { body: draft.body, contentType: op.body?.contentType ?? null } : defaultBodyOf(op, api.options);
      return {
        opKey,
        method: op.method,
        path: op.path,
        summary: op.summary,
        request: { method: op.method, url: `{{baseUrl}}${op.path}`, params, body, auth: draft?.auth ?? null },
        hasDraft: draft !== undefined,
        bodyContentType: contentType,
        missingInYaml: api.stale.get(key) ?? [],
        requiredParams: op.parameters.filter((p) => p.required).map((p): ParamRef => ({ location: p.in, name: p.name })),
        deleted,
        defaultAuth: op.security === "none" ? null : MOCK_DEFAULT_AUTH,
      };
    },

    async operationForget(projectId, opKey) {
      const api = apiOf(projectId);
      const key = opKeyString(opKey);
      if (!api.deleted.has(key)) throw validation("削除された API だけ消せます");
      api.deleted.delete(key);
      api.stale.delete(key);
      drafts.delete(`${projectId}|${key}`);
    },

    async sourceTokenSet(projectId, token) {
      const project = host.projectOf(projectId);
      if (!project.source) throw validation("取り込み元がありません");
      const updated: Project = { ...project, source: { ...project.source, hasToken: token !== "" } };
      host.updateProject(updated);
      return updated;
    },

    async editorOpen(projectId, file) {
      const project = host.projectOf(projectId);
      if (!project.source?.files.some((f) => f.path === file) && !mockSpecOps(2).some((o) => o.file === file)) {
        throw validation(`取り込んだファイルの一覧にありません: ${file}`);
      }
    },

    async draftSave(draft) {
      host.projectOf(draft.projectId);
      const key = `${draft.projectId}|${opKeyString(draft.opKey)}`;
      const saved: RequestDraft = { ...JSON.parse(JSON.stringify(draft)), id: drafts.get(key)?.id ?? host.newId("mock-draft"), updatedAt: host.now() };
      drafts.set(key, saved);
      // 画面で消して保存すると「yaml にない」の印も外れる
      const api = apis.get(draft.projectId);
      const marks = api?.stale.get(opKeyString(draft.opKey));
      if (api && marks) {
        const left = marks.filter((m) => saved.params.some((p) => p.location === m.location && p.name === m.name));
        if (left.length > 0) api.stale.set(opKeyString(draft.opKey), left);
        else api.stale.delete(opKeyString(draft.opKey));
      }
      return JSON.parse(JSON.stringify(saved)) as RequestDraft;
    },

    async draftGet(projectId, opKey) {
      host.projectOf(projectId);
      const d = drafts.get(`${projectId}|${opKeyString(opKey)}`);
      return d ? (JSON.parse(JSON.stringify(d)) as RequestDraft) : null;
    },
  };

  const controls: MockControls = {
    touchSource: () => {
      disk = 2;
    },
    breakSource: (on = true) => {
      broken = on;
    },
    requireHostConfirm: (on = true) => {
      needHostConfirm = on;
    },
    operationCount: (projectId) => apis.get(projectId)?.imported.size ?? 0,
    inflate: (projectId, count) => {
      const api = apiOf(projectId);
      const template = mockSpecOps(1)[4];
      for (let i = 0; i < count; i += 1) {
        const op: MockOp = {
          ...template,
          operationId: `bulkOp${i}`,
          path: `/api/v1/bulk/${i}`,
          tag: `bulk-tag-${i % 20}`,
          summary: `合成 API ${i + 1}`,
        };
        api.ops.push(op);
        api.imported.add(opKeyString(mockOpKey(op)));
      }
    },
  };

  /** 履歴の見出し（API の summary）。 */
  const summaryOf = (projectId: string, opKey: OpKey): string | null => {
    try {
      return findOp(projectId, opKey).op.summary;
    } catch {
      return null;
    }
  };

  return { methods, controls, defaultAuthName, matchOperation, summaryOf };
}
