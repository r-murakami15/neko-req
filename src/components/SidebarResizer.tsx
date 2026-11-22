import { useEffect, useState } from "react";

import { clampSidebarWidth, SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MIN_WIDTH, sidebarMaxWidth, useUiStore } from "../store/ui";
import { Resizer } from "./Resizer";

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
  return (
    <Resizer
      label="サイドバーの幅"
      value={width}
      min={SIDEBAR_MIN_WIDTH}
      max={max}
      direction={1}
      onChange={(value) => setSidebarWidth(clampSidebarWidth(value, windowWidth))}
      onReset={() => setSidebarWidth(clampSidebarWidth(SIDEBAR_DEFAULT_WIDTH, windowWidth))}
    />
  );
}
