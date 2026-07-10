import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type PricingRecord, type ProviderRecord } from "@/api";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
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

interface Draft {
  id: string | null;
  provider: string;
  model: string;
  input_per_1m: string;
  output_per_1m: string;
  cached_input_per_1m: string;
  cache_read_per_1m: string;
  cache_write_per_1m: string;
  currency: string;
  pricing_source: string;
}

function emptyDraft(): Draft {
  return {
    id: null,
    provider: "*",
    model: "*",
    input_per_1m: "",
    output_per_1m: "",
    cached_input_per_1m: "",
    cache_read_per_1m: "",
    cache_write_per_1m: "",
    currency: "USD",
    pricing_source: "",
  };
}

function draftFrom(record: PricingRecord): Draft {
  return {
    id: record.id,
    provider: record.provider,
    model: record.model,
    input_per_1m: record.input_per_1m,
    output_per_1m: record.output_per_1m,
    cached_input_per_1m: record.cached_input_per_1m ?? "",
    cache_read_per_1m: record.cache_read_per_1m ?? "",
    cache_write_per_1m: record.cache_write_per_1m ?? "",
    currency: record.currency,
    pricing_source: record.pricing_source ?? "",
  };
}

function optional(value: string): string | null {
  return value.trim() === "" ? null : value.trim();
}

export default function PricingPage() {
  const queryClient = useQueryClient();
  const pricingQuery = useQuery({ queryKey: ["pricing"], queryFn: api.listPricing });
  const providersQuery = useQuery({ queryKey: ["providers"], queryFn: api.listProviders });
  const [draft, setDraft] = useState<Draft | null>(null);
  const [error, setError] = useState<string | null>(null);

  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["pricing"] });

  const save = useMutation({
    mutationFn: async () => {
      const d = draft!;
      const payload = {
        provider: d.provider.trim(),
        model: d.model.trim(),
        input_per_1m: d.input_per_1m,
        output_per_1m: d.output_per_1m,
        cached_input_per_1m: optional(d.cached_input_per_1m),
        cache_read_per_1m: optional(d.cache_read_per_1m),
        cache_write_per_1m: optional(d.cache_write_per_1m),
        currency: d.currency.trim(),
        pricing_source: optional(d.pricing_source),
      };
      if (d.id) {
        await api.updatePricing(d.id, payload);
      } else {
        await api.createPricing(payload);
      }
    },
    onSuccess: () => {
      setDraft(null);
      setError(null);
      invalidate();
    },
    onError: (cause) => setError(cause instanceof Error ? cause.message : "保存失败"),
  });

  const remove = useMutation({
    mutationFn: (id: string) => api.deletePricing(id),
    onSuccess: invalidate,
  });

  const rows = pricingQuery.data?.pricing ?? [];
  const providerOptions = ["*", ...(providersQuery.data?.providers ?? []).map((p: ProviderRecord) => p.id)];

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-2">
        <h1 className="text-base font-semibold">定价管理</h1>
        <Button className="ml-auto" onClick={() => setDraft(emptyDraft())}>
          新增规则
        </Button>
      </div>

      {draft && (
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">{draft.id ? "编辑规则" : "新增规则"}</CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            <div className="flex flex-wrap gap-2">
              <Select
                value={draft.provider}
                onValueChange={(value) => setDraft({ ...draft, provider: String(value) })}
              >
                <SelectTrigger className="w-40">
                  <SelectValue>{draft.provider}</SelectValue>
                </SelectTrigger>
                <SelectContent>
                  {providerOptions.map((id) => (
                    <SelectItem key={id} value={id}>
                      {id}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <Input
                value={draft.model}
                onChange={(event) => setDraft({ ...draft, model: event.target.value })}
                placeholder="模型(或 *)"
                className="w-48"
              />
              <Input
                value={draft.currency}
                onChange={(event) => setDraft({ ...draft, currency: event.target.value })}
                placeholder="USD"
                className="w-24"
              />
            </div>
            <div className="flex flex-wrap gap-2">
              <Input
                value={draft.input_per_1m}
                onChange={(event) => setDraft({ ...draft, input_per_1m: event.target.value })}
                placeholder="input / 1M"
                className="w-32"
              />
              <Input
                value={draft.output_per_1m}
                onChange={(event) => setDraft({ ...draft, output_per_1m: event.target.value })}
                placeholder="output / 1M"
                className="w-32"
              />
              <Input
                value={draft.cached_input_per_1m}
                onChange={(event) => setDraft({ ...draft, cached_input_per_1m: event.target.value })}
                placeholder="cached input(可选)"
                className="w-40"
              />
              <Input
                value={draft.cache_read_per_1m}
                onChange={(event) => setDraft({ ...draft, cache_read_per_1m: event.target.value })}
                placeholder="cache read(可选)"
                className="w-36"
              />
              <Input
                value={draft.cache_write_per_1m}
                onChange={(event) => setDraft({ ...draft, cache_write_per_1m: event.target.value })}
                placeholder="cache write(可选)"
                className="w-36"
              />
            </div>
            {error && <p className="text-xs text-destructive">{error}</p>}
            <div className="flex gap-2">
              <Button onClick={() => save.mutate()} disabled={save.isPending}>
                {save.isPending ? "保存中…" : "保存"}
              </Button>
              <Button variant="ghost" onClick={() => setDraft(null)}>
                取消
              </Button>
            </div>
          </CardContent>
        </Card>
      )}

      <Card>
        <CardHeader>
          <CardTitle className="text-sm">定价规则</CardTitle>
        </CardHeader>
        <CardContent>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Provider</TableHead>
                <TableHead>模型</TableHead>
                <TableHead className="text-right">Input/1M</TableHead>
                <TableHead className="text-right">Output/1M</TableHead>
                <TableHead>币种</TableHead>
                <TableHead />
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((rule: PricingRecord) => (
                <TableRow key={rule.id}>
                  <TableCell className="font-medium">{rule.provider}</TableCell>
                  <TableCell>{rule.model}</TableCell>
                  <TableCell className="text-right tabular-nums">{rule.input_per_1m}</TableCell>
                  <TableCell className="text-right tabular-nums">{rule.output_per_1m}</TableCell>
                  <TableCell>{rule.currency}</TableCell>
                  <TableCell className="space-x-1 text-right">
                    <Button variant="ghost" size="sm" onClick={() => setDraft(draftFrom(rule))}>
                      编辑
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      className="text-destructive"
                      onClick={() => {
                        if (confirm(`确认删除 ${rule.provider}:${rule.model} 的定价?`)) {
                          remove.mutate(rule.id);
                        }
                      }}
                    >
                      删除
                    </Button>
                  </TableCell>
                </TableRow>
              ))}
              {rows.length === 0 && (
                <TableRow>
                  <TableCell colSpan={6} className="py-8 text-center text-muted-foreground">
                    {pricingQuery.isPending ? "加载中…" : "暂无定价规则"}
                  </TableCell>
                </TableRow>
              )}
            </TableBody>
          </Table>
        </CardContent>
      </Card>
    </div>
  );
}
