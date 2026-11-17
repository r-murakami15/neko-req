import type { ChangedEntry, DiffEntry, OpKey, SyncDiff } from "../../bindings";
import { methodColor } from "../../lib/spec";

export const diffIsEmpty = (d: SyncDiff) => d.added.length + d.changed.length + d.removed.length === 0;

const opKeyText = (k: OpKey) => (k.kind === "id" ? k.value : `${k.value.method} ${k.value.path}`);

function clip(text: string | null, max = 120): string {
  if (text === null) return "（なし）";
  const one = text.replace(/\s+/g, " ").trim();
  return one.length > max ? `${one.slice(0, max)}…` : one || "（空）";
}

function Head({ entry }: { entry: DiffEntry }) {
  return (
    <span className="diff-head">
      <span className="tree-method" style={{ color: methodColor(entry.method) }}>
        {entry.method}
      </span>
      <span className="diff-path mono">{entry.path}</span>
      <span className="diff-summary">{entry.summary ?? entry.operationId ?? ""}</span>
    </span>
  );
}

function ChangedRow({ c }: { c: ChangedEntry }) {
  return (
    <li className="diff-item">
      <Head entry={c.entry} />
      <ul className="diff-notes">
        {c.renamedFrom && (
          <li>
            operationId が変わりました: <span className="mono">{opKeyText(c.renamedFrom)}</span> → <span className="mono">{opKeyText(c.entry.opKey)}</span>（入力値は引き継ぎます）
          </li>
        )}
        {c.changes.map((ch, i) => (
          <li key={i}>
            <span className="diff-field">{FIELD_LABEL[ch.field]}</span>
            {ch.note}
            {ch.field === "summary" && (
              <span className="diff-before-after">
                「{clip(ch.before, 60)}」→「{clip(ch.after, 60)}」
              </span>
            )}
            {ch.field === "description" && (ch.before !== null || ch.after !== null) && (
              <details className="diff-details">
                <summary>前後を見る</summary>
                <div className="diff-pair">
                  <div>
                    <span className="diff-pair-label">前</span>
                    <pre>{clip(ch.before, 400)}</pre>
                  </div>
                  <div>
                    <span className="diff-pair-label">後</span>
                    <pre>{clip(ch.after, 400)}</pre>
                  </div>
                </div>
              </details>
            )}
          </li>
        ))}
        {c.removedParams.length > 0 && (
          <li>
            yaml から消えたパラメータ: <span className="mono">{c.removedParams.join("、")}</span>（入力値は残り、「yaml にない」と表示して送信から外します）
          </li>
        )}
      </ul>
    </li>
  );
}

const FIELD_LABEL = {
  summary: "概要: ",
  description: "説明: ",
  parameters: "パラメータ: ",
  requestBody: "ボディ: ",
  responses: "レスポンス: ",
  security: "認証: ",
  tags: "タグ: ",
  operationId: "operationId: ",
} as const;

/** 追加・変更・削除の一覧（同期の確認ダイアログと、取り込みの確認画面の「既存プロジェクトとの差分」で使う）。 */
export function DiffList({ diff }: { diff: SyncDiff }) {
  return (
    <div className="diff-list">
      {diff.added.length > 0 && (
        <section className="diff-section" aria-label="追加">
          <h3 className="diff-title diff-added">追加 <span className="diff-count">{diff.added.length}</span></h3>
          <ul>
            {diff.added.map((e) => (
              <li key={`a-${JSON.stringify(e.opKey)}`} className="diff-item">
                <Head entry={e} />
              </li>
            ))}
          </ul>
        </section>
      )}
      {diff.changed.length > 0 && (
        <section className="diff-section" aria-label="変更">
          <h3 className="diff-title diff-changed">変更 <span className="diff-count">{diff.changed.length}</span></h3>
          <ul>
            {diff.changed.map((c) => (
              <ChangedRow key={`c-${JSON.stringify(c.entry.opKey)}`} c={c} />
            ))}
          </ul>
        </section>
      )}
      {diff.removed.length > 0 && (
        <section className="diff-section" aria-label="削除">
          <h3 className="diff-title diff-removed">削除 <span className="diff-count">{diff.removed.length}</span></h3>
          <ul>
            {diff.removed.map((e) => (
              <li key={`r-${JSON.stringify(e.opKey)}`} className="diff-item">
                <Head entry={e} />
                <span className="diff-note">
                  {e.hasDraft ? "入力値があるので「削除された API」に残します" : "入力値がないので、ツリーから消えます"}
                </span>
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}
