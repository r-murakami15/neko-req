import { describe, expect, it } from "vitest";

import { formatBytes, formatMs } from "./format";
import { checkJson, formatJson, prettyResponseJson, tokenizeJson } from "./json";
import { suggestName } from "./spec";
import { describeLocation, indexUnresolved, pathParamNames, splitVars } from "./vars";

describe("変数", () => {
  it("{{変数}} とそれ以外に分ける", () => {
    expect(splitVars("{{baseUrl}}/api/{id}?q={{ q }}")).toEqual([
      { text: "{{baseUrl}}", name: "baseUrl" },
      { text: "/api/{id}?q=", name: null },
      { text: "{{ q }}", name: "q" },
    ]);
    expect(splitVars("{{}} {{a{b}}")).toEqual([{ text: "{{}} {{a{b}}", name: null }]);
  });

  it("URL のパスパラメータ名（{{変数}} は除く）", () => {
    expect(pathParamNames("{{baseUrl}}/clients/{clientId}/works/{workId}")).toEqual(["clientId", "workId"]);
    expect(pathParamNames("{{baseUrl}}/x")).toEqual([]);
  });

  it("展開できない変数を場所で引ける。場所は画面の言葉で言い表す", () => {
    const idx = indexUnresolved([
      { name: "baseUrl", location: { kind: "url" } },
      { name: "q", location: { kind: "param", location: "query", row: 2, part: "value" } },
      { name: "t", location: { kind: "auth", field: "token" } },
    ]);
    expect(idx.url.has("baseUrl")).toBe(true);
    expect(idx.param("query", 2, "value").has("q")).toBe(true);
    expect(idx.param("query", 1, "value").size).toBe(0);
    expect(idx.auth("token").has("t")).toBe(true);
    expect(idx.has("params")).toBe(true);
    expect(idx.has("headers")).toBe(false);
    expect(describeLocation({ kind: "param", location: "header", row: 1, part: "value" })).toBe("ヘッダー 1 行目の値");
    expect(describeLocation({ kind: "bodyForm", row: 3, part: "name" })).toBe("ボディ（フォーム）3 行目のキー");
  });
});

describe("JSON", () => {
  it("変数を含んでいても検査・整形できる（変数が文字列の外のときは整形だけできない）", () => {
    expect(checkJson("")).toBeNull();
    expect(checkJson('{"a": "{{x}}", "b": {{n}}}')?.ok).toBe(true);
    expect(checkJson('{"a": ')?.ok).toBe(false);
    expect(formatJson('{"email":"{{email}}","n":1}')).toEqual({
      ok: true,
      text: '{\n  "email": "{{email}}",\n  "n": 1\n}',
    });
    expect(formatJson('{"n": {{n}}}')).toEqual({ ok: false, error: "変数が文字列の外にあるため、整形できません" });
    const bad = formatJson('{"a":');
    expect(bad.ok).toBe(false);
  });

  it("レスポンスの整形と色分け", () => {
    expect(prettyResponseJson('{"a":[1,true,null]}')).toBe('{\n  "a": [\n    1,\n    true,\n    null\n  ]\n}');
    expect(prettyResponseJson("{")).toBeNull();
    const kinds = tokenizeJson('{\n  "a": "x",\n  "n": -1.5e3,\n  "t": true\n}').filter((t) => t.kind !== "plain");
    expect(kinds.map((t) => [t.kind, t.text])).toEqual([
      ["key", '"a"'],
      ["string", '"x"'],
      ["key", '"n"'],
      ["number", "-1.5e3"],
      ["key", '"t"'],
      ["literal", "true"],
    ]);
  });
});

describe("表示", () => {
  it("サイズと時間", () => {
    expect(formatBytes(318)).toBe("318 B");
    expect(formatBytes(1434)).toBe("1.4 KB");
    expect(formatBytes(25_165_824)).toBe("24.0 MB");
    expect(formatMs(38)).toBe("38 ms");
    expect(formatMs(0.2)).toBe("< 1 ms");
    expect(formatMs(1200)).toBe("1.20 s");
  });

  it("保存するときの名前の候補", () => {
    const spec = { method: "GET", url: "{{baseUrl}}/api/v1/clients?x=1", params: [], body: { kind: "none" as const }, auth: null };
    expect(suggestName(spec)).toBe("GET /api/v1/clients");
    expect(suggestName({ ...spec, url: "" })).toBe("");
  });
});
