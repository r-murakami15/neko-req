import type { ParamIn, RowPart, UnresolvedVar, VarLocation } from "../bindings";

/** `{{変数名}}` を含む文字列を、変数とそれ以外に分けたもの。 */
export interface VarSegment {
  text: string;
  /** 変数のとき、前後の空白を除いた名前。 */
  name: string | null;
}

// Rust 側（vars.rs の is_variable_name）と同じ: 空・波括弧・改行を含むものは変数ではない
const VAR_PATTERN = /\{\{([^{}\r\n]*)\}\}/g;

export function splitVars(text: string): VarSegment[] {
  const out: VarSegment[] = [];
  let last = 0;
  for (const m of text.matchAll(VAR_PATTERN)) {
    const name = m[1].trim();
    if (name === "") continue;
    const start = m.index ?? 0;
    if (start > last) out.push({ text: text.slice(last, start), name: null });
    out.push({ text: m[0], name });
    last = start + m[0].length;
  }
  if (last < text.length) out.push({ text: text.slice(last), name: null });
  return out;
}

/** URL に書かれたパスパラメータ名（`{clientId}`。`{{変数}}` は除く）。 */
export function pathParamNames(url: string): string[] {
  const names: string[] = [];
  const withoutVars = url.replace(/\{\{[^{}]*\}\}/g, "");
  for (const m of withoutVars.matchAll(/\{([^{}\s]+)\}/g)) {
    if (!names.includes(m[1])) names.push(m[1]);
  }
  return names;
}

export type Section = "url" | "params" | "headers" | "body" | "auth";

export function sectionOf(loc: VarLocation): Section {
  switch (loc.kind) {
    case "url":
      return "url";
    case "param":
      return loc.location === "header" ? "headers" : "params";
    case "body":
    case "bodyForm":
    case "bodyMultipart":
      return "body";
    case "auth":
      return "auth";
  }
}

const PARAM_LABEL: Record<ParamIn, string> = {
  query: "パラメータ（クエリ）",
  path: "パラメータ（パス）",
  header: "ヘッダー",
  cookie: "パラメータ（Cookie）",
};
const PART_LABEL: Record<RowPart, string> = { name: "キー", value: "値" };
const AUTH_FIELD_LABEL: Record<string, string> = {
  token: "トークン",
  username: "ユーザー名",
  password: "パスワード",
  name: "名前",
  value: "値",
};

/** 「展開できない変数」の場所を画面の言葉で言い表す。 */
export function describeLocation(loc: VarLocation): string {
  switch (loc.kind) {
    case "url":
      return "URL";
    case "param":
      return `${PARAM_LABEL[loc.location]} ${loc.row} 行目の${PART_LABEL[loc.part]}`;
    case "body":
      return "ボディ";
    case "bodyForm":
      return `ボディ（フォーム）${loc.row} 行目の${PART_LABEL[loc.part]}`;
    case "bodyMultipart":
      return `ボディ（multipart）${loc.row} 行目の${PART_LABEL[loc.part]}`;
    case "auth":
      return `認証（${AUTH_FIELD_LABEL[loc.field] ?? loc.field}）`;
  }
}

const EMPTY: ReadonlySet<string> = new Set();

/** 展開できない変数を、場所ごとにすぐ引けるようにしたもの。 */
export interface UnresolvedIndex {
  readonly all: UnresolvedVar[];
  readonly url: ReadonlySet<string>;
  readonly body: ReadonlySet<string>;
  param(location: ParamIn, row: number, part: RowPart): ReadonlySet<string>;
  bodyForm(row: number, part: RowPart): ReadonlySet<string>;
  bodyMultipart(row: number, part: RowPart): ReadonlySet<string>;
  auth(field: string): ReadonlySet<string>;
  /** タブの見出しに赤い印を付けるための、場所の大きな区分ごとの有無。 */
  has(section: Section): boolean;
}

export const NO_UNRESOLVED: UnresolvedIndex = indexUnresolved([]);

export function indexUnresolved(list: UnresolvedVar[]): UnresolvedIndex {
  const map = new Map<string, Set<string>>();
  const sections = new Set<Section>();
  const add = (key: string, name: string) => {
    const set = map.get(key) ?? new Set<string>();
    set.add(name);
    map.set(key, set);
  };
  const keyOf = (loc: VarLocation): string => {
    switch (loc.kind) {
      case "url":
        return "url";
      case "param":
        return `param:${loc.location}:${loc.row}:${loc.part}`;
      case "body":
        return "body";
      case "bodyForm":
        return `form:${loc.row}:${loc.part}`;
      case "bodyMultipart":
        return `multipart:${loc.row}:${loc.part}`;
      case "auth":
        return `auth:${loc.field}`;
    }
  };
  for (const v of list) {
    add(keyOf(v.location), v.name);
    sections.add(sectionOf(v.location));
  }
  const get = (key: string) => map.get(key) ?? EMPTY;
  return {
    all: list,
    url: get("url"),
    body: get("body"),
    param: (location, row, part) => get(`param:${location}:${row}:${part}`),
    bodyForm: (row, part) => get(`form:${row}:${part}`),
    bodyMultipart: (row, part) => get(`multipart:${row}:${part}`),
    auth: (field) => get(`auth:${field}`),
    has: (section) => sections.has(section),
  };
}
