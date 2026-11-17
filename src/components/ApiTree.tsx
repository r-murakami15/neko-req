import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useRef } from "react";

import type { AdHocRequest, OperationListItem } from "../bindings";
import { methodColor } from "../lib/spec";
import type { TreeRow } from "../lib/tree";
import { TrashIcon } from "./Icons";

/** 1 行の高さ。仮想スクロールの計算に使うので、CSS（.tree-row）の高さと合わせる。 */
export const TREE_ROW_HEIGHT = 28;

interface Props {
  rows: TreeRow[];
  /** 前面のタブの行（`TreeRow.key`）。 */
  activeKey: string | null;
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
 */
export function ApiTree({ rows, activeKey, emptyMessage, onToggle, onOpenOp, onOpenAdhoc, onDeleteAdhoc, onForgetOp }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => TREE_ROW_HEIGHT,
    overscan: 10,
    getItemKey: (i) => rows[i]?.key ?? i,
  });

  // タブを切り替えたら、その API の行が見えるところまでスクロールする
  const activeIndex = activeKey === null ? -1 : rows.findIndex((r) => r.key === activeKey);
  useEffect(() => {
    if (activeIndex >= 0) virtualizer.scrollToIndex(activeIndex, { align: "auto" });
    // 行が増減しても、前面のタブが変わったとき以外は動かさない
  }, [activeKey]);

  return (
    <div className="tree-scroll" ref={scrollRef} role="group" aria-label="API 一覧">
      {emptyMessage && <p className="tree-hint">{emptyMessage}</p>}
      <div className="tree-sizer" style={{ height: virtualizer.getTotalSize() }}>
        {virtualizer.getVirtualItems().map((v) => {
          const row = rows[v.index];
          if (!row) return null;
          return (
            <div
              key={row.key}
              className="tree-slot"
              data-index={v.index}
              style={{ height: v.size, transform: `translateY(${v.start}px)` }}
            >
              <TreeRowView
                row={row}
                active={row.key === activeKey}
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

function TreeRowView({
  row,
  active,
  onToggle,
  onOpenOp,
  onOpenAdhoc,
  onDeleteAdhoc,
  onForgetOp,
}: { row: TreeRow; active: boolean } & Omit<Props, "rows" | "activeKey" | "emptyMessage">) {
  switch (row.type) {
    case "folder":
      return (
        <button type="button" className="tree-folder" aria-expanded={row.open} onClick={() => onToggle(row.id)}>
          <span aria-hidden="true" className="tree-chevron">
            {row.open ? "▾" : "▸"}
          </span>
          <span className="tree-folder-name">{row.label}</span>
          <span className="tree-count">{row.count}</span>
        </button>
      );
    case "op": {
      const { item } = row;
      const name = item.summary ?? item.operationId ?? item.path;
      return (
        <div className={`tree-row${active ? " is-active" : ""}${item.deleted ? " is-deleted" : ""}`}>
          <button type="button" className="tree-item" title={`${item.method} ${item.path}`} onClick={() => onOpenOp(item)}>
            <span className="tree-method" style={{ color: methodColor(item.method) }}>
              {item.method}
            </span>
            <span className="tree-name">{name}</span>
          </button>
          {item.deleted && (
            <button type="button" className="icon-btn tree-delete" aria-label={`${name} を消す`} onClick={() => onForgetOp(item)}>
              <TrashIcon />
            </button>
          )}
        </div>
      );
    }
    case "adhoc":
      return (
        <div className={`tree-row${active ? " is-active" : ""}`}>
          <button type="button" className="tree-item" onClick={() => onOpenAdhoc(row.item)}>
            <span className="tree-method" style={{ color: methodColor(row.item.method) }}>
              {row.item.method}
            </span>
            <span className="tree-name">{row.item.name}</span>
          </button>
          <button type="button" className="icon-btn tree-delete" aria-label={`${row.item.name} を削除`} onClick={() => onDeleteAdhoc(row.item)}>
            <TrashIcon />
          </button>
        </div>
      );
    case "hint":
      return <p className="tree-hint">{row.text}</p>;
  }
}
