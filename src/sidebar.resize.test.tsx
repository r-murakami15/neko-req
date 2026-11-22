import { fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { SIDEBAR_DEFAULT_WIDTH, useUiStore } from "./store/ui";
import { openOp, setupWithApi, sidebar } from "./test/openapi";

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

describe("レスポンスの幅", () => {
  // jsdom は幅を測れない（0）ので、リクエストとレスポンスを並べた領域（.panes）だけ 1000px とみなす
  const original = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientWidth");
  beforeEach(() => {
    Object.defineProperty(HTMLElement.prototype, "clientWidth", {
      configurable: true,
      get(this: HTMLElement) {
        return this.classList.contains("panes") ? 1000 : 0;
      },
    });
  });
  afterEach(() => {
    if (original) Object.defineProperty(HTMLElement.prototype, "clientWidth", original);
    else delete (HTMLElement.prototype as { clientWidth?: number }).clientWidth;
  });

  const respHandle = () => screen.getByRole("separator", { name: "レスポンスの幅" });
  const respWidth = () => Number(respHandle().getAttribute("aria-valuenow"));
  const panes = () => document.querySelector(".panes") as HTMLElement;

  async function openRequest() {
    const ctx = await setupWithApi();
    await openOp(ctx.user, "video-client", /取引先一覧取得/);
    await waitFor(() => expect(screen.getByRole("button", { name: "送信" })).toBeEnabled());
    return ctx;
  }

  it("既定は半々で、→ キーでレスポンスが 16px 狭く・← キーで広くなり、最小幅（280px）でとまる", async () => {
    await openRequest();
    expect(respHandle()).toHaveAttribute("aria-orientation", "vertical");
    expect(respHandle()).toHaveAttribute("aria-valuemin", "280");
    expect(respHandle()).toHaveAttribute("aria-valuemax", "720");
    expect(respWidth()).toBe(500);
    expect(panes().style.getPropertyValue("--response-ratio")).toBe("0.5");

    fireEvent.keyDown(respHandle(), { key: "ArrowRight" });
    expect(respWidth()).toBe(484);
    fireEvent.keyDown(respHandle(), { key: "ArrowLeft" });
    fireEvent.keyDown(respHandle(), { key: "ArrowLeft" });
    expect(respWidth()).toBe(516);
    expect(useUiStore.getState().responseRatio).toBeCloseTo(0.516, 4);
    expect(JSON.parse(localStorage.getItem("nekoreq-ui") ?? "{}").state.responseRatio).toBeCloseTo(0.516, 4);

    for (let i = 0; i < 40; i++) fireEvent.keyDown(respHandle(), { key: "ArrowRight" });
    expect(respWidth()).toBe(280);
    for (let i = 0; i < 80; i++) fireEvent.keyDown(respHandle(), { key: "ArrowLeft" });
    expect(respWidth()).toBe(720);
  });

  it("ドラッグで、右へ動かすとレスポンスが狭くなる。範囲に収まり、ダブルクリックで半々に戻る", async () => {
    await openRequest();
    fireEvent.pointerDown(respHandle(), { button: 0, clientX: 500, pointerId: 1 });
    expect(document.body.style.cursor).toBe("col-resize");
    fireEvent.pointerMove(respHandle(), { clientX: 600, pointerId: 1 });
    expect(respWidth()).toBe(400);
    fireEvent.pointerMove(respHandle(), { clientX: 5000, pointerId: 1 });
    expect(respWidth()).toBe(280);
    fireEvent.pointerMove(respHandle(), { clientX: -5000, pointerId: 1 });
    expect(respWidth()).toBe(720);
    fireEvent.pointerUp(respHandle(), { pointerId: 1 });
    expect(document.body.style.cursor).toBe("");

    fireEvent.doubleClick(respHandle());
    expect(respWidth()).toBe(500);
    expect(useUiStore.getState().responseRatio).toBe(0.5);
  });

  it("覚えた割合は、領域が狭くなっても両側が最小幅を割らないように収める", async () => {
    useUiStore.setState({ responseRatio: 0.5 });
    await openRequest();
    useUiStore.setState({ responseRatio: 0.95 });
    await waitFor(() => expect(respWidth()).toBe(720));
    useUiStore.setState({ responseRatio: 0.05 });
    await waitFor(() => expect(respWidth()).toBe(280));
  });

  it("レスポンスを閉じているときと、設計タブのときは、つまみを出さない", async () => {
    const { user } = await openRequest();
    expect(respHandle()).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "レスポンスを閉じる" }));
    expect(screen.queryByRole("separator", { name: "レスポンスの幅" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "レスポンスを表示" }));
    expect(respHandle()).toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "設計" }));
    expect(screen.queryByRole("separator", { name: "レスポンスの幅" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "パラメータ" }));
    expect(respHandle()).toBeInTheDocument();
  });
});
