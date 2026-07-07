import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import CodeMirror from "@uiw/react-codemirror";
import { StreamLanguage } from "@codemirror/language";
import { toml } from "@codemirror/legacy-modes/mode/toml";
import { githubLight, githubDark } from "@uiw/codemirror-theme-github";
import { ApiError, api, type ConfigPayload } from "@/api";
import { Button } from "@/components/ui/button";
import { useTheme } from "@/lib/theme";

type Banner =
  | { kind: "success"; text: string; warnings: string[] }
  | { kind: "error"; text: string }
  | { kind: "conflict"; text: string; fresh?: ConfigPayload };

export default function ConfigPage() {
  const [, resolvedTheme] = useTheme();
  const queryClient = useQueryClient();
  const configQuery = useQuery({ queryKey: ["config"], queryFn: api.getConfig });
  // null = 未编辑,跟随服务器内容;编辑后保存失败也绝不丢草稿。
  const [draft, setDraft] = useState<string | null>(null);
  const [banner, setBanner] = useState<Banner | null>(null);

  const serverContent = configQuery.data?.content ?? "";
  const content = draft ?? serverContent;
  const dirty = draft !== null && draft !== serverContent;

  const save = useMutation({
    mutationFn: () => api.putConfig(content, configQuery.data!.hash),
    onSuccess: async (result) => {
      setBanner({
        kind: "success",
        text: result.message || "配置已保存并热重载",
        warnings: result.warnings ?? [],
      });
      setDraft(null);
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
    setDraft(null);
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

  return (
    <div className="space-y-4">
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
          <Button
            onClick={() => save.mutate()}
            disabled={save.isPending || !dirty}
          >
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
          <p className="font-medium">保存失败,草稿已保留</p>
          <p className="mt-1 whitespace-pre-wrap">{banner.text}</p>
        </div>
      )}
      {banner?.kind === "conflict" && (
        <div className="border border-amber-300 bg-amber-50 px-4 py-3 text-xs text-amber-800 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-200">
          <p className="font-medium">{banner.text}</p>
          <p className="mt-1">
            你的草稿仍在编辑器中。可以先复制自己的修改,再加载最新版本。
          </p>
          {banner.fresh && (
            <Button
              variant="outline"
              size="xs"
              className="mt-2 border-amber-400 bg-background hover:bg-amber-100 dark:border-amber-700 dark:hover:bg-amber-900"
              onClick={() => loadFresh(banner.fresh!)}
            >
              放弃草稿,加载磁盘上的最新版本
            </Button>
          )}
        </div>
      )}

      <div className="overflow-hidden border border-border bg-card">
        <CodeMirror
          value={content}
          height="560px"
          theme={resolvedTheme === "dark" ? githubDark : githubLight}
          extensions={[StreamLanguage.define(toml)]}
          onChange={(value) => setDraft(value)}
        />
      </div>
    </div>
  );
}
