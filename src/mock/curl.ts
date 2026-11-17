import type { Body, CurlWarning, KeyValue, MultipartPart, RequestSpec } from "../bindings";

/** シェルのように（引用符と行末の `\` を解釈して）単語に分ける。 */
export function shellSplit(text: string): { words: string[]; error: string | null } {
  const words: string[] = [];
  let cur = "";
  let inWord = false;
  let quote: "'" | '"' | null = null;
  const src = text.replace(/\\\r?\n/g, " ");
  for (let i = 0; i < src.length; i += 1) {
    const c = src[i];
    if (quote === "'") {
      if (c === "'") quote = null;
      else cur += c;
    } else if (quote === '"') {
      if (c === '"') quote = null;
      else if (c === "\\" && i + 1 < src.length && /["\\$`]/.test(src[i + 1])) {
        cur += src[i + 1];
        i += 1;
      } else cur += c;
    } else if (c === "'" || c === '"') {
      quote = c;
      inWord = true;
    } else if (c === "\\" && i + 1 < src.length) {
      cur += src[i + 1];
      inWord = true;
      i += 1;
    } else if (/\s/.test(c)) {
      if (inWord || cur !== "") words.push(cur);
      cur = "";
      inWord = false;
    } else {
      cur += c;
      inWord = true;
    }
  }
  if (quote) return { words, error: "引用符が閉じられていません" };
  if (inWord || cur !== "") words.push(cur);
  return { words, error: null };
}

export interface ParsedCurl {
  method: string;
  url: string;
  headers: { name: string; value: string }[];
  data: string | null;
  json: boolean;
  forms: { name: string; value: string; isFile: boolean }[];
  user: string | null;
  cookie: string | null;
  getMode: boolean;
  warnings: CurlWarning[];
}

const IGNORED = new Set(["-s", "--silent", "-S", "--show-error", "-L", "--location", "-k", "--insecure", "--compressed", "-v", "--verbose", "-i", "--include", "-g", "--globoff"]);
const WITH_VALUE_IGNORED = new Set(["-o", "--output", "-m", "--max-time", "--connect-timeout", "-A", "--user-agent", "-e", "--referer"]);

/** 簡易の cURL 解析（確認用のモック。実際の解析は Rust の nekoreq-curl）。 */
export function parseCurl(text: string): ParsedCurl | { error: string } {
  const { words, error } = shellSplit(text.trim());
  if (error) return { error };
  if (words.length === 0) return { error: "cURL コマンドを入力してください" };
  if (words[0] !== "curl") return { error: "curl で始まるコマンドを貼り付けてください" };
  const out: ParsedCurl = { method: "", url: "", headers: [], data: null, json: false, forms: [], user: null, cookie: null, getMode: false, warnings: [] };
  for (let i = 1; i < words.length; i += 1) {
    const w = words[i];
    const next = () => words[(i += 1)] ?? "";
    if (w === "-X" || w === "--request") out.method = next().toUpperCase();
    else if (w === "-H" || w === "--header") {
      const h = next();
      const at = h.indexOf(":");
      if (at > 0) out.headers.push({ name: h.slice(0, at).trim(), value: h.slice(at + 1).trim() });
    } else if (w === "-d" || w === "--data" || w === "--data-raw" || w === "--data-binary" || w === "--data-ascii") out.data = (out.data === null ? "" : `${out.data}&`) + next();
    else if (w === "--json") {
      out.data = next();
      out.json = true;
    } else if (w === "-F" || w === "--form") {
      const f = next();
      const at = f.indexOf("=");
      if (at > 0) {
        const value = f.slice(at + 1);
        out.forms.push({ name: f.slice(0, at), value: value.startsWith("@") ? value.slice(1) : value, isFile: value.startsWith("@") });
      }
    } else if (w === "-u" || w === "--user") out.user = next();
    else if (w === "-b" || w === "--cookie") out.cookie = next();
    else if (w === "-G" || w === "--get") out.getMode = true;
    else if (w === "--url") out.url = next();
    else if (IGNORED.has(w)) out.warnings.push({ kind: "ignored", option: w, message: `${w} は無視しました（アプリの設定で決まります）` });
    else if (WITH_VALUE_IGNORED.has(w)) {
      next();
      out.warnings.push({ kind: "ignored", option: w, message: `${w} は無視しました` });
    } else if (w.startsWith("-") && w.length > 1) out.warnings.push({ kind: "unknown", option: w, message: `${w} は読み取れないオプションなので無視しました` });
    else if (out.url === "") out.url = w;
  }
  if (out.url === "") return { error: "URL が見つかりません" };
  if (out.method === "") out.method = out.getMode ? "GET" : out.data !== null || out.forms.length > 0 ? "POST" : "GET";
  return out;
}

const kv = (location: KeyValue["location"], name: string, value: string): KeyValue => ({ location, name, value, enabled: true });

/** 解析結果の body（Content-Type から種類を決める）。 */
export function bodyOf(p: ParsedCurl): Body {
  if (p.forms.length > 0) {
    const value: MultipartPart[] = p.forms.map((f) => ({ name: f.name, value: f.isFile ? { kind: "file", value: f.value } : { kind: "text", value: f.value }, enabled: true }));
    return { kind: "multipart", value };
  }
  if (p.data === null || p.getMode) return { kind: "none" };
  const ct = p.headers.find((h) => h.name.toLowerCase() === "content-type")?.value.toLowerCase() ?? (p.json ? "application/json" : "");
  if (ct.includes("json") || (ct === "" && /^\s*[{[]/.test(p.data))) {
    let pretty = p.data;
    try {
      pretty = JSON.stringify(JSON.parse(p.data), null, 2);
    } catch {
      /* 読めなければそのまま */
    }
    return { kind: "json", value: pretty };
  }
  if (ct.includes("x-www-form-urlencoded") || ct === "") {
    return {
      kind: "form",
      value: p.data
        .split("&")
        .filter((s) => s !== "")
        .map((pair) => {
          const at = pair.indexOf("=");
          const dec = (s: string) => {
            try {
              return decodeURIComponent(s.replace(/\+/g, " "));
            } catch {
              return s;
            }
          };
          return kv("query", dec(at < 0 ? pair : pair.slice(0, at)), at < 0 ? "" : dec(pair.slice(at + 1)));
        }),
    };
  }
  return { kind: "text", value: p.data };
}

/** URL の path と query の行（クエリは有効な行にする）。 */
export function splitUrl(url: string): { origin: string; pathname: string; query: [string, string][] } | null {
  try {
    const u = new URL(url);
    return { origin: u.origin, pathname: decodeURIComponent(u.pathname), query: [...u.searchParams.entries()] };
  } catch {
    return null;
  }
}

export function headerRows(p: ParsedCurl): KeyValue[] {
  return p.headers.filter((h) => !["content-length", "host", "user-agent", "accept-encoding", "cookie"].includes(h.name.toLowerCase())).map((h) => kv("header", h.name, h.value));
}

/** 取り込み結果に使う `RequestSpec` の共通部分（method・ヘッダー・ボディ・Basic）。 */
export function applyCommon(spec: RequestSpec, p: ParsedCurl): RequestSpec {
  const next: RequestSpec = { ...spec, body: bodyOf(p) };
  for (const h of headerRows(p)) {
    const i = next.params.findIndex((x) => x.location === "header" && x.name.toLowerCase() === h.name.toLowerCase());
    next.params = i >= 0 ? next.params.map((x, j) => (j === i ? { ...x, value: h.value, enabled: true } : x)) : [...next.params, h];
  }
  if (p.cookie) {
    for (const c of p.cookie.split(";")) {
      const at = c.indexOf("=");
      if (at > 0) next.params = [...next.params, kv("cookie", c.slice(0, at).trim(), c.slice(at + 1).trim())];
    }
  }
  if (p.user !== null) {
    const at = p.user.indexOf(":");
    next.auth = { type: "basic", username: at < 0 ? p.user : p.user.slice(0, at), password: at < 0 ? "" : p.user.slice(at + 1) };
  }
  return next;
}

/** `curl_export` の書き出し（bash 風。行末は `\`）。 */
export function renderCurl(args: {
  method: string;
  url: string;
  headers: { name: string; value: string }[];
  bodyText: string | null;
  parts: { name: string; value: string; isFile: boolean }[];
}): string {
  const q = (s: string) => `'${s.replace(/'/g, `'\\''`)}'`;
  const lines = [`curl -X ${args.method} ${q(args.url)}`];
  for (const h of args.headers) {
    if (args.parts.length > 0 && h.name.toLowerCase() === "content-type") continue;
    lines.push(`-H ${q(`${h.name}: ${h.value}`)}`);
  }
  if (args.bodyText !== null) lines.push(`-d ${q(args.bodyText)}`);
  for (const part of args.parts) lines.push(`-F ${q(`${part.name}=${part.isFile ? "@" : ""}${part.value}`)}`);
  return lines.join(" \\\n  ");
}
