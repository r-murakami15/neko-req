import { defaultRangeExtractor, useVirtualizer, type Range } from "@tanstack/react-virtual";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type MouseEvent } from "react";

import type { AdHocRequest, OperationListItem } from "../bindings";
import { methodColor } from "../lib/spec";
import { isNavigable, type TreeMeta, type TreeRow, treeMeta } from "../lib/tree";
import { TrashIcon } from "./Icons";

/** 1 行の高さ。仮想スクロールの計算に使うので、CSS（.tree-row）の高さと合わせる。 */
export const TREE_ROW_HEIGHT = 28;

interface Props {
  rows: TreeRow[];
  /** 前面のタブの行（`TreeRow.key`）。 */
  activeKey: string | null;
  /** 前面のタブの行が入っているフォルダの `id`（フォルダに入らない・検索に当たっていない・ツリーにないときは `null`）。 */
  activeFolderId: string | null;
  /** 前面のタブが変わって、その行を示す（スクロールする）よう頼まれた。`nonce` が変わるたびにスクロールする。開閉の状態を変えた描画と同じ描画で届く。 */
  reveal: { key: string; nonce: number } | null;
  /** 検索中で、1 件も当たらない。 */
  emptyMessage: string | null;
  onToggle: (folderId: string) => void;
  onOpenOp: (item: OperationListItem) => void;
  onOpenAdhoc: (item: AdHocRequest) => void;
  onDeleteAdhoc: (item: AdHocRequest) => void;
  onForgetOp: (item: OperationListItem) => void;
}

/**
 * API ツリー。フォルダ（タグ）の開閉を含めて平らな行にし、見えている行だけを描く（TanStack Virtual）。
 * API が 1,000 件あっても引っかからない。
 *
 * キーボード操作は WAI-ARIA の tree パターン。行（`role="treeitem"`）のうち Tab で入れるのは 1 行だけ（roving tabindex）で、
 * ↑↓ で前後・Home / End で先頭と末尾・→ ← で開閉と親子の移動・Enter / Space で開く。
 * フォーカスのある行は、画面の外にあっても描く（仮想スクロールで消えないように）。
 *
 * 枠（`is-focus-ring`）は常に 1 つ。ツリーにフォーカスがある間はフォーカスのある行、ツリーの外にある間は前面のタブの行
 * （その行が閉じたフォルダの中に隠れているときは、そのフォルダの行）。前面のタブがツリーにないときは枠なし。
 */
