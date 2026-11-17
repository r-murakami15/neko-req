import type { RequestSpec } from "../bindings";
import { newKeyValue, replaceRows, rowsOf } from "../lib/spec";
import { type UnresolvedIndex, pathParamNames } from "../lib/vars";
import { KeyValueTable } from "./KeyValueTable";

interface Props {
  spec: RequestSpec;
  onChange: (update: (spec: RequestSpec) => RequestSpec) => void;
  unresolved: UnresolvedIndex;
}

/** パラメータのタブ（クエリ・パス・Cookie）。 */
export function ParamsPanel({ spec, onChange, unresolved }: Props) {
  const path = rowsOf(spec.params, "path");
  const missingRows = pathParamNames(spec.url).filter((n) => !path.some((r) => r.name === n));

  return (
    <div className="panel">
      <section className="kv-section" aria-label="クエリパラメータ">
        <div className="section-title">クエリパラメータ</div>
        <KeyValueTable
          label="クエリパラメータ"
          location="query"
          items={rowsOf(spec.params, "query")}
          onChange={(next) => onChange((s) => ({ ...s, params: replaceRows(s.params, "query", next) }))}
          unresolved={(row, part) => unresolved.param("query", row, part)}
        />
      </section>

      <section className="kv-section" aria-label="パスパラメータ">
        <div className="section-title">パスパラメータ</div>
        {missingRows.length > 0 && (
          <div className="hint">
            <span>
              URL に {missingRows.map((n) => `{${n}}`).join("、")} があります。値を入れる行を作ります。
            </span>
            <button
              type="button"
              className="btn btn-small"
              onClick={() =>
                onChange((s) => ({
                  ...s,
                  params: replaceRows(s.params, "path", [
                    ...rowsOf(s.params, "path"),
                    ...missingRows.map((name) => ({ ...newKeyValue("path"), name })),
                  ]),
                }))
              }
            >
              行を追加
            </button>
          </div>
        )}
        <KeyValueTable
          label="パスパラメータ"
          location="path"
          items={path}
          onChange={(next) => onChange((s) => ({ ...s, params: replaceRows(s.params, "path", next) }))}
          unresolved={(row, part) => unresolved.param("path", row, part)}
        />
      </section>

      <section className="kv-section" aria-label="Cookie">
        <div className="section-title">Cookie</div>
        <KeyValueTable
          label="Cookie"
          location="cookie"
          items={rowsOf(spec.params, "cookie")}
          onChange={(next) => onChange((s) => ({ ...s, params: replaceRows(s.params, "cookie", next) }))}
          unresolved={(row, part) => unresolved.param("cookie", row, part)}
        />
      </section>
    </div>
  );
}
