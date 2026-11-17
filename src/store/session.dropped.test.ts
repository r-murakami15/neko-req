import { describe, expect, it } from "vitest";

import { emptySpec } from "../lib/spec";
import { sessionOfProject } from "./session";
import { useTabsStore } from "./tabs";

describe("タブの保存（droppedFields の引き継ぎ）", () => {
  it("復元時の「認証の値を入れ直して」の案内は、閉じるまで次の保存にも droppedFields として残す", () => {
    const pid = "p-dropped";
    useTabsStore.getState().restoreProject(
      pid,
      [
        { name: "a", spec: emptySpec(), droppedNote: { source: "session", fields: ["auth.bearer.token"] } },
        { name: "b", spec: emptySpec(), droppedNote: { source: "history", fields: ["auth.basic.password"] } },
        { name: "c", spec: emptySpec() },
      ],
      0,
    );
    const saved = sessionOfProject(pid).tabs;
    expect(saved.map((t) => t.droppedFields)).toEqual([["auth.bearer.token"], [], []]);

    // 案内を閉じたら、もう引き継がない
    const first = useTabsStore.getState().tabs.find((t) => t.projectId === pid && t.name === "a")!;
    useTabsStore.getState().dismissDropped(first.id);
    expect(sessionOfProject(pid).tabs[0].droppedFields).toEqual([]);
  });
});
