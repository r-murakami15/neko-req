import { isTauri } from "@tauri-apps/api/core";

import { createMockBackend } from "../mock/backend";
import type { MockControls } from "../mock/openapiBackend";
import type { Backend } from "./backend";
import { createTauriBackend } from "./tauri";

export { ApiError, errorMessage, type Backend } from "./backend";
export { BackendProvider, useBackend } from "./context";

let backend: Backend | undefined;

/**
 * 既定のバックエンド。Tauri の中なら Rust を呼び、ブラウザで開いたとき（`npm run dev` の確認用）は
 * メモリ上のモックに切り替える。
 */
export function getDefaultBackend(): Backend {
  if (!backend) {
    if (isTauri()) {
      backend = createTauriBackend();
    } else {
      const mock = createMockBackend();
      // ブラウザで確認するとき、コンソールから yaml の更新などを再現できるようにする
      (window as unknown as { __nekoreqMock?: MockControls }).__nekoreqMock = mock.mock;
      backend = mock;
    }
  }
  return backend;
}
