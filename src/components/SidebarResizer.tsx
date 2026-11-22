import { useEffect, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";

import { clampSidebarWidth, SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MIN_WIDTH, sidebarMaxWidth, useUiStore } from "../store/ui";

/** キーで動かす 1 回ぶんの幅（px）。 */
const KEY_STEP = 16;

function useWindowWidth(): number {
  const [width, setWidth] = useState(() => window.innerWidth);
  useEffect(() => {
    const onResize = () => setWidth(window.innerWidth);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  return width;
}

/** 覚えている幅を、いまのウィンドウ幅に収めたもの。ウィンドウが狭まっても中央が潰れない。 */
export function useSidebarWidth() {
  const stored = useUiStore((s) => s.sidebarWidth);
  const windowWidth = useWindowWidth();
  return { width: clampSidebarWidth(stored, windowWidth), max: sidebarMaxWidth(windowWidth), windowWidth };
}

/** サイドバーと右側の境目にある、幅を変えるためのつまみ（ドラッグ・←→ キー・ダブルクリックで既定幅）。 */
export function SidebarResizer() {
  const setSidebarWidth = useUiStore((s) => s.setSidebarWidth);
  const { width, max, windowWidth } = useSidebarWidth();
  const [dragging, setDragging] = useState(false);
  const drag = useRef<{ startX: number; startWidth: number } | null>(null);
  const bodyStyle = useRef<{ userSelect: string; cursor: string } | null>(null);

  const releaseBody = () => {
    if (!bodyStyle.current) return;
    document.body.style.userSelect = bodyStyle.current.userSelect;
    document.body.style.cursor = bodyStyle.current.cursor;
    bodyStyle.current = null;
  };
  // ドラッグ中にアンマウントされても、body に付けた指定を残さない
  useEffect(() => releaseBody, []);

  const set = (value: number) => setSidebarWidth(clampSidebarWidth(value, windowWidth));

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    drag.current = { startX: e.clientX, startWidth: width };
    setDragging(true);
    bodyStyle.current = { userSelect: document.body.style.userSelect, cursor: document.body.style.cursor };
    document.body.style.userSelect = "none";
    document.body.style.cursor = "col-resize";
    e.currentTarget.setPointerCapture?.(e.pointerId);
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    if (!drag.current) return;
    set(drag.current.startWidth + e.clientX - drag.current.startX);
  };
  const endDrag = (e: PointerEvent<HTMLDivElement>) => {
    if (!drag.current) return;
    drag.current = null;
    setDragging(false);
    releaseBody();
    if (e.currentTarget.hasPointerCapture?.(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "ArrowLeft") set(width - KEY_STEP);
    else if (e.key === "ArrowRight") set(width + KEY_STEP);
    else return;
    e.preventDefault();
  };

  return (
    <div
      className={`sidebar-resizer${dragging ? " is-dragging" : ""}`}
      role="separator"
      aria-orientation="vertical"
      aria-label="サイドバーの幅"
      aria-valuenow={width}
      aria-valuemin={SIDEBAR_MIN_WIDTH}
      aria-valuemax={max}
      tabIndex={0}
      title="ドラッグで幅を変更（ダブルクリックで元に戻す）"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onKeyDown={onKeyDown}
      onDoubleClick={() => set(SIDEBAR_DEFAULT_WIDTH)}
    />
  );
}
