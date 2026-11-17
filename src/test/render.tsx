import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import App from "../App";
import { BackendProvider } from "../api";
import type { Backend } from "../api/backend";
import { createMockBackend } from "../mock/backend";
import { useTabsStore } from "../store/tabs";
import { useUiStore } from "../store/ui";

/** モックのバックエンドで App を描画する。 */
export function renderApp(backend: Backend = createMockBackend()) {
  try {
    localStorage.clear();
  } catch {
    /* 使えなくてもよい */
  }
  useUiStore.setState({ currentProjectId: null, createDialogOpen: false, screen: "main", newRequestDialogOpen: false });
  useTabsStore.getState().reset();
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const user = userEvent.setup();
  const view = render(
    <BackendProvider backend={backend}>
      <QueryClientProvider client={queryClient}>
        <App />
      </QueryClientProvider>
    </BackendProvider>,
  );
  return { user, backend, ...view };
}
