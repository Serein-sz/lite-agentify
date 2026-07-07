import { useEffect, useRef } from "react";
import * as echarts from "echarts";
import { useTheme } from "@/lib/theme";

/** 读取 <html> 上的 shadcn 设计变量当前值(随亮/暗主题变化)。 */
function cssVar(name: string): string {
  return getComputedStyle(document.documentElement)
    .getPropertyValue(name)
    .trim();
}

/**
 * 用 shadcn/ui 的设计 token 构建 ECharts 主题,而非内置 dark 皮肤:背景透明
 * 交给外层卡片,坐标轴/文字/网格线走 muted、border,系列走 --chart-1..5,
 * 于是图表在亮暗两态都与整体设计语言一致。
 */
function buildTheme() {
  const foreground = cssVar("--foreground");
  const muted = cssVar("--muted-foreground");
  const border = cssVar("--border");
  const axis = {
    axisLine: { lineStyle: { color: border } },
    axisTick: { lineStyle: { color: border } },
    axisLabel: { color: muted },
    splitLine: { lineStyle: { color: border } },
    nameTextStyle: { color: muted },
  };
  return {
    color: [
      cssVar("--chart-1"),
      cssVar("--chart-2"),
      cssVar("--chart-3"),
      cssVar("--chart-4"),
      cssVar("--chart-5"),
    ],
    backgroundColor: "transparent",
    textStyle: { color: foreground },
    categoryAxis: axis,
    valueAxis: axis,
    legend: { textStyle: { color: muted } },
    tooltip: {
      backgroundColor: cssVar("--popover"),
      borderColor: border,
      textStyle: { color: cssVar("--popover-foreground") },
    },
  };
}

const THEME_NAME = "shadcn";

/** 轻量 ECharts 封装:初始化/销毁、随窗口缩放、option 全量替换。 */
export function EChart({
  option,
  height = 320,
}: {
  option: echarts.EChartsOption;
  height?: number;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);
  const [, resolved] = useTheme();

  // 主题变化时重建实例:CSS 变量已随 .dark 类切换,重新注册主题读到新值。
  useEffect(() => {
    if (!containerRef.current) {
      return;
    }
    echarts.registerTheme(THEME_NAME, buildTheme());
    const chart = echarts.init(containerRef.current, THEME_NAME);
    chartRef.current = chart;
    const onResize = () => chart.resize();
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      chart.dispose();
      chartRef.current = null;
    };
  }, [resolved]);

  useEffect(() => {
    chartRef.current?.setOption(option, true);
  }, [option, resolved]);

  return <div ref={containerRef} style={{ height }} />;
}
