import { ApiError } from "../api/backend";
import type {
  AdHocRequest,
  DiffLine,
  HistoryDiff,
  HistoryEntry,
  HistoryListItem,
  HistoryOpen,
  HistoryPage,
  HistoryQuery,
  OpKey,
  RequestDraft,
  RequestInput,
  RequestSpec,
  SavedInput,
  SendOutcome,
} from "../bindings";
import type { Assembled } from "./build";
import { renderCurl } from "./curl";
import { dropMasked, isSecretHeader, MASK, redactText, scrubManualSecrets } from "./scrub";

/** 履歴の仮想スクロールなどを確かめるために、ブラウザ・テストから履歴を足す操作。 */
export interface HistoryControls {
  /** 過去の送信を `count` 件作る（日付をまたぐ。ステータスは 2xx・4xx・5xx・失敗が混ざる）。 */
  seedHistory(projectId: string, count: number): void;
  historyCount(projectId: string): number;
}

export interface HistoryHost {
  now(): string;
  newId(prefix: string): string;
  /** 保存する履歴の件数の上限（設定。0 は保存しない）。 */
  limit(): number;
  /** 1 MB を超えるボディを先頭だけ保存する設定。 */
  omitLargeBodies(): boolean;
  projectExists(projectId: string): boolean;
  saveDraft(projectId: string, opKey: OpKey, spec: RequestSpec): Promise<RequestDraft>;
  saveAdhoc(projectId: string, name: string, spec: RequestSpec): Promise<AdHocRequest>;
  send(input: RequestInput): Promise<string>;
}

type Stored = HistoryEntry & { seq: number };

const notFound = (id: string) => new ApiError({ kind: "notFound", message: `履歴が見つかりません: ${id}` });
const ONE_MB = 1_000_000;
const DEFAULTS = ["user-agent", "accept", "accept-encoding", "host"];

const statusClassOf = (e: HistoryEntry): string => (e.status === null ? "failed" : `${Math.floor(e.status / 100)}xx`);

function localDateOf(sentAt: string, offsetMinutes: number): string {
  return new Date(Date.parse(sentAt) + offsetMinutes * 60_000).toISOString().slice(0, 10);
}

function toItem(e: HistoryEntry, offset: number): HistoryListItem {
  return {
    id: e.id,
    sentAt: e.sentAt,
    localDate: localDateOf(e.sentAt, offset),
    method: e.method,
    url: e.url,
    status: e.status,
    statusText: e.statusText,
    totalMs: e.timing?.totalMs ?? null,
    environmentId: e.environmentId,
    environmentName: e.environmentName,
    opKey: e.opKey,
    opSummary: e.opSummary,
    failed: e.status === null,
    errorKind: e.errorKind,
    responseSize: e.responseSize,
  };
}

const statusLabel = (e: HistoryEntry) => (e.status === null ? `失敗: ${e.error ?? "不明"}` : `${e.status} ${e.statusText ?? ""}`.trim());

function prettyBody(e: HistoryEntry): { lines: string[]; note: string | null } {
  if (e.responseBinary) return { lines: [], note: "バイナリのため比べません" };
  const text = e.responseBody ?? "";
  try {
    return { lines: JSON.stringify(JSON.parse(text), null, 2).split("\n"), note: "JSON として整形して比べました" };
  } catch {
    return { lines: text === "" ? [] : text.split("\n"), note: null };
  }
}

/** 行単位の差分（LCS）。変わらない行は前後 3 行だけ残して `skip` にする。 */
export function diffLines(a: string[], b: string[]): { lines: DiffLine[]; changed: boolean } {
  const n = a.length;
  const m = b.length;
  const lcs: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i -= 1) for (let j = m - 1; j >= 0; j -= 1) lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
  const raw: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && a[i] === b[j]) {
      raw.push({ tag: "equal", text: a[i], leftLine: i + 1, rightLine: j + 1 });
      i += 1;
      j += 1;
    } else if (j < m && (i >= n || lcs[i][j + 1] >= lcs[i + 1][j])) {
      raw.push({ tag: "insert", text: b[j], leftLine: null, rightLine: j + 1 });
      j += 1;
    } else {
      raw.push({ tag: "delete", text: a[i], leftLine: i + 1, rightLine: null });
      i += 1;
    }
  }
  if (raw.every((l) => l.tag === "equal")) return { lines: [], changed: false };
  const keep = raw.map(() => false);
  raw.forEach((l, k) => {
    if (l.tag === "equal") return;
    for (let d = -3; d <= 3; d += 1) if (raw[k + d]) keep[k + d] = true;
  });
  const out: DiffLine[] = [];
  let skipped = 0;
  raw.forEach((l, k) => {
    if (keep[k]) {
      if (skipped > 0) out.push({ tag: "skip", text: `${skipped} 行省略`, leftLine: null, rightLine: null });
      skipped = 0;
      out.push(l);
    } else skipped += 1;
  });
  if (skipped > 0) out.push({ tag: "skip", text: `${skipped} 行省略`, leftLine: null, rightLine: null });
  return { lines: out, changed: true };
}

