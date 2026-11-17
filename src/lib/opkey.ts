import type { OpKey, ParamIn } from "../bindings";

/** OpKey を比べたり Map のキーにしたりするための文字列。 */
export function opKeyString(key: OpKey): string {
  return key.kind === "id" ? `id:${key.value}` : `mp:${key.value.method.toUpperCase()} ${key.value.path}`;
}

export function sameOpKey(a: OpKey, b: OpKey): boolean {
  return opKeyString(a) === opKeyString(b);
}

export const PARAM_IN_LABEL: Record<ParamIn, string> = {
  query: "クエリ",
  path: "パス",
  header: "ヘッダー",
  cookie: "Cookie",
};
