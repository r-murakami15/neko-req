import type { HistoryDiff, HistoryListItem } from "../../bindings";
import { formatMs } from "../../lib/format";
import { shortUrl, statusText, statusTone, timeOf } from "../../lib/history";
import { methodColor } from "../../lib/spec";
import { DiffView } from "./DiffView";

function Side({ label, item, status }: { label: string; item: HistoryListItem; status: string }) {
  return (
    <div className="compare-side" role="group" aria-label={label}>
      <span className="history-col-title">{label}</span>
      <div className="compare-line">
        <span className="history-method" style={{ color: methodColor(item.method) }}>
          {item.method}
        </span>
        <span className="mono compare-url">{shortUrl(item.url)}</span>
      </div>
      <div className="compare-line">
        <span className={`history-badge is-${statusTone(item)}`}>{status || statusText(item)}</span>
        <span className="muted-note">
          {item.environmentName ?? "環境なし"} · {item.localDate} {timeOf(item.sentAt)}
          {item.totalMs !== null ? ` · ${formatMs(item.totalMs)}` : ""}
        </span>
      </div>
    </div>
  );
}

/** 2 件のレスポンスの差分（ステータス・ヘッダー・ボディ）。 */
export function HistoryCompare({ diff, onClose }: { diff: HistoryDiff; onClose: () => void }) {
  const same = !diff.statusChanged && !diff.headersChanged && !diff.bodyChanged;
  return (
    <div className="history-detail" role="region" aria-label="履歴の比較">
      <div className="history-detail-head">
        <h2 className="history-title">履歴の比較</h2>
        <button type="button" className="btn btn-ghost history-delete" onClick={onClose}>
          比較をやめる
        </button>
      </div>
      <div className="compare-sides">
        <Side label="左（最初に選んだ履歴）" item={diff.left} status={diff.leftStatus} />
        <Side label="右（比べる履歴）" item={diff.right} status={diff.rightStatus} />
      </div>
      {same && (
        <div className="notice notice-ok" role="status">
          ステータス・ヘッダー・ボディに違いはありません。
        </div>
      )}

      <section className="compare-section" aria-label="ステータスの比較">
        <h3 className="compare-h">ステータス</h3>
        {diff.statusChanged ? (
          <p className="compare-status">
            <span className="mono">{diff.leftStatus}</span> → <span className="mono">{diff.rightStatus}</span>
          </p>
        ) : (
          <p className="muted-note">同じです（{diff.leftStatus}）</p>
        )}
      </section>

      <section className="compare-section" aria-label="ヘッダーの比較">
        <h3 className="compare-h">ヘッダー</h3>
        {diff.headersChanged ? <DiffView lines={diff.headers} label="ヘッダーの差分" /> : <p className="muted-note">違いはありません</p>}
      </section>

      <section className="compare-section" aria-label="ボディの比較">
        <h3 className="compare-h">ボディ</h3>
        {diff.bodyNote && <p className="muted-note">{diff.bodyNote}</p>}
        {diff.bodyChanged ? <DiffView lines={diff.body} label="ボディの差分" /> : <p className="muted-note">違いはありません</p>}
      </section>
    </div>
  );
}
