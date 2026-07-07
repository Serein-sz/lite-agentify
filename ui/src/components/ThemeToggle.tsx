import { Moon, Sun, MonitorSmartphone } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  setThemePreference,
  useTheme,
  type ThemePreference,
} from "@/lib/theme";

/** 三态循环:跟随系统 → 亮色 → 暗色 → 跟随系统。 */
const ORDER: ThemePreference[] = ["system", "light", "dark"];

const LABELS: Record<ThemePreference, string> = {
  system: "跟随系统",
  light: "亮色",
  dark: "暗色",
};

export function ThemeToggle() {
  const [preference] = useTheme();

  const next = () => {
    const index = ORDER.indexOf(preference);
    setThemePreference(ORDER[(index + 1) % ORDER.length]);
  };

  return (
    <Button
      variant="ghost"
      size="icon-sm"
      onClick={next}
      aria-label={`主题:${LABELS[preference]},点击切换`}
      title={`主题:${LABELS[preference]}`}
    >
      {preference === "system" ? (
        <MonitorSmartphone />
      ) : preference === "dark" ? (
        <Moon />
      ) : (
        <Sun />
      )}
    </Button>
  );
}
