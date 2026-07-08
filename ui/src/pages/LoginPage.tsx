import { useState } from "react";
import { useNavigate } from "react-router";
import { useMutation } from "@tanstack/react-query";
import { ApiError, api } from "@/api";
import { AnimatedGridPattern } from "@/components/magicui/animated-grid-pattern";
import { ShineBorder } from "@/components/magicui/shine-border";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";

export default function LoginPage() {
  const navigate = useNavigate();
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);

  const login = useMutation({
    mutationFn: () => api.login(password),
    onSuccess: () => navigate("/"),
    onError: (cause) => {
      if (cause instanceof ApiError && cause.status === 429) {
        setError("失败次数过多,已临时锁定,请稍后再试");
      } else if (cause instanceof ApiError && cause.status === 401) {
        setError("密码错误");
      } else {
        setError(cause instanceof Error ? cause.message : "登录失败");
      }
    },
  });

  return (
    <div className="relative flex min-h-screen items-center justify-center overflow-hidden bg-background">
      <AnimatedGridPattern
        numSquares={36}
        maxOpacity={0.08}
        duration={3}
        className="fill-foreground/10 stroke-foreground/10 [mask-image:radial-gradient(600px_circle_at_center,white,transparent)]"
      />
      <Card className="relative w-full max-w-sm overflow-hidden">
        <ShineBorder shineColor="var(--foreground)" duration={12} />
        <CardHeader>
          <CardTitle className="text-base">lite-agentify 管理台</CardTitle>
          <CardDescription>输入配置文件中设置的管理密码</CardDescription>
        </CardHeader>
        <CardContent>
          <form
            className="space-y-3"
            onSubmit={(event) => {
              event.preventDefault();
              setError(null);
              login.mutate();
            }}
          >
            <Input
              type="password"
              autoFocus
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              placeholder="管理密码"
            />
            {error && <p className="text-xs text-destructive">{error}</p>}
            <Button
              type="submit"
              className="w-full"
              disabled={login.isPending || password.length === 0}
            >
              {login.isPending ? "登录中…" : "登录"}
            </Button>
          </form>
        </CardContent>
      </Card>
    </div>
  );
}
