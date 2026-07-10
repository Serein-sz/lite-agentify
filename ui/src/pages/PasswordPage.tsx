import { useState } from "react";
import { useMutation } from "@tanstack/react-query";
import { api } from "@/api";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";

export default function PasswordPage() {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const change = useMutation({
    mutationFn: () => api.changeOwnPassword(current, next),
    onSuccess: () => {
      setMessage("密码已更新");
      setError(null);
      setCurrent("");
      setNext("");
      setConfirm("");
    },
    onError: (cause) => {
      setMessage(null);
      setError(cause instanceof Error ? cause.message : "修改失败");
    },
  });

  const mismatch = next.length > 0 && confirm.length > 0 && next !== confirm;

  return (
    <div className="space-y-6">
      <h1 className="text-base font-semibold">修改密码</h1>
      <Card className="max-w-sm">
        <CardHeader>
          <CardTitle className="text-sm">更新登录密码</CardTitle>
        </CardHeader>
        <CardContent>
          <form
            className="space-y-3"
            onSubmit={(event) => {
              event.preventDefault();
              change.mutate();
            }}
          >
            <Input
              type="password"
              autoComplete="current-password"
              value={current}
              onChange={(event) => setCurrent(event.target.value)}
              placeholder="当前密码"
            />
            <Input
              type="password"
              autoComplete="new-password"
              value={next}
              onChange={(event) => setNext(event.target.value)}
              placeholder="新密码(≥8 位)"
            />
            <Input
              type="password"
              autoComplete="new-password"
              value={confirm}
              onChange={(event) => setConfirm(event.target.value)}
              placeholder="确认新密码"
            />
            {mismatch && (
              <p className="text-xs text-destructive">两次输入的新密码不一致</p>
            )}
            {error && <p className="text-xs text-destructive">{error}</p>}
            {message && <p className="text-xs text-primary">{message}</p>}
            <Button
              type="submit"
              className="w-full"
              disabled={
                change.isPending ||
                current.length === 0 ||
                next.length < 8 ||
                mismatch
              }
            >
              {change.isPending ? "提交中…" : "更新密码"}
            </Button>
          </form>
        </CardContent>
      </Card>
    </div>
  );
}
