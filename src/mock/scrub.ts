import type { AuthOverride, KeyValue, RequestSpec } from "../bindings";

/** Rust 側（履歴の伏せ字・タブの復元）と同じ印。 */
export const MASK = "••••";

/** `{{変数}}` だけでできた値はテンプレートなので、そのまま残す。 */
export function isTemplateOnly(value: string): boolean {
  const rest = value.replace(/\{\{[^{}]*\}\}/g, "");
  return value.includes("{{") && rest.replace(/^\s*Bearer\s*$/i, "").trim() === "";
}

const SECRET_HEADERS = ["authorization", "proxy-authorization", "cookie", "x-xsrf-token"];

const needsScrub = (value: string) => value !== "" && value !== MASK && !isTemplateOnly(value);

/**
 * 手で書かれた認証の値（認証欄の Bearer・Basic のパスワード・API キー、Authorization などのヘッダー、Cookie の値）を
 * `replacement` に置き換える。置き換えたものの名前も返す。
 */
export function scrubManualSecrets(spec: RequestSpec, replacement: string): { spec: RequestSpec; fields: string[] } {
  const fields: string[] = [];
  const note = (name: string) => {
    if (!fields.includes(name)) fields.push(name);
  };
  let auth: AuthOverride | null = spec.auth;
  if (auth?.type === "bearer" && needsScrub(auth.token)) {
    auth = { ...auth, token: replacement };
    note("認証（Bearer のトークン）");
  } else if (auth?.type === "basic" && needsScrub(auth.password)) {
    auth = { ...auth, password: replacement };
    note("認証（Basic のパスワード）");
  } else if (auth?.type === "apiKey" && needsScrub(auth.value)) {
    auth = { ...auth, value: replacement };
    note("認証（API キーの値）");
  }
  const params: KeyValue[] = spec.params.map((p) => {
    if (p.location === "header" && SECRET_HEADERS.includes(p.name.trim().toLowerCase()) && needsScrub(p.value)) {
      note(`${p.name.trim()} ヘッダー`);
      return { ...p, value: replacement };
    }
    if (p.location === "cookie" && needsScrub(p.value)) {
      note(`Cookie（${p.name}）`);
      return { ...p, value: replacement };
    }
    return p;
  });
  return { spec: { ...spec, auth, params }, fields };
}

/** 「編集して開く」用: 伏せ字（`••••`）が入った行は外し、認証の値は空にする。 */
export function dropMasked(spec: RequestSpec): RequestSpec {
  let auth = spec.auth;
  if (auth?.type === "bearer" && auth.token === MASK) auth = { ...auth, token: "" };
  else if (auth?.type === "basic" && auth.password === MASK) auth = { ...auth, password: "" };
  else if (auth?.type === "apiKey" && auth.value === MASK) auth = { ...auth, value: "" };
  return { ...spec, auth, params: spec.params.filter((p) => p.value !== MASK) };
}

/** 文字列の中のシークレットの値を伏せ字にする。 */
export function redactText(text: string, secrets: string[]): string {
  let out = text;
  for (const s of secrets) if (s !== "") out = out.split(s).join(MASK);
  return out;
}

export function isSecretHeader(name: string): boolean {
  return SECRET_HEADERS.includes(name.trim().toLowerCase()) || name.trim().toLowerCase() === "set-cookie";
}
