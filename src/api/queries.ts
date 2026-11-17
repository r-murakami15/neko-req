import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";

import type { RequestPreview, RequestSpec } from "../bindings";
import { type RequestTab, useTabsStore } from "../store/tabs";
import { useUiStore } from "../store/ui";
import { useBackend } from "./context";

export const queryKeys = {
  projects: ["projects"] as const,
  appInfo: ["appInfo"] as const,
  environments: (projectId: string) => ["environments", projectId] as const,
  adhoc: (projectId: string) => ["adhoc", projectId] as const,
};

export function useProjects() {
  const backend = useBackend();
  return useQuery({ queryKey: queryKeys.projects, queryFn: () => backend.projectList() });
}

export function useAppInfo() {
  const backend = useBackend();
  return useQuery({ queryKey: queryKeys.appInfo, queryFn: () => backend.appInfo(), staleTime: Infinity });
}

/** 作ったプロジェクトをそのまま開く。 */
export function useCreateProject() {
  const backend = useBackend();
  const queryClient = useQueryClient();
  const setCurrentProjectId = useUiStore((s) => s.setCurrentProjectId);
  return useMutation({
    mutationFn: (name: string) => backend.projectCreate(name),
    onSuccess: async (project) => {
      await queryClient.invalidateQueries({ queryKey: queryKeys.projects });
      setCurrentProjectId(project.id);
    },
  });
}

/** プロジェクトの環境（グローバルが先頭）。 */
export function useEnvironments(projectId: string) {
  const backend = useBackend();
  return useQuery({ queryKey: queryKeys.environments(projectId), queryFn: () => backend.envList(projectId) });
}

/** ヘッダーの環境選択。「選んでいる環境」を変える。 */
export function useSelectEnvironment(projectId: string) {
  const backend = useBackend();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (environmentId: string | null) => backend.envSelect(projectId, environmentId),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.projects }),
  });
}

/** 手で作ったリクエスト（サイドバーの「その他」）。 */
export function useAdhocList(projectId: string) {
  const backend = useBackend();
  return useQuery({ queryKey: queryKeys.adhoc(projectId), queryFn: () => backend.adhocList(projectId) });
}

const PREVIEW_DEBOUNCE_MS = 150;

export interface PreviewState {
  /** まだ一度も返ってきていないうちは `null`。 */
  preview: RequestPreview | null;
  /** 入力を変えてから、プレビューが返ってくるまでの間。 */
  stale: boolean;
  error: string | null;
}

/**
 * 展開後の URL・展開できない変数・自動で付くヘッダー。入力のたびに（少し待って）Rust に聞く。
 * `refreshKey` が変わったとき（環境を選び直した・環境変数を保存した）も聞き直す。
 */
export function usePreview(projectId: string, spec: RequestSpec, refreshKey: string): PreviewState {
  const backend = useBackend();
  const [state, setState] = useState<PreviewState>({ preview: null, stale: true, error: null });
  const seq = useRef(0);

  useEffect(() => {
    const mine = (seq.current += 1);
    setState((s) => (s.stale ? s : { ...s, stale: true }));
    const timer = setTimeout(() => {
      backend
        .requestPreview({ projectId, environmentId: null, request: spec, overrides: [] })
        .then((preview) => {
          if (mine === seq.current) setState({ preview, stale: false, error: null });
        })
        .catch((e: unknown) => {
          if (mine === seq.current) {
            setState({ preview: null, stale: false, error: e instanceof Error ? e.message : "確認できませんでした" });
          }
        });
    }, PREVIEW_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [backend, projectId, spec, refreshKey]);

  return state;
}

/** タブの内容を AdHocRequest として保存する（新しいリクエストは `name`・`description` を渡して作る）。 */
export function useSaveTab() {
  const backend = useBackend();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ tab, name, description }: { tab: RequestTab; name?: string; description?: string }) =>
      backend.adhocSave({
        id: tab.adhocId ?? "",
        projectId: tab.projectId,
        name: name ?? tab.name,
        description: description ?? tab.description,
        method: tab.spec.method,
        url: tab.spec.url,
        params: tab.spec.params,
        body: tab.spec.body,
        auth: tab.spec.auth,
        createdAt: "",
        updatedAt: "",
      }),
    onSuccess: async (saved, { tab }) => {
      useTabsStore.getState().markSaved(tab.id, saved);
      await queryClient.invalidateQueries({ queryKey: queryKeys.adhoc(saved.projectId) });
    },
  });
}
