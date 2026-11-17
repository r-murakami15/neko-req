import { useLayoutEffect, useRef, type InputHTMLAttributes } from "react";

import { splitVars } from "../lib/vars";

interface Props extends Omit<InputHTMLAttributes<HTMLInputElement>, "value" | "onChange" | "className"> {
  value: string;
  onChange: (value: string) => void;
  /** 展開できない変数の名前（赤で示す）。 */
  unresolved?: ReadonlySet<string>;
  className?: string;
}

/**
 * `{{変数}}` を紫で強調表示する 1 行の入力欄。入力欄の後ろに同じ文字を色付きで重ねて見せている
 * （入力欄そのものの文字は透明）。展開できない変数は赤の波線にする。
 */
export function VarInput({ value, onChange, unresolved, className = "", ...rest }: Props) {
  const mirror = useRef<HTMLSpanElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const sync = () => {
    if (mirror.current && input.current) mirror.current.scrollLeft = input.current.scrollLeft;
  };
  useLayoutEffect(sync, [value]);

  const invalid = unresolved !== undefined && unresolved.size > 0;
  return (
    <span className={`var-input ${className}`} data-invalid={invalid ? "true" : undefined}>
      <span className="var-mirror" ref={mirror} aria-hidden="true">
        <span className="var-mirror-inner">
          {splitVars(value).map((seg, i) =>
            seg.name === null ? (
              <span key={i}>{seg.text}</span>
            ) : (
              <span key={i} className={unresolved?.has(seg.name) ? "var-token var-token-bad" : "var-token"}>
                {seg.text}
              </span>
            ),
          )}
        </span>
      </span>
      <input
        {...rest}
        ref={input}
        className="var-field"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onScroll={sync}
        onKeyUp={sync}
        onClick={sync}
        onSelect={sync}
        aria-invalid={invalid || undefined}
        spellCheck={false}
        autoComplete="off"
      />
    </span>
  );
}
