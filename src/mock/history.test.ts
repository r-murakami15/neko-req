import { describe, expect, it } from "vitest";

import { parseCurl, renderCurl, shellSplit } from "./curl";
import { diffLines } from "./history";
import { dropMasked, isTemplateOnly, MASK, scrubManualSecrets } from "./scrub";

describe("diffLines（モックの行単位の差分）", () => {
  it("同じなら空で、変わっていない", () => {
    expect(diffLines(["a", "b"], ["a", "b"])).toEqual({ lines: [], changed: false });
  });

  it("変わった行の前後 3 行だけ残し、あとは「N 行省略」にまとめる", () => {
    const a = Array.from({ length: 20 }, (_, i) => `line ${i + 1}`);
    const b = [...a];
    b[10] = "changed";
    const { lines, changed } = diffLines(a, b);
    expect(changed).toBe(true);
    expect(lines.filter((l) => l.tag === "delete").map((l) => l.text)).toEqual(["line 11"]);
    expect(lines.filter((l) => l.tag === "insert").map((l) => l.text)).toEqual(["changed"]);
    const skips = lines.filter((l) => l.tag === "skip");
    expect(skips.map((l) => l.text)).toEqual(["7 行省略", "6 行省略"]);
    expect(lines[0]).toMatchObject({ tag: "skip", leftLine: null, rightLine: null });
  });
});

describe("cURL の解析（モック）", () => {
  it("引用符と行末の \\ を解釈する", () => {
    expect(shellSplit("curl -H 'A: b c' \\\n  \"x y\"").words).toEqual(["curl", "-H", "A: b c", "x y"]);
    expect(shellSplit("curl 'abc").error).toBe("引用符が閉じられていません");
  });

  it("method・URL・ヘッダー・ボディ・警告を読む", () => {
    const p = parseCurl("curl -s -X PUT http://h/x -H 'Content-Type: application/json' -d '{\"a\":1}' --nope");
    expect(p).toMatchObject({ method: "PUT", url: "http://h/x", data: '{"a":1}' });
    if ("error" in p) throw new Error("読めるはず");
    expect(p.warnings.map((w) => w.kind)).toEqual(["ignored", "unknown"]);
    expect(parseCurl("curl -d a=1 http://h/")).toMatchObject({ method: "POST" });
    expect(parseCurl("curl")).toEqual({ error: "URL が見つかりません" });
    expect(parseCurl("ls")).toEqual({ error: "curl で始まるコマンドを貼り付けてください" });
  });

  it("書き出しは bash 風（行末は \\）", () => {
    const text = renderCurl({ method: "POST", url: "http://h/x", headers: [{ name: "A", value: "it's" }], bodyText: "{}", parts: [] });
    expect(text).toBe("curl -X POST 'http://h/x' \\\n  -H 'A: it'\\''s' \\\n  -d '{}'");
  });
});

describe("伏せ字（モック）", () => {
  it("{{変数}} だけの値は、そのまま残す", () => {
    expect(isTemplateOnly("{{token}}")).toBe(true);
    expect(isTemplateOnly("Bearer {{token}}")).toBe(true);
    expect(isTemplateOnly("Bearer abc")).toBe(false);
  });

  it("手で書いた認証の値だけ伏せて、名前を返す。外すときは伏せ字の行を落とす", () => {
    const spec = {
      method: "GET",
      url: "x",
      params: [
        { location: "header" as const, name: "Authorization", value: "Bearer abc", enabled: true },
        { location: "header" as const, name: "X-Other", value: "keep", enabled: true },
        { location: "cookie" as const, name: "SESSION", value: "{{session}}", enabled: true },
      ],
      body: { kind: "none" as const },
      auth: { type: "bearer" as const, token: "manual" },
    };
    const { spec: out, fields } = scrubManualSecrets(spec, MASK);
    expect(fields).toEqual(["認証（Bearer のトークン）", "Authorization ヘッダー"]);
    expect(out.params.map((p) => p.value)).toEqual([MASK, "keep", "{{session}}"]);
    const dropped = dropMasked(out);
    expect(dropped.params.map((p) => p.name)).toEqual(["X-Other", "SESSION"]);
    expect(dropped.auth).toEqual({ type: "bearer", token: "" });
  });
});
