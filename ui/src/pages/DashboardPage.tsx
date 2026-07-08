import { useMemo, useState } from "react";
import { useQuery, keepPreviousData } from "@tanstack/react-query";
import {
  createColumnHelper,
  flexRender,
  getCoreRowModel,
  useReactTable,
} from "@tanstack/react-table";
import type { EChartsOption } from "echarts";
import { api, type UsageRow } from "@/api";
import { EChart } from "@/components/EChart";
import { BlurFade } from "@/components/magicui/blur-fade";
import { NumberTicker } from "@/components/magicui/number-ticker";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import {
  costAmount,
  formatBucket,
  formatCost,
  formatDateTime,
  formatLatency,
  formatNumber,
  formatPercent,
  formatTokens,
} from "@/lib/format";

const RANGES = [
  { key: "24h", label: "近 24 小时", hours: 24, bucket: "hour" as const },
  { key: "7d", label: "近 7 天", hours: 24 * 7, bucket: "day" as const },
  { key: "30d", label: "近 30 天", hours: 24 * 30, bucket: "day" as const },
];

const STATUS_OPTIONS = [
  { value: "all", label: "全部状态" },
  { value: "4xx", label: "4xx" },
  { value: "5xx", label: "5xx" },
];

const PAGE_SIZE = 20;

function StatCard({
  title,
  value,
  format,
  sub,
}: {
  title: string;
  /** 数值走 NumberTicker 滚动;无法压成单一数值时传字符串静态展示。 */
  value: number | string;
  format?: (value: number) => string;
  sub?: string;
}) {
  return (
    <Card size="sm">
      <CardHeader>
        <CardDescription>{title}</CardDescription>
        <CardTitle className="truncate text-lg tabular-nums">
          {typeof value === "number" ? (
            <NumberTicker value={value} format={format} />
          ) : (
            value
          )}
        </CardTitle>
        {sub && (
          <CardDescription className="text-xs tabular-nums">
            {sub}
          </CardDescription>
        )}
      </CardHeader>
    </Card>
  );
}

function StatusBadge({ status }: { status: number }) {
  if (status >= 500) {
    return <Badge variant="destructive">{status}</Badge>;
  }
  if (status >= 400) {
    return (
      <Badge variant="outline" className="border-amber-300 text-amber-700 dark:border-amber-500/40 dark:text-amber-400">
        {status}
      </Badge>
    );
  }
  return <Badge variant="secondary">{status}</Badge>;
}

const columnHelper = createColumnHelper<UsageRow>();

const logColumns = [
  columnHelper.accessor("created_at", {
    header: "时间",
    cell: (info) => (
      <span className="whitespace-nowrap text-muted-foreground tabular-nums">
        {formatDateTime(info.getValue())}
      </span>
    ),
  }),
  columnHelper.accessor("provider_id", { header: "Provider" }),
  columnHelper.accessor(
    (row) => row.upstream_model ?? row.requested_model ?? "—",
    { id: "model", header: "模型" },
  ),
  columnHelper.accessor("status", {
    header: "状态",
    cell: (info) => <StatusBadge status={info.getValue()} />,
  }),
  columnHelper.accessor("total_tokens", {
    header: "Tokens",
    cell: (info) => {
      const value = info.getValue();
      return value === null ? (
        "—"
      ) : (
        <span className="tabular-nums">{formatTokens(value)}</span>
      );
    },
  }),
  columnHelper.accessor(
    (row) =>
      row.estimated_cost === null
        ? "—"
        : `${Number(row.estimated_cost).toLocaleString("zh-CN", { maximumFractionDigits: 4 })} ${row.currency ?? ""}`,
    {
      id: "cost",
      header: "成本",
      cell: (info) => <span className="tabular-nums">{info.getValue()}</span>,
    },
  ),
  columnHelper.accessor("latency_ms", {
    header: "延迟",
    cell: (info) => (
      <span className="tabular-nums">{formatLatency(info.getValue())}</span>
    ),
  }),
];

