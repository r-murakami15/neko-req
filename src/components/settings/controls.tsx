import { type ReactNode, useEffect, useId, useState } from "react";

/** 設定の 1 行（左にラベル、右に入力）。 */
export function Row({ label, htmlFor, children, hint }: { label: ReactNode; htmlFor?: string; children: ReactNode; hint?: ReactNode }) {
  return (
    <>
      {htmlFor ? (
        <label htmlFor={htmlFor} className="settings-label">
          {label}
        </label>
      ) : (
        <span className="settings-label">{label}</span>
      )}
      <div className="settings-control">
        {children}
        {hint && <span className="muted-note settings-hint">{hint}</span>}
      </div>
    </>
  );
}

/** オン・オフのスイッチ（チェックボックス）。 */
export function Switch({ label, checked, onChange, disabled }: { label: string; checked: boolean; onChange: (checked: boolean) => void; disabled?: boolean }) {
  const id = useId();
  return (
    <Row label={label} htmlFor={id}>
      <input id={id} type="checkbox" role="switch" className="settings-switch" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
    </Row>
  );
}

/** 選択肢（セレクトボックス）。 */
export function Select<T extends string | number>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: T;
  options: { value: T; label: string }[];
  onChange: (value: T) => void;
}) {
  const id = useId();
  return (
    <Row label={label} htmlFor={id}>
      <select
        id={id}
        className="select select-tall settings-select"
        value={String(value)}
        onChange={(e) => {
          const picked = options.find((o) => String(o.value) === e.target.value);
          if (picked) onChange(picked.value);
        }}
      >
        {options.map((o) => (
          <option key={String(o.value)} value={String(o.value)}>
            {o.label}
          </option>
        ))}
      </select>
    </Row>
  );
}

/**
 * 文字を入力する欄。入力の途中では保存せず、欄を離れたとき（Enter でも）に `onCommit` を呼ぶ。
 * `validate` が文言を返したら保存せず、その場に出す。
 */
export function CommitText({
  label,
  value,
  onCommit,
  validate,
  placeholder,
  width,
  mono,
  type = "text",
  suffix,
  ariaLabel,
  hint,
}: {
  label: string;
  value: string;
  onCommit: (value: string) => void;
  validate?: (value: string) => string | null;
  placeholder?: string;
  width?: number;
  mono?: boolean;
  type?: "text" | "password";
  suffix?: ReactNode;
  ariaLabel?: string;
  hint?: ReactNode;
}) {
  const id = useId();
  const [text, setText] = useState(value);
  const [error, setError] = useState<string | null>(null);
  // 保存された値が変わったら（検証で整えられた・別の操作で変わった）入力欄も合わせる
  useEffect(() => {
    setText(value);
    setError(null);
  }, [value]);

  const commit = () => {
    if (text === value) {
      setError(null);
      return;
    }
    const problem = validate?.(text) ?? null;
    setError(problem);
    if (problem === null) onCommit(text);
  };

  return (
    <Row label={label} htmlFor={id} hint={hint}>
      <span className="settings-inline">
        <input
          id={id}
          className={`input input-small${mono ? " mono" : ""}`}
          style={width ? { width } : undefined}
          type={type}
          value={text}
          placeholder={placeholder}
          aria-label={ariaLabel}
          aria-invalid={error !== null}
          autoComplete="off"
          spellCheck={false}
          onChange={(e) => setText(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              commit();
            }
          }}
        />
        {suffix}
      </span>
      {error && (
        <span className="form-error" role="alert">
          {error}
        </span>
      )}
    </Row>
  );
}
