/** macOS か（キーの表記とショートカットの修飾キーを変える）。 */
export function isMac(): boolean {
  if (typeof navigator === "undefined") return false;
  const nav = navigator as Navigator & { userAgentData?: { platform?: string } };
  const text = `${nav.userAgentData?.platform ?? ""} ${nav.platform ?? ""}`;
  return /mac|iphone|ipad/i.test(text) || (text.trim() === "" && /Mac OS X/i.test(nav.userAgent ?? ""));
}

export interface Shortcut {
  id: "send" | "save" | "newTab" | "closeTab" | "search" | "settings" | "toggleResponse";
  label: string;
  /** 修飾キーを除いたキー（`event.key` と比べる。小文字）。 */
  key: string;
  /** 表示用のキー。 */
  keyLabel: string;
}

/** 設定画面に出すショートカットの一覧（実際に効くもの）。 */
export const SHORTCUTS: Shortcut[] = [
  { id: "send", label: "リクエストを送信", key: "enter", keyLabel: "Enter" },
  { id: "save", label: "保存", key: "s", keyLabel: "S" },
  { id: "newTab", label: "空のリクエストを新しいタブで開く", key: "t", keyLabel: "T" },
  { id: "closeTab", label: "タブを閉じる", key: "w", keyLabel: "W" },
  { id: "search", label: "リクエストを検索", key: "k", keyLabel: "K" },
  { id: "settings", label: "設定を開く", key: ",", keyLabel: "," },
  { id: "toggleResponse", label: "レスポンス欄を開閉", key: "r", keyLabel: "Shift R" },
];

/** `⌘ Enter` / `Ctrl Enter` の形。 */
export function shortcutText(s: Shortcut, mac: boolean = isMac()): string {
  return `${mac ? "⌘" : "Ctrl"} ${s.keyLabel}`;
}
