/**
 * M13: App shell router — Auth → Dashboard → Editor.
 *
 * When a backend is available (/api/me returns 200 or 401),
 * the auth flow gates access. When no backend is detected
 * (fetch fails entirely), the app runs in offline guest mode
 * with the existing localStorage persistence.
 */

import { useEffect, useState } from "react";
import { App } from "./App";
import { AuthGate } from "./AuthView";
import { Dashboard } from "./Dashboard";

type Route =
  | { view: "detecting" }
  | { view: "offline" }
  | { view: "auth" }
  | { view: "dashboard" }
  | { view: "editor"; projectId?: string; initialDoc?: string };

export function AppShell() {
  // In dev mode without a backend proxy, skip the probe and go
  // straight to offline mode. This keeps Playwright tests fast.
  const skipProbe =
    typeof window !== "undefined" &&
    (window.location.port === "5173" || import.meta.env?.DEV) &&
    !import.meta.env?.VITE_API_PROXY;

  const [route, setRoute] = useState<Route>(
    skipProbe ? { view: "offline" } : { view: "detecting" },
  );

  // On mount: probe the backend with a short timeout. If it
  // responds (200 or 401), show the auth gate. If the fetch
  // fails or times out (no server), go to offline mode.
  useEffect(() => {
    if (skipProbe) return;
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 1000);
    fetch("/api/me", {
      credentials: "include",
      signal: controller.signal,
    })
      .then((res) => {
        clearTimeout(timer);
        if (res.ok) {
          setRoute({ view: "dashboard" });
        } else {
          setRoute({ view: "auth" });
        }
      })
      .catch(() => {
        clearTimeout(timer);
        setRoute({ view: "offline" });
      });
    return () => {
      controller.abort();
      clearTimeout(timer);
    };
  }, []);

  switch (route.view) {
    case "detecting":
      return <div className="fd-auth-loading">Connecting…</div>;

    case "offline":
      // Direct to editor with localStorage persistence.
      return <App />;

    case "auth":
      return (
        <AuthGate>
          <Dashboard
            onOpenProject={(id, doc) =>
              setRoute({ view: "editor", projectId: id, initialDoc: doc })
            }
          />
        </AuthGate>
      );

    case "dashboard":
      return (
        <AuthGate>
          <Dashboard
            onOpenProject={(id, doc) =>
              setRoute({ view: "editor", projectId: id, initialDoc: doc })
            }
          />
        </AuthGate>
      );

    case "editor":
      return <App />;
  }
}
