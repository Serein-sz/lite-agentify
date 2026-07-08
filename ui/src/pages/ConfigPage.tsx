import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { PlusIcon, Trash2Icon } from "lucide-react";
import { ApiError, api, type ConfigPayload } from "@/api";
import { Button } from "@/components/ui/button";
import { SecretInput } from "./config/fields";
import {
  isMasked,
  newKey,
  parseConfigForm,
  toStructuredConfig,
  type ConfigForm,
  type GatewayKeyEntry,
} from "./config/model";
import { PricingSection, ProvidersSection, RoutesSection } from "./config/sections";

type Banner =
  | { kind: "success"; text: string; warnings: string[] }
  | { kind: "error"; text: string }
  | { kind: "conflict"; text: string; fresh?: ConfigPayload };

export default function ConfigPage() {
  const queryClient = useQueryClient();
  const configQuery = useQuery({ queryKey: ["config"], queryFn: api.getConfig });
  const [form, setForm] = useState<ConfigForm | null>(null);
  const [parseError, setParseError] = useState<string | null>(null);
  // 编辑后置为 true:服务器内容更新不再覆盖表单,保存失败也绝不丢草稿。
  const [dirty, setDirty] = useState(false);
  const [banner, setBanner] = useState<Banner | null>(null);

  useEffect(() => {
    if (!configQuery.data || dirty) return;
    try {
      setForm(parseConfigForm(configQuery.data.content));
      setParseError(null);
    } catch (cause) {
      setParseError(cause instanceof Error ? cause.message : String(cause));
    }
  }, [configQuery.data, dirty]);

  const update = (next: ConfigForm) => {
    setForm(next);
    setDirty(true);
  };

  const save = useMutation({
    mutationFn: () => api.putConfigStructured(toStructuredConfig(form!), configQuery.data!.hash),
    onSuccess: async (result) => {
      setBanner({
        kind: "success",
        text: result.message || "配置已保存并热重载",
        warnings: result.warnings ?? [],
      });
      setDirty(false);
      // 重新拉取:拿到新的内容哈希,后续保存才能通过并发校验。
      await queryClient.invalidateQueries({ queryKey: ["config"] });
    },
    onError: (cause) => {
      if (cause instanceof ApiError && cause.status === 409) {
        const payload = cause.payload as Partial<ConfigPayload> | null;
        setBanner({
          kind: "conflict",
          text: "配置文件在磁盘上已被其他人修改,保存被拒绝。",
          fresh:
            payload?.content && payload?.hash
              ? { content: payload.content, hash: payload.hash }
              : undefined,
        });
      } else {
        setBanner({
          kind: "error",
          text: cause instanceof Error ? cause.message : String(cause),
        });
      }
    },
  });

  const loadFresh = (fresh: ConfigPayload) => {
    queryClient.setQueryData(["config"], fresh);
    setDirty(false);
    setBanner(null);
  };

  if (configQuery.isPending) {
    return <p className="text-xs text-muted-foreground">加载配置中…</p>;
  }
  if (configQuery.isError) {
    return (
      <p className="text-xs text-destructive">
        配置加载失败:
        {configQuery.error instanceof Error
          ? configQuery.error.message
          : String(configQuery.error)}
      </p>
    );
  }
  if (parseError) {
    return (
      <div className="border border-destructive/30 bg-destructive/10 px-4 py-3 text-xs text-destructive">
        <p className="font-medium">配置文件解析失败,无法以表单编辑</p>
        <p className="mt-1 whitespace-pre-wrap">{parseError}</p>
        <p className="mt-1">请直接编辑磁盘上的配置文件后重试。</p>
      </div>
    );
  }
  if (!form) {
    return <p className="text-xs text-muted-foreground">加载配置中…</p>;
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-3">
        <h1 className="text-base font-semibold">网关配置</h1>
        <span className="text-xs text-muted-foreground">
          密钥以 __MASKED__ 显示,保持不动即保留原值;保存后立即热重载
        </span>
        <div className="ml-auto flex items-center gap-2">
          {dirty && (
            <span className="text-xs text-amber-600 dark:text-amber-500">
              有未保存的修改
            </span>
          )}
          <Button onClick={() => save.mutate()} disabled={save.isPending || !dirty}>
            {save.isPending ? "保存中…" : "保存并重载"}
          </Button>
        </div>
      </div>

      {banner?.kind === "success" && (
        <div className="border border-emerald-200 bg-emerald-50 px-4 py-3 text-xs text-emerald-800 dark:border-emerald-900 dark:bg-emerald-950 dark:text-emerald-200">
          <p>{banner.text}</p>
          {banner.warnings.length > 0 && (
            <ul className="mt-2 list-disc pl-5 text-amber-700 dark:text-amber-300">
              {banner.warnings.map((warning) => (
                <li key={warning}>{warning}</li>
              ))}
            </ul>
          )}
        </div>
      )}
      {banner?.kind === "error" && (
        <div className="border border-destructive/30 bg-destructive/10 px-4 py-3 text-xs text-destructive">
          <p className="font-medium">保存失败,表单修改已保留</p>
          <p className="mt-1 whitespace-pre-wrap">{banner.text}</p>
        </div>
      )}
      {banner?.kind === "conflict" && (
        <div className="border border-amber-300 bg-amber-50 px-4 py-3 text-xs text-amber-800 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-200">
          <p className="font-medium">{banner.text}</p>
          <p className="mt-1">你的修改仍在表单中。加载最新版本将放弃当前修改。</p>
          {banner.fresh && (
            <Button
              variant="outline"
              size="xs"
              className="mt-2 border-amber-400 bg-background hover:bg-amber-100 dark:border-amber-700 dark:hover:bg-amber-900"
              onClick={() => loadFresh(banner.fresh!)}
            >
              放弃修改,加载磁盘上的最新版本
            </Button>
          )}
        </div>
      )}

      <GatewayKeysSection
        entries={form.gatewayKeys}
        onChange={(gatewayKeys) => update({ ...form, gatewayKeys })}
      />
      <ProvidersSection
        providers={form.providers}
        onChange={(providers) => update({ ...form, providers })}
      />
      <RoutesSection
        routes={form.routes}
        providerIds={form.providers.map((provider) => provider.id).filter(Boolean)}
        onChange={(routes) => update({ ...form, routes })}
      />
      <PricingSection
        pricing={form.pricing}
        providers={form.providers}
        onChange={(pricing) => update({ ...form, pricing })}
      />

      <section className="space-y-2 border-t border-border pt-4 text-xs text-muted-foreground">
        <p>
          管理密码 admin_password:
          {form.adminPasswordSet ? "已设置(单向哈希,不支持在此修改)" : "未设置"}
        </p>
        <p>listen_addr 与 usage_database 需重启生效,请直接编辑配置文件。</p>
      </section>
    </div>
  );
}

