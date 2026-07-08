/** 配置表单的通用小部件:字段包装、密钥输入(reveal + 复制)。 */

import { useEffect, useRef, useState, type ReactNode } from "react";
import { CheckIcon, CopyIcon } from "lucide-react";
import { api } from "@/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { isMasked } from "./model";

export function Field({
  label,
  children,
  className,
}: {
  label: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <label className={className}>
      <span className="mb-1 block text-xs text-muted-foreground">{label}</span>
      {children}
    </label>
  );
}

/** 写剪贴板,非安全上下文(如局域网 HTTP)退回隐藏 textarea。 */
async function copyText(text: string): Promise<void> {
  if (navigator.clipboard && window.isSecureContext) {
    await navigator.clipboard.writeText(text);
    return;
  }
  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.appendChild(textarea);
  textarea.select();
  try {
    document.execCommand("copy");
  } finally {
    document.body.removeChild(textarea);
  }
}

/**
 * 密钥输入:默认显示 __MASKED__ 哨兵,保持不动即保留原值,输入新值即替换。
 * 复制按钮对未改动的密钥调用 reveal 取回真实值(明文只进剪贴板,不进表单),
 * 已编辑的值直接复制输入框内容。
 */
export function SecretInput({
  value,
  onChange,
  revealField,
  placeholder,
}: {
  value: string;
  onChange: (value: string) => void;
  /** reveal 的字段引用;条目尚未持久化(新增)时为 null,只能复制输入框内容。 */
  revealField: string | null;
  placeholder?: string;
}) {
  const [copied, setCopied] = useState<"idle" | "copied" | "error">("idle");
  const resetTimer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(resetTimer.current), []);

  const flash = (state: "copied" | "error") => {
    setCopied(state);
    window.clearTimeout(resetTimer.current);
    resetTimer.current = window.setTimeout(() => setCopied("idle"), 1500);
  };

  const copy = async () => {
    try {
      const text =
        isMasked(value) && revealField
          ? (await api.revealSecret(revealField)).value
          : value;
      await copyText(text);
      flash("copied");
    } catch {
      flash("error");
    }
  };

  // 掩码但没有 reveal 引用(哨兵值出现在新增条目里)时复制无意义。
  const copyDisabled = isMasked(value) && !revealField;

  return (
    <div className="flex items-center gap-1">
      <Input
        value={value}
        onChange={(event) => onChange(event.target.value)}
        placeholder={placeholder}
        autoComplete="off"
        spellCheck={false}
        className="font-mono"
      />
      <Button
        type="button"
        variant="ghost"
        size="icon-sm"
        title={copied === "error" ? "复制失败" : "复制真实值"}
        disabled={copyDisabled}
        onClick={copy}
      >
        {copied === "copied" ? (
          <CheckIcon className="text-emerald-600" />
        ) : (
          <CopyIcon className={copied === "error" ? "text-destructive" : undefined} />
        )}
      </Button>
    </div>
  );
}
