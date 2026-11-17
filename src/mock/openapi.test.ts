import { describe, expect, it } from "vitest";

import type { ImportOptions, OpKey, SourceInput } from "../bindings";
import { createMockBackend } from "./backend";
import { generateExample, schemaRows, type Schema } from "./openapi";

const OPTIONS: ImportOptions = { tagFolders: true, fillFromExamples: true, applyCookieAuth: true, descriptionAsDoc: true };
const path = (p: string): SourceInput => ({ kind: "path", path: p });
const id = (v: string): OpKey => ({ kind: "id", value: v });

describe("モックの OpenAPI（取り込み・同期）", () => {
  it("特定の入力で、入口の候補・別ホスト・取り込めない問題を再現できる", async () => {
    const b = createMockBackend();
    expect((await b.openapiPreview(path("/mock/openapi.yaml"), null)).status).toBe("ready");
    expect(await b.openapiPreview(path("/mock/multiple"), null)).toMatchObject({ status: "needsEntry" });
    expect((await b.openapiPreview(path("/mock/multiple/openapi.yaml"), null)).status).toBe("ready");
    expect(await b.openapiPreview({ kind: "upload", paths: ["/m/multiple"], entry: null }, null)).toMatchObject({ status: "needsEntry", candidates: ["openapi.yaml", "legacy/openapi.yaml"] });
    const url: SourceInput = { kind: "url", url: "https://x.example/foreign.yaml", token: null, allowedHosts: [] };
    expect(await b.openapiPreview(url, null)).toMatchObject({ status: "needsConfirmation" });
    expect((await b.openapiPreview({ ...url, allowedHosts: ["assets.other-host.example"] } as SourceInput, null)).status).toBe("ready");
    for (const [name, kind] of [["missing", "missingFile"], ["broken", "yamlError"], ["swagger", "unsupportedVersion"], ["notopenapi", "notOpenApi"], ["unreadable", "unreadable"]] as const) {
      const r = await b.openapiPreview(path(`/mock/${name}`), null);
      expect(r.status === "blocked" && r.problems[0].kind).toBe(kind);
    }
  });

  it("選んだ API だけ取り込み、選ばなかったものは同期でも「追加」にならない。選択が空なら検証エラー", async () => {
    const b = createMockBackend();
    await expect(b.openapiImport({ source: path("/mock/openapi.yaml"), projectId: null, projectName: null, selected: [], options: OPTIONS })).rejects.toMatchObject({ kind: "validation" });
    const r = await b.openapiImport({ source: path("/mock/openapi.yaml"), projectId: null, projectName: "P", selected: [id("listClients"), id("getWork")], options: OPTIONS });
    if (r.status !== "imported") throw new Error("imported ではありません");
    expect(r.operationCount).toBe(2);
    expect((await b.operationList(r.project.id)).map((o) => o.operationId).sort()).toEqual(["getWork", "listClients"]);
    b.mock.touchSource();
    const check = await b.openapiSyncCheck(r.project.id, null);
    if (check.status !== "ready") throw new Error("ready ではありません");
    expect(check.diff.added.map((e) => e.operationId)).toEqual(["getClient"]);
    await b.openapiSyncApply(r.project.id, null);
    // 選んでいなかった API は、同期でも一覧に出ない
    expect((await b.operationList(r.project.id)).map((o) => o.operationId)).not.toContain("deleteClient");
  });

  it("同期: 入力値のある API が消えると「削除された API」に残り、forget で消せる。入力値のないものは消える", async () => {
    const b = createMockBackend();
    const r = await b.openapiImport({ source: path("/mock/openapi.yaml"), projectId: null, projectName: "P", selected: null, options: OPTIONS });
    if (r.status !== "imported") throw new Error("imported ではありません");
    const pid = r.project.id;
    await b.draftSave({ id: "", projectId: pid, opKey: id("legacyPing"), params: [], body: { kind: "none" }, auth: null, updatedAt: "" });
    expect((await b.openapiSourceChanged(pid)).changed).toBe(false);
    b.mock.touchSource();
    expect(await b.openapiSourceChanged(pid)).toMatchObject({ changed: true });

    const applied = await b.openapiSyncApply(pid, null);
    expect(applied.status === "applied" && applied.report).toMatchObject({ added: 1, changed: 1, removed: 1, keptAsDeleted: 1 });
    const list = await b.operationList(pid);
    expect(list.find((o) => o.operationId === "legacyPing")).toMatchObject({ deleted: true, hasDraft: true });
    expect((await b.operationOpen(pid, id("legacyPing"))).deleted).toBe(true);
    expect((await b.openapiSyncCheck(pid, null)).status === "ready").toBe(true);
    expect((await b.openapiSourceChanged(pid)).changed).toBe(false);

    await expect(b.operationForget(pid, id("getSession"))).rejects.toMatchObject({ kind: "validation" });
    await b.operationForget(pid, id("legacyPing"));
    expect((await b.operationList(pid)).some((o) => o.operationId === "legacyPing")).toBe(false);
    expect(await b.draftGet(pid, id("legacyPing"))).toBeNull();
  });

  it("コピー方式の同期は needsUpload、壊れた yaml は blocked", async () => {
    const b = createMockBackend();
    const r = await b.openapiImport({ source: { kind: "upload", paths: ["/m/api"], entry: null }, projectId: null, projectName: "P", selected: null, options: OPTIONS });
    if (r.status !== "imported") throw new Error("imported ではありません");
    expect(await b.openapiSyncCheck(r.project.id, null)).toMatchObject({ status: "needsUpload", entry: "openapi.yaml" });
    b.mock.breakSource();
    expect(await b.openapiSyncCheck(r.project.id, { paths: ["/m/api"], entry: null })).toMatchObject({ status: "blocked" });
  });

  it("operationOpen: path は有効・それ以外は無効で並び、必須と既定の認証が付く。入力値があればそれを返す", async () => {
    const b = createMockBackend();
    const r = await b.openapiImport({ source: path("/mock/openapi.yaml"), projectId: null, projectName: "P", selected: null, options: OPTIONS });
    if (r.status !== "imported") throw new Error("imported ではありません");
    const del = await b.operationOpen(r.project.id, id("deleteClient"));
    expect(del.request.url).toBe("{{baseUrl}}/api/v1/video/clients/{clientId}");
    expect(del.request.params).toEqual([{ location: "path", name: "clientId", value: "1", enabled: true }]);
    const list = await b.operationOpen(r.project.id, id("listClients"));
    expect(list.request.params.map((p) => [p.name, p.enabled])).toEqual([["q", false], ["contractType", false], ["status", false], ["perPage", false]]);
    expect(list.requiredParams).toEqual([{ location: "query", name: "perPage" }]);
    expect(list.defaultAuth).toMatchObject({ type: "apiKey", scheme: "cookieAuth", variable: "session" });
    expect((await b.operationOpen(r.project.id, id("createPasswordLogin"))).defaultAuth).toBeNull();
    expect((await b.operationOpen(r.project.id, { kind: "methodPath", value: { method: "POST", path: "/api/v1/files" } })).request.body.kind).toBe("multipart");
    await expect(b.operationOpen(r.project.id, id("nope"))).rejects.toMatchObject({ kind: "notFound" });
  });
});

describe("スキーマ表と例の生成（モック）", () => {
  const circular: Schema = {
    type: "object",
    "x-nekoreq-ref": "Node",
    required: ["id"],
    properties: {
      id: { type: "integer" },
      next: { type: "object", "x-nekoreq-ref": "Node", "x-nekoreq-circular": true },
      tags: { type: "array", items: { type: "string", enum: ["a", "b"] } },
    },
  };

  it("入れ子は深さつき、循環は 1 回で止まる", () => {
    const rows = schemaRows(circular);
    expect(rows.map((r) => [r.depth, r.name, r.typeLabel, r.circular])).toEqual([
      [0, "(ルート)", "Node", false],
      [1, "id", "integer", false],
      [1, "next", "Node", true],
      [1, "tags", "string[]", false],
    ]);
    expect(rows[2].constraints).toContain("循環");
    expect(rows[1].required).toBe(true);
  });

  it("スキーマだけから作る例と、example・enum を使う例", () => {
    expect(generateExample(circular, false)).toEqual({ id: 0, next: null, tags: ["string"] });
    expect(generateExample(circular, true)).toEqual({ id: 0, next: null, tags: ["a"] });
  });
});
