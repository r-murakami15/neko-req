import { useState } from "react";

import type { AutoHeader, KeyValue, RequestSpec } from "../bindings";
import { type ParamMeta, replaceRows, rowInfoFor, rowsOf } from "../lib/spec";
import type { UnresolvedIndex } from "../lib/vars";
import { KeyValueTable } from "./KeyValueTable";

interface Props {
  spec: RequestSpec;
  onChange: (update: (spec: RequestSpec) => RequestSpec) => void;
  unresolved: UnresolvedIndex;
  autoHeaders: AutoHeader[];
  /** yaml の API のとき: 説明・必須・「yaml にない」の表示（表示専用）。 */
  meta?: ParamMeta;
}

/** ヘッダーのタブ。手入力の表と、アプリが自動で付けるヘッダー。 */
export function HeadersPanel({ spec, onChange, unresolved, autoHeaders, meta }: Props) {
  const [showAuto, setShowAuto] = useState(true);

  return (
    <div className="panel">
      <section className="kv-section" aria-label="ヘッダー">
        <div className="section-title">ヘッダー</div>
        <KeyValueTable
          label="ヘッダー"
          location="header"
          items={rowsOf(spec.params, "header")}
          onChange={(next) => onChange((s) => ({ ...s, params: replaceRows(s.params, "header", next) }))}
          unresolved={(row, part) => unresolved.param("header", row, part)}
          info={meta ? (item: KeyValue) => rowInfoFor(meta, item) : undefined}
        />
      </section>

      <section className="auto-headers" aria-label="自動で付与されるヘッダー">
        <label className="check-label">
          <input type="checkbox" checked={showAuto} onChange={(e) => setShowAuto(e.target.checked)} />
          自動で付与されるヘッダーを表示
        </label>
        {showAuto && (
          <div className="auto-box">
            {autoHeaders.length === 0 ? (
              <div className="auto-empty">自動で付与されるヘッダーはありません</div>
            ) : (
              autoHeaders.map((h, i) => (
                <div key={`${h.name}-${i}`} className="auto-row">
                  <span className="mono">{h.name}</span>
                  <span className="mono auto-value">{h.value}</span>
                  <span className="auto-source">{h.source}</span>
                </div>
              ))
            )}
          </div>
        )}
      </section>
    </div>
  );
}
