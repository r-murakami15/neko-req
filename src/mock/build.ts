import type { AutoHeader, KeyValue, ParamIn, RequestPreview, RequestSpec, UnresolvedVar, VarLocation } from "../bindings";

/** 変数の値。シークレットなら `secret` が立つ。 */
export interface ScopedValue {
  value: string;
  secret: boolean;
}
export type Lookup = (name: string) => ScopedValue | undefined;

const MASK = "••••";

function isVariableName(name: string): boolean {
  return name !== "" && !/[{}\r\n]/.test(name);
}

function dynamic(name: string): string | undefined {
  switch (name) {
    case "$uuid":
      return "00000000-0000-4000-8000-000000000000";
    case "$timestamp":
      return String(Math.floor(Date.now() / 1000));
    case "$isoTimestamp":
      return new Date().toISOString();
    case "$randomInt":
      return String(Math.floor(Math.random() * 1001));
    default:
      return undefined;
  }
}

interface Expanded {
  text: string;
  unresolved: string[];
  secrets: string[];
}

/** Rust 側（core の vars.rs）と同じ: 1 回だけ展開し、展開できない変数は `{{name}}` のまま残す。 */
export function expand(template: string, lookup: Lookup): Expanded {
  const out: Expanded = { text: "", unresolved: [], secrets: [] };
  let rest = template;
  for (;;) {
    const start = rest.indexOf("{{");
    if (start < 0) break;
    out.text += rest.slice(0, start);
    const after = rest.slice(start + 2);
    const end = after.indexOf("}}");
    if (end < 0) {
      out.text += rest.slice(start);
      return out;
    }
    const raw = after.slice(0, end);
    const name = raw.trim();
    if (!isVariableName(name)) {
      out.text += "{";
      rest = rest.slice(start + 1);
      continue;
    }
    let found: ScopedValue | undefined;
    if (name.startsWith("$")) {
      const v = dynamic(name);
      found = v === undefined ? undefined : { value: v, secret: false };
    } else {
      found = lookup(name);
    }
    if (found) {
      if (found.secret && !out.secrets.includes(found.value)) out.secrets.push(found.value);
      out.text += found.value;
    } else {
      if (!out.unresolved.includes(name)) out.unresolved.push(name);
      out.text += `{{${raw}}}`;
    }
    rest = after.slice(end + 2);
  }
  out.text += rest;
  return out;
}

function redact(text: string, secrets: string[]): string {
  let out = text;
  for (const s of secrets) if (s !== "") out = out.split(s).join(MASK);
  return out;
}

function rowsOf(params: KeyValue[], location: ParamIn): { row: number; p: KeyValue }[] {
  return params.filter((p) => p.location === location).map((p, i) => ({ row: i + 1, p }));
}

function encode(s: string, keep: string[] = []): string {
  if (keep.length === 0) return encodeURIComponent(s);
  // 展開できなかった変数 `{{name}}` は、プレビューで見えるようにそのまま残す（Rust 側の encode_query_component と同じ）
  let out = "";
  let rest = s;
  for (;;) {
    const start = rest.indexOf("{{");
    if (start < 0) break;
    const end = rest.indexOf("}}", start + 2);
    if (end >= 0 && keep.includes(rest.slice(start + 2, end).trim())) {
      out += encodeURIComponent(rest.slice(0, start)) + rest.slice(start, end + 2);
      rest = rest.slice(end + 2);
    } else {
      out += encodeURIComponent(rest.slice(0, start + 1));
      rest = rest.slice(start + 1);
    }
  }
  return out + encodeURIComponent(rest);
}

