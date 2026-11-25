import * as React from "react";
import { Search, X } from "lucide-react";
import { Input } from "@/components/ui/input";
import { SidebarInput } from "@/components/ui/sidebar";
import { cn } from "cn";

export function SearchInput({
  value,
  onChange,
  placeholder = "搜索",
  ariaLabel = "搜索",
  variant = "default",
  autoFocus,
  stopEscapePropagation,
  className,
  inputClassName,
}: {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  ariaLabel?: string;
  variant?: "default" | "sidebar";
  autoFocus?: boolean;
  stopEscapePropagation?: boolean;
  className?: string;
  inputClassName?: string;
}) {
  const Field = variant === "sidebar" ? SidebarInput : Input;
  return (
    <div className={cn("relative min-w-0 flex-1", className)}>
      <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
      <Field
        autoFocus={autoFocus}
        value={value}
        placeholder={placeholder}
        aria-label={ariaLabel}
        className={cn("h-8 pl-8 text-xs", value && "pr-7", inputClassName)}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={(event) => {
          if (event.nativeEvent.isComposing || event.keyCode === 229) return;
          if (event.key === "Escape" && value) {
            event.preventDefault();
            if (stopEscapePropagation) event.stopPropagation();
            onChange("");
          }
        }}
      />
      {value ? (
        <button
          type="button"
          aria-label="清空"
          onClick={() => onChange("")}
          className="absolute top-1/2 right-1.5 -translate-y-1/2 rounded-sm p-0.5 text-muted-foreground hover:bg-muted hover:text-foreground"
        >
          <X className="size-3.5" />
        </button>
      ) : null}
    </div>
  );
}
