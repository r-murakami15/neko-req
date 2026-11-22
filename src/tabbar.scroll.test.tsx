import { screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { setupWithApi } from "./test/openapi";

// jsdom にはレイアウトがないので、タブ 1 枚 = 100px、タブ列の見える幅 = 250px として、寸法をスタブする
const TAB_W = 100;
const VIEW_W = 250;
const restore: Array<() => void> = [];

function stub(proto: object, prop: string, get: (this: HTMLElement) => number) {
  const old = Object.getOwnPropertyDescriptor(proto, prop);
  Object.defineProperty(proto, prop, { configurable: true, get });
  restore.push(() => (old ? Object.defineProperty(proto, prop, old) : delete (proto as Record<string, unknown>)[prop]));
}

beforeEach(() => {
  const isTab = (el: HTMLElement) => el.classList.contains("tab");
  const isList = (el: HTMLElement) => el.classList.contains("tablist");
  stub(HTMLElement.prototype, "offsetWidth", function () {
    return isTab(this) ? TAB_W : 0;
  });
  stub(HTMLElement.prototype, "offsetLeft", function () {
    return isTab(this) ? Array.from(this.parentElement!.children).indexOf(this) * TAB_W : 0;
  });
  stub(HTMLElement.prototype, "clientWidth", function () {
    return isList(this) ? VIEW_W : 0;
  });
  stub(HTMLElement.prototype, "scrollWidth", function () {
    return isList(this) ? this.querySelectorAll(".tab").length * TAB_W : 0;
  });
});
afterEach(() => restore.splice(0).forEach((r) => r()));

const list = () => screen.getByRole("tablist", { name: "開いているリクエスト" });

async function openTabs(count: number) {
  const ctx = await setupWithApi();
  for (let i = 0; i < count; i += 1) {
    await ctx.user.click(screen.getByRole("button", { name: "新しいタブ" }));
  }
  return ctx;
}

describe("タブ列の横スクロール", () => {
  it("＋はタブ列（tablist）の外に置く。タブ列はスクロールするが、＋は動かない", async () => {
    await openTabs(1);
    expect(list()).not.toContainElement(screen.getByRole("button", { name: "新しいタブ" }));
    expect(list().parentElement).toContainElement(screen.getByRole("button", { name: "新しいタブ" }));
  });

  it("前面のタブが変わると、そのタブが見える位置まで横にずれる", async () => {
    const { user } = await openTabs(5);
    // 5 枚目（右端: 400〜500）が前面。見える幅 250 なので scrollLeft は 250
    expect(list().scrollLeft).toBe(500 - VIEW_W);

    // 1 枚目を前面にすると左端に戻る
    await user.click(screen.getAllByRole("tab")[0]);
    expect(list().scrollLeft).toBe(0);

    // すでに見えているタブを前面にしてもずれない
    await user.click(screen.getAllByRole("tab")[1]);
    expect(list().scrollLeft).toBe(0);
  });

  it("縦のホイール回転は横スクロールに変える。余地がないときと、端で外に向かうときは変えない", async () => {
    await openTabs(5);
    list().scrollLeft = 100;
    const wheel = (init: WheelEventInit) => {
      const ev = new WheelEvent("wheel", { bubbles: true, cancelable: true, ...init });
      list().dispatchEvent(ev);
      return ev;
    };

    const down = wheel({ deltaY: 40 });
    expect(down.defaultPrevented).toBe(true);
    expect(list().scrollLeft).toBe(140);

    // 横成分があるもの（トラックパッドの横スワイプ）はブラウザに任せる
    const swipe = wheel({ deltaX: 30, deltaY: 2 });
    expect(swipe.defaultPrevented).toBe(false);
    expect(list().scrollLeft).toBe(140);

    // 端まで来たら、それより先には変えない
    list().scrollLeft = 250;
    expect(wheel({ deltaY: 40 }).defaultPrevented).toBe(false);
    list().scrollLeft = 0;
    expect(wheel({ deltaY: -40 }).defaultPrevented).toBe(false);
  });

  it("入りきっているときは、縦ホイールを変換しない", async () => {
    await openTabs(1);
    const ev = new WheelEvent("wheel", { bubbles: true, cancelable: true, deltaY: 40 });
    list().dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(false);
    expect(list().scrollLeft).toBe(0);
  });
});
