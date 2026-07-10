import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, Check } from "lucide-react";
import { api, type ApiKeyRecord, type Role } from "@/api";
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
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { formatDateTime } from "@/lib/format";
import { cn } from "@/lib/utils";

/** 紧凑金额:仅数字,表头注明 USD。 */
function usd(amount: string | null | undefined): string {
  const value = Number(amount ?? "0");
  return Number.isFinite(value)
    ? value.toLocaleString("zh-CN", { maximumFractionDigits: 2 })
    : String(amount);
}

/** 密钥累计消费与上限:有上限时带进度条,达限红显(此时请求会被 402)。 */
function CapProgress({
  spent,
  cap,
}: {
  spent: string | null | undefined;
  cap: string | null;
}) {
  if (cap === null) {
    return (
      <span className="text-xs text-muted-foreground tabular-nums">
        {usd(spent)} / 不限
      </span>
    );
  }
  const spentValue = Number(spent ?? "0");
  const capValue = Number(cap);
  const ratio = capValue > 0 ? Math.min(1, Math.max(0, spentValue / capValue)) : 1;
  const exhausted = spentValue >= capValue;
  return (
    <div className="min-w-28 space-y-1">
      <div
        className={cn(
          "text-xs tabular-nums",
          exhausted ? "font-medium text-destructive" : "text-muted-foreground",
        )}
      >
        {usd(spent)} / {usd(cap)}
      </div>
      <div className="h-1 w-full overflow-hidden rounded-full bg-muted">
        <div
          className={cn("h-full rounded-full", exhausted ? "bg-destructive" : "bg-primary")}
          style={{ width: `${ratio * 100}%` }}
        />
      </div>
    </div>
  );
}

