// Pretty-printing of response bodies.

/** Pretty-prints JSON (unchanged when it does not parse). */
export function formatJson(text: string): string {
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return text;
  }
}

/**
 * Indents XML/HTML-like markup, one element per line; text and CDATA are kept as they are.
 * Unbalanced input is formatted as far as it goes, never changed otherwise.
 */
export function formatXml(text: string, indent = "  "): string {
  const tokens = text.match(/<!\[CDATA\[[\s\S]*?\]\]>|<!--[\s\S]*?-->|<[^>]+>|[^<]+/g);
  if (!tokens) return text;
  let depth = 0;
  const out: string[] = [];
  for (let i = 0; i < tokens.length; i++) {
    const t = tokens[i]!;
    if (!t.startsWith("<")) {
      const s = t.trim();
      if (s) out.push(indent.repeat(depth) + s);
      continue;
    }
    const closing = t.startsWith("</");
    const selfClosing = t.endsWith("/>") || /^<[?!]/.test(t);
    if (closing) depth = Math.max(0, depth - 1);
    // `<a>text</a>` stays on one line.
    const next = tokens[i + 1];
    const after = tokens[i + 2];
    if (!closing && !selfClosing && next && !next.startsWith("<") && after?.startsWith("</")) {
      out.push(indent.repeat(depth) + t + next.trim() + after);
      i += 2;
      continue;
    }
    out.push(indent.repeat(depth) + t);
    if (!closing && !selfClosing) depth++;
  }
  return out.join("\n");
}
