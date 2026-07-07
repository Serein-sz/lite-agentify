import type { CostEntry } from "@/api";

export function formatNumber(value: number): string {
  return new Intl.NumberFormat("zh-CN", {
    notation: value >= 100_000 ? "compact" : "standard",
    maximumFractionDigits: 1,
  }).format(value);
}

/** Token 数以 K/M 为单位:< 1K 原样,>= 1K 用 K,>= 1M 用 M,保留一位小数并去掉多余的 .0。 */
export function formatTokens(value: number): string {
  const scaled =
    value >= 1_000_000
      ? { n: value / 1_000_000, unit: "M" }
      : value >= 1_000
        ? { n: value / 1_000, unit: "K" }
        : { n: value, unit: "" };
  const text = scaled.n
    .toFixed(scaled.unit ? 1 : 0)
    .replace(/\.0$/, "");
  return `${text}${scaled.unit}`;
}

export function formatCost(cost: CostEntry[]): string {
  if (cost.length === 0) {
    return "—";
  }
  return cost
    .map((entry) => {
      const amount = Number(entry.amount).toLocaleString("zh-CN", {
        maximumFractionDigits: 4,
      });
      return `${amount} ${entry.currency}`;
    })
    .join(" + ");
}

/** 图表用:只累加主币种(汇总里排第一的币种)的金额。 */
export function costAmount(cost: CostEntry[], currency: string | undefined): number {
  return cost
    .filter((entry) => currency === undefined || entry.currency === currency)
    .reduce((sum, entry) => sum + Number(entry.amount), 0);
}

export function formatLatency(ms: number): string {
  return ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(2)} s`;
}

export function formatPercent(value: number): string {
  return `${(value * 100).toFixed(1)}%`;
}

export function formatDateTime(iso: string): string {
  return new Date(iso).toLocaleString("zh-CN", { hour12: false });
}

export function formatBucket(iso: string, bucket: "hour" | "day"): string {
  const date = new Date(iso);
  if (bucket === "day") {
    return date.toLocaleDateString("zh-CN", { month: "2-digit", day: "2-digit" });
  }
  return date.toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    hour12: false,
  });
}
