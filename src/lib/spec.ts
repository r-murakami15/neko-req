import type { AdHocRequest, Body, KeyValue, ParamDoc, ParamIn, ParamRef, RequestSpec } from "../bindings";

export const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] as const;

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

/** パラメータ・ヘッダーの表に出す、yaml 側の情報（表示専用）。 */
export interface ParamMeta {
  docs: ParamDoc[];
  /** yaml から消えたパラメータ。 */
  missing: ParamRef[];
  /** yaml の必須パラメータ。 */
  required: ParamRef[];
}

/** 表の 1 行に付く、yaml 側の情報。 */
export interface RowInfo {
  description: string | null;
  /** yaml では必須。 */
  required: boolean;
  /** 必須なのに、無効または空（警告。送信はできる）。 */
  emptyRequired: boolean;
  /** yaml にない（入力値は残してあるが、送信から外す）。 */
  missing: boolean;
}

const sameRef = (r: ParamRef, item: Pick<KeyValue, "location" | "name">) => r.location === item.location && r.name === item.name;

export function rowInfoFor(meta: ParamMeta, item: KeyValue): RowInfo {
  const required = meta.required.some((r) => sameRef(r, item));
  return {
    description: meta.docs.find((d) => d.location === item.location && d.name === item.name)?.description ?? null,
    required,
    emptyRequired: required && (!item.enabled || item.value.trim() === ""),
    missing: meta.missing.some((r) => sameRef(r, item)),
  };
}

/** 送る内容。yaml にないパラメータは、有効にしてあっても送らない（行は残すので、行番号は変わらない）。 */
export function specForSend(spec: RequestSpec, missing: ParamRef[]): RequestSpec {
  if (missing.length === 0 || !spec.params.some((p) => p.enabled && missing.some((m) => sameRef(m, p)))) return spec;
  return { ...spec, params: spec.params.map((p) => (missing.some((m) => sameRef(m, p)) ? { ...p, enabled: false } : p)) };
}

/** yaml の必須パラメータで、行がない・無効・空のもの（path は URL の警告で出るので除く）。 */
export function emptyRequiredParams(spec: RequestSpec, required: ParamRef[]): ParamRef[] {
  return required.filter((r) => {
    if (r.location === "path") return false;
    const row = spec.params.find((p) => sameRef(r, p));
    return !row || !row.enabled || row.value.trim() === "";
  });
}
