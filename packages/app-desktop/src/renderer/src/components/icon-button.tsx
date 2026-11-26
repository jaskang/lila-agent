import * as React from "react";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "cn";

export function IconButton({
  label,
  shortcut,
  disabled,
  size = "icon-sm",
  ariaPressed,
  className,
  buttonRef,
  onClick,
  children,
}: {
  label: string;
  shortcut?: string | undefined;
  disabled?: boolean;
  size?: "icon-xs" | "icon-sm";
  ariaPressed?: boolean;
  className?: string;
  buttonRef?: React.Ref<HTMLButtonElement>;
  onClick?: () => void;
  children: React.ReactNode;
}) {
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <Button
            ref={buttonRef}
            type="button"
            size={size}
            variant="ghost"
            aria-label={label}
            aria-pressed={ariaPressed}
            disabled={disabled}
            className={cn("text-muted-foreground hover:text-foreground", className)}
            onClick={onClick}
          />
        }
      >
        {children}
      </TooltipTrigger>
      <TooltipContent>
        {label}
        {shortcut ? <Kbd>{shortcut}</Kbd> : null}
      </TooltipContent>
    </Tooltip>
  );
}