export default function DashboardPage() {
  const [rangeKey, setRangeKey] = useState("7d");
  const range = RANGES.find((entry) => entry.key === rangeKey) ?? RANGES[1];

  const [page, setPage] = useState(1);
  const [providerInput, setProviderInput] = useState("");
  const [providerFilter, setProviderFilter] = useState("");
  const [statusFilter, setStatusFilter] = useState("all");

  // from 只随范围切换变化,避免每次渲染生成新时间导致无限重取。
  const from = useMemo(
    () => new Date(Date.now() - range.hours * 3_600_000).toISOString(),
    [range.hours],
  );

  const summaryQuery = useQuery({
    queryKey: ["usage-summary", from, range.bucket],
    // 切范围时保留旧数据:页面不闪加载态,数字从旧值滚向新值。
    placeholderData: keepPreviousData,
    queryFn: () =>
      api.usageSummary(new URLSearchParams({ from, bucket: range.bucket })),
  });

  const listQuery = useQuery({
    queryKey: ["usage-list", from, page, providerFilter, statusFilter],
    placeholderData: keepPreviousData,
    queryFn: () => {
      const params = new URLSearchParams({
        from,
        page: String(page),
        page_size: String(PAGE_SIZE),
      });
      if (providerFilter) params.set("provider", providerFilter);
      if (statusFilter !== "all") params.set("status", statusFilter);
      return api.usageList(params);
    },
  });

  const rows = listQuery.data?.rows ?? [];
  const total = listQuery.data?.total ?? 0;
  const pageCount = Math.max(1, Math.ceil(total / PAGE_SIZE));

  const table = useReactTable({
    data: rows,
    columns: logColumns,
    getCoreRowModel: getCoreRowModel(),
    manualPagination: true,
    pageCount,
  });

  const summary = summaryQuery.data;
  const primaryCurrency = summary?.totals.cost[0]?.currency;

  const chartOption = useMemo<EChartsOption>(() => {
    const series = summary?.series ?? [];
    return {
      tooltip: {
        trigger: "axis",
        formatter: (params) => {
          const items = Array.isArray(params) ? params : [params];
          const title = items[0]?.name ?? "";
          const lines = items.map((item) => {
            const value = item.value as number;
            const text =
              item.seriesName === "Tokens"
                ? formatTokens(value)
                : value.toLocaleString("zh-CN", { maximumFractionDigits: 4 });
            return `${item.marker}${item.seriesName} ${text}`;
          });
          return [title, ...lines].join("<br/>");
        },
      },
      legend: { data: ["Tokens", "成本"] },
      grid: { left: 64, right: 64, top: 40, bottom: 32 },
      xAxis: {
        type: "category",
        data: series.map((point) => formatBucket(point.bucket_start, range.bucket)),
      },
      yAxis: [
        {
          type: "value",
          name: "Tokens",
          axisLabel: { formatter: (value: number) => formatTokens(value) },
        },
        { type: "value", name: primaryCurrency ?? "成本" },
      ],
      // 颜色留空:由 EChart 的 shadcn 主题调色板(--chart-1..5)统一着色。
      series: [
        {
          name: "Tokens",
          type: "bar",
          data: series.map((point) => point.total_tokens),
        },
        {
          name: "成本",
          type: "line",
          yAxisIndex: 1,
          smooth: true,
          // 主指标线加粗:无彩体系里层级靠明度与笔重。
          lineStyle: { width: 3 },
          data: series.map((point) => costAmount(point.cost, primaryCurrency)),
        },
      ],
    };
  }, [summary, range.bucket, primaryCurrency]);

  if (summaryQuery.isPending) {
    return <p className="text-xs text-muted-foreground">加载用量数据中…</p>;
  }
  if (summaryQuery.isError) {
    return (
      <p className="text-xs text-destructive">
        用量数据加载失败:
        {summaryQuery.error instanceof Error
          ? summaryQuery.error.message
          : String(summaryQuery.error)}
      </p>
    );
  }

  if (summary && !summary.usage_enabled) {
    return (
      <Card>
        <CardContent className="py-16 text-center">
          <p className="text-base font-medium">未启用用量记录</p>
          <p className="mt-2 text-xs text-muted-foreground">
            在配置文件的 [usage_database] 中配置 PostgreSQL 并重启网关后,
            这里会展示请求、token 与成本统计。
          </p>
        </CardContent>
      </Card>
    );
  }

  const totals = summary!.totals;
  // 成本能压成单一主币种数值时才滚动;混合币种退化为静态文本(正确优先于动效)。
  const costValue: number | string =
    totals.cost.length === 1
      ? costAmount(totals.cost, primaryCurrency)
      : formatCost(totals.cost);

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-2">
        <h1 className="text-base font-semibold">用量仪表盘</h1>
        <div className="ml-auto flex gap-1">
          {RANGES.map((entry) => (
            <Button
              key={entry.key}
              size="sm"
              variant={entry.key === rangeKey ? "default" : "ghost"}
              onClick={() => {
                setRangeKey(entry.key);
                setPage(1);
              }}
            >
              {entry.label}
            </Button>
          ))}
        </div>
      </div>

      <BlurFade className="grid grid-cols-2 gap-3 md:grid-cols-5">
        <StatCard
          title="请求数"
          value={totals.requests}
          format={(count) => formatNumber(Math.round(count))}
        />
        <StatCard
          title="Tokens"
          value={totals.total_tokens}
          format={formatTokens}
          sub={`输入 ${formatTokens(totals.input_tokens)} / 输出 ${formatTokens(totals.output_tokens)}`}
        />
        <StatCard
          title="成本"
          value={costValue}
          format={(amount) =>
            `${amount.toLocaleString("zh-CN", { maximumFractionDigits: 4 })} ${primaryCurrency ?? ""}`
          }
        />
        <StatCard
          title="平均延迟"
          value={totals.avg_latency_ms}
          format={formatLatency}
        />
        <StatCard
          title="错误率"
          value={totals.error_rate}
          format={formatPercent}
        />
      </BlurFade>

      <div className="grid gap-4 lg:grid-cols-3">
        <BlurFade className="lg:col-span-2" delay={0.06}>
          <Card className="h-full">
            <CardHeader>
              <CardTitle>用量趋势</CardTitle>
            </CardHeader>
            <CardContent>
              <EChart option={chartOption} height={300} />
            </CardContent>
          </Card>
        </BlurFade>
        <BlurFade delay={0.12}>
          <Card className="h-full">
            <CardHeader>
              <CardTitle>Provider × 模型</CardTitle>
            </CardHeader>
            <CardContent>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Provider / 模型</TableHead>
                    <TableHead className="text-right">请求</TableHead>
                    <TableHead className="text-right">成本</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {summary!.breakdown
                    .slice()
                    .sort(
                      (a, b) =>
                        costAmount(b.cost, primaryCurrency) -
                        costAmount(a.cost, primaryCurrency),
                    )
                    .map((row) => (
                      <TableRow key={`${row.provider_id}:${row.model ?? ""}`}>
                        <TableCell>
                          <span className="font-medium">{row.provider_id}</span>
                          <span className="text-muted-foreground">
                            {" "}
                            / {row.model ?? "—"}
                          </span>
                        </TableCell>
                        <TableCell className="text-right tabular-nums">
                          {formatNumber(row.requests)}
                        </TableCell>
                        <TableCell className="text-right tabular-nums">
                          {formatCost(row.cost)}
                        </TableCell>
                      </TableRow>
                    ))}
                  {summary!.breakdown.length === 0 && (
                    <TableRow>
                      <TableCell
                        colSpan={3}
                        className="py-6 text-center text-muted-foreground"
                      >
                        该时间段暂无数据
                      </TableCell>
                    </TableRow>
                  )}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
        </BlurFade>
      </div>

      <BlurFade delay={0.18}>
        <Card>
          <CardHeader>
            <CardTitle>请求明细</CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            <div className="flex flex-wrap items-center gap-2">
              <div className="ml-auto flex items-center gap-2">
                <Input
                  value={providerInput}
                  onChange={(event) => setProviderInput(event.target.value)}
                  onBlur={() => {
                    setProviderFilter(providerInput.trim());
                    setPage(1);
                  }}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") {
                      setProviderFilter(providerInput.trim());
                      setPage(1);
                    }
                  }}
                  placeholder="按 provider 筛选"
                  className="w-44"
                />
                <Select
                  value={statusFilter}
                  onValueChange={(value) => {
                    setStatusFilter(String(value));
                    setPage(1);
                  }}
                >
                  <SelectTrigger className="w-28">
                    <SelectValue>
                      {
                        STATUS_OPTIONS.find(
                          (option) => option.value === statusFilter,
                        )?.label
                      }
                    </SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    {STATUS_OPTIONS.map((option) => (
                      <SelectItem key={option.value} value={option.value}>
                        {option.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            </div>

            <Table>
              <TableHeader>
                {table.getHeaderGroups().map((headerGroup) => (
                  <TableRow key={headerGroup.id}>
                    {headerGroup.headers.map((header) => (
                      <TableHead key={header.id}>
                        {flexRender(
                          header.column.columnDef.header,
                          header.getContext(),
                        )}
                      </TableHead>
                    ))}
                  </TableRow>
                ))}
              </TableHeader>
              <TableBody>
                {table.getRowModel().rows.map((row) => (
                  <TableRow key={row.id}>
                    {row.getVisibleCells().map((cell) => (
                      <TableCell key={cell.id}>
                        {flexRender(cell.column.columnDef.cell, cell.getContext())}
                      </TableCell>
                    ))}
                  </TableRow>
                ))}
                {rows.length === 0 && (
                  <TableRow>
                    <TableCell
                      colSpan={logColumns.length}
                      className="py-8 text-center text-muted-foreground"
                    >
                      {listQuery.isPending ? "加载中…" : "该条件下暂无请求记录"}
                    </TableCell>
                  </TableRow>
                )}
              </TableBody>
            </Table>

            <div className="flex items-center gap-3 text-xs text-muted-foreground">
              <span className="tabular-nums">
                共 {formatNumber(total)} 条 · 第 {page} / {pageCount} 页
              </span>
              <div className="ml-auto flex gap-1">
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => setPage((current) => Math.max(1, current - 1))}
                  disabled={page <= 1}
                >
                  上一页
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() =>
                    setPage((current) => Math.min(pageCount, current + 1))
                  }
                  disabled={page >= pageCount}
                >
                  下一页
                </Button>
              </div>
            </div>
          </CardContent>
        </Card>
      </BlurFade>
    </div>
  );
}
