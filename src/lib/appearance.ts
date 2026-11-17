import type { AppearanceSettings, Theme } from "../bindings";

const THEME_CACHE_KEY = "nekoreq-theme";

/** テーマを `<html data-theme>` に反映する。「OS に合わせる」は属性を外し、CSS の `prefers-color-scheme` に任せる。 */
export function applyTheme(theme: Theme, root: HTMLElement = document.documentElement) {
  if (theme === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", theme);
  try {
    // 次の起動で、設定を読み込む前の最初の描画から同じテーマにするため（ちらつき防止）
    localStorage.setItem(THEME_CACHE_KEY, theme);
  } catch {
    /* 保存できなくても動く */
  }
}

/** 前回のテーマを、設定を読み込む前に反映する（最初の描画が違う色にならないように）。 */
export function applyCachedTheme(root: HTMLElement = document.documentElement) {
  try {
    const cached = localStorage.getItem(THEME_CACHE_KEY);
    if (cached === "dark" || cached === "light") root.setAttribute("data-theme", cached);
  } catch {
    /* 読めなければ、OS の設定に従う */
  }
}

/** エディタの文字サイズ（CSS 変数 `--editor-font-size`）。 */
export function applyEditorFontSize(px: number, root: HTMLElement = document.documentElement) {
  root.style.setProperty("--editor-font-size", `${px}px`);
}

export function applyAppearance(a: AppearanceSettings, root: HTMLElement = document.documentElement) {
  applyTheme(a.theme, root);
  applyEditorFontSize(a.editorFontSize, root);
}
