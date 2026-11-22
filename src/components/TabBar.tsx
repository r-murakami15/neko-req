import * as ContextMenu from "@radix-ui/react-context-menu";
import { useEffect, useLayoutEffect, useRef, type KeyboardEvent, type RefObject } from "react";
import { useBackend } from "../api";
import { useAdhocList, useOperations } from "../api/queries";
import { copyCurlOfTab } from "../lib/curl";
import { opKeyString } from "../lib/opkey";
import { isMac, SHORTCUTS, shortcutText } from "../lib/platform";
import { methodColor } from "../lib/spec";
import { locateRow } from "../lib/tree";
import { openBlankTab } from "../store/ops";
import { closeTab, closeTabs } from "../store/send";
import { isDirty, type RequestTab, useTabsStore } from "../store/tabs";
import { useUiStore } from "../store/ui";
import { CloseIcon } from "./Icons";

/** タブの右クリックメニューを、キーボード（Shift+F10・コンテキストメニューキー）でも開く。Radix は `contextmenu` イベントだけで開くため、macOS の WebKit など、キーでそのイベントが出ない環境向けに発火させる。 */
function openMenuByKey(e: KeyboardEvent<HTMLElement>) {
  if (e.key !== "ContextMenu" && !(e.key === "F10" && e.shiftKey)) return;
  e.preventDefault();
  const rect = (e.target as HTMLElement).getBoundingClientRect();
  e.currentTarget.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: rect.left, clientY: rect.bottom }));
}

/** タブの右クリックメニューの中身。開いたときだけ描かれる（ツリーの位置の調べものは、開いたタブ 1 枚ぶんだけ）。 */
function TabMenuContent({ tab, tabs, trigger }: { tab: RequestTab; tabs: RequestTab[]; trigger: RefObject<HTMLElement> }) {
  const backend = useBackend();
  const operations = useOperations(tab.projectId);
  const adhoc = useAdhocList(tab.projectId);
  const treeQuery = useUiStore((s) => s.treeQuery);

  const index = tabs.findIndex((t) => t.id === tab.id);
  const others = tabs.filter((t) => t.id !== tab.id);
  const rights = tabs.slice(index + 1);
  // ツリーに行がある（検索で隠れてもいない）ときだけ「ツリーで表示」できる
  const rowKey = tab.op ? `op:${opKeyString(tab.op.opKey)}` : tab.adhocId ? `adhoc:${tab.adhocId}` : null;
  const location = rowKey === null ? null : locateRow(rowKey, { ops: operations.data ?? [], adhocs: adhoc.data ?? [], query: treeQuery });
  const canReveal = rowKey !== null && location?.status === "ok";
  // yaml の API のタブは、複製すると認証（yaml の security に従う既定）や設計タブの意味が変わるので複製しない
  const canDuplicate = tab.op === null;
  const mod = shortcutText(SHORTCUTS.find((s) => s.id === "closeTab")!);

  const activate = () => useTabsStore.getState().activate(tab.projectId, tab.id);

  return (
    <ContextMenu.Content
      className="menu"
      aria-label={`${tab.name} のメニュー`}
      onCloseAutoFocus={(e) => {
        // 閉じたら、そのタブにフォーカスを戻す（ダイアログが開いたときは、ダイアログに任せる）
        e.preventDefault();
        if (!document.querySelector('[role="dialog"]')) trigger.current?.querySelector<HTMLElement>(".tab-main")?.focus();
      }}
    >
      <ContextMenu.Item className="menu-item" aria-keyshortcuts={isMac() ? "Meta+W" : "Control+W"} onSelect={() => closeTab(backend, tab.id)}>
        <span className="menu-item-text">タブを閉じる</span>
        <span className="menu-item-shortcut" aria-hidden="true">
          {mod}
        </span>
      </ContextMenu.Item>
      <ContextMenu.Item className="menu-item" disabled={others.length === 0} onSelect={() => closeTabs(backend, others.map((t) => t.id))}>
        <span className="menu-item-text">他のタブをすべて閉じる</span>
      </ContextMenu.Item>
      <ContextMenu.Item className="menu-item" disabled={rights.length === 0} onSelect={() => closeTabs(backend, rights.map((t) => t.id))}>
        <span className="menu-item-text">右側のタブをすべて閉じる</span>
      </ContextMenu.Item>
      <ContextMenu.Item className="menu-item" onSelect={() => closeTabs(backend, tabs.map((t) => t.id))}>
        <span className="menu-item-text">すべてのタブを閉じる</span>
      </ContextMenu.Item>
      <ContextMenu.Separator className="menu-separator" />
      <ContextMenu.Item
        className="menu-item"
        disabled={!canDuplicate}
        title={canDuplicate ? undefined : "OpenAPI の API のタブは複製できません"}
        onSelect={() => useTabsStore.getState().duplicate(tab.id)}
      >
        <span className="menu-item-text">タブを複製</span>
      </ContextMenu.Item>
      <ContextMenu.Item
        className="menu-item"
        disabled={!canReveal}
        title={canReveal ? undefined : rowKey === null ? "保存していないリクエストは、ツリーにありません" : "ツリーに出ていません（検索で絞り込んでいる場合は、検索を消してください）"}
        onSelect={() => {
          if (rowKey === null) return;
          activate();
          useUiStore.getState().requestReveal(rowKey);
        }}
      >
        <span className="menu-item-text">ツリーで表示</span>
      </ContextMenu.Item>
      <ContextMenu.Item
        className="menu-item"
        onSelect={() => {
          // 結果の知らせは、そのタブの画面の上に出る。前面でないタブなら、前面にしてから出す
          activate();
          void copyCurlOfTab(backend, tab, "redacted");
        }}
      >
        <span className="menu-item-text">cURL としてコピー</span>
      </ContextMenu.Item>
    </ContextMenu.Content>
  );
}

