import { describe, expect, it } from "vitest";

import { ApiError } from "../api/backend";
import { createMockBackend } from "./backend";

describe("モックのバックエンド", () => {
  it("プロジェクトの作成・一覧・名前変更・削除", async () => {
    const b = createMockBackend();
    expect(await b.projectList()).toEqual([]);

    const a = await b.projectCreate("  A  ");
    expect(a.name).toBe("A");
    await b.projectCreate("B");
    expect((await b.projectList()).map((p) => p.name)).toEqual(["A", "B"]);

    const renamed = await b.projectRename(a.id, "A2");
    expect(renamed.name).toBe("A2");

    await b.projectDelete(a.id);
    expect((await b.projectList()).map((p) => p.name)).toEqual(["B"]);
  });

  it("Rust 側と同じ検証をする", async () => {
    const b = createMockBackend();
    await expect(b.projectCreate("   ")).rejects.toMatchObject({ kind: "validation" });
    await expect(b.projectCreate("あ".repeat(101))).rejects.toBeInstanceOf(ApiError);
    await expect(b.projectCreate("あ".repeat(100))).resolves.toBeDefined();
    await expect(b.projectRename("nope", "x")).rejects.toMatchObject({ kind: "notFound" });
    await expect(b.projectDelete("nope")).rejects.toMatchObject({ kind: "notFound" });
  });

  it("appInfo を返す", async () => {
    const info = await createMockBackend().appInfo();
    expect(info.name).toBe("NekoReq");
    expect(info.schemaVersion).toBeGreaterThanOrEqual(1);
  });
});

describe("モックの送信まわり", () => {
  const spec = (url: string) => ({ method: "GET", url, params: [], body: { kind: "none" as const }, auth: null });

  it("環境の変数で展開し、シークレットは伏せ字にして、展開できない変数を場所つきで返す", async () => {
    const b = createMockBackend();
    const p = await b.projectCreate("P");
    const env = await b.envCreate(p.id, "ローカル");
    expect((await b.projectList())[0].defaultEnvironmentId).toBe(env.id); // 最初の環境が選ばれる
    const [global] = await b.envList(p.id);
    expect(global.isGlobal).toBe(true);
    await b.envSave({
      ...global,
      vars: [{ name: "token", value: { kind: "secret" }, description: "", enabled: true }],
    });
    await b.secretSet(p.id, global.id, "token", "s3cret");
    await b.envSave({
      ...env,
      vars: [{ name: "baseUrl", value: { kind: "plain", value: "http://localhost:8080" }, description: "", enabled: true }],
    });

    const input = { projectId: p.id, environmentId: null, overrides: [] };
    const ok = await b.requestPreview({
      ...input,
      request: { ...spec("{{baseUrl}}/a"), auth: { type: "bearer", token: "{{token}}" }, params: [{ location: "query", name: "k", value: "{{token}}", enabled: true }] },
    });
    expect(ok.unresolved).toEqual([]);
    expect(ok.url).toBe("http://localhost:8080/a?k=••••");
    expect(ok.autoHeaders).toContainEqual({ name: "Authorization", value: "Bearer ••••••••", source: "認証（Bearer）" });

    const bad = await b.requestPreview({
      ...input,
      request: { ...spec("{{host}}/a"), params: [{ location: "header", name: "X", value: "{{nope}}", enabled: true }] },
    });
    expect(bad.unresolved).toEqual([
      { name: "host", location: { kind: "url" } },
      { name: "nope", location: { kind: "param", location: "header", row: 1, part: "value" } },
    ]);
  });

  it("プレビューの URL は、展開できない変数と値のないパスパラメータを書かれたまま見せる", async () => {
    const b = createMockBackend();
    const p = await b.projectCreate("P");
    const input = { projectId: p.id, environmentId: null, overrides: [] };
    const q = await b.requestPreview({
      ...input,
      request: { ...spec("http://h/x"), params: [{ location: "query", name: "q", value: "{{keyword}}", enabled: true }] },
    });
    expect(q.url).toBe("http://h/x?q={{keyword}}");
    const path = await b.requestPreview({ ...input, request: spec("http://h/clients/{clientId}") });
    expect(path.url).toBe("http://h/clients/{clientId}");
    expect(path.missingPathParams).toEqual(["clientId"]);
  });

  it("送信の結果はイベントで届き、中止もできる", async () => {
    const b = createMockBackend([], { sendDelayMs: 5 });
    const p = await b.projectCreate("P");
    const events: { sendId: string; status: string }[] = [];
    const off = await b.onRequestFinished((e) => events.push({ sendId: e.sendId, status: e.result.status }));
    const input = (url: string) => ({ projectId: p.id, environmentId: null, overrides: [], request: spec(url) });

    const id = await b.requestSend(input("http://localhost:8080/ok"), null);
    await new Promise((r) => setTimeout(r, 30));
    expect(events).toEqual([{ sendId: id, status: "success" }]);
    await expect(b.responseSaveBody(id, "/tmp/x")).resolves.toBeUndefined();

    const slow = createMockBackend([], { sendDelayMs: 1000 });
    const p2 = await slow.projectCreate("P");
    const got: string[] = [];
    await slow.onRequestFinished((e) => got.push(e.result.status === "failure" ? e.result.error.kind : "success"));
    const id2 = await slow.requestSend({ projectId: p2.id, environmentId: null, overrides: [], request: spec("http://x/y") }, null);
    expect(await slow.requestCancel(id2)).toBe(true);
    expect(await slow.requestCancel(id2)).toBe(false);
    await new Promise((r) => setTimeout(r, 20));
    expect(got).toEqual(["cancelled"]);
    off();
  });

  it("手で作ったリクエストの保存・更新・削除", async () => {
    const b = createMockBackend();
    const p = await b.projectCreate("P");
    const base = { id: "", projectId: p.id, name: " 一覧 ", method: "get", url: "{{baseUrl}}/x", params: [], body: { kind: "none" as const }, auth: null, createdAt: "", updatedAt: "" };
    const a = await b.adhocSave(base);
    expect(a).toMatchObject({ name: "一覧", method: "GET" });
    expect(a.id).not.toBe("");
    const again = await b.adhocSave({ ...a, url: "{{baseUrl}}/y" });
    expect(again.id).toBe(a.id);
    expect(again.createdAt).toBe(a.createdAt);
    expect(await b.adhocList(p.id)).toHaveLength(1);
    // 説明は省略すると空。往復で保たれる
    expect(a.description).toBe("");
    const described = await b.adhocSave({ ...a, description: "ログイン状態を確かめる" });
    expect(described.description).toBe("ログイン状態を確かめる");
    expect((await b.adhocList(p.id))[0].description).toBe("ログイン状態を確かめる");
    await expect(b.adhocSave({ ...base, name: "  " })).rejects.toMatchObject({ kind: "validation" });
    await b.adhocDelete(a.id);
    expect(await b.adhocList(p.id)).toEqual([]);
    await expect(b.adhocDelete(a.id)).rejects.toMatchObject({ kind: "notFound" });
  });
});
