import type { AdHocRequest, OperationListItem } from "../bindings";
import { opKeyString } from "./opkey";

/** サイドバーのツリーの 1 行（仮想スクロールで並べるので、開いているフォルダの中身まで平らな配列にする）。 */
export type TreeRow =
  | { type: "folder"; key: string; id: string; label: string; count: number; open: boolean; group: "tag" | "deleted" | "adhoc" }
  | { type: "op"; key: string; item: OperationListItem }
  | { type: "adhoc"; key: string; item: AdHocRequest }
  | { type: "hint"; key: string; text: string };

export const NO_TAG_LABEL = "（タグなし）";
export const DELETED_LABEL = "削除された API";
export const ADHOC_LABEL = "その他";

/** 検索語（空白区切りは AND）が、API の概要・operationId・path・method・タグに当たるか。大文字小文字は区別しない。 */
export function matchesOp(item: OperationListItem, query: string): boolean {
  const words = query.toLowerCase().split(/\s+/).filter((w) => w !== "");
  if (words.length === 0) return true;
  const hay = [item.summary, item.operationId, item.path, item.method, ...item.tags].filter(Boolean).join("\n").toLowerCase();
  return words.every((w) => hay.includes(w));
}

export function matchesAdhoc(item: AdHocRequest, query: string): boolean {
  const words = query.toLowerCase().split(/\s+/).filter((w) => w !== "");
  if (words.length === 0) return true;
  const hay = [item.name, item.url, item.method].join("\n").toLowerCase();
  return words.every((w) => hay.includes(w));
}

export interface TreeInput {
  ops: OperationListItem[];
  adhocs: AdHocRequest[];
  /** 「その他」の読み込みが終わった（空のときの案内を出してよい）。 */
  adhocLoaded: boolean;
  query: string;
  /** フォルダの開閉（フォルダの `id` → 開いているか）。なければ既定（最初のタグと「その他」だけ開く）。 */
  expanded: Record<string, boolean | undefined>;
}

/** 件数の表示（「88 件 · タグ 19」）用。 */
export function summarize(ops: OperationListItem[]): { count: number; tags: number } {
  const live = ops.filter((o) => !o.deleted);
  return { count: live.length, tags: new Set(live.map((o) => o.folder).filter((f): f is string => f !== null)).size };
}

export function buildTree({ ops, adhocs, adhocLoaded, query, expanded }: TreeInput): TreeRow[] {
  const searching = query.trim() !== "";
  const rows: TreeRow[] = [];
  const open = (id: string, fallback: boolean) => (searching ? true : (expanded[id] ?? fallback));

  const live = ops.filter((o) => !o.deleted && matchesOp(o, query));
  const hasFolders = ops.some((o) => !o.deleted && o.folder !== null);
  if (hasFolders) {
    const order: (string | null)[] = [];
    for (const o of live) if (!order.includes(o.folder)) order.push(o.folder);
    // 既定で開くのは、並びの最初のタグ（検索で絞り込む前の並び）
    const firstTag = ops.find((o) => !o.deleted && o.folder !== null)?.folder ?? null;
    for (const folder of order) {
      const label = folder ?? NO_TAG_LABEL;
      const id = `tag:${folder ?? ""}`;
      const inside = live.filter((o) => o.folder === folder);
      const isOpen = open(id, folder !== null && folder === firstTag);
      rows.push({ type: "folder", key: id, id, label, count: inside.length, open: isOpen, group: "tag" });
      if (isOpen) for (const item of inside) rows.push({ type: "op", key: `op:${opKeyString(item.opKey)}`, item });
    }
  } else {
    for (const item of live) rows.push({ type: "op", key: `op:${opKeyString(item.opKey)}`, item });
  }

  const gone = ops.filter((o) => o.deleted && matchesOp(o, query));
  if (gone.length > 0) {
    const id = "deleted";
    const isOpen = open(id, false);
    rows.push({ type: "folder", key: id, id, label: DELETED_LABEL, count: gone.length, open: isOpen, group: "deleted" });
    if (isOpen) for (const item of gone) rows.push({ type: "op", key: `op:${opKeyString(item.opKey)}`, item });
  }

  const mine = adhocs.filter((a) => matchesAdhoc(a, query));
  if (!searching || mine.length > 0) {
    const id = "adhoc";
    const isOpen = open(id, true);
    rows.push({ type: "folder", key: id, id, label: ADHOC_LABEL, count: mine.length, open: isOpen, group: "adhoc" });
    if (isOpen) {
      for (const item of mine) rows.push({ type: "adhoc", key: `adhoc:${item.id}`, item });
      if (mine.length === 0 && adhocLoaded) rows.push({ type: "hint", key: "adhoc-hint", text: "保存したリクエストがここに並びます。" });
    }
  }
  return rows;
}
