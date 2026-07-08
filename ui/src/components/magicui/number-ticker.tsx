// Vendored from Magic UI (https://magicui.design/r/number-ticker).
// Local changes: `format` prop replaces the hard-coded Intl formatting, reduced
// motion renders the final value immediately, color inherits from surrounding text.
import {
  useCallback,
  useEffect,
  useRef,
  type ComponentPropsWithoutRef,
} from "react"
import {
  useInView,
  useMotionValue,
  useReducedMotion,
  useSpring,
} from "motion/react"

import { cn } from "@/lib/utils"

interface NumberTickerProps extends ComponentPropsWithoutRef<"span"> {
  value: number
  startValue?: number
  direction?: "up" | "down"
  delay?: number
  decimalPlaces?: number
  format?: (value: number) => string
}

export function NumberTicker({
  value,
  startValue = 0,
  direction = "up",
  delay = 0,
  className,
  decimalPlaces = 0,
  format,
  ...props
}: NumberTickerProps) {
  const ref = useRef<HTMLSpanElement>(null)
  const reducedMotion = useReducedMotion()
  const motionValue = useMotionValue(direction === "down" ? value : startValue)
  const springValue = useSpring(motionValue, {
    damping: 60,
    stiffness: 100,
  })
  const isInView = useInView(ref, { once: true, margin: "0px" })

  const target = direction === "down" ? startValue : value

  const formatValue = useCallback(
    (latest: number) =>
      format
        ? format(latest)
        : Intl.NumberFormat("en-US", {
            minimumFractionDigits: decimalPlaces,
            maximumFractionDigits: decimalPlaces,
          }).format(Number(latest.toFixed(decimalPlaces))),
    [format, decimalPlaces],
  )

  useEffect(() => {
    if (!isInView) return
    if (reducedMotion) {
      if (ref.current) ref.current.textContent = formatValue(target)
      return
    }
    const timer = setTimeout(() => {
      motionValue.set(target)
    }, delay * 1000)
    return () => clearTimeout(timer)
  }, [motionValue, isInView, delay, target, reducedMotion, formatValue])

  useEffect(
    () =>
      springValue.on("change", (latest) => {
        if (ref.current) {
          ref.current.textContent = formatValue(latest)
        }
      }),
    [springValue, formatValue],
  )

  return (
    <span
      ref={ref}
      className={cn("inline-block tabular-nums", className)}
      {...props}
    >
      {formatValue(
        reducedMotion ? target : direction === "down" ? value : startValue,
      )}
    </span>
  )
}
