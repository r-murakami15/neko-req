import { useEffect } from "react";

import { useBackend } from "../api";
import { formatBytes } from "../lib/format";
import { useUpdateStore } from "../store/update";
import { MarkdownView } from "./Markdown";

/**
 * 更新の知らせの内容と操作。新しい版・更新内容・「今すぐ更新」（入れ替える）・進捗・「再起動して更新を完了」。
 * 勝手に入れ替えず、ユーザーが選んだときだけ入れ替える。バナーと設定画面の両方で使う。
 */
export function UpdateView({ inline = false }: { inline?: boolean }) {
  const backend = useBackend();
  const { info, phase, progress, error } = useUpdateStore();
  const install = useUpdateStore((s) => s.install);
  const restart = useUpdateStore((s) => s.restart);
  const dismiss = useUpdateStore((s) => s.dismiss);
  if (!info) return null;

  const busy = phase === "downloading" || phase === "installing";
  const percent = progress?.total ? Math.min(100, Math.round((progress.downloaded / progress.total) * 100)) : null;

  return (
    <div className={`update${inline ? " is-inline" : ""}`} role="region" aria-label="アップデート">
      <div className="update-head">
        <span className="update-title">
          新しい版 <span className="mono">{info.version}</span> があります
        </span>
        <span className="muted-note">現在 {info.currentVersion}</span>
      </div>
      {info.notes && phase !== "installed" && phase !== "restarting" && (
        <details className="update-notes">
          <summary>更新内容</summary>
          <MarkdownView source={info.notes} />
        </details>
      )}

      {phase === "idle" || phase === "error" ? (
        <div className="update-actions">
          {phase === "error" && (
            <span className="form-error" role="alert">
              更新できませんでした: {error}
            </span>
          )}
          <button type="button" className="btn btn-primary btn-small" onClick={() => void install(backend)}>
            {phase === "error" ? "もう一度更新する" : "今すぐ更新"}
          </button>
          {!inline && (
            <button type="button" className="btn btn-ghost btn-small" onClick={dismiss}>
              あとで
            </button>
          )}
        </div>
      ) : busy ? (
        <div className="update-progress" role="status" aria-label="更新の進み具合">
          <span>{phase === "installing" ? "入れ替えています…" : percent === null ? "ダウンロードしています…" : `ダウンロードしています… ${percent}%`}</span>
          {progress?.total ? (
            <span className="muted-note">
              {formatBytes(progress.downloaded)} / {formatBytes(progress.total)}
            </span>
          ) : null}
          <progress value={percent ?? undefined} max={100} aria-label="ダウンロードの進み具合" />
        </div>
      ) : (
        <div className="update-actions">
          <span role="status">{phase === "restarting" ? "再起動しています…" : "新しい版に入れ替えました。再起動すると、新しい版になります。"}</span>
          <button type="button" className="btn btn-primary btn-small" disabled={phase === "restarting"} onClick={() => void restart(backend)}>
            再起動して更新を完了
          </button>
        </div>
      )}
    </div>
  );
}

/** ヘッダーの下に出す更新の知らせ。「あとで」で隠せる（設定画面からはまた入れ替えられる）。 */
export function UpdateBanner() {
  const info = useUpdateStore((s) => s.info);
  const dismissed = useUpdateStore((s) => s.dismissed);
  const phase = useUpdateStore((s) => s.phase);
  // 入れ替え中・済みのときは、「あとで」を押していても出す（再起動の案内を見失わないため）
  if (!info || (dismissed && phase === "idle")) return null;
  return (
    <div className="update-banner">
      <UpdateView />
    </div>
  );
}

/**
 * 更新の知らせの購読。起動時の取りこぼしは `update_pending` で取り出す。
 * アプリの上位で 1 度だけ使う。
 */
export function useUpdateNotifications() {
  const backend = useBackend();
  useEffect(() => {
    let alive = true;
    const unlisten: (() => void)[] = [];
    void backend.onUpdateAvailable((info) => useUpdateStore.getState().found(info)).then((f) => (alive ? unlisten.push(f) : f()));
    void backend.onUpdateProgress((p) => useUpdateStore.getState().progressed(p)).then((f) => (alive ? unlisten.push(f) : f()));
    void backend
      .updatePending()
      .then((info) => {
        if (alive && info) useUpdateStore.getState().found(info);
      })
      .catch(() => {
        /* 確認できなくても、画面には出さない */
      });
    return () => {
      alive = false;
      for (const f of unlisten) f();
    };
  }, [backend]);
}
