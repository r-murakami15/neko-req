import { useLayoutEffect, useState, type RefObject } from "react";

import { clampResponseRatio, paneMinWidth, RESPONSE_DEFAULT_RATIO, useUiStore } from "../store/ui";
import { Resizer } from "./Resizer";

/** 要素の幅（px）を追う。まだ測れない間は 0。 */
function useElementWidth(ref: RefObject<HTMLElement | null>): number {
  const [width, setWidth] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const update = () => setWidth(el.clientWidth);
    update();
    if (typeof ResizeObserver !== "undefined") {
      const observer = new ResizeObserver(update);
      observer.observe(el);
      return () => observer.disconnect();
    }
    window.addEventListener("resize", update);
    return () => window.removeEventListener("resize", update);
  }, [ref]);
  return width;
}

/**
 * リクエストとレスポンスを並べた領域（`.panes`）の幅と、そこでのレスポンスの割合。
 * 覚えている割合を、いまの幅で両側が最小幅を割らない範囲に収めたもの（ウィンドウやサイドバーの幅が変わっても崩れない）。
 */
export function useResponseSplit(panesRef: RefObject<HTMLElement | null>) {
  const stored = useUiStore((s) => s.responseRatio);
  const total = useElementWidth(panesRef);
  return { total, ratio: clampResponseRatio(stored, total) };
}

/** リクエストとレスポンスの境目にある、レスポンスの幅を変えるためのつまみ。右へ動かすとレスポンスが狭くなる。 */
export function ResponseResizer({ total, ratio }: { total: number; ratio: number }) {
  const setResponseRatio = useUiStore((s) => s.setResponseRatio);
  const min = paneMinWidth(total);
  const set = (px: number) => {
    if (total > 0) setResponseRatio(clampResponseRatio(px / total, total));
  };
  return (
    <Resizer
      className="pane-resizer"
      label="レスポンスの幅"
      value={Math.round(ratio * total)}
      min={min}
      max={total - min}
      direction={-1}
      onChange={set}
      onReset={() => setResponseRatio(clampResponseRatio(RESPONSE_DEFAULT_RATIO, total))}
    />
  );
}
