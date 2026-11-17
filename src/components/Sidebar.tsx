import { useState } from "react";

import { errorMessage, useBackend } from "../api";
import { queryKeys, useAdhocList } from "../api/queries";
import type { AdHocRequest, Project } from "../bindings";
import { methodColor } from "../lib/spec";
import { closeTab } from "../store/send";
import { useTabsStore } from "../store/tabs";
import { useUiStore } from "../store/ui";
import { useQueryClient } from "@tanstack/react-query";
import { ConfirmDialog } from "./Dialog";
import { ClockIcon, ListIcon, SearchIcon, TrashIcon } from "./Icons";

/** 左のサイドバー。「その他」（手で作ったリクエスト）・環境変数。API ツリーは M3、履歴は M6 で入る。 */
export function Sidebar({ project }: { project: Project }) {
  const backend = useBackend();
  const queryClient = useQueryClient();
  const adhoc = useAdhocList(project.id);
  const tabs = useTabsStore((s) => s.tabs);
  const activeId = useTabsStore((s) => s.activeByProject[project.id]);
  const openAdhoc = useTabsStore((s) => s.openAdhoc);
  const setNewRequestDialogOpen = useUiStore((s) => s.setNewRequestDialogOpen);
  const setScreen = useUiStore((s) => s.setScreen);
  const [open, setOpen] = useState(true);
  const [deleting, setDeleting] = useState<AdHocRequest | null>(null);
  const [error, setError] = useState<string | null>(null);

  const items = adhoc.data ?? [];
  const activeAdhocId = tabs.find((t) => t.id === activeId)?.adhocId ?? null;

  const remove = async (item: AdHocRequest) => {
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

  return (
    <nav className="sidebar" aria-label="API ツリー">
      <div className="sidebar-search">
        <label className="search-field">
          <SearchIcon />
          <input aria-label="API を検索" placeholder="API・operationId を検索" disabled title="API の検索は M3 で実装" />
        </label>
        <button type="button" className="btn btn-icon" aria-label="新規作成" onClick={() => setNewRequestDialogOpen(true)}>
          +
        </button>
      </div>
      <span className="sidebar-count">{items.length} 件</span>

      <div className="tree">
        <button type="button" className="tree-folder" aria-expanded={open} onClick={() => setOpen(!open)}>
          <span aria-hidden="true" className="tree-chevron">
            {open ? "▾" : "▸"}
          </span>
          <span className="tree-folder-name">その他</span>
          <span className="tree-count">{items.length}</span>
        </button>
        {open &&
          items.map((item) => (
            <div key={item.id} className={`tree-row${item.id === activeAdhocId ? " is-active" : ""}`}>
              <button type="button" className="tree-item" onClick={() => openAdhoc(item)}>
                <span className="tree-method" style={{ color: methodColor(item.method) }}>
                  {item.method}
                </span>
                <span className="tree-name">{item.name}</span>
              </button>
              <button type="button" className="icon-btn tree-delete" aria-label={`${item.name} を削除`} onClick={() => setDeleting(item)}>
                <TrashIcon />
              </button>
            </div>
          ))}
        {open && items.length === 0 && adhoc.isSuccess && <p className="tree-hint">保存したリクエストがここに並びます。</p>}
        {error && (
          <p className="form-error" role="alert">
            {error}
          </p>
        )}
      </div>

      <div className="tree-empty">
        <p>OpenAPI を取り込むと、タグごとの API ツリーがここに並びます（M3 で実装）。</p>
      </div>

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
        onConfirm={() => deleting && void remove(deleting)}
      />
    </nav>
  );
}
