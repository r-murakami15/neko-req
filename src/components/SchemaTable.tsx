import { useMemo } from "react";

import type { ExampleSource, JsonValue, SchemaRow } from "../bindings";
import { tokenizeJson } from "../lib/json";

const CIRCULAR = "循環";

/** スキーマ表（名前・型・必須・制約・説明）。入れ子は字下げ、循環参照は「循環」と出して展開を止める。 */
export function SchemaTable({ rows, label }: { rows: SchemaRow[]; label: string }) {
  const hasExample = rows.some((r) => r.example !== null);
  return (
    <div className="schema-wrap">
      <table className="schema-table" aria-label={label}>
        <thead>
          <tr>
            <th>名前</th>
            <th>型</th>
            <th>必須</th>
            <th>制約</th>
            {hasExample && <th>例</th>}
            <th>説明</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r, i) => {
            const structural = r.baseType === "object" || r.baseType === "array";
            const constraints = r.constraints.filter((c) => c !== CIRCULAR);
            return (
              <tr key={i} className={r.circular ? "is-circular" : undefined}>
                <td className={`schema-name mono${r.depth === 0 ? " is-root" : ""}`} style={{ paddingLeft: 14 + r.depth * 22 }}>
                  {r.name === "" ? "(ルート)" : r.name}
                </td>
                <td className={`schema-type mono${structural ? " is-struct" : ""}`}>{r.typeLabel}</td>
                <td className="schema-required">{r.required ? "必須" : ""}</td>
                <td className="schema-constraints">
                  {r.circular && <span className="chip chip-circular">{CIRCULAR}</span>}
                  {constraints.map((c) => (
                    <span key={c} className="chip">
                      {c}
                    </span>
                  ))}
                </td>
                {hasExample && <td className="schema-example mono">{r.example ?? ""}</td>}
                <td className="schema-desc">{r.description ?? ""}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

const SOURCE_NOTE: Record<ExampleSource, string> = {
  yaml: "（yaml の example）",
  schemaExample: "（スキーマの example から生成）",
  generated: "（スキーマから自動生成したモック）",
};

/** JSON の例（色付き）。「スキーマから自動生成したモック」などの出どころを明記する。 */
export function ExampleBlock({
  value,
  source,
  onApply,
  heading = "例",
}: {
  value: JsonValue;
  source: ExampleSource | null;
  onApply?: () => void;
  heading?: string;
}) {
  const text = useMemo(() => JSON.stringify(value, null, 2), [value]);
  const tokens = useMemo(() => tokenizeJson(text), [text]);
  return (
    <div className="example-box">
      <div className="example-head">
        <span className="example-title">{heading}</span>
        {source && <span className="muted-note">{SOURCE_NOTE[source]}</span>}
        {onApply && (
          <button type="button" className="link-btn example-apply" onClick={onApply}>
            ボディに反映
          </button>
        )}
      </div>
      <pre className="example-pre" aria-label={`${heading}（JSON）`}>
        {tokens.map((t, i) => (t.kind === "plain" ? t.text : <span key={i} className={`tok-${t.kind}`}>{t.text}</span>))}
      </pre>
    </div>
  );
}
