import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy } from "lucide-react";
import { ApiError, api, type ProviderRecord } from "@/api";
import { Badge } from "@/components/ui/badge";
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

const MASK_PREFIX = "__MASKED__";

interface Draft {
  id: string;
  protocol: string;
  base_url: string;
  api_key: string;
  anthropic_version: string;
  model_aliases: string;
  /** null for a new provider, the existing id for an edit. */
  editing: string | null;
}

function emptyDraft(): Draft {
  return {
    id: "",
    protocol: "openai",
    base_url: "",
    api_key: "",
    anthropic_version: "",
    model_aliases: "",
    editing: null,
  };
}

function draftFrom(provider: ProviderRecord): Draft {
  return {
    id: provider.id,
    protocol: provider.protocol,
    base_url: provider.base_url,
    api_key: provider.api_key,
    anthropic_version: provider.anthropic_version ?? "",
    model_aliases: Object.entries(provider.model_aliases)
      .map(([alias, target]) => `${alias}=${target}`)
      .join("\n"),
    editing: provider.id,
  };
}

function parseAliases(text: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const line of text.split("\n")) {
    const [alias, target] = line.split("=");
    if (alias?.trim() && target?.trim()) out[alias.trim()] = target.trim();
  }
  return out;
}

export default function ProvidersPage() {
  const queryClient = useQueryClient();
  const providersQuery = useQuery({ queryKey: ["providers"], queryFn: api.listProviders });
  const [draft, setDraft] = useState<Draft | null>(null);
  const [error, setError] = useState<string | null>(null);

  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["providers"] });

  const save = useMutation({
    mutationFn: async () => {
      const d = draft!;
      const payload = {
        protocol: d.protocol,
        base_url: d.base_url,
        api_key: d.api_key,
        anthropic_version: d.anthropic_version.trim() === "" ? null : d.anthropic_version,
        model_aliases: parseAliases(d.model_aliases),
      };
      if (d.editing) {
        await api.updateProvider(d.editing, payload);
      } else {
        await api.createProvider({ id: d.id.trim(), ...payload });
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
    mutationFn: (id: string) => api.deleteProvider(id),
    onSuccess: invalidate,
    onError: (cause) => {
      if (cause instanceof ApiError && cause.status === 409) {
        alert(cause.message);
      }
    },
  });

  const providers = providersQuery.data?.providers ?? [];

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-2">
        <h1 className="text-base font-semibold">Provider 管理</h1>
        <Button className="ml-auto" onClick={() => setDraft(emptyDraft())}>
          新增 Provider
        </Button>
      </div>

      {draft && (
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">
              {draft.editing ? `编辑 ${draft.editing}` : "新增 Provider"}
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            <div className="flex flex-wrap gap-2">
              <Input
                value={draft.id}
                disabled={draft.editing !== null}
                onChange={(event) => setDraft({ ...draft, id: event.target.value })}
                placeholder="provider id"
                className="w-40"
              />
              <Select
                value={draft.protocol}
                onValueChange={(value) => setDraft({ ...draft, protocol: String(value) })}
              >
                <SelectTrigger className="w-36">
                  <SelectValue>{draft.protocol}</SelectValue>
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="openai">openai</SelectItem>
                  <SelectItem value="anthropic">anthropic</SelectItem>
                </SelectContent>
              </Select>
              <Input
                value={draft.base_url}
                onChange={(event) => setDraft({ ...draft, base_url: event.target.value })}
                placeholder="https://api.openai.com"
                className="flex-1 min-w-[16rem]"
              />
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <Input
                value={draft.api_key}
                onChange={(event) => setDraft({ ...draft, api_key: event.target.value })}
                placeholder="上游 API key"
                className="flex-1 min-w-[16rem] font-mono"
              />
              {draft.editing && draft.api_key.startsWith(MASK_PREFIX) && (
                <Button
                  variant="outline"
                  size="sm"
                  onClick={async () => {
                    const { value } = await api.revealProviderKey(draft.editing!);
                    await navigator.clipboard.writeText(value);
                  }}
                >
                  <Copy className="size-3.5" />
                  复制明文
                </Button>
              )}
              <Input
                value={draft.anthropic_version}
                onChange={(event) => setDraft({ ...draft, anthropic_version: event.target.value })}
                placeholder="anthropic_version(可选)"
                className="w-52"
              />
            </div>
            <textarea
              value={draft.model_aliases}
              onChange={(event) => setDraft({ ...draft, model_aliases: event.target.value })}
              placeholder={"模型别名,每行 alias=upstream_model"}
              rows={3}
              className="w-full rounded-md border border-input bg-transparent px-3 py-2 text-xs font-mono"
            />
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
          <CardTitle className="text-sm">Provider 列表</CardTitle>
        </CardHeader>
        <CardContent>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>ID</TableHead>
                <TableHead>协议</TableHead>
                <TableHead>Base URL</TableHead>
                <TableHead>别名</TableHead>
                <TableHead />
              </TableRow>
            </TableHeader>
            <TableBody>
              {providers.map((provider: ProviderRecord) => (
                <TableRow key={provider.id}>
                  <TableCell className="font-medium">{provider.id}</TableCell>
                  <TableCell>
                    <Badge variant="secondary">{provider.protocol}</Badge>
                  </TableCell>
                  <TableCell className="text-muted-foreground">{provider.base_url}</TableCell>
                  <TableCell className="tabular-nums">
                    {Object.keys(provider.model_aliases).length}
                  </TableCell>
                  <TableCell className="space-x-1 text-right">
                    <Button variant="ghost" size="sm" onClick={() => setDraft(draftFrom(provider))}>
                      编辑
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      className="text-destructive"
                      onClick={() => {
                        if (confirm(`确认删除 provider「${provider.id}」?`)) {
                          remove.mutate(provider.id);
                        }
                      }}
                    >
                      删除
                    </Button>
                  </TableCell>
                </TableRow>
              ))}
              {providers.length === 0 && (
                <TableRow>
                  <TableCell colSpan={5} className="py-8 text-center text-muted-foreground">
                    {providersQuery.isPending ? "加载中…" : "暂无 provider"}
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
