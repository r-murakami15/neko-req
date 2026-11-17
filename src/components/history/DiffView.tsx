import type { DiffLine } from "../../bindings";

const MARK: Record<DiffLine["tag"], string> = { equal: " ", insert: "+", delete: "-", skip: "⋯" };

/** 行単位の差分。左の番号は左側、右の番号は右側の行番号。変わらない行が続くところは「N 行省略」。 */
export function DiffView({ lines, label }: { lines: DiffLine[]; label: string }) {
  return (
    <div className="diff" role="table" aria-label={label}>
      {lines.map((l, i) => (
        <div key={i} role="row" className={`diff-line is-${l.tag}`}>
          <span role="cell" className="diff-no" aria-hidden="true">
            {l.leftLine ?? ""}
          </span>
          <span role="cell" className="diff-no" aria-hidden="true">
            {l.rightLine ?? ""}
          </span>
          <span role="cell" className="diff-mark" aria-label={l.tag === "insert" ? "追加" : l.tag === "delete" ? "削除" : undefined}>
            {MARK[l.tag]}
          </span>
          <span role="cell" className="diff-text">
            {l.text}
          </span>
        </div>
      ))}
    </div>
  );
}
