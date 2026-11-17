import type { AdHocRequest, Body, KeyValue, ParamIn, RequestSpec, SendSettings } from "../bindings";

export const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] as const;

/** v1 の設定画面（M6）ができるまでの送信の既定値。 */
export const DEFAULT_SEND_SETTINGS: SendSettings = {
  timeoutMs: 30_000,
  followRedirects: true,
  verifyCertificates: true,
};

export const NEW_REQUEST_NAME = "新しいリクエスト";

export function emptySpec(): RequestSpec {
  return { method: "GET", url: "", params: [], body: { kind: "none" }, auth: null };
}

export function specOfAdhoc(r: AdHocRequest): RequestSpec {
  return { method: r.method, url: r.url, params: r.params, body: r.body, auth: r.auth };
}

export function newKeyValue(location: ParamIn): KeyValue {
  return { location, name: "", value: "", enabled: true };
}

/** 画面の表に並ぶ、`location` の行（行番号は 1 始まりで、無効の行も数える）。 */
export function rowsOf(params: KeyValue[], location: ParamIn): KeyValue[] {
  return params.filter((p) => p.location === location);
}

/** `location` の行を `next` に置き換える（ほかの種類の行は残す）。 */
export function replaceRows(params: KeyValue[], location: ParamIn, next: KeyValue[]): KeyValue[] {
  return [...params.filter((p) => p.location !== location), ...next];
}

/** 送られる行（有効で、名前が入っているもの）の数。タブの見出しの件数に使う。 */
export function countActive(params: KeyValue[], locations: ParamIn[]): number {
  return params.filter((p) => locations.includes(p.location) && p.enabled && p.name.trim() !== "").length;
}

export function bodyIsEmpty(body: Body): boolean {
  return body.kind === "none";
}

/** 名前を付けるときの候補。`{{baseUrl}}/api/v1/clients?x=1` → `GET /api/v1/clients`。 */
export function suggestName(spec: RequestSpec): string {
  const withoutQuery = spec.url.split(/[?#]/)[0];
  const path = withoutQuery.replace(/^\s*(\{\{[^}]*\}\}|https?:\/\/[^/]*)/, "").trim();
  return path ? `${spec.method} ${path}` : "";
}

const METHOD_COLORS: Record<string, string> = {
  GET: "var(--method-get)",
  POST: "var(--method-post)",
  PUT: "var(--method-put)",
  PATCH: "var(--method-patch)",
  DELETE: "var(--method-delete)",
};

/** メソッドの色（HEAD・OPTIONS は控えめな色）。 */
export function methodColor(method: string): string {
  return METHOD_COLORS[method.toUpperCase()] ?? "var(--method-other)";
}