/** `://` が最初の `/` `?` `#` より前にあればスキームがある。 */
function hasScheme(url: string): boolean {
  const i = url.indexOf("://");
  if (i < 0) return false;
  const d = url.search(/[/?#]/);
  return d < 0 || i < d;
}

export interface Assembled {
  preview: RequestPreview;
  secrets: string[];
  hasBody: boolean;
  /** 展開後（シークレットも実際の値）。 */
  rawUrl: string;
}

/** 送信用の組み立て（core の build.rs を、確認用に必要な範囲で真似たもの）。 */
export function assemble(spec: RequestSpec, lookup: Lookup): Assembled {
  const unresolved: UnresolvedVar[] = [];
  const secrets: string[] = [];
  const ex = (template: string, location: VarLocation): string => {
    const e = expand(template, lookup);
    for (const name of e.unresolved) {
      if (!unresolved.some((u) => u.name === name && JSON.stringify(u.location) === JSON.stringify(location))) {
        unresolved.push({ name, location });
      }
    }
    for (const s of e.secrets) if (!secrets.includes(s)) secrets.push(s);
    return e.text;
  };

  // パスパラメータ
  const pathValues: [string, string][] = [];
  for (const { row, p } of rowsOf(spec.params, "path")) {
    if (!p.enabled) continue;
    const name = ex(p.name, { kind: "param", location: "path", row, part: "name" });
    const value = ex(p.value, { kind: "param", location: "path", row, part: "value" });
    if (name.trim() !== "") pathValues.push([name.trim(), value]);
  }

  // URL
  const expandedUrl = ex(spec.url.trim(), { kind: "url" });
  const missing: string[] = [];
  let urlText = "";
  {
    let rest = expandedUrl;
    for (;;) {
      const i = rest.indexOf("{");
      if (i < 0) break;
      urlText += rest.slice(0, i);
      const tail = rest.slice(i);
      if (tail.startsWith("{{")) {
        const end = tail.indexOf("}}", 2);
        if (end < 0) {
          urlText += "{{";
          rest = tail.slice(2);
        } else {
          urlText += tail.slice(0, end + 2);
          rest = tail.slice(end + 2);
        }
        continue;
      }
      const end = tail.indexOf("}");
      if (end < 0) {
        urlText += "{";
        rest = tail.slice(1);
        continue;
      }
      const name = tail.slice(1, end);
      const hit = pathValues.find(([n]) => n === name);
      if (hit && hit[1] !== "" && hit[1] !== "." && hit[1] !== "..") {
        urlText += encodeURIComponent(hit[1]);
      } else {
        // 行がない・値が空・値が `.` / `..`（1 区間として送れない）は埋めずに警告
        if ((hit || /^[\p{L}\p{N}_.-]+$/u.test(name)) && !missing.includes(name)) missing.push(name);
        urlText += tail.slice(0, end + 1);
      }
      rest = tail.slice(end + 1);
    }
    urlText += rest;
  }

  // クエリ
  const query: [string, string][] = [];
  for (const { row, p } of rowsOf(spec.params, "query")) {
    if (!p.enabled) continue;
    const name = ex(p.name, { kind: "param", location: "query", row, part: "name" });
    const value = ex(p.value, { kind: "param", location: "query", row, part: "value" });
    if (name !== "") query.push([name, value]);
  }

  // ヘッダー・Cookie
  const headers: { name: string; value: string }[] = [];
  for (const { row, p } of rowsOf(spec.params, "header")) {
    if (!p.enabled) continue;
    const name = ex(p.name, { kind: "param", location: "header", row, part: "name" });
    const value = ex(p.value, { kind: "param", location: "header", row, part: "value" });
    if (name.trim() !== "") headers.push({ name: name.trim(), value });
  }
  const cookies: string[] = [];
  for (const { row, p } of rowsOf(spec.params, "cookie")) {
    if (!p.enabled) continue;
    const name = ex(p.name, { kind: "param", location: "cookie", row, part: "name" });
    const value = ex(p.value, { kind: "param", location: "cookie", row, part: "value" });
    if (name.trim() !== "") cookies.push(`${name.trim()}=${value}`);
  }

  const auto: AutoHeader[] = [];
  const hasHeader = (name: string) => headers.some((h) => h.name.toLowerCase() === name.toLowerCase());
  const addAuto = (name: string, value: string, source: string) => {
    if (hasHeader(name)) return;
    auto.push({ name, value, source });
    headers.push({ name, value });
  };

  // 認証
  const a = spec.auth;
  if (a?.type === "bearer") {
    const token = ex(a.token, { kind: "auth", field: "token" });
    addAuto("Authorization", `Bearer ${token.trim()}`, "認証（Bearer）");
  } else if (a?.type === "basic") {
    const user = ex(a.username, { kind: "auth", field: "username" });
    const pass = ex(a.password, { kind: "auth", field: "password" });
    addAuto("Authorization", `Basic ${btoa(unescape(encodeURIComponent(`${user}:${pass}`)))}`, "認証（Basic）");
  } else if (a?.type === "apiKey") {
    const name = ex(a.name, { kind: "auth", field: "name" }).trim();
    const value = ex(a.value, { kind: "auth", field: "value" });
    if (name !== "") {
      if (a.location === "header") addAuto(name, value, "認証（API キー）");
      else if (a.location === "query") query.push([name, value]);
      else cookies.push(`${name}=${value}`);
    }
  }
  if (cookies.length > 0) addAuto("Cookie", cookies.join("; "), "Cookie・認証");

  // ボディ
  let hasBody = false;
  const b = spec.body;
  switch (b.kind) {
    case "json": {
      const t = ex(b.value, { kind: "body" });
      if (t.trim() !== "") {
        hasBody = true;
        addAuto("Content-Type", "application/json", "ボディ");
      }
      break;
    }
    case "text": {
      const t = ex(b.value, { kind: "body" });
      if (t !== "") {
        hasBody = true;
        addAuto("Content-Type", "text/plain; charset=utf-8", "ボディ");
      }
      break;
    }
    case "binary": {
      const t = ex(b.value, { kind: "body" });
      if (t.trim() !== "") {
        hasBody = true;
        addAuto("Content-Type", "application/octet-stream", "ボディ");
      }
      break;
    }
    case "form": {
      let any = false;
      b.value.forEach((r, i) => {
        if (!r.enabled) return;
        const name = ex(r.name, { kind: "bodyForm", row: i + 1, part: "name" });
        ex(r.value, { kind: "bodyForm", row: i + 1, part: "value" });
        if (name !== "") any = true;
      });
      if (any) {
        hasBody = true;
        addAuto("Content-Type", "application/x-www-form-urlencoded", "ボディ");
      }
      break;
    }
    case "multipart": {
      let any = false;
      b.value.forEach((r, i) => {
        if (!r.enabled) return;
        const name = ex(r.name, { kind: "bodyMultipart", row: i + 1, part: "name" });
        ex(r.value.value, { kind: "bodyMultipart", row: i + 1, part: "value" });
        if (name !== "") any = true;
      });
      if (any) {
        hasBody = true;
        addAuto("Content-Type", "multipart/form-data; boundary=----NekoReqBoundary0000", "ボディ");
      }
      break;
    }
    case "none":
      break;
  }

  // URL を確定
  let urlError: string | null = null;
  let url = "";
  const base = urlText.trim();
  if (base === "") {
    urlError = "URL を入力してください";
  } else {
    const withScheme = hasScheme(base) ? base : `http://${base}`;
    const [beforeFragment, fragment] = splitOnce(withScheme, "#");
    const [pathPart, existing] = splitOnce(beforeFragment, "?");
    const keep = unresolved.map((u) => u.name);
    const q = [query.length > 0 ? (existing ?? "").replace(/&$/, "") : (existing ?? ""), ...query.map(([k, v]) => `${encode(k, keep)}=${encode(v, keep)}`)]
      .filter((s) => s !== "")
      .join("&");
    url = pathPart + (q ? `?${q}` : "") + (fragment !== undefined ? `#${fragment}` : "");
    if (unresolved.length === 0) {
      try {
        const u = new URL(url);
        if (!/^https?:$/.test(u.protocol)) urlError = "http または https の URL を指定してください";
        else url = u.toString();
      } catch {
        urlError = "URL を読めません";
      }
    }
  }

  const autoHeaders = auto.map((h) => ({ ...h, value: maskHeader(h.name, h.value, secrets) }));
  for (const [name, value] of [
    ["User-Agent", "NekoReq/0.1.0"],
    ["Accept", "*/*"],
    ["Accept-Encoding", "gzip, br, deflate"],
  ] as const) {
    if (!hasHeader(name)) autoHeaders.push({ name, value, source: "アプリ既定" });
  }
  if (!hasHeader("Host")) {
    try {
      autoHeaders.push({ name: "Host", value: new URL(url).host, source: "URL から" });
    } catch {
      /* URL を読めなければ付けない */
    }
  }

  return {
    preview: {
      url: displayMissingPathParams(redact(url, secrets), missing),
      unresolved,
      missingPathParams: missing,
      urlError,
      autoHeaders,
    },
    secrets,
    hasBody,
    rawUrl: url,
  };
}

/** 値が入らなかった `{name}` は、URL を整えると `%7Bname%7D` になるので、プレビューでは `{name}` のまま見せる。 */
function displayMissingPathParams(url: string, missing: string[]): string {
  let out = url;
  for (const name of missing) out = out.split(`%7B${encodeURIComponent(name)}%7D`).join(`{${name}}`);
  return out;
}

function maskHeader(name: string, value: string, secrets: string[]): string {
  const n = name.toLowerCase();
  if (n === "authorization") return value.replace(/^(\w+)\s.*$/, "$1 ••••••••");
  if (n === "cookie") return value.replace(/=([^;]*)/g, "=••••••••");
  return redact(value, secrets);
}

function splitOnce(s: string, sep: string): [string, string | undefined] {
  const i = s.indexOf(sep);
  return i < 0 ? [s, undefined] : [s.slice(0, i), s.slice(i + sep.length)];
}
