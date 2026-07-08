import React from "react";
import ReactDOM from "react-dom/client";
import { BrowserRouter } from "react-router";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MotionConfig } from "motion/react";
import App from "./App";
import "./index.css";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { retry: 1, refetchOnWindowFocus: false, staleTime: 15_000 },
  },
});

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <BrowserRouter basename="/admin">
        <MotionConfig reducedMotion="user">
          <App />
        </MotionConfig>
      </BrowserRouter>
    </QueryClientProvider>
  </React.StrictMode>,
);
