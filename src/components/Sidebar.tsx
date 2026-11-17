import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";

import { errorMessage, useBackend } from "../api";
import { queryKeys, useAdhocList, useOperations, useSourceChanged } from "../api/queries";
import type { AdHocRequest, OperationListItem, Project } from "../bindings";
import { opKeyString } from "../lib/opkey";
import { buildTree, summarize } from "../lib/tree";
import { closeTab } from "../store/send";
import { openOperation } from "../store/ops";
import { useTabsStore } from "../store/tabs";
import { useUiStore } from "../store/ui";
import { ApiTree } from "./ApiTree";
import { ConfirmDialog } from "./Dialog";
import { ClockIcon, ListIcon, SearchIcon, SyncIcon } from "./Icons";

/** 左のサイドバー。API ツリー（タグのフォルダ・削除された API・その他）・検索・同期ボタン・環境変数。履歴は M6。 */
export function Sidebar({ project }: { project: Project }) {
  const backend = useBackend();
  const queryClient = useQueryClient();
  const adhoc = useAdhocList(project.id);
  const operations = useOperations(project.id);
  const sourceChanged = useSourceChanged(project);
  const tabs = useTabsStore((s) => s.tabs);
  const activeId = useTabsStore((s) => s.activeByProject[project.id]);
  const openAdhoc = useTabsStore((s) => s.openAdhoc);
  const setNewRequestDialogOpen = useUiStore((s) => s.setNewRequestDialogOpen);
  const setSyncDialogOpen = useUiStore((s) => s.setSyncDialogOpen);
  const setScreen = useUiStore((s) => s.setScreen);
  const [query, setQuery] = useState("");
  const [expanded, setExpanded] = useState<Record<string, boolean | undefined>>({});
  const [deleting, setDeleting] = useState<AdHocRequest | null>(null);
  const [forgetting, setForgetting] = useState<OperationListItem | null>(null);
  const [error, setError] = useState<string | null>(null);

  const ops = useMemo(() => operations.data ?? [], [operations.data]);
  const items = useMemo(() => adhoc.data ?? [], [adhoc.data]);
  const activeTab = tabs.find((t) => t.projectId === project.id && t.id === activeId) ?? tabs.find((t) => t.projectId === project.id);
  const activeKey = activeTab ? (activeTab.op ? `op:${opKeyString(activeTab.op.opKey)}` : activeTab.adhocId ? `adhoc:${activeTab.adhocId}` : null) : null;

  const rows = useMemo(
    () => buildTree({ ops, adhocs: items, adhocLoaded: adhoc.isSuccess, query, expanded }),
    [ops, items, adhoc.isSuccess, query, expanded],
  );
  const summary = summarize(ops);
  const hits = rows.filter((r) => r.type === "op" || r.type === "adhoc").length;

  // タブで開いた API のフォルダが閉じていたら開く
  const activeOp = activeTab?.op ?? null;
  useEffect(() => {
    if (!activeOp) return;
    const key = opKeyString(activeOp.opKey);
    const item = ops.find((o) => !o.deleted && opKeyString(o.opKey) === key);
    if (!item) return;
    const id = `tag:${item.folder ?? ""}`;
    setExpanded((prev) => (prev[id] === undefined ? { ...prev, [id]: true } : prev));
  }, [activeOp, ops]);

  const toggle = (id: string) =>
    setExpanded((prev) => {
      const row = rows.find((r) => r.type === "folder" && r.id === id);
      const current = row?.type === "folder" ? row.open : false;
      return { ...prev, [id]: !current };
    });

  const open = async (item: OperationListItem) => {
    setError(null);
    try {
      await openOperation(backend, project.id, item.opKey);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const removeAdhoc = async (item: AdHocRequest) => {
    setDeleting(null);
    setError(null);
    try {
      await backend.adhocDelete(item.id);
      const tab = useTabsStore.getState().tabs.find((t) => t.adhocId === item.id);
      if (tab) closeTab(backend, tab.id, true);
      await queryClient.invalidateQueries({ queryKey: queryKeys.adhoc(project.id) });
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const forget = async (item: OperationListItem) => {
    setForgetting(null);
    setError(null);
    try {
      await backend.operationForget(project.id, item.opKey);
      const tab = useTabsStore.getState().findOpTab(project.id, item.opKey);
      if (tab) closeTab(backend, tab.id, true);
      await queryClient.invalidateQueries({ queryKey: queryKeys.operations(project.id) });
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const countText = summary.count > 0 ? `${summary.count} 件${summary.tags > 0 ? ` · タグ ${summary.tags}` : ""}` : `${items.length} 件`;
  const searching = query.trim() !== "";
  const emptyMessage = searching && hits === 0 && !rows.some((r) => r.type === "folder") ? "該当する API がありません。" : null;

  return (
    <nav className="sidebar" aria-label="API ツリー">
      <div className="sidebar-search">
        <label className="search-field">
          <SearchIcon />
          <input
            aria-label="API を検索"
            placeholder="API・operationId を検索"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            autoComplete="off"
            spellCheck={false}
          />
        </label>
        <button type="button" className="btn btn-icon" aria-label="新規作成" onClick={() => setNewRequestDialogOpen(true)}>
          +
        </button>
      </div>
      <div className="sidebar-meta">
        <span className="sidebar-count">{countText}</span>
        {searching && <span className="sidebar-count">検索: {hits} 件</span>}
        {project.source && (
          <button
            type="button"
            className={`sync-btn${sourceChanged ? " is-changed" : ""}`}
            aria-label={sourceChanged ? "OpenAPI を同期（yaml に変更があります）" : "OpenAPI を同期"}
            title={sourceChanged ? "取り込み元の yaml が変わっています。押すと差分を確認できます" : "yaml を読み直して、差分を確認します"}
            onClick={() => setSyncDialogOpen(true)}
          >
            <SyncIcon />
            同期
            {sourceChanged && <span className="sync-dot" role="img" aria-label="変更あり" />}
          </button>
        )}
      </div>

      <ApiTree
        rows={rows}
        activeKey={activeKey}
        emptyMessage={emptyMessage}
        onToggle={toggle}
        onOpenOp={(item) => void open(item)}
        onOpenAdhoc={openAdhoc}
        onDeleteAdhoc={setDeleting}
        onForgetOp={setForgetting}
      />
      {operations.isSuccess && ops.length === 0 && (
        <div className="tree-empty">
          <p>OpenAPI を取り込むと、タグごとの API ツリーがここに並びます。ヘッダーの「OpenAPI取り込み」から取り込めます。</p>
        </div>
      )}
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      <div className="sidebar-links">
        <button type="button" className="nav-link" onClick={() => setScreen("environments")}>
          <ListIcon />
          環境変数
        </button>
        <button type="button" className="nav-link" disabled title="履歴は M6 で実装">
          <ClockIcon />
          履歴
        </button>
      </div>

      <ConfirmDialog
        open={deleting !== null}
        onOpenChange={(o) => !o && setDeleting(null)}
        title="リクエストを削除しますか？"
        description={deleting ? `「${deleting.name}」を削除します。この操作は取り消せません。` : ""}
        confirmLabel="削除する"
        danger
        onConfirm={() => deleting && void removeAdhoc(deleting)}
      />
      <ConfirmDialog
        open={forgetting !== null}
        onOpenChange={(o) => !o && setForgetting(null)}
        title="削除された API を消しますか？"
        description={
          forgetting
            ? `「${forgetting.summary ?? forgetting.path}」は yaml から削除されています。消すと、入力値も一緒に消えます。この操作は取り消せません。`
            : ""
        }
        confirmLabel="消す"
        danger
        onConfirm={() => forgetting && void forget(forgetting)}
      />
    </nav>
  );
}
