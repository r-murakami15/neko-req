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

/** 前面のタブの行が、ツリーのどこにあるか。 */
export interface RowLocation {
  /**
   * `ok` = ツリーに出せる。`hidden` = データにはあるが、検索に当たっていない。
   * `missing` = データにない（取り込みや読み込みがまだ・消えた）。
   */
  status: "ok" | "hidden" | "missing";
  /** 入っているフォルダの `id`（フォルダに入らない並びのときは `null`）。`ok` のときだけ。 */
  folderId: string | null;
}

/** 行のキー（`TreeRow.key`）から、その行が入るフォルダと、いま検索で見えるかを調べる。`buildTree` の並べ方と合わせる。 */
export function locateRow(key: string, { ops, adhocs, query }: { ops: OperationListItem[]; adhocs: AdHocRequest[]; query: string }): RowLocation {
  const missing: RowLocation = { status: "missing", folderId: null };
  if (key.startsWith("adhoc:")) {
    const found = adhocs.find((a) => `adhoc:${a.id}` === key);
    if (!found) return missing;
    return matchesAdhoc(found, query) ? { status: "ok", folderId: "adhoc" } : { status: "hidden", folderId: null };
  }
  const item = ops.find((o) => `op:${opKeyString(o.opKey)}` === key);
  if (!item) return missing;
  if (!matchesOp(item, query)) return { status: "hidden", folderId: null };
  if (item.deleted) return { status: "ok", folderId: "deleted" };
  const hasFolders = ops.some((o) => !o.deleted && o.folder !== null);
  return { status: "ok", folderId: hasFolders ? `tag:${item.folder ?? ""}` : null };
}

/** WAI-ARIA の tree 用の、行ごとの位置（`aria-level`・`aria-setsize`・`aria-posinset`）と親の行。案内の行（`hint`）は項目ではない。 */
export interface TreeMeta {
  /** 1 = いちばん上（フォルダ・フォルダに入らない API）、2 = フォルダの中。案内の行は 0。 */
  level: number;
  setSize: number;
  posInSet: number;
  /** 親のフォルダの行の番号。いちばん上は -1。 */
  parent: number;
}

/** キーボードで移れる行（案内の行を除く）か。 */
export const isNavigable = (row: TreeRow | undefined): boolean => row !== undefined && row.type !== "hint";

export function treeMeta(rows: TreeRow[]): TreeMeta[] {
  const meta: TreeMeta[] = rows.map(() => ({ level: 0, setSize: 0, posInSet: 0, parent: -1 }));
  const groups = new Map<number, number[]>();
  const add = (parent: number, index: number) => {
    const list = groups.get(parent);
    if (list) list.push(index);
    else groups.set(parent, [index]);
  };
  let folder = -1;
  rows.forEach((row, i) => {
    const m = meta[i];
    if (!m || row.type === "hint") return;
    if (row.type === "folder") {
      folder = i;
      m.level = 1;
      add(-1, i);
      return;
    }
    // フォルダが 1 つもないとき、API は先頭からフォルダに入らずに並ぶ
    m.level = folder === -1 ? 1 : 2;
    m.parent = folder;
    add(folder, i);
  });
  for (const list of groups.values()) {
    list.forEach((i, n) => {
      const m = meta[i];
      if (m) {
        m.setSize = list.length;
        m.posInSet = n + 1;
      }
    });
  }
  return meta;
}