export function createHistoryMock(host: HistoryHost) {
  let entries: Stored[] = [];
  let seq = 0;

  const find = (id: string): Stored => {
    const e = entries.find((x) => x.id === id);
    if (!e) throw notFound(id);
    return e;
  };

  function trim(projectId: string) {
    const limit = host.limit();
    const mine = entries.filter((e) => e.projectId === projectId).sort((a, b) => b.seq - a.seq);
    for (const old of mine.slice(Math.max(limit, 0))) entries = entries.filter((e) => e.id !== old.id);
  }

  /** 送信（成功・失敗）を 1 件残す。伏せ字にしてから保存する。 */
  function record(args: {
    input: RequestInput;
    environmentId: string | null;
    environmentName: string | null;
    opSummary: string | null;
    assembled: Assembled;
    outcome: SendOutcome;
    sentAt?: string;
  }): HistoryEntry | null {
    if (host.limit() <= 0) return null;
    const { input, assembled: a, outcome } = args;
    const secrets = a.secrets;
    const red = (t: string) => redactText(t, secrets);
    const headers = [...a.headers, { name: "User-Agent", value: "NekoReq/0.1.0" }, { name: "Accept", value: "*/*" }, { name: "Accept-Encoding", value: "gzip, br, deflate" }]
      .filter((h, i, all) => all.findIndex((x) => x.name.toLowerCase() === h.name.toLowerCase()) === i)
      .map((h) => ({ name: h.name, value: isSecretHeader(h.name) ? MASK : red(h.value) }));

    const body = input.request.body;
    let requestBody: string | null = null;
    let kind: HistoryEntry["requestBodyKind"] = "none";
    let size = 0;
    let truncated = false;
    const parts = a.parts.map((p) => ({ name: p.name, isFile: p.isFile, value: red(p.value), contentType: null }));
    let requestFile: string | null = null;
    if (a.bodyText !== null) {
      kind = "text";
      size = new TextEncoder().encode(a.bodyText).length;
      requestBody = red(a.bodyText);
      if (host.omitLargeBodies() && requestBody.length > ONE_MB) {
        requestBody = requestBody.slice(0, ONE_MB);
        truncated = true;
      }
    } else if (parts.length > 0) {
      kind = "multipart";
      requestBody = parts.filter((p) => !p.isFile).map((p) => `${p.name}=${p.value}`).join("\n");
    } else if (body.kind === "binary" && a.hasBody) {
      kind = "file";
      requestFile = red(body.value);
    }

    const scrubbed = scrubManualSecrets(input.request, MASK);
    const base: HistoryEntry = {
      id: host.newId("mock-history"),
      projectId: input.projectId,
      sentAt: args.sentAt ?? host.now(),
      environmentId: args.environmentId,
      environmentName: args.environmentName,
      opKey: input.opKey ?? null,
      opSummary: args.opSummary,
      method: input.request.method,
      url: red(a.rawUrl),
      finalUrl: null,
      requestHeaders: headers,
      requestBody,
      requestBodyKind: kind,
      requestBodySize: size,
      requestBodyTruncated: truncated,
      requestParts: parts,
      requestFile,
      status: null,
      statusText: null,
      responseHeaders: [],
      responseContentType: null,
      responseBody: null,
      responseBinary: false,
      responseBodyTruncated: false,
      responseSize: 0,
      timing: null,
      error: null,
      errorKind: null,
      input: scrubbed.spec,
      maskedFields: scrubbed.fields,
      overrideNames: input.overrides.map((o) => o.name),
    };
    if (outcome.status === "success") {
      const r = outcome.response;
      const binary = r.body.encoding === "base64";
      base.status = r.status;
      base.statusText = r.statusText;
      base.responseHeaders = r.headers.map((h) => ({ name: h.name, value: isSecretHeader(h.name) ? MASK : h.value }));
      base.responseContentType = r.body.contentType;
      base.responseBinary = binary;
      base.responseBody = binary ? null : red(r.body.content);
      base.responseBodyTruncated = r.body.truncated;
      base.responseSize = r.body.size;
      base.timing = r.timing;
      if (r.finalUrl !== a.rawUrl) base.finalUrl = red(r.finalUrl);
    } else {
      base.error = red(outcome.error.message);
      base.errorKind = outcome.error.kind;
    }
    seq += 1;
    entries.push({ ...base, seq });
    trim(input.projectId);
    return base;
  }

  const methods = {
    async historyList(projectId: string, query: HistoryQuery): Promise<HistoryPage> {
      const offset = query.utcOffsetMinutes ?? -new Date().getTimezoneOffset();
      const text = (query.text ?? "").trim().toLowerCase();
      const statuses = query.statuses ?? [];
      let list = entries.filter((e) => e.projectId === projectId);
      if (statuses.length > 0) list = list.filter((e) => statuses.includes(statusClassOf(e) as never));
      if (query.environmentId) list = list.filter((e) => e.environmentId === query.environmentId);
      if (query.from) list = list.filter((e) => localDateOf(e.sentAt, offset) >= (query.from as string));
      if (query.to) list = list.filter((e) => localDateOf(e.sentAt, offset) <= (query.to as string));
      if (text !== "") list = list.filter((e) => [e.url, e.requestBody ?? "", e.responseBody ?? ""].some((t) => t.toLowerCase().includes(text)));
      list = [...list].sort((a, b) => (a.sentAt === b.sentAt ? b.seq - a.seq : a.sentAt < b.sentAt ? 1 : -1));
      const limit = Math.min(Math.max(query.limit ?? 100, 1), 500);
      const start = query.cursor ? Number.parseInt(query.cursor, 10) || 0 : 0;
      const page = list.slice(start, start + limit);
      return { items: page.map((e) => toItem(e, offset)), total: list.length, nextCursor: start + limit < list.length ? String(start + limit) : null };
    },

    async historyGet(id: string): Promise<HistoryEntry> {
      const { seq: _seq, ...entry } = find(id);
      void _seq;
      return JSON.parse(JSON.stringify(entry)) as HistoryEntry;
    },

    async historyDelete(id: string): Promise<void> {
      find(id);
      entries = entries.filter((e) => e.id !== id);
    },

    async historyClear(projectId: string): Promise<number> {
      const n = entries.filter((e) => e.projectId === projectId).length;
      entries = entries.filter((e) => e.projectId !== projectId);
      return n;
    },

    async historyOpen(id: string): Promise<HistoryOpen> {
      const e = find(id);
      if (!e.input) throw new ApiError({ kind: "validation", message: "この履歴には元の入力がありません" });
      return {
        opKey: e.opKey,
        summary: e.opSummary,
        request: dropMasked(e.input),
        environmentId: e.environmentId,
        environmentName: e.environmentName,
        droppedFields: [...e.maskedFields],
        overrideNames: [...e.overrideNames],
      };
    },

    async historyResend(id: string): Promise<string> {
      const e = find(id);
      if (!e.input) throw new ApiError({ kind: "validation", message: "この履歴には元の入力がありません" });
      return host.send({ projectId: e.projectId, environmentId: null, request: dropMasked(e.input), overrides: [], opKey: e.opKey });
    },

    async historySaveAsInput(id: string, name: string | null): Promise<SavedInput> {
      const e = find(id);
      if (!e.input) throw new ApiError({ kind: "validation", message: "この履歴には元の入力がありません" });
      const spec = dropMasked(e.input);
      if (e.opKey) {
        return { kind: "draft", draft: await host.saveDraft(e.projectId, e.opKey, spec), dropped: [...e.maskedFields] };
      }
      let path = e.url;
      try {
        path = new URL(e.url).pathname;
      } catch {
        /* そのまま */
      }
      const label = (name ?? "").trim() || `${e.method} ${path}`;
      return { kind: "adhoc", request: await host.saveAdhoc(e.projectId, label, spec), dropped: [...e.maskedFields] };
    },

    async historyCurl(id: string): Promise<string> {
      const e = find(id);
      return renderCurl({
        method: e.method,
        url: e.url,
        headers: e.requestHeaders.filter((h) => !DEFAULTS.includes(h.name.toLowerCase())),
        bodyText: e.requestBodyKind === "text" ? e.requestBody : null,
        parts: e.requestParts.map((p) => ({ name: p.name, value: p.value, isFile: p.isFile })),
      });
    },

    async historyCompare(leftId: string, rightId: string): Promise<HistoryDiff> {
      const l = find(leftId);
      const r = find(rightId);
      const offset = -new Date().getTimezoneOffset();
      const hdr = (e: HistoryEntry) =>
        [...e.responseHeaders].sort((a, b) => a.name.toLowerCase().localeCompare(b.name.toLowerCase())).map((h) => `${h.name.toLowerCase()}: ${h.value}`);
      const headers = diffLines(hdr(l), hdr(r));
      const lb = prettyBody(l);
      const rb = prettyBody(r);
      const body = l.responseBinary || r.responseBinary ? { lines: [], changed: false } : diffLines(lb.lines, rb.lines);
      const bodyNote = l.responseBinary || r.responseBinary ? "バイナリのため比べません" : (rb.note ?? lb.note);
      return {
        left: toItem(l, offset),
        right: toItem(r, offset),
        leftStatus: statusLabel(l),
        rightStatus: statusLabel(r),
        statusChanged: statusLabel(l) !== statusLabel(r),
        headers: headers.lines,
        headersChanged: headers.changed,
        body: body.lines,
        bodyChanged: body.changed,
        bodyNote,
      };
    },
  };

  const controls: HistoryControls = {
    seedHistory(projectId, count) {
      if (!host.projectExists(projectId)) return;
      const kinds = [
        { method: "GET", path: "/api/v1/video/clients?q=Netflix", status: 200, text: "OK" },
        { method: "POST", path: "/api/v1/auth/password-login", status: 400, text: "Bad Request" },
        { method: "GET", path: "/api/v1/auth/session", status: 200, text: "OK" },
        { method: "PUT", path: "/api/v1/video/works/1001", status: 422, text: "Unprocessable Entity" },
        { method: "DELETE", path: "/api/v1/video/media/3", status: 500, text: "Internal Server Error" },
        { method: "GET", path: "/api/v1/video/invoices?overdue=true", status: 0, text: "" },
      ];
      const now = Date.now();
      for (let i = 0; i < count; i += 1) {
        const k = kinds[i % kinds.length];
        const failed = k.status === 0;
        const problem = k.status >= 400;
        const body = problem
          ? JSON.stringify({ title: k.text, status: k.status, code: k.status === 500 ? "INTERNAL_ERROR" : "VALIDATION_FAILED", detail: "入力内容に誤りがあります" })
          : JSON.stringify({ data: { items: [{ clientId: i, clientName: `取引先 ${i}` }], total: 1 } });
        seq += 1;
        entries.push({
          id: host.newId("mock-history"),
          projectId,
          sentAt: new Date(now - i * 47 * 60_000 - 5_000).toISOString(),
          environmentId: null,
          environmentName: "ローカル (Dev Container)",
          opKey: null,
          opSummary: null,
          method: k.method,
          url: `http://localhost:8080${k.path}`,
          finalUrl: null,
          requestHeaders: [
            { name: "Content-Type", value: "application/json" },
            { name: "X-XSRF-TOKEN", value: MASK },
          ],
          requestBody: k.method === "GET" || k.method === "DELETE" ? null : JSON.stringify({ email: "", password: MASK }, null, 2),
          requestBodyKind: k.method === "GET" || k.method === "DELETE" ? "none" : "text",
          requestBodySize: 0,
          requestBodyTruncated: false,
          requestParts: [],
          requestFile: null,
          status: failed ? null : k.status,
          statusText: failed ? null : k.text,
          responseHeaders: failed ? [] : [{ name: "content-type", value: problem ? "application/problem+json" : "application/json" }],
          responseContentType: failed ? null : problem ? "application/problem+json" : "application/json",
          responseBody: failed ? null : body,
          responseBinary: false,
          responseBodyTruncated: false,
          responseSize: body.length,
          timing: failed ? null : { dnsMs: 1, connectMs: 1, tlsMs: null, sendMs: 1, waitMs: 10 + (i % 40), downloadMs: 2, totalMs: 15 + (i % 40) },
          error: failed ? "接続できません（localhost:8080 に接続を拒否されました）" : null,
          errorKind: failed ? "connect" : null,
          input: { method: k.method, url: `{{baseUrl}}${k.path}`, params: [], body: { kind: "none" }, auth: null },
          maskedFields: [],
          overrideNames: [],
          seq,
        });
      }
      trim(projectId);
    },
    historyCount: (projectId) => entries.filter((e) => e.projectId === projectId).length,
  };

  return { methods, controls, record };
}
