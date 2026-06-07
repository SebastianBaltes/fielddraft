import { useEffect, useState } from "react";

/**
 * Subscribe to a CSS media query and re-render on change.
 *
 * Returns `false` in environments without `matchMedia` (e.g. jsdom),
 * so callers can treat "no match" as the desktop default.
 */
export function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState<boolean>(() => readMatch(query));

  useEffect(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
      return;
    }
    const mql = window.matchMedia(query);
    const onChange = () => setMatches(mql.matches);
    // Sync once in case the query changed between render and effect.
    onChange();
    mql.addEventListener("change", onChange);
    return () => mql.removeEventListener("change", onChange);
  }, [query]);

  return matches;
}

function readMatch(query: string): boolean {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
    return false;
  }
  return window.matchMedia(query).matches;
}

/**
 * Phone-width breakpoint. Kept in sync with the `768px` literal used by
 * the responsive CSS (`@media (max-width: 768px)` in shell/outliner/…).
 * Media queries can't read CSS custom properties, so this single string
 * is the JS half of that contract — change both together.
 */
export const MOBILE_QUERY = "(max-width: 768px)";
