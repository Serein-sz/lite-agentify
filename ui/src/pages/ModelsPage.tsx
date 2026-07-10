import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowDown, ArrowUp, Trash2 } from "lucide-react";
import { api, type ModelRecord, type ProviderRecord } from "@/api";
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

interface DeploymentDraft {
  provider_id: string;
  upstream_model: string;
}

interface Draft {
  /** null = creating a new model; otherwise the model being edited. */
  original: string | null;
  name: string;
  enabled: boolean;
  deployments: DeploymentDraft[];
}

function emptyDraft(): Draft {
  return { original: null, name: "", enabled: false, deployments: [] };
}

function draftFrom(model: ModelRecord): Draft {
  return {
    original: model.name,
    name: model.name,
    enabled: model.enabled,
    deployments: model.deployments.map((deployment) => ({
      provider_id: deployment.provider_id,
      upstream_model: deployment.upstream_model,
    })),
  };
}

export default function ModelsPage() {
  const queryClient = useQueryClient();
  const modelsQuery = useQuery({ queryKey: ["models"], queryFn: api.listModels });
  const providersQuery = useQuery({ queryKey: ["providers"], queryFn: api.listProviders });
  const [draft, setDraft] = useState<Draft | null>(null);
  const [error, setError] = useState<string | null>(null);

  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["models"] });

  const save = useMutation({
    mutationFn: async () => {
      const d = draft!;
      const deployments = d.deployments.map((deployment) => ({
        provider_id: deployment.provider_id,
        upstream_model: deployment.upstream_model.trim(),
      }));
      if (d.original) {
        await api.updateModel(d.original, deployments, d.enabled);
      } else {
        await api.createModel(d.name.trim(), deployments, d.enabled);
      }
    },
    onSuccess: () => {
      setDraft(null);
      setError(null);
      invalidate();
    },
    onError: (cause) => setError(cause instanceof Error ? cause.message : "保存失败"),
  });

  const toggle = useMutation({
    mutationFn: async (model: ModelRecord) => {
      const deployments = model.deployments.map((deployment) => ({
        provider_id: deployment.provider_id,
        upstream_model: deployment.upstream_model,
      }));
      await api.updateModel(model.name, deployments, !model.enabled);
    },
    onSuccess: () => {
      setError(null);
      invalidate();
    },
    onError: (cause) => setError(cause instanceof Error ? cause.message : "操作失败"),
  });

  const remove = useMutation({
    mutationFn: (name: string) => api.deleteModel(name),
    onSuccess: invalidate,
  });

  const models = modelsQuery.data?.models ?? [];
  const providers = providersQuery.data?.providers ?? [];

  const moveDeployment = (index: number, delta: number) => {
    if (!draft) return;
    const next = [...draft.deployments];
    const target = index + delta;
    if (target < 0 || target >= next.length) return;
    [next[index], next[target]] = [next[target], next[index]];
    setDraft({ ...draft, deployments: next });
  };

  const updateDeployment = (index: number, patch: Partial<DeploymentDraft>) => {
    if (!draft) return;
    const next = draft.deployments.map((deployment, i) =>
      i === index ? { ...deployment, ...patch } : deployment,
    );
    setDraft({ ...draft, deployments: next });
  };

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-2">
        <h1 className="text-base font-semibold">模型目录</h1>
        <p className="text-xs text-muted-foreground">
          模型是对外的唯一请求入口;每个模型按顺序在多个 Provider 之间自动切换
        </p>
        <Button
          className="ml-auto"
          onClick={() => {
            setDraft(emptyDraft());
            setError(null);
          }}
        >
          新增模型
        </Button>
      </div>

      {draft && (
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">
              {draft.original ? `编辑模型 ${draft.original}` : "新增模型"}
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            <div className="flex flex-wrap items-center gap-2">
              <Input
                value={draft.name}
                onChange={(event) => setDraft({ ...draft, name: event.target.value })}
                placeholder="模型名(客户端请求的 model)"
                className="w-64"
                disabled={draft.original !== null}
              />
              <label className="flex items-center gap-1.5 text-xs">
                <input
                  type="checkbox"
                  checked={draft.enabled}
                  onChange={(event) => setDraft({ ...draft, enabled: event.target.checked })}
                />
                上架(需要所有部署都有定价)
              </label>
            </div>

            <div className="space-y-2">
              <p className="text-xs text-muted-foreground">
                部署链(从上到下为切换顺序;上游模型名 = 转发给该 Provider 的 model)
              </p>
              {draft.deployments.map((deployment, index) => (
                <div key={index} className="flex flex-wrap items-center gap-2">
                  <span className="w-5 text-right text-xs tabular-nums text-muted-foreground">
                    {index + 1}.
                  </span>
                  <Select
                    value={deployment.provider_id}
                    onValueChange={(value) =>
                      updateDeployment(index, { provider_id: String(value) })
                    }
                  >
                    <SelectTrigger className="w-44">
                      <SelectValue>{deployment.provider_id || "选择 Provider"}</SelectValue>
                    </SelectTrigger>
                    <SelectContent>
                      {providers.map((provider: ProviderRecord) => (
                        <SelectItem key={provider.id} value={provider.id}>
                          {provider.id} ({provider.protocol})
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <Input
                    value={deployment.upstream_model}
                    onChange={(event) =>
                      updateDeployment(index, { upstream_model: event.target.value })
                    }
                    placeholder="上游模型名"
                    className="w-56"
                  />
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    disabled={index === 0}
                    onClick={() => moveDeployment(index, -1)}
                  >
                    <ArrowUp className="size-3.5" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    disabled={index === draft.deployments.length - 1}
                    onClick={() => moveDeployment(index, 1)}
                  >
                    <ArrowDown className="size-3.5" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    className="text-destructive"
                    onClick={() =>
                      setDraft({
                        ...draft,
                        deployments: draft.deployments.filter((_, i) => i !== index),
                      })
                    }
                  >
                    <Trash2 className="size-3.5" />
                  </Button>
                </div>
              ))}
              <Button
                variant="outline"
                size="sm"
                onClick={() =>
                  setDraft({
                    ...draft,
                    deployments: [
                      ...draft.deployments,
                      { provider_id: providers[0]?.id ?? "", upstream_model: "" },
                    ],
                  })
                }
              >
                添加部署
              </Button>
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
          <CardTitle className="text-sm">模型列表</CardTitle>
        </CardHeader>
        <CardContent>
          {error && !draft && <p className="mb-2 text-xs text-destructive">{error}</p>}
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>模型</TableHead>
                <TableHead>状态</TableHead>
                <TableHead>部署链</TableHead>
                <TableHead>定价缺口</TableHead>
                <TableHead />
              </TableRow>
            </TableHeader>
            <TableBody>
              {models.map((model: ModelRecord) => (
                <TableRow key={model.name}>
                  <TableCell className="font-medium">{model.name}</TableCell>
                  <TableCell>
                    <Badge variant={model.enabled ? "default" : "secondary"}>
                      {model.enabled ? "已上架" : "已下架"}
                    </Badge>
                  </TableCell>
                  <TableCell className="text-xs text-muted-foreground">
                    {model.deployments.length === 0
                      ? "—"
                      : model.deployments
                          .map(
                            (deployment) =>
                              `${deployment.provider_id}→${deployment.upstream_model}`,
                          )
                          .join("  ·  ")}
                  </TableCell>
                  <TableCell className="text-xs">
                    {model.uncovered.length === 0 ? (
                      <span className="text-muted-foreground">无</span>
                    ) : (
                      <span className="text-destructive">{model.uncovered.join(", ")}</span>
                    )}
                  </TableCell>
                  <TableCell className="space-x-1 text-right">
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => toggle.mutate(model)}
                      disabled={toggle.isPending}
                    >
                      {model.enabled ? "下架" : "上架"}
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => {
                        setDraft(draftFrom(model));
                        setError(null);
                      }}
                    >
                      编辑
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      className="text-destructive"
                      onClick={() => {
                        if (confirm(`确认删除模型 ${model.name}?正在使用它的请求会立即 404`)) {
                          remove.mutate(model.name);
                        }
                      }}
                    >
                      删除
                    </Button>
                  </TableCell>
                </TableRow>
              ))}
              {models.length === 0 && (
                <TableRow>
                  <TableCell colSpan={5} className="py-8 text-center text-muted-foreground">
                    {modelsQuery.isPending
                      ? "加载中…"
                      : "目录为空 — 请先创建模型,并为其部署链配置 Provider 与定价"}
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
