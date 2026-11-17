/** JSON の整形・検査。`{{変数}}` が入っていても扱えるようにする。 */

const VAR = /\{\{[^{}]*\}\}/g;

export type FormatResult = { ok: true; text: string } | { ok: false; error: string };

function describeError(e: unknown, source: string): string {
  const message = e instanceof Error ? e.message : "";
  const m = /position (\d+)/.exec(message);
  if (m) {
    const pos = Number(m[1]);
    const before = source.slice(0, pos);
    const line = before.split("\n").length;
    return `JSON として読めません（${line} 行目付近）`;
  }
  return "JSON として読めません";
}

/** JSON として正しいか。`{{変数}}` は文字列の中でも外でも値として数える。空は `null`（検査しない）。 */
export function checkJson(text: string): FormatResult | null {
  if (text.trim() === "") return null;
  try {
    JSON.parse(text.replace(VAR, "0"));
    return { ok: true, text };
  } catch (e) {
    return { ok: false, error: describeError(e, text) };
  }
}

/** 2 スペースで整形する。変数が文字列の外にあるときは整形できない（変数をそのまま残すため）。 */
export function formatJson(text: string): FormatResult {
  const vars: string[] = [];
  const masked = text.replace(VAR, (m) => {
    vars.push(m);
    return `__NEKOVAR${vars.length - 1}__`;
  });
  try {
    const pretty = JSON.stringify(JSON.parse(masked), null, 2);
    return { ok: true, text: pretty.replace(/__NEKOVAR(\d+)__/g, (_, i: string) => vars[Number(i)] ?? "") };
  } catch (e) {
    if (checkJson(text)?.ok) {
      return { ok: false, error: "変数が文字列の外にあるため、整形できません" };
    }
    return { ok: false, error: describeError(e, text) };
  }
}

/** レスポンスの表示用に整形する。読めなければ `null`。 */
export function prettyResponseJson(text: string): string | null {
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return null;
  }
}

export type JsonTokenKind = "key" | "string" | "number" | "literal" | "plain";
export interface JsonToken {
  text: string;
  kind: JsonTokenKind;
}

const TOKEN =
  /("(?:\\.|[^"\\])*")(\s*:)?|(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)|\b(true|false|null)\b/g;

/** 整形済みの JSON を、色を付ける単位に分ける。 */
export function tokenizeJson(text: string): JsonToken[] {
  const out: JsonToken[] = [];
  let last = 0;
  const push = (t: string, kind: JsonTokenKind) => {
    if (t !== "") out.push({ text: t, kind });
  };
  for (const m of text.matchAll(TOKEN)) {
    const start = m.index ?? 0;
    push(text.slice(last, start), "plain");
    if (m[1] !== undefined) {
      if (m[2] !== undefined) {
        push(m[1], "key");
        push(m[2], "plain");
      } else {
        push(m[1], "string");
      }
    } else if (m[3] !== undefined) {
      push(m[3], "number");
    } else {
      push(m[4], "literal");
    }
    last = start + m[0].length;
  }
  push(text.slice(last), "plain");
  return out;
}
