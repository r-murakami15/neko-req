import type { HistoryListItem, StatusClass } from "../bindings";

export const STATUS_CLASSES: { id: StatusClass; label: string }[] = [
  { id: "2xx", label: "2xx" },
  { id: "3xx", label: "3xx" },
  { id: "4xx", label: "4xx" },
  { id: "5xx", label: "5xx" },
  { id: "failed", label: "失敗" },
];

const pad = (n: number) => String(n).padStart(2, "0");

/** `2026-11-17` をローカルの日付として `今日` / `昨日` / `2026/11/10` にする。 */
export function dayLabel(localDate: string, now: Date = new Date()): string {
  const key = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
  if (localDate === key(now)) return "今日";
  const yesterday = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1);
  if (localDate === key(yesterday)) return "昨日";
  return localDate.replace(/-/g, "/");
}

/** 送信時刻を、端末の時刻で `10:24` の形にする。 */
export function timeOf(sentAt: string): string {
  const d = new Date(sentAt);
  if (Number.isNaN(d.getTime())) return "";
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** 一覧に出す URL（スキームとホストを除いた path と query）。読めなければそのまま。 */
export function shortUrl(url: string): string {
  try {
    const u = new URL(url);
    return `${u.pathname}${u.search}`;
  } catch {
    return url;
  }
}

/** ステータスの表示（失敗は「失敗」）。 */
export function statusText(item: Pick<HistoryListItem, "status" | "failed">): string {
  return item.failed || item.status === null ? "失敗" : String(item.status);
}

/** ステータスの色分けのクラス名。 */
export function statusTone(item: Pick<HistoryListItem, "status" | "failed">): "ok" | "info" | "warn" | "danger" | "fail" {
  if (item.failed || item.status === null) return "fail";
  if (item.status >= 500) return "danger";
  if (item.status >= 400) return "warn";
  if (item.status >= 300) return "info";
  return "ok";
}

/** 日付の見出しと項目を平らな行にする（仮想スクロール用）。 */
export type HistoryRow = { kind: "day"; key: string; label: string } | { kind: "item"; key: string; item: HistoryListItem };

export function buildHistoryRows(items: HistoryListItem[], now: Date = new Date()): HistoryRow[] {
  const rows: HistoryRow[] = [];
  let day: string | null = null;
  for (const item of items) {
    if (item.localDate !== day) {
      day = item.localDate;
      rows.push({ kind: "day", key: `day:${day}`, label: dayLabel(day, now) });
    }
    rows.push({ kind: "item", key: item.id, item });
  }
  return rows;
}
