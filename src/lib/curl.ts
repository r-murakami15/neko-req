import { errorMessage } from "../api";
import type { Backend } from "../api/backend";
import type { RequestInput } from "../bindings";
import { useUiStore } from "../store/ui";
import type { RequestTab } from "../store/tabs";
import { copyText } from "./clipboard";
import { specForSend } from "./spec";

/**
 * タブの入力から、送信・プレビュー・cURL に渡す `RequestInput` を作る（前面のタブでなくても作れる）。
 * yaml にない（`missingInYaml`）パラメータは有効にしてあっても外す。環境は選んでいる環境（`environmentId: null`）。
 */
export function requestInputOf(tab: RequestTab): RequestInput {
  return {
    projectId: tab.projectId,
    environmentId: null,
    request: specForSend(tab.spec, tab.op?.missingInYaml ?? []),
    overrides: [],
    opKey: tab.op?.opKey ?? null,
  };
}

/**
 * タブの入力を cURL にしてコピーし、結果を `curlNotice`（そのタブの画面の上に出る知らせ）に入れる。
 * 既定は秘密を伏せる。`raw` は秘密の値がそのまま入る。コピーした文字列は、クリップボード以外には残さない。
 */
export async function copyCurlOfTab(backend: Backend, tab: RequestTab, mode: "redacted" | "raw"): Promise<void> {
  const ui = useUiStore.getState();
  ui.setCurlNotice(null);
  try {
    const text = await backend.curlExport(requestInputOf(tab), mode);
    const ok = await copyText(text);
    ui.setCurlNotice(
      ok
        ? { tabId: tab.id, ok: true, text: mode === "raw" ? "cURL をコピーしました（秘密の値を含みます）" : "cURL をコピーしました（秘密の値は伏せています）" }
        : { tabId: tab.id, ok: false, text: "クリップボードにコピーできませんでした" },
    );
  } catch (e) {
    ui.setCurlNotice({ tabId: tab.id, ok: false, text: `cURL にできませんでした: ${errorMessage(e)}` });
  }
}
