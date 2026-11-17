import { useState } from "react";

import { errorMessage, useBackend } from "../../api";
import { useAppInfo } from "../../api/queries";
import { isMac } from "../../lib/platform";
import { useUpdateStore } from "../../store/update";
import { UpdateView } from "../UpdateBanner";

type CheckState = { kind: "idle" } | { kind: "checking" } | { kind: "latest" } | { kind: "found" } | { kind: "error"; message: string };

/** 「このアプリについて」。版と「アップデートを確認」。 */
export function AboutSection() {
  const backend = useBackend();
  const appInfo = useAppInfo();
  const info = useUpdateStore((s) => s.info);
  const [state, setState] = useState<CheckState>({ kind: "idle" });

  const check = async () => {
    setState({ kind: "checking" });
    try {
      const found = await backend.updateCheck();
      if (found) {
        useUpdateStore.getState().found(found);
        setState({ kind: "found" });
      } else {
        setState({ kind: "latest" });
      }
    } catch (e) {
      setState({ kind: "error", message: errorMessage(e) });
    }
  };

  const version = appInfo.data?.version ?? "…";
  return (
    <div className="settings-about">
      <span className="settings-label">
        {appInfo.data?.name ?? "NekoReq"} バージョン {version} · {isMac() ? "macOS" : "Windows"}
      </span>
      <div className="settings-inline">
        <button type="button" className="btn btn-small" disabled={state.kind === "checking"} onClick={() => void check()}>
          アップデートを確認
        </button>
        {state.kind === "checking" && (
          <span className="muted-note" role="status">
            確認しています…
          </span>
        )}
        {state.kind === "latest" && (
          <span className="settings-ok" role="status">
            最新です（バージョン {version}）
          </span>
        )}
        {state.kind === "error" && (
          <span className="form-error" role="alert">
            確認できませんでした: {state.message}
          </span>
        )}
      </div>
      {info && <UpdateView inline />}
    </div>
  );
}
