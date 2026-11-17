import { afterEach, describe, expect, it, vi } from "vitest";

import { applyAppearance, applyCachedTheme, applyEditorFontSize, applyTheme } from "./appearance";
import { buildHistoryRows, dayLabel, shortUrl, statusText, statusTone, timeOf } from "./history";
import { isMac, SHORTCUTS, shortcutText } from "./platform";
import { parseProblem } from "./problem";

afterEach(() => vi.restoreAllMocks());

describe("platform", () => {
  it("OS でキーの表記を変える", () => {
    const send = SHORTCUTS.find((s) => s.id === "send")!;
    expect(isMac()).toBe(false);
    expect(shortcutText(send)).toBe("Ctrl Enter");
    expect(shortcutText(send, true)).toBe("⌘ Enter");
    vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
    expect(isMac()).toBe(true);
    expect(shortcutText(send)).toBe("⌘ Enter");
  });

  it("ショートカットは送信・保存・新しいタブ・タブを閉じる・検索・設定", () => {
    expect(SHORTCUTS.map((s) => s.label)).toEqual(["リクエストを送信", "保存", "新しいタブ", "タブを閉じる", "リクエストを検索", "設定を開く"]);
  });
});

describe("appearance", () => {
  it("テーマは data-theme に反映し、「OS に合わせる」は属性を外す", () => {
    const root = document.createElement("div");
    applyTheme("light", root);
    expect(root.getAttribute("data-theme")).toBe("light");
    applyTheme("dark", root);
    expect(root.getAttribute("data-theme")).toBe("dark");
    applyTheme("system", root);
    expect(root.hasAttribute("data-theme")).toBe(false);
    applyEditorFontSize(14, root);
    expect(root.style.getPropertyValue("--editor-font-size")).toBe("14px");
    applyAppearance({ theme: "light", editorFontSize: 15, restoreTabs: true }, root);
    expect(root.getAttribute("data-theme")).toBe("light");
    expect(root.style.getPropertyValue("--editor-font-size")).toBe("15px");
  });
});

describe("テーマの前回の値", () => {
  it("設定を読み込む前の最初の描画から、前回のテーマにする（OS に合わせるときは属性なし）", () => {
    const root = document.createElement("div");
    applyTheme("light", root);
    const next = document.createElement("div");
    applyCachedTheme(next);
    expect(next.getAttribute("data-theme")).toBe("light");
    applyTheme("system", root);
    const again = document.createElement("div");
    applyCachedTheme(again);
    expect(again.hasAttribute("data-theme")).toBe(false);
  });
});

describe("history の表示の部品", () => {
  const now = new Date(2026, 10, 17, 12, 0, 0);

  it("日付の見出しは今日・昨日・それ以前の日付", () => {
    expect(dayLabel("2026-11-17", now)).toBe("今日");
    expect(dayLabel("2026-11-16", now)).toBe("昨日");
    expect(dayLabel("2026-11-10", now)).toBe("2026/11/10");
  });

  it("URL は path と query だけにする（読めなければそのまま）。時刻は端末の時刻", () => {
    expect(shortUrl("http://localhost:8080/api/v1/x?a=1")).toBe("/api/v1/x?a=1");
    expect(shortUrl("{{baseUrl}}/x")).toBe("{{baseUrl}}/x");
    expect(timeOf(new Date(2026, 10, 17, 9, 5).toISOString())).toBe("09:05");
    expect(timeOf("不正")).toBe("");
  });

  it("ステータスの表示と色", () => {
    expect(statusText({ status: 200, failed: false })).toBe("200");
    expect(statusText({ status: null, failed: true })).toBe("失敗");
    expect(statusTone({ status: 204, failed: false })).toBe("ok");
    expect(statusTone({ status: 302, failed: false })).toBe("info");
    expect(statusTone({ status: 404, failed: false })).toBe("warn");
    expect(statusTone({ status: 503, failed: false })).toBe("danger");
    expect(statusTone({ status: null, failed: true })).toBe("fail");
  });

  it("日付ごとに見出しの行を挟んで平らにする", () => {
    const item = (id: string, localDate: string) =>
      ({ id, localDate, sentAt: "", method: "GET", url: "", status: 200, statusText: "OK", totalMs: 1, environmentId: null, environmentName: null, opKey: null, opSummary: null, failed: false, errorKind: null, responseSize: 0 }) as never;
    const rows = buildHistoryRows([item("a", "2026-11-17"), item("b", "2026-11-17"), item("c", "2026-11-16")], now);
    expect(rows.map((r) => (r.kind === "day" ? `day:${r.label}` : r.key))).toEqual(["day:今日", "a", "b", "day:昨日", "c"]);
  });
});

describe("problem+json", () => {
  it("title・code・detail を取り出す。problem+json 以外・読めないものは null", () => {
    expect(parseProblem("application/problem+json", JSON.stringify({ title: "T", code: "C", detail: "D" }))).toEqual({ title: "T", code: "C", detail: "D" });
    expect(parseProblem("application/json", JSON.stringify({ title: "T" }))).toBeNull();
    expect(parseProblem("application/problem+json", "not json")).toBeNull();
    expect(parseProblem("application/problem+json", null)).toBeNull();
    expect(parseProblem("application/problem+json", JSON.stringify({ status: 400 }))).toBeNull();
  });
});
