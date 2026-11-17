import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useState } from "react";

import { errorMessage, useBackend } from "../api";
import { queryKeys, useAppSettings, useEnvironments } from "../api/queries";
import type { HistoryDiff, HistoryListItem, HistoryQuery, Project, StatusClass } from "../bindings";
import { copyText } from "../lib/clipboard";
import { useDebounced } from "../lib/hooks";
import { buildHistoryRows, STATUS_CLASSES } from "../lib/history";
import { openFromHistory } from "../store/ops";
import { resendInTab } from "../store/send";
import { useUiStore } from "../store/ui";
import { ConfirmDialog } from "./Dialog";
import { ChevronLeftIcon, SearchIcon } from "./Icons";
import { HistoryCompare } from "./history/HistoryCompare";
import { HistoryDetail } from "./history/HistoryDetail";
import { HistoryList } from "./history/HistoryList";

/** 1 回に読む件数（続きは `nextCursor` で読む）。 */
export const HISTORY_PAGE_SIZE = 100;
const SEARCH_DEBOUNCE_MS = 250;

/** 履歴の画面。日付ごとの一覧・絞り込み・検索・詳細・再送信・比較。 */
export function HistoryScreen({ project }: { project: Project }) {
  const backend = useBackend();
  const queryClient = useQueryClient();
  const setScreen = useUiStore((s) => s.setScreen);
  const settings = useAppSettings();
  const envs = useEnvironments(project.id);

  const [text, setText] = useState("");
  const [statuses, setStatuses] = useState<StatusClass[]>([]);
  const [environmentId, setEnvironmentId] = useState("");
  const [periodOpen, setPeriodOpen] = useState(false);
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [comparing, setComparing] = useState(false);
  const [diff, setDiff] = useState<HistoryDiff | null>(null);
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState<"one" | "all" | null>(null);

  const debouncedText = useDebounced(text, SEARCH_DEBOUNCE_MS);
  const query = useMemo<HistoryQuery>(
    () => ({
      text: debouncedText.trim() === "" ? null : debouncedText.trim(),
      statuses,
      environmentId: environmentId === "" ? null : environmentId,
      from: from === "" ? null : from,
      to: to === "" ? null : to,
      // 日付の区切りは、この PC の時刻で決める
      utcOffsetMinutes: -new Date().getTimezoneOffset(),
      limit: HISTORY_PAGE_SIZE,
    }),
    [debouncedText, statuses, environmentId, from, to],
  );

  const list = useInfiniteQuery({
    queryKey: queryKeys.history(project.id, query),
    queryFn: ({ pageParam }) => backend.historyList(project.id, { ...query, cursor: pageParam }),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.nextCursor,
  });
  const items = useMemo(() => list.data?.pages.flatMap((p) => p.items) ?? [], [list.data]);
  const total = list.data?.pages[0]?.total ?? 0;
  const rows = useMemo(() => buildHistoryRows(items), [items]);
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = list;
  const needMore = useCallback(() => {
    if (hasNextPage && !isFetchingNextPage) void fetchNextPage();
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);

  // 選んでいる履歴がなくなった（絞り込み・削除）ら、先頭を選ぶ
  useEffect(() => {
    if (list.isPending) return;
    if (selectedId === null || !items.some((i) => i.id === selectedId)) {
      // 続きを読む前の一覧にまだ出ていないだけのことがあるので、先頭を選ぶのは、選択がないときだけ
      if (selectedId === null || !hasNextPage) setSelectedId(items[0]?.id ?? null);
    }
  }, [items, selectedId, list.isPending, hasNextPage]);

  const entry = useQuery({
    queryKey: queryKeys.historyEntry(selectedId ?? ""),
    queryFn: () => backend.historyGet(selectedId as string),
    enabled: selectedId !== null,
  });

  const limit = settings.data?.history.limit;
  const limitText = limit === undefined ? "" : limit === 0 ? "履歴は保存しない設定です" : `直近 ${new Intl.NumberFormat("ja-JP").format(limit)} 件まで保存`;
  const filtering = text.trim() !== "" || statuses.length > 0 || environmentId !== "" || from !== "" || to !== "";
  const envList = (envs.data ?? []).filter((e) => !e.isGlobal);
  const envName = envs.data?.find((e) => e.id === project.defaultEnvironmentId)?.name ?? null;

  const refresh = () => queryClient.invalidateQueries({ queryKey: queryKeys.history(project.id) });

  const toggleStatus = (id: StatusClass) => setStatuses((cur) => (cur.includes(id) ? cur.filter((s) => s !== id) : [...cur, id]));

  const select = async (item: HistoryListItem) => {
    setMessage(null);
    if (comparing && selectedId !== null && item.id !== selectedId) {
      try {
        setDiff(await backend.historyCompare(selectedId, item.id));
      } catch (e) {
        setMessage({ ok: false, text: `比較できませんでした: ${errorMessage(e)}` });
      }
      return;
    }
    if (!comparing) setDiff(null);
    setSelectedId(item.id);
  };

  const stopComparing = () => {
    setComparing(false);
    setDiff(null);
  };

  /** 操作を実行する（失敗はメッセージに出す）。 */
  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setMessage(null);
    try {
      await action();
    } catch (e) {
      setMessage({ ok: false, text: errorMessage(e) });
    } finally {
      setBusy(false);
    }
  };

  const actions = {
    onResend: () =>
      void run(async () => {
        if (selectedId === null) return;
        const id = selectedId;
        // 元の入力をタブに開き、そのタブで結果を受け取る（今の環境で送る）
        const opened = await backend.historyOpen(id);
        const tabId = await openFromHistory(backend, project.id, opened);
        void resendInTab(backend, tabId, envName, () => backend.historyResend(id, null));
        setScreen("main");
      }),
    onOpen: () =>
      void run(async () => {
        if (selectedId === null) return;
        const opened = await backend.historyOpen(selectedId);
        await openFromHistory(backend, project.id, opened);
        setScreen("main");
      }),
    onSave: () =>
      void run(async () => {
        if (selectedId === null) return;
        const saved = await backend.historySaveAsInput(selectedId, null);
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: queryKeys.operations(project.id) }),
          queryClient.invalidateQueries({ queryKey: queryKeys.adhoc(project.id) }),
        ]);
        const where = saved.kind === "draft" ? "この API の入力値として保存しました" : `「その他」に「${saved.request.name}」として保存しました`;
        const dropped = saved.dropped.length > 0 ? `（伏せ字の値は保存していません: ${saved.dropped.join("、")}）` : "";
        setMessage({ ok: true, text: `${where}${dropped}` });
      }),
    onCopyCurl: () =>
      void run(async () => {
        if (selectedId === null) return;
        const curl = await backend.historyCurl(selectedId);
        const ok = await copyText(curl);
        setMessage(ok ? { ok: true, text: "cURL をコピーしました（秘密の値は伏せ字のままです）" } : { ok: false, text: "クリップボードにコピーできませんでした" });
      }),
    onToggleCompare: () => {
      if (comparing) stopComparing();
      else {
        setComparing(true);
        setMessage(null);
      }
    },
    onDelete: () => setConfirm("one"),
  };

  const deleteOne = () =>
    void run(async () => {
      if (selectedId === null) return;
      await backend.historyDelete(selectedId);
      queryClient.removeQueries({ queryKey: queryKeys.historyEntry(selectedId) });
      setSelectedId(null);
      stopComparing();
      await refresh();
      setMessage({ ok: true, text: "履歴を 1 件削除しました" });
    });

  const deleteAll = () =>
    void run(async () => {
      const n = await backend.historyClear(project.id);
      setSelectedId(null);
      stopComparing();
      await refresh();
      setMessage({ ok: true, text: `履歴を ${n} 件すべて削除しました` });
    });

  return (
    <div className="history-screen">
      <header className="screen-header">
        <button type="button" className="btn btn-ghost screen-back" onClick={() => setScreen("main")}>
          <ChevronLeftIcon />
          戻る
        </button>
        <h1 className="screen-title">履歴</h1>
        <span className="muted-note">{limitText}</span>
        <button type="button" className="btn btn-danger-ghost screen-right" disabled={list.isPending || (total === 0 && !filtering)} onClick={() => setConfirm("all")}>
          すべて削除
        </button>
      </header>

      <div className="history-body">
        <section className="history-side" aria-label="履歴の一覧">
          <div className="history-filters">
            <label className="search-field">
              <SearchIcon />
              <input aria-label="URLやボディで検索" placeholder="URL やボディで検索" value={text} onChange={(e) => setText(e.target.value)} autoComplete="off" spellCheck={false} />
            </label>
            <div className="chips" role="group" aria-label="絞り込み">
              <button type="button" className="chip-btn" aria-pressed={statuses.length === 0} onClick={() => setStatuses([])}>
                すべて
              </button>
              {STATUS_CLASSES.map((s) => (
                <button key={s.id} type="button" className="chip-btn" aria-pressed={statuses.includes(s.id)} onClick={() => toggleStatus(s.id)}>
                  {s.label}
                </button>
              ))}
              <button type="button" className="chip-btn" aria-pressed={periodOpen || from !== "" || to !== ""} onClick={() => setPeriodOpen((v) => !v)}>
                期間
              </button>
            </div>
            <div className="history-filter-row">
              <label className="history-env">
                環境
                <select className="select select-small" aria-label="環境で絞り込み" value={environmentId} onChange={(e) => setEnvironmentId(e.target.value)}>
                  <option value="">すべての環境</option>
                  {envList.map((e) => (
                    <option key={e.id} value={e.id}>
                      {e.name}
                    </option>
                  ))}
                </select>
              </label>
              <span className="muted-note history-total" role="status" aria-label="件数">
                {list.isPending ? "読み込み中…" : `${total.toLocaleString("ja-JP")} 件${filtering ? "（絞り込み中）" : ""}`}
              </span>
            </div>
            {(periodOpen || from !== "" || to !== "") && (
              <div className="history-period" role="group" aria-label="期間">
                <label>
                  <span className="muted-note">開始</span>
                  <input type="date" className="input input-small" aria-label="期間の開始" value={from} max={to || undefined} onChange={(e) => setFrom(e.target.value)} />
                </label>
                <span aria-hidden="true">〜</span>
                <label>
                  <span className="muted-note">終了</span>
                  <input type="date" className="input input-small" aria-label="期間の終了" value={to} min={from || undefined} onChange={(e) => setTo(e.target.value)} />
                </label>
                {(from !== "" || to !== "") && (
                  <button
                    type="button"
                    className="link-btn"
                    onClick={() => {
                      setFrom("");
                      setTo("");
                    }}
                  >
                    期間をクリア
                  </button>
                )}
              </div>
            )}
          </div>

          {comparing && (
            <div className="notice history-compare-banner" role="status">
              比較する 2 件目の履歴を、一覧から選んでください。
              <button type="button" className="btn btn-small" onClick={stopComparing}>
                やめる
              </button>
            </div>
          )}

          {list.isError ? (
            <div className="alert alert-danger" role="alert">
              履歴を読み込めませんでした: {errorMessage(list.error)}
            </div>
          ) : !list.isPending && items.length === 0 ? (
            <p className="history-empty">{filtering ? "条件に当てはまる履歴はありません。" : "まだ履歴がありません。リクエストを送ると、ここに残ります。"}</p>
          ) : (
            <HistoryList rows={rows} selectedId={selectedId} comparing={comparing} hasMore={hasNextPage} fetching={isFetchingNextPage} onSelect={(i) => void select(i)} onNeedMore={needMore} />
          )}
          {hasNextPage && (
            <button type="button" className="btn btn-ghost btn-small history-more" disabled={isFetchingNextPage} onClick={needMore}>
              {isFetchingNextPage ? "読み込んでいます…" : `さらに読み込む（残り ${(total - items.length).toLocaleString("ja-JP")} 件）`}
            </button>
          )}
        </section>

        <section className="history-main" aria-label="履歴の詳細">
          {diff ? (
            <HistoryCompare diff={diff} onClose={stopComparing} />
          ) : selectedId === null ? (
            <div className="pane-placeholder">履歴を選ぶと、送った内容と受け取ったレスポンスがここに出ます。</div>
          ) : entry.isError ? (
            <div className="alert alert-danger" role="alert">
              履歴を読み込めませんでした: {errorMessage(entry.error)}
            </div>
          ) : entry.data ? (
            <HistoryDetail entry={entry.data} actions={actions} comparing={comparing} message={message} busy={busy} />
          ) : (
            <div className="pane-placeholder" role="status">
              読み込んでいます…
            </div>
          )}
          {diff && message && (
            <div className={`notice ${message.ok ? "notice-ok" : "notice-bad"}`} role={message.ok ? "status" : "alert"}>
              {message.text}
            </div>
          )}
        </section>
      </div>

      <ConfirmDialog
        open={confirm === "one"}
        onOpenChange={(o) => !o && setConfirm(null)}
        title="この履歴を削除しますか？"
        description="選んでいる履歴を 1 件削除します。この操作は取り消せません。"
        confirmLabel="削除する"
        danger
        onConfirm={() => {
          setConfirm(null);
          deleteOne();
        }}
      />
      <ConfirmDialog
        open={confirm === "all"}
        onOpenChange={(o) => !o && setConfirm(null)}
        title="履歴をすべて削除しますか？"
        description="このプロジェクトの履歴を、絞り込み中でもすべて削除します。この操作は取り消せません。"
        confirmLabel="すべて削除する"
        danger
        onConfirm={() => {
          setConfirm(null);
          deleteAll();
        }}
      />
    </div>
  );
}