function CopyButton({ value }: { value: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <Button
      variant="outline"
      size="sm"
      onClick={async () => {
        await navigator.clipboard.writeText(value);
        setCopied(true);
        setTimeout(() => setCopied(false), 1500);
      }}
    >
      {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
      {copied ? "已复制" : "复制"}
    </Button>
  );
}

/** Checkbox picker over the enabled catalog. Empty selection = all models. */
function ModelPicker({
  models,
  selected,
  onChange,
}: {
  models: string[];
  selected: string[];
  onChange: (next: string[]) => void;
}) {
  if (models.length === 0) {
    return <p className="text-xs text-muted-foreground">目录暂无已上架模型</p>;
  }
  return (
    <div className="flex flex-wrap gap-x-4 gap-y-1.5">
      {models.map((model) => (
        <label key={model} className="flex items-center gap-1.5 text-xs">
          <input
            type="checkbox"
            checked={selected.includes(model)}
            onChange={(event) =>
              onChange(
                event.target.checked
                  ? [...selected, model]
                  : selected.filter((name) => name !== model),
              )
            }
          />
          {model}
        </label>
      ))}
    </div>
  );
}

export default function KeysPage({ role }: { role: Role }) {
  const queryClient = useQueryClient();
  const isAdmin = role === "admin";
  const [name, setName] = useState("");
  const [newKeyModels, setNewKeyModels] = useState<string[]>([]);
  const [newKeyCap, setNewKeyCap] = useState("");
  const [created, setCreated] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** The key whose limits (allowed models + spend cap) are being edited. */
  const [editing, setEditing] = useState<{
    id: string;
    name: string;
    selected: string[];
    cap: string;
  } | null>(null);

  const keysQuery = useQuery({ queryKey: ["keys"], queryFn: api.listKeys });
  const modelNamesQuery = useQuery({
    queryKey: ["model-names"],
    queryFn: api.listModelNames,
  });
  const modelNames = modelNamesQuery.data?.models ?? [];

  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["keys"] });

  const createKey = useMutation({
    mutationFn: () =>
      api.createKey(
        name.trim(),
        newKeyModels.length > 0 ? newKeyModels : null,
        newKeyCap.trim() || null,
      ),
    onSuccess: (result) => {
      setCreated(result.key);
      setName("");
      setNewKeyModels([]);
      setNewKeyCap("");
      setError(null);
      invalidate();
    },
    onError: (cause) =>
      setError(cause instanceof Error ? cause.message : "创建失败"),
  });

  const updateKey = useMutation({
    mutationFn: (input: { id: string; models: string[] | null; cap: string | null }) =>
      api.updateKey(input.id, input.models, input.cap),
    onSuccess: () => {
      setEditing(null);
      setError(null);
      invalidate();
    },
    onError: (cause) =>
      setError(cause instanceof Error ? cause.message : "保存失败"),
  });

  const revokeKey = useMutation({
    mutationFn: (id: string) => api.revokeKey(id),
    onSuccess: invalidate,
  });

  const keys = keysQuery.data?.keys ?? [];

  const allowedLabel = (key: ApiKeyRecord) =>
    key.allowed_models === null ? "全部模型" : key.allowed_models.join(", ");

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-2">
        <h1 className="text-base font-semibold">API 密钥</h1>
      </div>

      {created && (
        <Card className="border-primary/50">
          <CardHeader>
            <CardTitle className="text-sm">密钥已创建</CardTitle>
            <CardDescription className="text-destructive">
              请立即复制保存,此密钥只显示这一次,关闭后无法再次查看。
            </CardDescription>
          </CardHeader>
          <CardContent className="flex items-center gap-2">
            <code className="flex-1 truncate rounded bg-muted px-3 py-2 text-xs">
              {created}
            </code>
            <CopyButton value={created} />
            <Button variant="ghost" size="sm" onClick={() => setCreated(null)}>
              我已保存
            </Button>
          </CardContent>
        </Card>
      )}

      <Card>
        <CardHeader>
          <CardTitle className="text-sm">创建密钥</CardTitle>
          <CardDescription>
            可限制密钥能调用的模型与累计消费上限;不勾选模型 = 全部已上架模型,上限留空 = 不限
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          <div className="flex items-center gap-2">
            <Input
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="密钥名称(便于识别)"
              className="max-w-xs"
            />
            <Input
              value={newKeyCap}
              onChange={(event) => setNewKeyCap(event.target.value)}
              placeholder="消费上限 (USD),留空不限"
              className="max-w-48"
            />
            <Button
              onClick={() => createKey.mutate()}
              disabled={createKey.isPending || name.trim().length === 0}
            >
              {createKey.isPending ? "创建中…" : "创建"}
            </Button>
            {error && <span className="text-xs text-destructive">{error}</span>}
          </div>
          <ModelPicker
            models={modelNames}
            selected={newKeyModels}
            onChange={setNewKeyModels}
          />
        </CardContent>
      </Card>

      {editing && (
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">编辑密钥「{editing.name}」的限制</CardTitle>
            <CardDescription>
              不勾选任何模型 = 允许调用全部已上架模型;上限留空 = 不限消费
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            <ModelPicker
              models={modelNames}
              selected={editing.selected}
              onChange={(next) => setEditing({ ...editing, selected: next })}
            />
            <Input
              value={editing.cap}
              onChange={(event) => setEditing({ ...editing, cap: event.target.value })}
              placeholder="消费上限 (USD),留空不限"
              className="max-w-48"
            />
            <div className="flex gap-2">
              <Button
                onClick={() =>
                  updateKey.mutate({
                    id: editing.id,
                    models: editing.selected.length > 0 ? editing.selected : null,
                    cap: editing.cap.trim() || null,
                  })
                }
                disabled={updateKey.isPending}
              >
                {updateKey.isPending ? "保存中…" : "保存"}
              </Button>
              <Button variant="ghost" onClick={() => setEditing(null)}>
                取消
              </Button>
            </div>
          </CardContent>
        </Card>
      )}

      <Card>
        <CardHeader>
          <CardTitle className="text-sm">
            {isAdmin ? "全部密钥" : "我的密钥"}
          </CardTitle>
        </CardHeader>
        <CardContent>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>前缀</TableHead>
                <TableHead>名称</TableHead>
                {isAdmin && <TableHead>所属用户</TableHead>}
                <TableHead>状态</TableHead>
                <TableHead>可用模型</TableHead>
                <TableHead>消费 / 上限 (USD)</TableHead>
                <TableHead>创建时间</TableHead>
                <TableHead>最近使用</TableHead>
                <TableHead />
              </TableRow>
            </TableHeader>
            <TableBody>
              {keys.map((key: ApiKeyRecord) => (
                <TableRow key={key.id}>
                  <TableCell className="font-mono text-xs">{key.prefix}…</TableCell>
                  <TableCell>{key.name}</TableCell>
                  {isAdmin && (
                    <TableCell className="text-muted-foreground">
                      {key.username ?? "—"}
                    </TableCell>
                  )}
                  <TableCell>
                    {key.status === "active" ? (
                      <Badge variant="secondary">有效</Badge>
                    ) : (
                      <Badge variant="outline">已吊销</Badge>
                    )}
                  </TableCell>
                  <TableCell className="max-w-48 truncate text-xs text-muted-foreground">
                    {allowedLabel(key)}
                  </TableCell>
                  <TableCell>
                    <CapProgress spent={key.spent_usd} cap={key.spend_cap_usd} />
                  </TableCell>
                  <TableCell className="whitespace-nowrap text-muted-foreground tabular-nums">
                    {formatDateTime(key.created_at)}
                  </TableCell>
                  <TableCell className="whitespace-nowrap text-muted-foreground tabular-nums">
                    {key.last_used_at ? formatDateTime(key.last_used_at) : "—"}
                  </TableCell>
                  <TableCell className="space-x-1 whitespace-nowrap text-right">
                    {key.status === "active" && (
                      <>
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() =>
                            setEditing({
                              id: key.id,
                              name: key.name,
                              selected: key.allowed_models ?? [],
                              cap: key.spend_cap_usd ?? "",
                            })
                          }
                        >
                          编辑限制
                        </Button>
                        <Button
                          variant="ghost"
                          size="sm"
                          className="text-destructive"
                          onClick={() => {
                            if (confirm(`确认吊销密钥「${key.name}」?此操作不可撤销。`)) {
                              revokeKey.mutate(key.id);
                            }
                          }}
                        >
                          吊销
                        </Button>
                      </>
                    )}
                  </TableCell>
                </TableRow>
              ))}
              {keys.length === 0 && (
                <TableRow>
                  <TableCell
                    colSpan={isAdmin ? 9 : 8}
                    className="py-8 text-center text-muted-foreground"
                  >
                    {keysQuery.isPending ? "加载中…" : "暂无密钥"}
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