/** タブ 1 枚。右クリック（Shift+F10・コンテキストメニューキー）で、そのタブが対象のメニューを開く。 */
function TabItem({ tab, tabs, activeId, projectId }: { tab: RequestTab; tabs: RequestTab[]; activeId: string | undefined; projectId: string }) {
  const backend = useBackend();
  const activate = useTabsStore((s) => s.activate);
  const ref = useRef<HTMLDivElement>(null);
  return (
    <ContextMenu.Root>
      <ContextMenu.Trigger asChild>
        <div ref={ref} className={`tab${tab.id === activeId ? " is-active" : ""}`} role="presentation" onKeyDown={openMenuByKey}>
          <button type="button" role="tab" aria-selected={tab.id === activeId} className="tab-main" onClick={() => activate(projectId, tab.id)}>
            <span className="tab-method" style={{ color: methodColor(tab.spec.method) }}>
              {tab.spec.method}
            </span>
            <span className="tab-name">{tab.name}</span>
            {isDirty(tab) && (
              <span className="tab-dirty" role="img" aria-label="未保存">
                ●
              </span>
            )}
          </button>
          <button type="button" className="tab-close" aria-label={`${tab.name} を閉じる`} onClick={() => closeTab(backend, tab.id)}>
            <CloseIcon />
          </button>
        </div>
      </ContextMenu.Trigger>
      <ContextMenu.Portal>
        <TabMenuContent tab={tab} tabs={tabs} trigger={ref} />
      </ContextMenu.Portal>
    </ContextMenu.Root>
  );
}

/** 開いているリクエストのタブ列。 */
export function TabBar({ projectId, tabs, activeId }: { projectId: string; tabs: RequestTab[]; activeId: string | undefined }) {
  const listRef = useRef<HTMLDivElement>(null);

  // 前面のタブが変わったら、そのタブが見える位置まで横にずらす（外側のスクロールは動かさない）
  useLayoutEffect(() => {
    const list = listRef.current;
    const active = list?.querySelector<HTMLElement>(".tab.is-active");
    if (!list || !active) return;
    const left = active.offsetLeft;
    const right = left + active.offsetWidth;
    if (left < list.scrollLeft) list.scrollLeft = left;
    else if (right > list.scrollLeft + list.clientWidth) list.scrollLeft = right - list.clientWidth;
  }, [activeId, projectId]);

  // マウスホイールの縦回転（横成分なし）を横スクロールに変える。passive では preventDefault できないので直接つける
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const onWheel = (e: WheelEvent) => {
      if (e.deltaX !== 0 || e.deltaY === 0 || e.ctrlKey) return;
      const max = list.scrollWidth - list.clientWidth;
      if (max <= 0) return;
      if ((e.deltaY < 0 && list.scrollLeft <= 0) || (e.deltaY > 0 && list.scrollLeft >= max)) return;
      list.scrollLeft += e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY;
      e.preventDefault();
    };
    list.addEventListener("wheel", onWheel, { passive: false });
    return () => list.removeEventListener("wheel", onWheel);
  }, []);

  return (
    <div className="tabbar">
      <div ref={listRef} className="tablist" role="tablist" aria-label="開いているリクエスト">
        {tabs.length === 0 && <span className="tabbar-empty">開いているリクエストはありません</span>}
        {tabs.map((t) => (
          <TabItem key={t.id} tab={t} tabs={tabs} activeId={activeId} projectId={projectId} />
        ))}
      </div>
      <button type="button" className="tab-add" aria-label="新しいタブ" title="空のリクエストを新しいタブで開く" onClick={() => openBlankTab(projectId)}>
        +
      </button>
    </div>
  );
}
