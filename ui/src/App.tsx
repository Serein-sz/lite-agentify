import { NavLink, Navigate, Outlet, Route, Routes, useNavigate } from "react-router";
import { useMutation, useQuery } from "@tanstack/react-query";
import { Waypoints } from "lucide-react";
import { api, type Me } from "./api";
import { Button } from "@/components/ui/button";
import { ThemeToggle } from "@/components/ThemeToggle";
import { cn } from "@/lib/utils";
import LoginPage from "./pages/LoginPage";
import DashboardPage from "./pages/DashboardPage";
import KeysPage from "./pages/KeysPage";
import UsersPage from "./pages/UsersPage";
import PasswordPage from "./pages/PasswordPage";
import ProvidersPage from "./pages/ProvidersPage";
import PricingPage from "./pages/PricingPage";
import ModelsPage from "./pages/ModelsPage";
import CreditsPage from "./pages/CreditsPage";

function TabLink({ to, label }: { to: string; label: string }) {
  return (
    <NavLink
      to={to}
      end={to === "/"}
      className={({ isActive }) =>
        cn(
          "inline-flex h-7 items-center rounded-full px-3 text-xs font-medium transition-colors",
          isActive
            ? "bg-primary text-primary-foreground"
            : "text-muted-foreground hover:bg-muted hover:text-foreground",
        )
      }
    >
      {label}
    </NavLink>
  );
}

function Layout({ me }: { me: Me }) {
  const navigate = useNavigate();
  const isAdmin = me.role === "admin";
  const logout = useMutation({
    mutationFn: api.logout,
    onSettled: () => navigate("/login"),
  });

  return (
    <div className="min-h-screen bg-background text-foreground">
      <header className="sticky top-0 z-40 border-b border-border bg-card/80 backdrop-blur">
        <div className="mx-auto flex max-w-6xl items-center gap-6 px-6 py-3">
          <span className="flex items-center gap-2 text-sm font-semibold tracking-tight">
            <span className="flex size-6 items-center justify-center rounded-md bg-primary text-primary-foreground">
              <Waypoints className="size-3.5" />
            </span>
            lite-agentify
          </span>
          <nav className="flex gap-1">
            <TabLink to="/" label="仪表盘" />
            <TabLink to="/keys" label="密钥" />
            {isAdmin && <TabLink to="/users" label="用户" />}
            {isAdmin && <TabLink to="/credits" label="额度" />}
            {isAdmin && <TabLink to="/models" label="模型" />}
            {isAdmin && <TabLink to="/providers" label="Provider" />}
            {isAdmin && <TabLink to="/pricing" label="定价" />}
            <TabLink to="/password" label="修改密码" />
          </nav>
          <div className="ml-auto flex items-center gap-3">
            <span className="text-xs text-muted-foreground">
              {me.username}
              <span className="ml-1 rounded bg-muted px-1.5 py-0.5">
                {isAdmin ? "管理员" : "用户"}
              </span>
            </span>
            <ThemeToggle />
            <Button
              variant="ghost"
              size="sm"
              className="text-muted-foreground"
              onClick={() => logout.mutate()}
            >
              退出登录
            </Button>
          </div>
        </div>
      </header>
      <main className="mx-auto max-w-6xl px-6 py-6">
        <Outlet />
      </main>
    </div>
  );
}

/** Loads the current session identity; unauthenticated users go to login. */
function AuthedApp() {
  const navigate = useNavigate();
  const me = useQuery({ queryKey: ["me"], queryFn: api.me, retry: false });

  if (me.isLoading) {
    return (
      <div className="flex min-h-screen items-center justify-center text-sm text-muted-foreground">
        加载中…
      </div>
    );
  }
  if (me.isError || !me.data) {
    navigate("/login", { replace: true });
    return null;
  }

  const isAdmin = me.data.role === "admin";
  return (
    <Routes>
      <Route element={<Layout me={me.data} />}>
        <Route index element={<DashboardPage role={me.data.role} />} />
        <Route path="keys" element={<KeysPage role={me.data.role} />} />
        <Route path="password" element={<PasswordPage />} />
        <Route
          path="users"
          element={isAdmin ? <UsersPage /> : <Navigate to="/" replace />}
        />
        <Route
          path="credits"
          element={isAdmin ? <CreditsPage /> : <Navigate to="/" replace />}
        />
        <Route
          path="models"
          element={isAdmin ? <ModelsPage /> : <Navigate to="/" replace />}
        />
        <Route
          path="providers"
          element={isAdmin ? <ProvidersPage /> : <Navigate to="/" replace />}
        />
        <Route
          path="pricing"
          element={isAdmin ? <PricingPage /> : <Navigate to="/" replace />}
        />
      </Route>
    </Routes>
  );
}

export default function App() {
  return (
    <Routes>
      <Route path="/login" element={<LoginPage />} />
      <Route path="/*" element={<AuthedApp />} />
    </Routes>
  );
}
