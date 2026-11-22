import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { SIDEBAR_DEFAULT_WIDTH, useUiStore } from "./store/ui";
import { setupWithApi, sidebar } from "./test/openapi";

// jsdom に PointerEvent がなく、button・clientX が落ちるので、MouseEvent ベースで補う
if (typeof window.PointerEvent === "undefined") {
  class PointerEventPolyfill extends MouseEvent {
    pointerId: number;
    constructor(type: string, init: PointerEventInit = {}) {
      super(type, init);
      this.pointerId = init.pointerId ?? 0;
    }
  }
  Object.defineProperty(window, "PointerEvent", { value: PointerEventPolyfill, configurable: true });
}

const handle = () => screen.getByRole("separator", { name: "サイドバーの幅" });
const width = () => Number(handle().getAttribute("aria-valuenow"));

describe("サイドバーの幅", () => {
  it("既定幅で始まり、←→ キーで 16px ずつ変わり、最小・最大でとまる", async () => {
    await setupWithApi();
    expect(handle()).toHaveAttribute("aria-orientation", "vertical");
    expect(handle()).toHaveAttribute("aria-valuemin", "200");
    expect(width()).toBe(SIDEBAR_DEFAULT_WIDTH);
    expect(sidebar().style.getPropertyValue("--sidebar-w")).toBe("280px");

    fireEvent.keyDown(handle(), { key: "ArrowRight" });
    expect(width()).toBe(296);
    expect(sidebar().style.getPropertyValue("--sidebar-w")).toBe("296px");
    fireEvent.keyDown(handle(), { key: "ArrowLeft" });
    fireEvent.keyDown(handle(), { key: "ArrowLeft" });
    expect(width()).toBe(264);

    for (let i = 0; i < 20; i++) fireEvent.keyDown(handle(), { key: "ArrowLeft" });
    expect(width()).toBe(200);
    for (let i = 0; i < 60; i++) fireEvent.keyDown(handle(), { key: "ArrowRight" });
    const max = Number(handle().getAttribute("aria-valuemax"));
    expect(max).toBeLessThanOrEqual(600);
    expect(width()).toBe(max);
  });

  it("最大はウィンドウ幅 − 400px も超えない（jsdom は 1024px なので 600px が上限、狭ければそちら）", async () => {
    const original = window.innerWidth;
    window.innerWidth = 700;
    try {
      await setupWithApi();
      expect(handle()).toHaveAttribute("aria-valuemax", "300");
      for (let i = 0; i < 20; i++) fireEvent.keyDown(handle(), { key: "ArrowRight" });
      expect(width()).toBe(300);
    } finally {
      window.innerWidth = original;
    }
  });

  it("ドラッグで幅が変わり、終わると body の指定を戻す。ダブルクリックで既定幅に戻る", async () => {
    await setupWithApi();
    fireEvent.pointerDown(handle(), { button: 0, clientX: 280, pointerId: 1 });
    expect(document.body.style.cursor).toBe("col-resize");
    expect(document.body.style.userSelect).toBe("none");
    fireEvent.pointerMove(handle(), { clientX: 380, pointerId: 1 });
    expect(width()).toBe(380);
    fireEvent.pointerMove(handle(), { clientX: 5000, pointerId: 1 });
    expect(width()).toBe(Number(handle().getAttribute("aria-valuemax")));
    fireEvent.pointerMove(handle(), { clientX: -100, pointerId: 1 });
    expect(width()).toBe(200);
    fireEvent.pointerUp(handle(), { pointerId: 1 });
    expect(document.body.style.cursor).toBe("");
    expect(document.body.style.userSelect).toBe("");
    // 終わったあとの移動は無視する
    fireEvent.pointerMove(handle(), { clientX: 400, pointerId: 1 });
    expect(width()).toBe(200);

    expect(useUiStore.getState().sidebarWidth).toBe(200);
    fireEvent.doubleClick(handle());
    expect(width()).toBe(SIDEBAR_DEFAULT_WIDTH);
  });
});
