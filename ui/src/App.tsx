import { NavLink, Outlet, Route, Routes, useNavigate } from "react-router";
import { useMutation } from "@tanstack/react-query";
import { Waypoints } from "lucide-react";
import { api } from "./api";
import { Button } from "@/components/ui/button";
import { ThemeToggle } from "@/components/ThemeToggle";
import { cn } from "@/lib/utils";
import LoginPage from "./pages/LoginPage";
import DashboardPage from "./pages/DashboardPage";
import ConfigPage from "./pages/ConfigPage";

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

function Layout() {
  const navigate = useNavigate();
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
            <TabLink to="/config" label="配置" />
          </nav>
          <div className="ml-auto flex items-center gap-1">
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

export default function App() {
  return (
    <Routes>
      <Route path="/login" element={<LoginPage />} />
      <Route element={<Layout />}>
        <Route index element={<DashboardPage />} />
        <Route path="config" element={<ConfigPage />} />
      </Route>
    </Routes>
  );
}
