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

export interface ProviderRecord {
  id: string;
  protocol: string;
  base_url: string;
  /** Masked (`__MASKED__…`) in list/read responses. */
  api_key: string;
  anthropic_version: string | null;
  model_aliases: Record<string, string>;
}

export interface DeploymentRecord {
  id: string;
  provider_id: string;
  upstream_model: string;
}

export interface ModelRecord {
  name: string;
  enabled: boolean;
  created_at: string;
  deployments: DeploymentRecord[];
  /** `provider:upstream_model` pairs missing a pricing rule. */
  uncovered: string[];
}

export interface PricingRecord {
  id: string;
  provider: string;
  model: string;
  input_per_1m: string;
  output_per_1m: string;
  cached_input_per_1m: string | null;
  cache_read_per_1m: string | null;
  cache_write_per_1m: string | null;
  currency: string;
  pricing_source: string | null;
}

export interface SaveResult {
  message: string;
  warnings: string[];
}

export type Role = "admin" | "user";

export interface Me {
  user_id: string;
  username: string;
  role: Role;
}

export interface UserRecord {
  id: string;
  username: string;
  role: Role;
  status: "active" | "disabled";
  created_at: string;
}

export interface ApiKeyRecord {
  id: string;
  user_id: string;
  prefix: string;
  name: string;
  status: "active" | "revoked";
  created_at: string;
  last_used_at: string | null;
  /** Model names this key may call; null = every enabled model. */
  allowed_models: string[] | null;
  /** Cumulative USD spend cap; null = uncapped. Decimal as string. */
  spend_cap_usd: string | null;
  /** Cumulative USD spent through this key (live counter), when available. */
  spent_usd?: string | null;
  /** Present only in the admin all-keys listing. */
  username?: string;
}

export interface CreatedKey {
  /** The plaintext key, shown exactly once. */
  key: string;
  record: ApiKeyRecord;
  warning?: string;
}

/** Own credit position: Σ grants, Σ usage cost, and their difference. */
export interface BalanceSummary {
  granted: string;
  spent: string;
  balance: string;
}

export interface UserBalance {
  user_id: string;
  username: string;
  status: "active" | "disabled";
  granted: string;
  spent: string;
  balance: string;
}

