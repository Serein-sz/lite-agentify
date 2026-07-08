/** 配置表单的三个可重复分区:提供商、路由、计价。 */

import { ArrowDownIcon, ArrowUpIcon, PlusIcon, Trash2Icon, XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Field, SecretInput } from "./fields";
import {
  newKey,
  type AliasEntry,
  type PricingEntry,
  type ProviderEntry,
  type RouteEntry,
} from "./model";

const PROTOCOLS = ["openai", "anthropic"];
const WILDCARD = "*";

function replaceAt<T>(list: T[], index: number, value: T): T[] {
  return list.map((entry, i) => (i === index ? value : entry));
}

function removeAt<T>(list: T[], index: number): T[] {
  return list.filter((_, i) => i !== index);
}

function SectionHeader({ title, hint, onAdd }: { title: string; hint: string; onAdd: () => void }) {
  return (
    <div className="flex items-center gap-3">
      <h2 className="text-sm font-medium">{title}</h2>
      <span className="text-xs text-muted-foreground">{hint}</span>
      <Button type="button" variant="outline" size="xs" className="ml-auto" onClick={onAdd}>
        <PlusIcon data-icon="inline-start" />
        添加
      </Button>
    </div>
  );
}

function EmptyHint({ text }: { text: string }) {
  return <p className="text-xs text-muted-foreground">{text}</p>;
}

// --- 提供商 ---

