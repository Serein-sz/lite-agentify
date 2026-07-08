/** 管理 API 客户端:同源 fetch + JSON,401 统一跳回登录页。 */

const LOGIN_URL = "/admin/login";

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
    public payload?: unknown,
  ) {
    super(message);
  }
}

export interface CostEntry {
  currency: string;
  /** rust_decimal 序列化为字符串,例如 "4.50"。 */
  amount: string;
}

export interface UsageTotals {
  requests: number;
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  avg_latency_ms: number;
  error_rate: number;
  cost: CostEntry[];
}

export interface UsageSeriesPoint {
  bucket_start: string;
  requests: number;
  total_tokens: number;
  cost: CostEntry[];
}

export interface UsageBreakdownRow {
  provider_id: string;
  model: string | null;
  requests: number;
  total_tokens: number;
  cost: CostEntry[];
}

export interface UsageSummaryResponse {
  usage_enabled: boolean;
  totals: UsageTotals;
  series: UsageSeriesPoint[];
  breakdown: UsageBreakdownRow[];
}

export interface UsageRow {
  request_id: string;
  created_at: string;
  provider_id: string;
  protocol: string;
  path: string;
  requested_model: string | null;
  upstream_model: string | null;
  status: number;
  latency_ms: number;
  input_tokens: number | null;
  output_tokens: number | null;
  total_tokens: number | null;
  estimated_cost: string | null;
  currency: string | null;
  usage_source: string;
}

export interface UsageListResponse {
  usage_enabled: boolean;
  rows: UsageRow[];
  total: number;
  page: number;
  page_size: number;
}

export interface ConfigPayload {
  content: string;
  hash: string;
}

/** 结构化配置 DTO,与后端 `StructuredConfig` 一致;密钥字段可携带 __MASKED__ 哨兵。 */
export interface StructuredProvider {
  id: string;
  protocol: string;
  base_url: string;
  api_key: string;
  anthropic_version?: string;
  model_aliases: Record<string, string>;
}

export interface StructuredRoute {
  path_prefix: string;
  providers: string[];
  model_prefix?: string;
}

/** 单价字段为字符串,后端按原文写入 TOML 以保留精度。 */
export interface StructuredPricing {
  provider: string;
  model: string;
  input_per_1m: string;
  output_per_1m: string;
  cached_input_per_1m?: string;
  cache_read_per_1m?: string;
  cache_write_per_1m?: string;
  currency: string;
  pricing_source?: string;
}

export interface StructuredConfig {
  gateway_keys: string[];
  providers: StructuredProvider[];
  routes: StructuredRoute[];
  pricing: StructuredPricing[];
}

export interface SaveResult {
  message: string;
  warnings: string[];
}

interface RequestOptions {
  method?: string;
  body?: string;
  redirectOn401?: boolean;
}

async function request<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const { method = "GET", body, redirectOn401 = true } = options;
  const response = await fetch(`/admin/api${path}`, {
    method,
    body,
    credentials: "same-origin",
    headers: body ? { "Content-Type": "application/json" } : undefined,
  });

  const text = await response.text();
  let payload: unknown = null;
  try {
    payload = text ? JSON.parse(text) : null;
  } catch {
    payload = null;
  }

  if (response.status === 401 && redirectOn401) {
    window.location.href = LOGIN_URL;
    throw new ApiError(401, "登录已失效,请重新登录");
  }
  if (!response.ok) {
    const message =
      (payload as { error?: string } | null)?.error ??
      (text || `请求失败 (${response.status})`);
    throw new ApiError(response.status, message, payload);
  }
  return payload as T;
}

export const api = {
  login: (password: string) =>
    request<{ ok: boolean }>("/login", {
      method: "POST",
      body: JSON.stringify({ password }),
      redirectOn401: false,
    }),
  logout: () => request<{ ok: boolean }>("/logout", { method: "POST" }),
  getConfig: () => request<ConfigPayload>("/config"),
  putConfig: (content: string, baseHash: string) =>
    request<SaveResult>("/config", {
      method: "PUT",
      body: JSON.stringify({ content, base_hash: baseHash }),
    }),
  putConfigStructured: (config: StructuredConfig, baseHash: string) =>
    request<SaveResult>("/config/structured", {
      method: "PUT",
      body: JSON.stringify({ config, base_hash: baseHash }),
    }),
  revealSecret: (field: string) =>
    request<{ value: string }>("/config/reveal", {
      method: "POST",
      body: JSON.stringify({ field }),
    }),
  usageSummary: (params: URLSearchParams) =>
    request<UsageSummaryResponse>(`/usage/summary?${params.toString()}`),
  usageList: (params: URLSearchParams) =>
    request<UsageListResponse>(`/usage?${params.toString()}`),
};
