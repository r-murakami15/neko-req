import type { KeyValue, ParamIn, RowPart } from "../bindings";
import { newKeyValue } from "../lib/spec";
import { CloseIcon } from "./Icons";
import { VarInput } from "./VarInput";

interface Props {
  /** 画面の言葉での表の名前（「クエリパラメータ」など）。操作の名前に使う。 */
  label: string;
  items: KeyValue[];
  onChange: (items: KeyValue[]) => void;
  /** 追加する行の `location`。 */
  location: ParamIn;
  /** 行（1 始まり）の、展開できない変数。 */
  unresolved: (row: number, part: RowPart) => ReadonlySet<string>;
}

/** 有効チェック・キー・値の表（パラメータ・ヘッダー・フォーム）。 */
export function KeyValueTable({ label, items, onChange, location, unresolved }: Props) {
  const update = (index: number, patch: Partial<KeyValue>) =>
    onChange(items.map((it, i) => (i === index ? { ...it, ...patch } : it)));

  return (
    <div className="kv-wrap">
      <table className="kv-table">
        <thead>
          <tr>
            <th className="kv-check">
              <span className="sr-only">有効</span>
            </th>
            <th>キー</th>
            <th>値</th>
            <th className="kv-remove">
              <span className="sr-only">削除</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {items.map((item, i) => (
            <tr key={i} className={item.enabled ? undefined : "kv-off"}>
              <td className="kv-check">
                <input
                  type="checkbox"
                  checked={item.enabled}
                  onChange={(e) => update(i, { enabled: e.target.checked })}
                  aria-label={`${label} ${i + 1} 行目を有効にする`}
                />
              </td>
              <td>
                <VarInput
                  className="kv-input"
                  value={item.name}
                  onChange={(name) => update(i, { name })}
                  unresolved={unresolved(i + 1, "name")}
                  aria-label={`${label} ${i + 1} 行目のキー`}
                  placeholder="キー"
                />
              </td>
              <td>
                <VarInput
                  className="kv-input"
                  value={item.value}
                  onChange={(value) => update(i, { value })}
                  unresolved={unresolved(i + 1, "value")}
                  aria-label={`${label} ${i + 1} 行目の値`}
                  placeholder="値"
                />
              </td>
              <td className="kv-remove">
                <button
                  type="button"
                  className="icon-btn"
                  aria-label={`${label} ${i + 1} 行目を削除`}
                  onClick={() => onChange(items.filter((_, j) => j !== i))}
                >
                  <CloseIcon />
                </button>
              </td>
            </tr>
          ))}
          <tr className="kv-add-row">
            <td />
            <td colSpan={3}>
              <button type="button" className="kv-add" aria-label={`${label}を追加`} onClick={() => onChange([...items, newKeyValue(location)])}>
                ＋ キーを追加
              </button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  );
}
