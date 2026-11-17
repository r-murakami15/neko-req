import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

afterEach(() => {
  cleanup();
});

// jsdom には CodeMirror が使う描画まわりの API がないので、空の値を返すものを足す
if (typeof Range !== "undefined") {
  const emptyRects = () => ({ length: 0, item: () => null, [Symbol.iterator]: function* () {} }) as unknown as DOMRectList;
  Range.prototype.getClientRects ??= emptyRects;
  Range.prototype.getBoundingClientRect ??= () => new DOMRect(0, 0, 0, 0);
}
if (typeof Element !== "undefined") {
  Element.prototype.scrollIntoView ??= () => {};
}

// jsdom にはレイアウトがないので、仮想スクロール（API ツリー）の入れ物の高さだけ決めておく
if (typeof HTMLElement !== "undefined") {
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get(this: HTMLElement) {
      return this.classList?.contains("tree-scroll") || this.classList?.contains("history-scroll") ? 560 : 0;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "clientWidth", {
    configurable: true,
    get(this: HTMLElement) {
      return this.classList?.contains("tree-scroll") || this.classList?.contains("history-scroll") ? 280 : 0;
    },
  });
}
