import { useEffect, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";

/** キーで動かす 1 回ぶんの幅（px）。 */
export const RESIZER_KEY_STEP = 16;

interface ResizerProps {
  /** 位置などを決める追加の class（共通の見た目は `resizer`）。 */
  className?: string;
  label: string;
  /** 動かす対象の、いまの幅（px）。 */
  value: number;
  min: number;
  max: number;
  /** つまみを右へ動かしたとき、`value` が増える向きなら 1、減る向きなら -1。 */
  direction: 1 | -1;
  /** 動かした先の幅（px。範囲に収める前の値）。呼び出し側で min〜max に収めて保存する。 */
  onChange: (value: number) => void;
  /** ダブルクリックで、既定の幅に戻す。 */
  onReset: () => void;
}

/**
 * 2 つの領域の境目にある、縦のつまみ（ドラッグ・←→ キー・ダブルクリックで既定に戻す）。
 * 値は「動かす対象の幅（px）」で扱い、つまみを右へ動かしたとき増えるか減るかは `direction` で決める。
 */
export function Resizer({ className, label, value, min, max, direction, onChange, onReset }: ResizerProps) {
  const [dragging, setDragging] = useState(false);
  const drag = useRef<{ startX: number; startValue: number } | null>(null);
  const bodyStyle = useRef<{ userSelect: string; cursor: string } | null>(null);

  const releaseBody = () => {
    if (!bodyStyle.current) return;
    document.body.style.userSelect = bodyStyle.current.userSelect;
    document.body.style.cursor = bodyStyle.current.cursor;
    bodyStyle.current = null;
  };
  // ドラッグ中にアンマウントされても、body に付けた指定を残さない
  useEffect(() => releaseBody, []);

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    drag.current = { startX: e.clientX, startValue: value };
    setDragging(true);
    bodyStyle.current = { userSelect: document.body.style.userSelect, cursor: document.body.style.cursor };
    document.body.style.userSelect = "none";
    document.body.style.cursor = "col-resize";
    e.currentTarget.setPointerCapture?.(e.pointerId);
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    if (!drag.current) return;
    onChange(drag.current.startValue + direction * (e.clientX - drag.current.startX));
  };
  const endDrag = (e: PointerEvent<HTMLDivElement>) => {
    if (!drag.current) return;
    drag.current = null;
    setDragging(false);
    releaseBody();
    if (e.currentTarget.hasPointerCapture?.(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    // つまみが右へ動く向きのキー（→）は、direction が 1 なら増やす・-1 なら減らす
    if (e.key === "ArrowLeft") onChange(value - direction * RESIZER_KEY_STEP);
    else if (e.key === "ArrowRight") onChange(value + direction * RESIZER_KEY_STEP);
    else return;
    e.preventDefault();
  };

  return (
    <div
      className={`resizer${className ? ` ${className}` : ""}${dragging ? " is-dragging" : ""}`}
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      aria-valuenow={value}
      aria-valuemin={min}
      aria-valuemax={max}
      tabIndex={0}
      title="ドラッグで幅を変更（ダブルクリックで元に戻す）"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onKeyDown={onKeyDown}
      onDoubleClick={onReset}
    />
  );
}
