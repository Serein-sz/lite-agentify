import { useSyncExternalStore } from "react";

/** 用户选择的主题偏好。"system" 跟随操作系统。 */
export type ThemePreference = "light" | "dark" | "system";
/** 实际生效的主题(system 已解析)。 */
export type ResolvedTheme = "light" | "dark";

const STORAGE_KEY = "lite-agentify-theme";

const media = window.matchMedia("(prefers-color-scheme: dark)");

function readPreference(): ThemePreference {
  const stored = localStorage.getItem(STORAGE_KEY);
  return stored === "light" || stored === "dark" || stored === "system"
    ? stored
    : "system";
}

export function resolveTheme(preference: ThemePreference): ResolvedTheme {
  if (preference === "system") {
    return media.matches ? "dark" : "light";
  }
  return preference;
}

/** 把解析后的主题应用到 <html>,与防闪白的内联脚本保持一致。 */
export function applyTheme(resolved: ResolvedTheme) {
  const root = document.documentElement;
  root.classList.toggle("dark", resolved === "dark");
  root.style.colorScheme = resolved;
}

// --- useSyncExternalStore 订阅:localStorage 变更 + 系统偏好变更 ---

const listeners = new Set<() => void>();

function emit() {
  const resolved = resolveTheme(readPreference());
  applyTheme(resolved);
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  // 系统偏好切换时,若当前是 "system" 需要重新解析。
  media.addEventListener("change", emit);
  // 其他标签页改了偏好也同步过来。
  window.addEventListener("storage", emit);
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) {
      media.removeEventListener("change", emit);
      window.removeEventListener("storage", emit);
    }
  };
}

export function setThemePreference(preference: ThemePreference) {
  localStorage.setItem(STORAGE_KEY, preference);
  emit();
}

/** 返回 [偏好, 解析后的主题],随偏好或系统变化重渲染。 */
export function useTheme(): [ThemePreference, ResolvedTheme] {
  const preference = useSyncExternalStore(subscribe, readPreference, () => "system" as const);
  return [preference, resolveTheme(preference)];
}
