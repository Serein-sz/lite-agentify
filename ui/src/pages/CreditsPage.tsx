import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type UserBalance } from "@/api";
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
import { formatDateTime, formatUsd } from "@/lib/format";

/** 余额为负时红显:软限模式下在途请求可能超扣。 */
function BalanceCell({ amount }: { amount: string }) {
  const negative = Number(amount) < 0;
  return (
    <span className={negative ? "font-medium text-destructive tabular-nums" : "tabular-nums"}>
      {formatUsd(amount)}
    </span>
  );
}

export default function CreditsPage() {
  const queryClient = useQueryClient();
  const [userId, setUserId] = useState("");
  const [amount, setAmount] = useState("");
  const [note, setNote] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [ledgerUser, setLedgerUser] = useState("all");

  const balancesQuery = useQuery({ queryKey: ["balances"], queryFn: api.listBalances });
  const balances = balancesQuery.data?.balances ?? [];

  const ledgerQuery = useQuery({
    queryKey: ["ledger", ledgerUser],
    queryFn: () => api.listLedger(ledgerUser === "all" ? undefined : ledgerUser),
  });
  const grants = ledgerQuery.data?.grants ?? [];

  const createGrant = useMutation({
    mutationFn: () => api.createGrant(userId, amount.trim(), note.trim() || null),
    onSuccess: (result) => {
      setAmount("");
      setNote("");
      setError(null);
      setMessage(result.warning ?? "已入账");
      queryClient.invalidateQueries({ queryKey: ["balances"] });
      queryClient.invalidateQueries({ queryKey: ["ledger"] });
      queryClient.invalidateQueries({ queryKey: ["my-balance"] });
    },
    onError: (cause) => {
      setMessage(null);
      setError(cause instanceof Error ? cause.message : "充值失败");
    },
  });

  const submitGrant = () => {
    const value = Number(amount.trim());
    if (!userId || amount.trim() === "" || !Number.isFinite(value) || value === 0) {
      setError("请选择用户并输入非零金额");
      return;
    }
    // 负数是余额修正(扣减),不可逆,先确认。
    if (
      value < 0 &&
      !confirm(
        `确认对该用户执行负数修正 ${amount.trim()} USD?余额将立即扣减,该操作会记入账本且不可删除。`,
      )
    ) {
      return;
    }
    createGrant.mutate();
  };

  const selectedUsername = balances.find((row) => row.user_id === userId)?.username;

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-2">
        <h1 className="text-base font-semibold">额度管理</h1>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-sm">充值 / 修正</CardTitle>
          <CardDescription>
            预付累计模式:余额 = 累计充值 − 累计消费;负数金额用于修正(扣减)
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-wrap items-center gap-2">
          <Select value={userId || undefined} onValueChange={(value) => setUserId(String(value))}>
            <SelectTrigger className="w-44">
              <SelectValue placeholder="选择用户">{selectedUsername ?? "选择用户"}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              {balances.map((row) => (
                <SelectItem key={row.user_id} value={row.user_id}>
                  {row.username}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Input
            value={amount}
            onChange={(event) => setAmount(event.target.value)}
            placeholder="金额 (USD),如 20 或 -5"
            className="w-44"
          />
          <Input
            value={note}
            onChange={(event) => setNote(event.target.value)}
            placeholder="备注(可选)"
            className="w-56"
          />
          <Button onClick={submitGrant} disabled={createGrant.isPending}>
            {createGrant.isPending ? "提交中…" : "入账"}
          </Button>
          {error && <span className="text-xs text-destructive">{error}</span>}
          {message && <span className="text-xs text-muted-foreground">{message}</span>}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-sm">用户余额</CardTitle>
          <CardDescription>
            消费为实时计数,与账本每分钟对账一次;余额耗尽后该用户的请求返回 402
          </CardDescription>
        </CardHeader>
        <CardContent>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>用户</TableHead>
                <TableHead>状态</TableHead>
                <TableHead className="text-right">累计充值</TableHead>
                <TableHead className="text-right">累计消费</TableHead>
                <TableHead className="text-right">余额</TableHead>
                <TableHead />
              </TableRow>
            </TableHeader>
            <TableBody>
              {balances.map((row: UserBalance) => (
                <TableRow key={row.user_id}>
                  <TableCell className="font-medium">{row.username}</TableCell>
                  <TableCell>
                    {row.status === "active" ? (
                      <Badge variant="secondary">正常</Badge>
                    ) : (
                      <Badge variant="outline">已禁用</Badge>
                    )}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {formatUsd(row.granted)}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {formatUsd(row.spent)}
                  </TableCell>
                  <TableCell className="text-right">
                    <BalanceCell amount={row.balance} />
                  </TableCell>
                  <TableCell className="text-right">
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => setUserId(row.user_id)}
                    >
                      充值
                    </Button>
                  </TableCell>
                </TableRow>
              ))}
              {balances.length === 0 && (
                <TableRow>
                  <TableCell colSpan={6} className="py-8 text-center text-muted-foreground">
                    {balancesQuery.isPending ? "加载中…" : "暂无用户"}
                  </TableCell>
                </TableRow>
              )}
            </TableBody>
          </Table>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-sm">充值账本</CardTitle>
          <CardDescription>只增不改的审计记录;修正以负数条目出现</CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          <Select value={ledgerUser} onValueChange={(value) => setLedgerUser(String(value))}>
            <SelectTrigger className="w-44">
              <SelectValue>
                {ledgerUser === "all"
                  ? "全部用户"
                  : (balances.find((row) => row.user_id === ledgerUser)?.username ?? "已选用户")}
              </SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">全部用户</SelectItem>
              {balances.map((row) => (
                <SelectItem key={row.user_id} value={row.user_id}>
                  {row.username}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>时间</TableHead>
                <TableHead>用户</TableHead>
                <TableHead className="text-right">金额</TableHead>
                <TableHead>备注</TableHead>
                <TableHead>操作人</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {grants.map((grant) => (
                <TableRow key={grant.id}>
                  <TableCell className="whitespace-nowrap text-muted-foreground tabular-nums">
                    {formatDateTime(grant.created_at)}
                  </TableCell>
                  <TableCell>{grant.username ?? grant.user_id}</TableCell>
                  <TableCell className="text-right">
                    <BalanceCell amount={grant.amount_usd} />
                  </TableCell>
                  <TableCell className="max-w-64 truncate text-muted-foreground">
                    {grant.note ?? "—"}
                  </TableCell>
                  <TableCell className="text-muted-foreground">
                    {grant.granted_by ?? "—"}
                  </TableCell>
                </TableRow>
              ))}
              {grants.length === 0 && (
                <TableRow>
                  <TableCell colSpan={5} className="py-8 text-center text-muted-foreground">
                    {ledgerQuery.isPending ? "加载中…" : "暂无充值记录"}
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
