import * as Dialog from "@radix-ui/react-dialog";
import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useId, useRef, useState } from "react";

import { errorMessage, useBackend } from "../../api";
import { queryKeys } from "../../api/queries";
import type { ForeignRef, ImportReport, LoadProblem, Project, SyncDiff, UploadInput } from "../../bindings";
import { opInfoOf, useTabsStore } from "../../store/tabs";
import { useUiStore } from "../../store/ui";
import { CloseIcon } from "../Icons";
import { DiffList, diffIsEmpty } from "./DiffList";
import { ForeignHosts, LoadProblems } from "./Problems";
import { UploadZone } from "./UploadZone";

type State =
  | { kind: "checking" }
  | { kind: "ready"; diff: SyncDiff; changedFiles: string[] }
  | { kind: "needsUpload"; entry: string }
  | { kind: "needsEntry"; candidates: string[] }
  | { kind: "needsConfirmation"; foreign: ForeignRef[] }
  | { kind: "blocked"; problems: LoadProblem[] }
  | { kind: "error"; message: string }
  | { kind: "applying" }
  | { kind: "applied"; diff: SyncDiff; report: ImportReport };

/** 同期ボタンの確認ダイアログ。yaml を読み直して、追加・変更・削除を見せてから反映する。 */
export function SyncDialog({ project }: { project: Project }) {
  const open = useUiStore((s) => s.syncDialogOpen);
  const setOpen = useUiStore((s) => s.setSyncDialogOpen);
  return (
    <Dialog.Root open={open} onOpenChange={setOpen}>
      <Dialog.Portal>
        <Dialog.Overlay className="dialog-overlay" />
        <Dialog.Content className="dialog sync-dialog" aria-describedby={undefined}>
          <SyncFlow project={project} />
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function SyncFlow({ project }: { project: Project }) {
  const backend = useBackend();
  const queryClient = useQueryClient();
  const [state, setState] = useState<State>({ kind: "checking" });
  const [uploadPaths, setUploadPaths] = useState<string[]>([]);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const check = useCallback(
    async (upload: UploadInput | null) => {
      setState({ kind: "checking" });
      try {
        const r = await backend.openapiSyncCheck(project.id, upload);
        if (!alive.current) return;
        switch (r.status) {
          case "ready":
            setState({ kind: "ready", diff: r.diff, changedFiles: r.changedFiles });
            // 差分がなければ反映はしないが、更新日時は覚え直されるので「変更あり」の印を確かめ直す
            if (diffIsEmpty(r.diff)) void queryClient.invalidateQueries({ queryKey: queryKeys.sourceChanged(project.id) });
            break;
          case "needsUpload":
            setState({ kind: "needsUpload", entry: r.entry });
            break;
          case "needsEntry":
            setState({ kind: "needsEntry", candidates: r.candidates });
            break;
          case "needsConfirmation":
            setState({ kind: "needsConfirmation", foreign: r.foreign });
            break;
          case "blocked":
            setState({ kind: "blocked", problems: r.problems });
            break;
        }
      } catch (e) {
        if (alive.current) setState({ kind: "error", message: errorMessage(e) });
      }
    },
    [backend, project.id, queryClient],
  );

  // 開いたら、すぐ yaml を読み直して差分を出す（コピー方式は、ファイルを選び直してもらう）
  useEffect(() => {
    void check(null);
  }, [check]);

  const upload = (entry: string | null): UploadInput => ({ paths: uploadPaths, entry });

  const apply = async () => {
    if (state.kind !== "ready") return;
    const used = uploadPaths.length > 0 ? upload(null) : null;
    setState({ kind: "applying" });
    try {
      const r = await backend.openapiSyncApply(project.id, used);
      if (!alive.current) return;
      if (r.status === "applied") {
        setState({ kind: "applied", diff: r.diff, report: r.report });
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: queryKeys.projects }),
          queryClient.invalidateQueries({ queryKey: queryKeys.operations(project.id) }),
          queryClient.invalidateQueries({ queryKey: ["operation", project.id] }),
          queryClient.invalidateQueries({ queryKey: queryKeys.sourceChanged(project.id) }),
        ]);
        // 開いている API のタブは、入力した内容はそのままに、yaml 側の情報（必須・yaml にない・既定の認証）だけ更新する
        for (const tab of useTabsStore.getState().tabs) {
          if (tab.projectId !== project.id || !tab.op) continue;
          try {
            const opened = await backend.operationOpen(project.id, tab.op.opKey);
            useTabsStore.getState().setOpInfo(tab.id, opInfoOf(opened));
          } catch {
            /* 消えた API などは、そのままにする */
          }
        }
      } else if (r.status === "blocked") {
        setState({ kind: "blocked", problems: r.problems });
      } else if (r.status === "needsConfirmation") {
        setState({ kind: "needsConfirmation", foreign: r.foreign });
      } else if (r.status === "needsEntry") {
        setState({ kind: "needsEntry", candidates: r.candidates });
      } else {
        setState({ kind: "needsUpload", entry: r.entry });
      }
    } catch (e) {
      if (alive.current) setState({ kind: "error", message: errorMessage(e) });
    }
  };

  return (
    <>
      <header className="sync-header">
        <Dialog.Title className="import-title">OpenAPI の同期</Dialog.Title>
        <Dialog.Close className="icon-btn" aria-label="閉じる">
          <CloseIcon />
        </Dialog.Close>
      </header>

      <div className="sync-body">
        {state.kind === "checking" && (
          <div className="import-checking" role="status">
            <span className="spinner" aria-hidden="true" />
            yaml を読み直しています…
          </div>
        )}
        {state.kind === "applying" && (
          <div className="import-checking" role="status">
            <span className="spinner" aria-hidden="true" />
            反映しています…
          </div>
        )}

        {state.kind === "ready" &&
          (diffIsEmpty(state.diff) ? (
            <div className="sync-empty" role="status">
              <span className="import-done-mark" aria-hidden="true">
                ✓
              </span>
              変更はありません
            </div>
          ) : (
            <>
              <p className="sync-lead" role="status">
                追加 {state.diff.added.length} · 変更 {state.diff.changed.length} · 削除 {state.diff.removed.length}
                {state.changedFiles.length > 0 && <span className="muted-note">（変わったファイル: {state.changedFiles.join("、")}）</span>}
              </p>
              <p className="muted-note">入力した値（パラメータ・ボディ・認証）は消えません。反映するまで、今の API をそのまま使えます。</p>
              <DiffList diff={state.diff} />
            </>
          ))}

        {state.kind === "applied" && (
          <div className="sync-empty" role="status">
            <span className="import-done-mark" aria-hidden="true">
              ✓
            </span>
            反映しました（追加 {state.report.added} · 変更 {state.report.changed} · 削除 {state.report.removed}）
            {state.report.keptAsDeleted > 0 && <span className="muted-note">入力値があるので「削除された API」に残したもの: {state.report.keptAsDeleted} 件</span>}
          </div>
        )}

        {state.kind === "needsUpload" && (
          <div className="import-issue" role="group" aria-label="もう一度アップロード">
            <div className="import-issue-title">
              コピー方式で取り込んだ yaml です。同期するには、もう一度ファイルを選んでください（入口: <span className="mono">{state.entry}</span>）。
            </div>
            <UploadZone paths={uploadPaths} onChange={setUploadPaths} />
            <div className="import-issue-actions">
              <button type="button" className="btn btn-primary btn-small" disabled={uploadPaths.length === 0} onClick={() => void check(upload(null))}>
                差分を確認
              </button>
            </div>
          </div>
        )}
        {state.kind === "needsEntry" && (
          <div className="import-issue" role="group" aria-label="入口ファイルの選択">
            <div className="import-issue-title">入口ファイルが複数あります。入口を選んでください。</div>
            <ul className="entry-list">
              {state.candidates.map((c) => (
                <li key={c}>
                  <span className="mono entry-name">{c}</span>
                  <button type="button" className="btn btn-small" onClick={() => void check(upload(c))}>
                    これを入口にする
                  </button>
                </li>
              ))}
            </ul>
          </div>
        )}
        {state.kind === "needsConfirmation" && (
          <div className="import-issue is-warn" role="group" aria-label="別のホストの確認">
            <div className="import-issue-title">入口と別のサイトを指す参照があります。確認が済んでいないので、同期できません。</div>
            <ForeignHosts foreign={state.foreign} />
            <p className="muted-note">「OpenAPI取り込み」から取り込み直すと、読みに行ってよいか確認できます。</p>
          </div>
        )}
        {state.kind === "blocked" && (
          <>
            <LoadProblems problems={state.problems} upload={uploadPaths.length > 0} title="yaml を読めないため、同期を中止しました。" />
            <p className="muted-note">前回取り込んだ API をそのまま使い続けます。yaml を直してから、もう一度同期してください。</p>
          </>
        )}
        {state.kind === "error" && (
          <div className="alert alert-danger" role="alert">
            同期できませんでした: {state.message}
          </div>
        )}
      </div>

      {project.source?.kind === "url" && <TokenField project={project} onSaved={() => void check(null)} />}

      <footer className="sync-footer">
        {(state.kind === "blocked" || state.kind === "error") && (
          <button type="button" className="btn" onClick={() => void check(uploadPaths.length > 0 ? upload(null) : null)}>
            もう一度確認
          </button>
        )}
        <Dialog.Close className="btn import-footer-right" type="button">
          {state.kind === "applied" || (state.kind === "ready" && diffIsEmpty(state.diff)) ? "閉じる" : "キャンセル"}
        </Dialog.Close>
        {state.kind === "ready" && !diffIsEmpty(state.diff) && (
          <button type="button" className="btn btn-primary" onClick={() => void apply()}>
            反映する
          </button>
        )}
      </footer>
    </>
  );
}

