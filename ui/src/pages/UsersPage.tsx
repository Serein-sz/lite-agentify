import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type Role, type UserRecord } from "@/api";
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
import { formatDateTime } from "@/lib/format";

export default function UsersPage() {
  const queryClient = useQueryClient();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [role, setRole] = useState<Role>("user");
  const [error, setError] = useState<string | null>(null);

  const usersQuery = useQuery({ queryKey: ["users"], queryFn: api.listUsers });
  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ["users"] });

  const createUser = useMutation({
    mutationFn: () => api.createUser(username.trim(), password, role),
    onSuccess: () => {
      setUsername("");
      setPassword("");
      setRole("user");
      setError(null);
      invalidate();
    },
    onError: (cause) =>
      setError(cause instanceof Error ? cause.message : "创建失败"),
  });

  const setStatus = useMutation({
    mutationFn: ({ id, disable }: { id: string; disable: boolean }) =>
      disable ? api.disableUser(id) : api.enableUser(id),
    onSuccess: invalidate,
  });

  const resetPassword = useMutation({
    mutationFn: ({ id, password }: { id: string; password: string }) =>
      api.resetUserPassword(id, password),
  });

  const users = usersQuery.data?.users ?? [];

  return (
    <div className="space-y-6">
      <h1 className="text-base font-semibold">用户管理</h1>

      <Card>
        <CardHeader>
          <CardTitle className="text-sm">创建用户</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-wrap items-center gap-2">
          <Input
            value={username}
            onChange={(event) => setUsername(event.target.value)}
            placeholder="用户名"
            className="max-w-[10rem]"
          />
          <Input
            type="password"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            placeholder="初始密码(≥8 位)"
            className="max-w-[12rem]"
          />
          <Select value={role} onValueChange={(value) => setRole(value as Role)}>
            <SelectTrigger className="w-28">
              <SelectValue>{role === "admin" ? "管理员" : "用户"}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="user">用户</SelectItem>
              <SelectItem value="admin">管理员</SelectItem>
            </SelectContent>
          </Select>
          <Button
            onClick={() => createUser.mutate()}
            disabled={
              createUser.isPending ||
              username.trim().length === 0 ||
              password.length < 8
            }
          >
            {createUser.isPending ? "创建中…" : "创建"}
          </Button>
          {error && <span className="text-xs text-destructive">{error}</span>}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-sm">用户列表</CardTitle>
        </CardHeader>
        <CardContent>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>用户名</TableHead>
                <TableHead>角色</TableHead>
                <TableHead>状态</TableHead>
                <TableHead>创建时间</TableHead>
                <TableHead />
              </TableRow>
            </TableHeader>
            <TableBody>
              {users.map((user: UserRecord) => (
                <TableRow key={user.id}>
                  <TableCell className="font-medium">{user.username}</TableCell>
                  <TableCell>
                    <Badge variant={user.role === "admin" ? "default" : "secondary"}>
                      {user.role === "admin" ? "管理员" : "用户"}
                    </Badge>
                  </TableCell>
                  <TableCell>
                    {user.status === "active" ? (
                      <Badge variant="secondary">正常</Badge>
                    ) : (
                      <Badge variant="outline">已停用</Badge>
                    )}
                  </TableCell>
                  <TableCell className="whitespace-nowrap text-muted-foreground tabular-nums">
                    {formatDateTime(user.created_at)}
                  </TableCell>
                  <TableCell className="space-x-1 text-right">
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => {
                        const next = prompt(`为「${user.username}」设置新密码(≥8 位)`);
                        if (next && next.length >= 8) {
                          resetPassword.mutate({ id: user.id, password: next });
                        } else if (next !== null) {
                          alert("密码至少 8 位");
                        }
                      }}
                    >
                      重置密码
                    </Button>
                    {user.status === "active" ? (
                      <Button
                        variant="ghost"
                        size="sm"
                        className="text-destructive"
                        onClick={() => {
                          if (confirm(`确认停用用户「${user.username}」?其会话与密钥将立即失效。`)) {
                            setStatus.mutate({ id: user.id, disable: true });
                          }
                        }}
                      >
                        停用
                      </Button>
                    ) : (
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() =>
                          setStatus.mutate({ id: user.id, disable: false })
                        }
                      >
                        启用
                      </Button>
                    )}
                  </TableCell>
                </TableRow>
              ))}
              {users.length === 0 && (
                <TableRow>
                  <TableCell
                    colSpan={5}
                    className="py-8 text-center text-muted-foreground"
                  >
                    {usersQuery.isPending ? "加载中…" : "暂无用户"}
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