/** 网关密钥列表:掩码输入 + 复制;新增/删除会改变列表长度,此时其余
 * 未改动的掩码值无法按位置回填,后端会提示重新输入真实值。 */
function GatewayKeysSection({
  entries,
  onChange,
}: {
  entries: GatewayKeyEntry[];
  onChange: (entries: GatewayKeyEntry[]) => void;
}) {
  return (
    <section className="space-y-3">
      <div className="flex items-center gap-3">
        <h2 className="text-sm font-medium">网关密钥 gateway_keys</h2>
        <span className="text-xs text-muted-foreground">客户端访问本网关所用的 API key</span>
        <Button
          type="button"
          variant="outline"
          size="xs"
          className="ml-auto"
          onClick={() =>
            onChange([...entries, { key: newKey(), value: "", originalIndex: null }])
          }
        >
          <PlusIcon data-icon="inline-start" />
          添加
        </Button>
      </div>
      {entries.length === 0 && (
        <p className="text-xs text-muted-foreground">
          尚无密钥;至少需要一个,否则保存将被拒绝。
        </p>
      )}
      <div className="max-w-xl space-y-1.5">
        {entries.map((entry, index) => (
          <div key={entry.key} className="flex items-center gap-1">
            <div className="flex-1">
              <SecretInput
                value={entry.value}
                onChange={(value) =>
                  onChange(entries.map((e, i) => (i === index ? { ...e, value } : e)))
                }
                revealField={
                  entry.originalIndex !== null && isMasked(entry.value)
                    ? `gateway_keys.${entry.originalIndex}`
                    : null
                }
                placeholder="新的网关密钥"
              />
            </div>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              title="删除此密钥"
              onClick={() => onChange(entries.filter((_, i) => i !== index))}
            >
              <Trash2Icon />
            </Button>
          </div>
        ))}
      </div>
    </section>
  );
}