/** URL 方式のアクセストークン（非公開リポジトリ用）。キーチェーンに保存し、空で保存すると消す。値は画面に残さない。 */
function TokenField({ project, onSaved }: { project: Project; onSaved: () => void }) {
  const backend = useBackend();
  const queryClient = useQueryClient();
  const id = useId();
  const [token, setToken] = useState("");
  const [state, setState] = useState<{ kind: "idle" } | { kind: "saving" } | { kind: "saved"; cleared: boolean } | { kind: "error"; message: string }>({ kind: "idle" });

  const save = async () => {
    setState({ kind: "saving" });
    try {
      await backend.sourceTokenSet(project.id, token.trim());
      await queryClient.invalidateQueries({ queryKey: queryKeys.projects });
      setState({ kind: "saved", cleared: token.trim() === "" });
      setToken("");
      onSaved();
    } catch (e) {
      setState({ kind: "error", message: errorMessage(e) });
    }
  };

  return (
    <div className="sync-token">
      <label htmlFor={id} className="muted-note">
        アクセストークン（{project.source?.hasToken ? "保存済み" : "未設定"}）。非公開のリポジトリを読むときに使い、OS のキーチェーンに保存します。空のまま保存すると消します。
      </label>
      <div className="sync-token-row">
        <input
          id={id}
          className="input"
          type="password"
          autoComplete="off"
          aria-label="新しいアクセストークン"
          placeholder="新しいアクセストークン"
          value={token}
          onChange={(e) => setToken(e.target.value)}
        />
        <button type="button" className="btn" disabled={state.kind === "saving" || (token.trim() === "" && !project.source?.hasToken)} onClick={() => void save()}>
          {token.trim() === "" && project.source?.hasToken ? "トークンを消す" : "トークンを保存"}
        </button>
      </div>
      {state.kind === "saved" && (
        <p className="muted-note" role="status">
          {state.cleared ? "トークンを消しました。" : "トークンを保存しました。"}
        </p>
      )}
      {state.kind === "error" && (
        <p className="form-error" role="alert">
          保存できませんでした: {state.message}
        </p>
      )}
    </div>
  );
}
