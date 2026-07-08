/** 结构化配置表单的状态模型:掩码 TOML → 表单状态 → 提交 DTO。 */

import { parse } from "smol-toml";
import type { StructuredConfig } from "@/api";

export const MASK_PREFIX = "__MASKED__";

export function isMasked(value: string): boolean {
  return value.startsWith(MASK_PREFIX);
}

/** 每个可重复条目的稳定 React key,与数据内容无关。 */
let nextKey = 0;
export function newKey(): number {
  return nextKey++;
}

export interface GatewayKeyEntry {
  key: number;
  value: string;
  /** 磁盘上的原始下标,用于 reveal 引用;新增的条目为 null。 */
  originalIndex: number | null;
}

export interface AliasEntry {
  key: number;
  alias: string;
  target: string;
}

export interface ProviderEntry {
  key: number;
  id: string;
  protocol: string;
  base_url: string;
  api_key: string;
  anthropic_version: string;
  model_aliases: AliasEntry[];
  /** 磁盘上的原始 id,用于 reveal 引用;新增的条目为 null。 */
  originalId: string | null;
}

export interface RouteEntry {
  key: number;
  path_prefix: string;
  providers: string[];
  model_prefix: string;
}

export interface PricingEntry {
  key: number;
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

export interface ConfigForm {
  gatewayKeys: GatewayKeyEntry[];
  providers: ProviderEntry[];
  routes: RouteEntry[];
  pricing: PricingEntry[];
  adminPasswordSet: boolean;
}

/** TOML 数字(如 2.0)与字符串统一成字符串;其余类型按显示需要粗略转换。 */
function asString(value: unknown): string {
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return "";
}

function asStringArray(value: unknown): string[] {
  return Array.isArray(value) ? value.map(asString) : [];
}

function asRecordArray(value: unknown): Record<string, unknown>[] {
  if (!Array.isArray(value)) return [];
  return value.filter(
    (entry): entry is Record<string, unknown> =>
      typeof entry === "object" && entry !== null && !Array.isArray(entry),
  );
}

/** 把 GET /config 返回的掩码 TOML 解析成表单状态。 */
export function parseConfigForm(content: string): ConfigForm {
  const doc = parse(content) as Record<string, unknown>;

  const gatewayKeys = asStringArray(doc.gateway_keys).map((value, index) => ({
    key: newKey(),
    value,
    originalIndex: index,
  }));

  const providers = asRecordArray(doc.providers).map((entry) => {
    const aliases = entry.model_aliases;
    const model_aliases: AliasEntry[] =
      typeof aliases === "object" && aliases !== null && !Array.isArray(aliases)
        ? Object.entries(aliases as Record<string, unknown>).map(([alias, target]) => ({
            key: newKey(),
            alias,
            target: asString(target),
          }))
        : [];
    const id = asString(entry.id);
    return {
      key: newKey(),
      id,
      protocol: asString(entry.protocol),
      base_url: asString(entry.base_url),
      api_key: asString(entry.api_key),
      anthropic_version: asString(entry.anthropic_version),
      model_aliases,
      originalId: id || null,
    };
  });

  const routes = asRecordArray(doc.routes).map((entry) => ({
    key: newKey(),
    path_prefix: asString(entry.path_prefix),
    providers: asStringArray(entry.providers),
    model_prefix: asString(entry.model_prefix),
  }));

  const pricing = asRecordArray(doc.pricing).map((entry) => ({
    key: newKey(),
    provider: asString(entry.provider),
    model: asString(entry.model),
    input_per_1m: asString(entry.input_per_1m),
    output_per_1m: asString(entry.output_per_1m),
    cached_input_per_1m: asString(entry.cached_input_per_1m),
    cache_read_per_1m: asString(entry.cache_read_per_1m),
    cache_write_per_1m: asString(entry.cache_write_per_1m),
    currency: asString(entry.currency),
    pricing_source: asString(entry.pricing_source),
  }));

  return {
    gatewayKeys,
    providers,
    routes,
    pricing,
    adminPasswordSet: typeof doc.admin_password === "string",
  };
}

/** 空字符串的可选字段省略,让后端从文档中删除对应键。 */
function optional(value: string): string | undefined {
  return value.trim() === "" ? undefined : value;
}

/** 表单状态 → PUT /config/structured 的 DTO。 */
export function toStructuredConfig(form: ConfigForm): StructuredConfig {
  return {
    gateway_keys: form.gatewayKeys.map((entry) => entry.value),
    providers: form.providers.map((provider) => ({
      id: provider.id,
      protocol: provider.protocol,
      base_url: provider.base_url,
      api_key: provider.api_key,
      anthropic_version: optional(provider.anthropic_version),
      model_aliases: Object.fromEntries(
        provider.model_aliases
          .filter((entry) => entry.alias.trim() !== "")
          .map((entry) => [entry.alias, entry.target]),
      ),
    })),
    routes: form.routes.map((route) => ({
      path_prefix: route.path_prefix,
      providers: route.providers,
      model_prefix: optional(route.model_prefix),
    })),
    pricing: form.pricing.map((entry) => ({
      provider: entry.provider,
      model: entry.model,
      input_per_1m: entry.input_per_1m,
      output_per_1m: entry.output_per_1m,
      cached_input_per_1m: optional(entry.cached_input_per_1m),
      cache_read_per_1m: optional(entry.cache_read_per_1m),
      cache_write_per_1m: optional(entry.cache_write_per_1m),
      currency: entry.currency,
      pricing_source: optional(entry.pricing_source),
    })),
  };
}
