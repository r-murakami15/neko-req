import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import App from "./App";
import { BackendProvider, getDefaultBackend } from "./api";
import "./styles/fonts.css";
import "./styles/theme.css";
import "./styles/app.css";
import "./styles/request.css";

const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } },
});

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <BackendProvider backend={getDefaultBackend()}>
      <QueryClientProvider client={queryClient}>
        <App />
      </QueryClientProvider>
    </BackendProvider>
  </StrictMode>,
);