export function ProvidersSection({
  providers,
  onChange,
}: {
  providers: ProviderEntry[];
  onChange: (providers: ProviderEntry[]) => void;
}) {
  const addProvider = () =>
    onChange([
      ...providers,
      {
        key: newKey(),
        id: "",
        protocol: "openai",
        base_url: "",
        api_key: "",
        anthropic_version: "",
        model_aliases: [],
        originalId: null,
      },
    ]);

  return (
    <section className="space-y-3">
      <SectionHeader
        title="提供商 providers"
        hint="上游 LLM 服务;api_key 保持掩码即保留原值"
        onAdd={addProvider}
      />
      {providers.length === 0 && <EmptyHint text="尚无提供商,点击「添加」创建第一个。" />}
      {providers.map((provider, index) => (
        <Card key={provider.key} size="sm">
          <CardHeader>
            <CardTitle className="font-mono text-xs">
              {provider.id || "(未命名提供商)"}
            </CardTitle>
            <div className="col-start-2 row-start-1 justify-self-end">
              <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                title="删除此提供商"
                onClick={() => onChange(removeAt(providers, index))}
              >
                <Trash2Icon />
              </Button>
            </div>
          </CardHeader>
          <CardContent className="space-y-3">
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
              <Field label="id">
                <Input
                  value={provider.id}
                  onChange={(event) =>
                    onChange(replaceAt(providers, index, { ...provider, id: event.target.value }))
                  }
                  placeholder="openai"
                />
              </Field>
              <Field label="protocol">
                <Select
                  value={provider.protocol}
                  onValueChange={(value) =>
                    onChange(replaceAt(providers, index, { ...provider, protocol: String(value) }))
                  }
                >
                  <SelectTrigger className="w-full">
                    <SelectValue>{provider.protocol}</SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    {PROTOCOLS.map((protocol) => (
                      <SelectItem key={protocol} value={protocol}>
                        {protocol}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </Field>
              <Field label="base_url" className="sm:col-span-2">
                <Input
                  value={provider.base_url}
                  onChange={(event) =>
                    onChange(
                      replaceAt(providers, index, { ...provider, base_url: event.target.value }),
                    )
                  }
                  placeholder="https://api.openai.com"
                />
              </Field>
            </div>
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              <Field label="api_key">
                <SecretInput
                  value={provider.api_key}
                  onChange={(value) =>
                    onChange(replaceAt(providers, index, { ...provider, api_key: value }))
                  }
                  revealField={
                    provider.originalId ? `providers.${provider.originalId}.api_key` : null
                  }
                  placeholder="sk-…"
                />
              </Field>
              <Field label="anthropic_version(可选)">
                <Input
                  value={provider.anthropic_version}
                  onChange={(event) =>
                    onChange(
                      replaceAt(providers, index, {
                        ...provider,
                        anthropic_version: event.target.value,
                      }),
                    )
                  }
                  placeholder="2023-06-01"
                />
              </Field>
            </div>
            <AliasEditor
              aliases={provider.model_aliases}
              onChange={(model_aliases) =>
                onChange(replaceAt(providers, index, { ...provider, model_aliases }))
              }
            />
          </CardContent>
        </Card>
      ))}
    </section>
  );
}

function AliasEditor({
  aliases,
  onChange,
}: {
  aliases: AliasEntry[];
  onChange: (aliases: AliasEntry[]) => void;
}) {
  return (
    <div>
      <div className="mb-1 flex items-center gap-2">
        <span className="text-xs text-muted-foreground">model_aliases(别名 → 上游模型)</span>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          title="添加别名"
          onClick={() => onChange([...aliases, { key: newKey(), alias: "", target: "" }])}
        >
          <PlusIcon />
        </Button>
      </div>
      {aliases.length === 0 && <EmptyHint text="无别名:请求中的模型名将原样转发。" />}
      <div className="space-y-1.5">
        {aliases.map((entry, index) => (
          <div key={entry.key} className="flex items-center gap-1.5">
            <Input
              value={entry.alias}
              onChange={(event) =>
                onChange(replaceAt(aliases, index, { ...entry, alias: event.target.value }))
              }
              placeholder="别名,如 gpt-4o"
              className="font-mono"
            />
            <span className="text-xs text-muted-foreground">→</span>
            <Input
              value={entry.target}
              onChange={(event) =>
                onChange(replaceAt(aliases, index, { ...entry, target: event.target.value }))
              }
              placeholder="上游模型,如 gpt-4o-2024-11-20"
              className="font-mono"
            />
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              title="删除别名"
              onClick={() => onChange(removeAt(aliases, index))}
            >
              <Trash2Icon />
            </Button>
          </div>
        ))}
      </div>
    </div>
  );
}

// --- 路由 ---

export function RoutesSection({
  routes,
  providerIds,
  onChange,
}: {
  routes: RouteEntry[];
  providerIds: string[];
  onChange: (routes: RouteEntry[]) => void;
}) {
  const addRoute = () =>
    onChange([...routes, { key: newKey(), path_prefix: "", providers: [], model_prefix: "" }]);

  return (
    <section className="space-y-3">
      <SectionHeader
        title="路由 routes"
        hint="按路径前缀转发;提供商顺序即故障转移优先级"
        onAdd={addRoute}
      />
      {routes.length === 0 && <EmptyHint text="尚无路由,点击「添加」创建第一条。" />}
      {routes.map((route, index) => (
        <Card key={route.key} size="sm">
          <CardHeader>
            <CardTitle className="font-mono text-xs">{route.path_prefix || "(未设置路径)"}</CardTitle>
            <div className="col-start-2 row-start-1 justify-self-end">
              <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                title="删除此路由"
                onClick={() => onChange(removeAt(routes, index))}
              >
                <Trash2Icon />
              </Button>
            </div>
          </CardHeader>
          <CardContent className="space-y-3">
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              <Field label="path_prefix">
                <Input
                  value={route.path_prefix}
                  onChange={(event) =>
                    onChange(
                      replaceAt(routes, index, { ...route, path_prefix: event.target.value }),
                    )
                  }
                  placeholder="/v1/chat/completions"
                  className="font-mono"
                />
              </Field>
              <Field label="model_prefix(可选,按模型前缀匹配)">
                <Input
                  value={route.model_prefix}
                  onChange={(event) =>
                    onChange(
                      replaceAt(routes, index, { ...route, model_prefix: event.target.value }),
                    )
                  }
                  placeholder="claude-"
                  className="font-mono"
                />
              </Field>
            </div>
            <RouteProvidersEditor
              selected={route.providers}
              providerIds={providerIds}
              onChange={(providers) => onChange(replaceAt(routes, index, { ...route, providers }))}
            />
          </CardContent>
        </Card>
      ))}
    </section>
  );
}

/** 有序多选:从现有提供商中挑选,可上下调整故障转移顺序;悬空引用高亮但不阻塞。 */
function RouteProvidersEditor({
  selected,
  providerIds,
  onChange,
}: {
  selected: string[];
  providerIds: string[];
  onChange: (selected: string[]) => void;
}) {
  const available = providerIds.filter((id) => id && !selected.includes(id));
  const move = (from: number, to: number) => {
    const next = [...selected];
    const [entry] = next.splice(from, 1);
    next.splice(to, 0, entry);
    onChange(next);
  };

  return (
    <div>
      <span className="mb-1 block text-xs text-muted-foreground">
        providers(按顺序故障转移)
      </span>
      <div className="flex flex-wrap items-center gap-1.5">
        {selected.map((id, index) => {
          const dangling = !providerIds.includes(id);
          return (
            <span
              key={`${id}-${index}`}
              className={
                dangling
                  ? "inline-flex items-center gap-0.5 border border-amber-400 bg-amber-50 py-0.5 pl-2 pr-0.5 font-mono text-xs text-amber-800 dark:border-amber-700 dark:bg-amber-950 dark:text-amber-200"
                  : "inline-flex items-center gap-0.5 border border-border bg-muted py-0.5 pl-2 pr-0.5 font-mono text-xs"
              }
              title={dangling ? `提供商 "${id}" 不存在,保存将被拒绝` : undefined}
            >
              {index + 1}. {id}
              <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                title="上移(提高优先级)"
                disabled={index === 0}
                onClick={() => move(index, index - 1)}
              >
                <ArrowUpIcon />
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                title="下移(降低优先级)"
                disabled={index === selected.length - 1}
                onClick={() => move(index, index + 1)}
              >
                <ArrowDownIcon />
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                title="移除"
                onClick={() => onChange(selected.filter((_, i) => i !== index))}
              >
                <XIcon />
              </Button>
            </span>
          );
        })}
        {available.length > 0 && (
          <Select
            value=""
            onValueChange={(value) => {
              if (value) onChange([...selected, String(value)]);
            }}
          >
            <SelectTrigger size="sm">
              <SelectValue>+ 添加提供商</SelectValue>
            </SelectTrigger>
            <SelectContent>
              {available.map((id) => (
                <SelectItem key={id} value={id}>
                  {id}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
        {selected.length === 0 && available.length === 0 && (
          <span className="text-xs text-muted-foreground">先在上方创建提供商</span>
        )}
      </div>
      {selected.some((id) => !providerIds.includes(id)) && (
        <p className="mt-1 text-xs text-amber-700 dark:text-amber-400">
          存在指向不存在提供商的引用(高亮项),保存时后端将拒绝。
        </p>
      )}
    </div>
  );
}

// --- 计价 ---

export function PricingSection({
  pricing,
  providers,
  onChange,
}: {
  pricing: PricingEntry[];
  providers: ProviderEntry[];
  onChange: (pricing: PricingEntry[]) => void;
}) {
  const providerIds = providers.map((provider) => provider.id).filter(Boolean);
  const addPricing = () =>
    onChange([
      ...pricing,
      {
        key: newKey(),
        provider: WILDCARD,
        model: WILDCARD,
        input_per_1m: "",
        output_per_1m: "",
        cached_input_per_1m: "",
        cache_read_per_1m: "",
        cache_write_per_1m: "",
        currency: "USD",
        pricing_source: "",
      },
    ]);

  /** 模型建议:选中提供商的别名目标;通配提供商时汇总全部。 */
  const modelSuggestions = (entry: PricingEntry): string[] => {
    const pool =
      entry.provider === WILDCARD
        ? providers
        : providers.filter((provider) => provider.id === entry.provider);
    const targets = pool.flatMap((provider) =>
      provider.model_aliases.flatMap((alias) => [alias.target, alias.alias]),
    );
    return [...new Set([WILDCARD, ...targets.filter(Boolean)])];
  };

  return (
    <section className="space-y-3">
      <SectionHeader
        title="计价 pricing"
        hint="每百万 token 单价;provider/model 支持通配符 *"
        onAdd={addPricing}
      />
      {pricing.length === 0 && <EmptyHint text="尚无计价规则,用量成本将无法估算。" />}
      {pricing.map((entry, index) => (
        <Card key={entry.key} size="sm">
          <CardHeader>
            <CardTitle className="font-mono text-xs">
              {entry.provider || "?"} : {entry.model || "?"}
            </CardTitle>
            <div className="col-start-2 row-start-1 justify-self-end">
              <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                title="删除此计价规则"
                onClick={() => onChange(removeAt(pricing, index))}
              >
                <Trash2Icon />
              </Button>
            </div>
          </CardHeader>
          <CardContent className="space-y-3">
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
              <Field label="provider(* = 全部)">
                <Select
                  value={entry.provider}
                  onValueChange={(value) =>
                    onChange(replaceAt(pricing, index, { ...entry, provider: String(value) }))
                  }
                >
                  <SelectTrigger className="w-full">
                    <SelectValue>{entry.provider || "选择提供商"}</SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value={WILDCARD}>*(全部提供商)</SelectItem>
                    {providerIds.map((id) => (
                      <SelectItem key={id} value={id}>
                        {id}
                      </SelectItem>
                    ))}
                    {entry.provider !== WILDCARD && !providerIds.includes(entry.provider) && (
                      <SelectItem value={entry.provider}>
                        {entry.provider}(不存在)
                      </SelectItem>
                    )}
                  </SelectContent>
                </Select>
              </Field>
              <Field label="model(* = 全部)">
                <>
                  <Input
                    value={entry.model}
                    onChange={(event) =>
                      onChange(replaceAt(pricing, index, { ...entry, model: event.target.value }))
                    }
                    list={`pricing-models-${entry.key}`}
                    className="font-mono"
                  />
                  <datalist id={`pricing-models-${entry.key}`}>
                    {modelSuggestions(entry).map((model) => (
                      <option key={model} value={model} />
                    ))}
                  </datalist>
                </>
              </Field>
              <Field label="currency(ISO 三字母)">
                <Input
                  value={entry.currency}
                  onChange={(event) =>
                    onChange(
                      replaceAt(pricing, index, {
                        ...entry,
                        currency: event.target.value.toUpperCase(),
                      }),
                    )
                  }
                  placeholder="USD"
                  maxLength={3}
                />
              </Field>
              <Field label="pricing_source(可选备注)">
                <Input
                  value={entry.pricing_source}
                  onChange={(event) =>
                    onChange(
                      replaceAt(pricing, index, { ...entry, pricing_source: event.target.value }),
                    )
                  }
                  placeholder="官网价格页"
                />
              </Field>
            </div>
            <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-5">
              {(
                [
                  ["input_per_1m", "输入"],
                  ["output_per_1m", "输出"],
                  ["cached_input_per_1m", "缓存输入(可选)"],
                  ["cache_read_per_1m", "缓存读(可选)"],
                  ["cache_write_per_1m", "缓存写(可选)"],
                ] as const
              ).map(([field, label]) => (
                <Field key={field} label={label}>
                  <Input
                    value={entry[field]}
                    onChange={(event) =>
                      onChange(
                        replaceAt(pricing, index, { ...entry, [field]: event.target.value }),
                      )
                    }
                    placeholder="2.00"
                    inputMode="decimal"
                    className="font-mono"
                  />
                </Field>
              ))}
            </div>
          </CardContent>
        </Card>
      ))}
    </section>
  );
}
