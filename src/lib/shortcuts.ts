import { useEffect } from "react";

import { useBackend } from "../api";
import { useActionsStore } from "../store/actions";
import { openBlankTab } from "../store/ops";
import { closeTab } from "../store/send";
import { activeTabOf, useTabsStore } from "../store/tabs";
import { useUiStore } from "../store/ui";
import { isMac } from "./platform";

/** 検索欄（サイドバーの API 検索）に入力を移す。 */
export function focusSearch() {
  document.querySelector<HTMLInputElement>('input[aria-label="API を検索"]')?.focus();
}

/**
 * ショートカット（macOS は ⌘、Windows は Ctrl）: 送信 `Enter`・保存 `S`・空のタブを開く `T`（ダイアログなし）・タブを閉じる `W`・
 * 検索 `K`・設定 `,`・レスポンス欄の開閉 `Shift R`。ダイアログが開いているときは効かせない。
 */
export function useGlobalShortcuts(projectId: string) {
  const backend = useBackend();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = isMac() ? e.metaKey : e.ctrlKey;
      if (!mod || e.altKey || e.isComposing) return;
      if (document.querySelector('[role="dialog"]')) return;
      const ui = useUiStore.getState();
      const main = ui.screen === "main";
      const key = e.key.toLowerCase();
      // Shift を押すのはレスポンス欄の開閉（Shift+R）だけ
      if (e.shiftKey) {
        if (key !== "r" || !main) return;
        e.preventDefault();
        const { tabs, activeByProject } = useTabsStore.getState();
        const active = activeTabOf(tabs, activeByProject, projectId);
        // 設計タブ（レスポンス欄を隠している）や、タブがないときは、開閉を変えない
        if (active && !(active.op !== null && active.requestPane === "design")) ui.setResponseOpen(!ui.responseOpen);
        return;
      }
      switch (key) {
        case "enter":
          if (!main) return;
          e.preventDefault();
          useActionsStore.getState().editor?.send();
          return;
        case "s":
          if (!main) return;
          e.preventDefault();
          useActionsStore.getState().editor?.save();
          return;
        case "t":
          if (!main) return;
          e.preventDefault();
          openBlankTab(projectId);
          return;
        case "w": {
          if (!main) return;
          e.preventDefault();
          const { tabs, activeByProject } = useTabsStore.getState();
          const active = activeTabOf(tabs, activeByProject, projectId);
          if (active) closeTab(backend, active.id);
          return;
        }
        case "k":
          e.preventDefault();
          if (!main) {
            ui.setScreen("main");
            setTimeout(focusSearch, 0);
          } else focusSearch();
          return;
        case ",":
          e.preventDefault();
          ui.setSettingsDialogOpen(true);
          return;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [backend, projectId]);
}
