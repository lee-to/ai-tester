/**
 * Compile a user-supplied regex pattern from a scenario YAML into a JS RegExp.
 * Accepts PCRE/Python-style inline flags at the start of the pattern — e.g.
 * `(?i)foo` or `(?ims)foo` — and rewrites them into a JS `flags` argument,
 * which V8 does not accept inline.
 *
 * Supported flags: i, m, s. Others are passed through as-is (which will
 * typically throw at construction time, surfacing the author error).
 */
export function compilePattern(pattern: string): RegExp {
  const inlineFlagMatch = /^\(\?([imsux-]+)\)/.exec(pattern);
  if (!inlineFlagMatch) return new RegExp(pattern);
  const raw = inlineFlagMatch[1]!;
  const src = pattern.slice(inlineFlagMatch[0].length);
  const flags = Array.from(raw)
    .filter((ch) => "ims".includes(ch))
    .join("");
  return new RegExp(src, flags);
}