export function ApiTree({ rows, activeKey, activeFolderId, reveal, emptyMessage, onToggle, onOpenOp, onOpenAdhoc, onDeleteAdhoc, onForgetOp }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const treeRef = useRef<HTMLDivElement>(null);
  const meta = useMemo(() => treeMeta(rows), [rows]);
  const [focusedKey, setFocusedKey] = useState<string | null>(null);
  /** キーで移ったあと、その行にフォーカスを移す（描画のあとで）。 */
  const pendingFocus = useRef<string | null>(null);
  /**
   * ツリーにフォーカスがある間は、フォーカスのある行に枠のクラスを付ける（キーでもクリックでも）。
   * macOS の WebView（WebKit）は、キーで移したあとの el.focus() やクリックに :focus-visible を付けないことがあり、
   * CSS の :focus-visible だけでは枠が消えるため。
   */
  const [treeFocused, setTreeFocused] = useState(false);

  // 前面のタブの行。閉じたフォルダに隠れているときは、そのフォルダの行（どこにいるか分かるように）
  const activeIndex = activeKey === null ? -1 : rows.findIndex((r) => r.key === activeKey);
  const shownIndex = activeIndex >= 0 || activeFolderId === null ? activeIndex : rows.findIndex((r) => r.type === "folder" && r.id === activeFolderId);

  // Tab で入る 1 行: 最後にフォーカスした行 → 前面のタブの行 → 先頭の行
  const tabbableIndex = useMemo(() => {
    const focused = focusedKey === null ? -1 : rows.findIndex((r) => r.key === focusedKey);
    if (focused >= 0) return focused;
    if (shownIndex >= 0) return shownIndex;
    return rows.findIndex(isNavigable);
  }, [rows, focusedKey, shownIndex]);
  const ringKey = treeFocused ? focusedKey : (rows[shownIndex]?.key ?? null);

  const rangeExtractor = useCallback(
    (range: Range) => {
      const base = defaultRangeExtractor(range);
      if (tabbableIndex < 0 || base.includes(tabbableIndex)) return base;
      return [...base, tabbableIndex].sort((a, b) => a - b);
    },
    [tabbableIndex],
  );

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => TREE_ROW_HEIGHT,
    overscan: 10,
    rangeExtractor,
    getItemKey: (i) => rows[i]?.key ?? i,
  });

  // 前面のタブが変わったら、Tab で入る行も前面のタブの行に合わせる（ツリーにフォーカスがある間は、フォーカスのある行のまま）
  const treeFocusedRef = useRef(false);
  treeFocusedRef.current = treeFocused;
  useEffect(() => {
    if (!treeFocusedRef.current) setFocusedKey(null);
  }, [activeKey]);

  // 前面のタブが変わったら、その行が見えるところまでスクロールする。フォルダを開いて行が増えたあとの位置で行う
  // （Sidebar が開閉の状態と一緒に `reveal` を出すので、この描画の `rows` にはもう行がある）
  useEffect(() => {
    if (!reveal) return;
    const index = rows.findIndex((r) => r.key === reveal.key);
    if (index >= 0) virtualizer.scrollToIndex(index, { align: "auto" });
    // 行が増減しても、頼まれたとき以外は動かさない
  }, [reveal]);

  // キーで移った行にフォーカスを移す
  useLayoutEffect(() => {
    const key = pendingFocus.current;
    if (key === null) return;
    const index = rows.findIndex((r) => r.key === key);
    const el = index < 0 ? null : treeRef.current?.querySelector<HTMLElement>(`[role="treeitem"][data-index="${index}"]`);
    if (el) {
      pendingFocus.current = null;
      el.focus({ preventScroll: true });
    }
  });

  const moveTo = (index: number) => {
    const row = rows[index];
    if (!row || !isNavigable(row)) return;
    pendingFocus.current = row.key;
    setFocusedKey(row.key);
    virtualizer.scrollToIndex(index, { align: "auto" });
  };

  const onFocusRow = (key: string) => {
    setFocusedKey(key);
    setTreeFocused(true);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    // フォーカスが行そのものにあるときだけ（行の中の削除ボタンなどは、そのボタンの動きに任せる）。⌘・Ctrl の組み合わせは画面全体のショートカット
    const target = e.target as HTMLElement;
    if (target.getAttribute("role") !== "treeitem" || e.ctrlKey || e.metaKey || e.altKey || e.shiftKey) return;
    const index = Number(target.dataset.index);
    const row = rows[index];
    if (!row || Number.isNaN(index)) return;
    const m = meta[index];

    const step = (from: number, dir: 1 | -1) => {
      for (let i = from + dir; i >= 0 && i < rows.length; i += dir) if (isNavigable(rows[i])) return i;
      return -1;
    };
    let handled = true;
    switch (e.key) {
      case "ArrowDown": {
        const next = step(index, 1);
        if (next >= 0) moveTo(next);
        break;
      }
      case "ArrowUp": {
        const prev = step(index, -1);
        if (prev >= 0) moveTo(prev);
        break;
      }
      case "Home":
        moveTo(step(-1, 1));
        break;
      case "End":
        moveTo(step(rows.length, -1));
        break;
      case "ArrowRight":
        if (row.type === "folder") {
          if (!row.open) onToggle(row.id);
          else if (meta[index + 1]?.parent === index && isNavigable(rows[index + 1])) moveTo(index + 1);
        }
        break;
      case "ArrowLeft":
        if (row.type === "folder") {
          if (row.open) onToggle(row.id);
        } else if (m && m.parent >= 0) moveTo(m.parent);
        break;
      case "Enter":
      case " ":
        if (row.type === "folder") onToggle(row.id);
        else if (row.type === "op") onOpenOp(row.item);
        else if (row.type === "adhoc") onOpenAdhoc(row.item);
        break;
      case "Delete":
        if (row.type === "adhoc") onDeleteAdhoc(row.item);
        else if (row.type === "op" && row.item.deleted) onForgetOp(row.item);
        break;
      default:
        handled = false;
    }
    if (handled) e.preventDefault();
  };

  return (
    <div className="tree-scroll" ref={scrollRef}>
      {emptyMessage && <p className="tree-hint">{emptyMessage}</p>}
      <div
        className="tree-sizer"
        ref={treeRef}
        role="tree"
        aria-label="API 一覧"
        style={{ height: virtualizer.getTotalSize() }}
        onKeyDown={onKeyDown}
        onBlur={(e) => {
          // ツリーの外へフォーカスが出たら枠を消す（行どうしの移動では消さない）
          if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setTreeFocused(false);
        }}
      >
        {virtualizer.getVirtualItems().map((v) => {
          const row = rows[v.index];
          if (!row) return null;
          return (
            <div
              key={row.key}
              className="tree-slot"
              role="none"
              style={{ height: v.size, transform: `translateY(${v.start}px)` }}
            >
              <TreeRowView
                row={row}
                index={v.index}
                meta={meta[v.index]}
                tabbable={v.index === tabbableIndex}
                active={row.key === activeKey}
                focusRing={row.key === ringKey}
                onFocusRow={onFocusRow}
                onToggle={onToggle}
                onOpenOp={onOpenOp}
                onOpenAdhoc={onOpenAdhoc}
                onDeleteAdhoc={onDeleteAdhoc}
                onForgetOp={onForgetOp}
              />
            </div>
          );
        })}
      </div>
    </div>
  );
}

