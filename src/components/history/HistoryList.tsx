import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useRef } from "react";

import type { HistoryListItem } from "../../bindings";
import { formatMs } from "../../lib/format";
import { type HistoryRow, shortUrl, statusText, statusTone, timeOf } from "../../lib/history";
import { methodColor } from "../../lib/spec";

/** 1 行の高さ。仮想スクロールの計算に使うので、CSS（.history-row・.history-day）の高さと合わせる。 */
export const HISTORY_ROW_HEIGHT = 34;
export const HISTORY_DAY_HEIGHT = 32;

interface Props {
  rows: HistoryRow[];
  selectedId: string | null;
  /** 比較の 2 件目を選んでいる最中。 */
  comparing: boolean;
  hasMore: boolean;
  fetching: boolean;
  onSelect: (item: HistoryListItem) => void;
  onNeedMore: () => void;
}

/**
 * 履歴の一覧。日付の見出しを含めて平らな行にし、見えている行だけを描く（TanStack Virtual）。
 * 履歴が 10,000 件あっても引っかからない。終わり近くまでスクロールしたら、続きを読む。
 */
export function HistoryList({ rows, selectedId, comparing, hasMore, fetching, onSelect, onNeedMore }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => (rows[i]?.kind === "day" ? HISTORY_DAY_HEIGHT : HISTORY_ROW_HEIGHT),
    overscan: 12,
    getItemKey: (i) => rows[i]?.key ?? i,
  });
  const items = virtualizer.getVirtualItems();
  const lastIndex = items.length > 0 ? items[items.length - 1].index : -1;

  useEffect(() => {
    if (hasMore && !fetching && lastIndex >= 0 && lastIndex >= rows.length - 10) onNeedMore();
  }, [hasMore, fetching, lastIndex, rows.length, onNeedMore]);

  return (
    <div className="history-scroll" ref={scrollRef} role="group" aria-label="履歴の一覧">
      <div className="history-sizer" style={{ height: virtualizer.getTotalSize() }}>
        {items.map((v) => {
          const row = rows[v.index];
          if (!row) return null;
          return (
            <div key={row.key} className="history-slot" data-index={v.index} style={{ height: v.size, transform: `translateY(${v.start}px)` }}>
              {row.kind === "day" ? (
                <div className="history-day">{row.label}</div>
              ) : (
                <HistoryRowView item={row.item} selected={row.item.id === selectedId} comparing={comparing} onSelect={onSelect} />
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function HistoryRowView({ item, selected, comparing, onSelect }: { item: HistoryListItem; selected: boolean; comparing: boolean; onSelect: (item: HistoryListItem) => void }) {
  const path = shortUrl(item.url);
  return (
    <button
      type="button"
      className={`history-row${selected ? " is-selected" : ""}`}
      aria-current={selected ? "true" : undefined}
      aria-label={`${item.method} ${path} ${statusText(item)}${comparing && !selected ? "（この履歴と比較する）" : ""}`}
      title={`${item.opSummary ? `${item.opSummary}\n` : ""}${item.url}`}
      onClick={() => onSelect(item)}
    >
      <span className="history-method" style={{ color: methodColor(item.method) }}>
        {item.method}
      </span>
      <span className="history-path">{path}</span>
      <span className={`history-status is-${statusTone(item)}`}>{statusText(item)}</span>
      <span className="history-ms">{item.totalMs === null ? "—" : formatMs(item.totalMs)}</span>
      <span className="history-time">{timeOf(item.sentAt)}</span>
    </button>
  );
}