export interface GrantRow {
  id: string;
  user_id: string;
  username: string | null;
  /** Positive = grant, negative = correction. Decimal as string. */
  amount_usd: string;
  note: string | null;
  granted_by: string | null;
  created_at: string;
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
  login: (username: string, password: string) =>
    request<{ ok: boolean; username: string; role: Role }>("/login", {
      method: "POST",
      body: JSON.stringify({ username, password }),
      redirectOn401: false,
    }),
  logout: () => request<{ ok: boolean }>("/logout", { method: "POST" }),
  me: () => request<Me>("/me", { redirectOn401: false }),
  changeOwnPassword: (currentPassword: string, newPassword: string) =>
    request<{ ok: boolean }>("/me/password", {
      method: "POST",
      body: JSON.stringify({
        current_password: currentPassword,
        new_password: newPassword,
      }),
    }),
  listUsers: () => request<{ users: UserRecord[] }>("/users"),
  createUser: (username: string, password: string, role: Role) =>
    request<{ user: UserRecord }>("/users", {
      method: "POST",
      body: JSON.stringify({ username, password, role }),
    }),
  disableUser: (id: string) =>
    request<{ ok: boolean }>(`/users/${id}/disable`, { method: "POST" }),
  enableUser: (id: string) =>
    request<{ ok: boolean }>(`/users/${id}/enable`, { method: "POST" }),
  resetUserPassword: (id: string, password: string) =>
    request<{ ok: boolean }>(`/users/${id}/reset-password`, {
      method: "POST",
      body: JSON.stringify({ password }),
    }),
  listKeys: () => request<{ keys: ApiKeyRecord[] }>("/keys"),
  createKey: (name: string, allowedModels: string[] | null, spendCapUsd: string | null) =>
    request<CreatedKey>("/keys", {
      method: "POST",
      body: JSON.stringify({
        name,
        allowed_models: allowedModels,
        spend_cap_usd: spendCapUsd,
      }),
    }),
  updateKey: (id: string, allowedModels: string[] | null, spendCapUsd: string | null) =>
    request<{ ok: boolean }>(`/keys/${id}`, {
      method: "PUT",
      body: JSON.stringify({
        allowed_models: allowedModels,
        spend_cap_usd: spendCapUsd,
      }),
    }),
  revokeKey: (id: string) =>
    request<{ ok: boolean }>(`/keys/${id}/revoke`, { method: "POST" }),
  myBalance: () => request<BalanceSummary>("/me/balance"),
  listBalances: () => request<{ balances: UserBalance[] }>("/credits"),
  createGrant: (userId: string, amountUsd: string, note: string | null) =>
    request<{ warning?: string }>("/credits/grants", {
      method: "POST",
      body: JSON.stringify({ user_id: userId, amount_usd: amountUsd, note }),
    }),
  listLedger: (userId?: string, limit = 200) => {
    const params = new URLSearchParams({ limit: String(limit) });
    if (userId) params.set("user_id", userId);
    return request<{ grants: GrantRow[] }>(`/credits/ledger?${params.toString()}`);
  },
  listProviders: () => request<{ providers: ProviderRecord[] }>("/providers"),
  createProvider: (provider: Omit<ProviderRecord, "id"> & { id: string }) =>
    request<{ ok: boolean }>("/providers", {
      method: "POST",
      body: JSON.stringify(provider),
    }),
  updateProvider: (id: string, provider: Omit<ProviderRecord, "id">) =>
    request<{ ok: boolean; warning?: string }>(`/providers/${encodeURIComponent(id)}`, {
      method: "PUT",
      body: JSON.stringify(provider),
    }),
  deleteProvider: (id: string) =>
    request<{ ok: boolean }>(`/providers/${encodeURIComponent(id)}`, {
      method: "DELETE",
    }),
  revealProviderKey: (id: string) =>
    request<{ value: string }>(`/providers/${encodeURIComponent(id)}/reveal`, {
      method: "POST",
    }),
  listPricing: () => request<{ pricing: PricingRecord[] }>("/pricing"),
  createPricing: (pricing: Omit<PricingRecord, "id">) =>
    request<{ ok: boolean }>("/pricing", {
      method: "POST",
      body: JSON.stringify(pricing),
    }),
  updatePricing: (id: string, pricing: Omit<PricingRecord, "id">) =>
    request<{ ok: boolean }>(`/pricing/${id}`, {
      method: "PUT",
      body: JSON.stringify(pricing),
    }),
  deletePricing: (id: string) =>
    request<{ ok: boolean }>(`/pricing/${id}`, { method: "DELETE" }),
  listModels: () => request<{ models: ModelRecord[] }>("/models"),
  /** Enabled model names; readable by every signed-in user (key editor picker). */
  listModelNames: () => request<{ models: string[] }>("/models/names"),
  createModel: (
    name: string,
    deployments: { provider_id: string; upstream_model: string }[],
    enabled: boolean,
  ) =>
    request<{ ok: boolean; warning?: string }>("/models", {
      method: "POST",
      body: JSON.stringify({ name, deployments, enabled }),
    }),
  updateModel: (
    name: string,
    deployments: { provider_id: string; upstream_model: string }[],
    enabled: boolean,
  ) =>
    request<{ ok: boolean; warning?: string }>(`/models/${encodeURIComponent(name)}`, {
      method: "PUT",
      body: JSON.stringify({ deployments, enabled }),
    }),
  deleteModel: (name: string) =>
    request<{ ok: boolean }>(`/models/${encodeURIComponent(name)}`, {
      method: "DELETE",
    }),
  usageSummary: (params: URLSearchParams) =>
    request<UsageSummaryResponse>(`/usage/summary?${params.toString()}`),
  usageList: (params: URLSearchParams) =>
    request<UsageListResponse>(`/usage?${params.toString()}`),
};