interface RowProps extends Omit<Props, "rows" | "activeKey" | "activeFolderId" | "reveal" | "emptyMessage"> {
  row: TreeRow;
  index: number;
  meta: TreeMeta | undefined;
  tabbable: boolean;
  active: boolean;
  /** 枠を描く行（ツリーにフォーカスがあればフォーカスのある行、なければ前面のタブの行）。 */
  focusRing: boolean;
  onFocusRow: (key: string) => void;
}

/** 行の中のボタン（削除）を押したときは、行そのものの動き（開く）を起こさない。 */
function stop(e: MouseEvent, action: () => void) {
  e.stopPropagation();
  action();
}

function TreeRowView({ row, index, meta, tabbable, active, focusRing, onFocusRow, onToggle, onOpenOp, onOpenAdhoc, onDeleteAdhoc, onForgetOp }: RowProps) {
  // 行そのものが treeitem（フォーカスを受ける）。中のボタンは Tab では止まらず、削除は Delete キーでも押せる
  const item = {
    role: "treeitem" as const,
    tabIndex: tabbable ? 0 : -1,
    "data-index": index,
    "aria-level": meta?.level,
    "aria-setsize": meta?.setSize,
    "aria-posinset": meta?.posInSet,
    onFocus: () => onFocusRow(row.key),
  };
  const ring = focusRing ? " is-focus-ring" : "";
  switch (row.type) {
    case "folder":
      return (
        <div {...item} className={`tree-folder${ring}`} aria-expanded={row.open} onClick={() => onToggle(row.id)}>
          <span aria-hidden="true" className="tree-chevron">
            {row.open ? "▾" : "▸"}
          </span>
          <span className="tree-folder-name">{row.label}</span>
          <span className="tree-count">{row.count}</span>
        </div>
      );
    case "op": {
      const { item: op } = row;
      const name = op.summary ?? op.operationId ?? op.path;
      return (
        <div
          {...item}
          aria-selected={active}
          className={`tree-row${active ? " is-active" : ""}${op.deleted ? " is-deleted" : ""}${ring}`}
          onClick={() => onOpenOp(op)}
        >
          <div className="tree-item" title={`${op.method} ${op.path}`}>
            <span className="tree-method" style={{ color: methodColor(op.method) }}>
              {op.method}
            </span>
            <span className="tree-name">{name}</span>
          </div>
          {op.deleted && (
            <button type="button" tabIndex={-1} className="icon-btn tree-delete" aria-label={`${name} を消す`} onClick={(e) => stop(e, () => onForgetOp(op))}>
              <TrashIcon />
            </button>
          )}
        </div>
      );
    }
    case "adhoc":
      return (
        <div {...item} aria-selected={active} className={`tree-row${active ? " is-active" : ""}${ring}`} onClick={() => onOpenAdhoc(row.item)}>
          <div className="tree-item">
            <span className="tree-method" style={{ color: methodColor(row.item.method) }}>
              {row.item.method}
            </span>
            <span className="tree-name">{row.item.name}</span>
          </div>
          <button type="button" tabIndex={-1} className="icon-btn tree-delete" aria-label={`${row.item.name} を削除`} onClick={(e) => stop(e, () => onDeleteAdhoc(row.item))}>
            <TrashIcon />
          </button>
        </div>
      );
    case "hint":
      return <p className="tree-hint">{row.text}</p>;
  }
}
