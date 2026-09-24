export {};

export interface RichTextContext {
  tr?: (source: string) => string;
  runtimeIcon?: (name: string) => SVGSVGElement;
  setText?: (id: string, value: unknown) => void;
}

declare function tr(source: string): string;
declare function runtimeIcon(name: string): SVGSVGElement;
declare function setText(id: string, value: unknown): void;

function resolveTr(source: string, ctx?: RichTextContext): string {
  if (ctx?.tr) return ctx.tr(source);
  if (typeof tr === "function") return tr(source);
  return source;
}

function resolveIcon(name: string, ctx?: RichTextContext): SVGSVGElement {
  if (ctx?.runtimeIcon) return ctx.runtimeIcon(name);
  if (typeof runtimeIcon === "function") return runtimeIcon(name);
  return document.createElementNS("http://www.w3.org/2000/svg", "svg");
}

function resolveSetText(id: string, value: unknown, ctx?: RichTextContext): void {
  if (ctx?.setText) { ctx.setText(id, value); return; }
  if (typeof setText === "function") { setText(id, value); return; }
  const node = document.getElementById(id);
  if (node) node.textContent = value == null ? "—" : String(value);
}

export function appendLinkifiedText(parent: HTMLElement, text: string): void {
  const pattern = /https?:\/\/[^\s<>{}\[\]]+/g;
  let cursor = 0;
  for (const match of text.matchAll(pattern)) {
    const index = match.index || 0;
    if (index > cursor) parent.appendChild(document.createTextNode(text.slice(cursor, index)));
    let href = match[0];
    let trailing = "";
    while (/[.,;:!?)]$/.test(href)) {
      trailing = href.slice(-1) + trailing;
      href = href.slice(0, -1);
    }
    const link = document.createElement("a");
    link.href = href;
    link.target = "_blank";
    link.rel = "noopener noreferrer";
    link.textContent = href;
    parent.appendChild(link);
    if (trailing) parent.appendChild(document.createTextNode(trailing));
    cursor = index + match[0].length;
  }
  if (cursor < text.length) parent.appendChild(document.createTextNode(text.slice(cursor)));
}

export function messageLineStartsBlock(line: string): boolean {
  return /^```/.test(line)
    || /^#{1,3}\s+/.test(line)
    || /^>\s?/.test(line)
    || /^\s*[-*+]\s+/.test(line)
    || /^\s*\d+[.)]\s+/.test(line);
}

export function appendMessageParagraph(parent: HTMLElement, lines: string[]): void {
  if (!lines.length) return;
  const paragraph = document.createElement("p");
  paragraph.className = "message-paragraph";
  lines.forEach((line, index) => {
    if (index) paragraph.appendChild(document.createElement("br"));
    appendLinkifiedText(paragraph, line);
  });
  parent.appendChild(paragraph);
}

export function appendMessageCode(
  parent: HTMLElement,
  language: string,
  codeText: string,
  ctx?: RichTextContext
): void {
  const block = document.createElement("section");
  block.className = "message-code";
  const header = document.createElement("header");
  const label = document.createElement("span");
  label.textContent = language || "code";
  const copy = document.createElement("button");
  copy.type = "button";
  copy.className = "message-code-copy";
  copy.appendChild(resolveIcon("copy", ctx));
  const copyLabel = document.createElement("span");
  copyLabel.textContent = resolveTr("Copy code", ctx);
  copy.appendChild(copyLabel);
  copy.title = resolveTr("Copy code", ctx);
  copy.setAttribute("aria-label", resolveTr("Copy code", ctx));
  copy.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(codeText);
      copyLabel.textContent = resolveTr("Code copied", ctx);
      resolveSetText("runtime-message-announcer", resolveTr("Code copied", ctx), ctx);
      window.setTimeout(() => { copyLabel.textContent = resolveTr("Copy code", ctx); }, 1400);
    } catch {
      copyLabel.textContent = resolveTr("Unable to copy code", ctx);
      resolveSetText("runtime-message-announcer", resolveTr("Unable to copy code", ctx), ctx);
      window.setTimeout(() => { copyLabel.textContent = resolveTr("Copy code", ctx); }, 1800);
    }
  });
  header.appendChild(label);
  header.appendChild(copy);
  const pre = document.createElement("pre");
  const code = document.createElement("code");
  if (language) code.dataset.language = language;
  code.textContent = codeText;
  pre.appendChild(code);
  block.appendChild(header);
  block.appendChild(pre);
  parent.appendChild(block);
}

export function appendRichMessage(
  bubble: HTMLElement,
  sourceValue: unknown,
  ctx?: RichTextContext
): void {
  const source = String(sourceValue || "").replace(/\r\n?/g, "\n");
  const lines = source.split("\n");
  const body = document.createElement("div");
  body.className = "message-body";
  let index = 0;
  while (index < lines.length) {
    const line = lines[index];
    if (!line.trim()) { index += 1; continue; }
    const fence = /^```\s*([^\s`]*)/.exec(line);
    if (fence) {
      const codeLines: string[] = [];
      index += 1;
      while (index < lines.length && !/^```\s*$/.test(lines[index])) {
        codeLines.push(lines[index]);
        index += 1;
      }
      if (index < lines.length) index += 1;
      appendMessageCode(body, fence[1] || "", codeLines.join("\n"), ctx);
      continue;
    }
    const heading = /^(#{1,3})\s+(.+)$/.exec(line);
    if (heading) {
      const title = document.createElement(heading[1].length === 1 ? "h3" : heading[1].length === 2 ? "h4" : "h5");
      title.className = "message-heading";
      appendLinkifiedText(title, heading[2]);
      body.appendChild(title);
      index += 1;
      continue;
    }
    if (/^>\s?/.test(line)) {
      const quote = document.createElement("blockquote");
      const quoteLines: string[] = [];
      while (index < lines.length && /^>\s?/.test(lines[index])) {
        quoteLines.push(lines[index].replace(/^>\s?/, ""));
        index += 1;
      }
      appendMessageParagraph(quote, quoteLines);
      body.appendChild(quote);
      continue;
    }
    const unordered = /^\s*[-*+]\s+/.test(line);
    const ordered = /^\s*\d+[.)]\s+/.test(line);
    if (unordered || ordered) {
      const list = document.createElement(ordered ? "ol" : "ul");
      const pattern = ordered ? /^\s*\d+[.)]\s+/ : /^\s*[-*+]\s+/;
      while (index < lines.length && pattern.test(lines[index])) {
        const item = document.createElement("li");
        appendLinkifiedText(item, lines[index].replace(pattern, ""));
        list.appendChild(item);
        index += 1;
      }
      body.appendChild(list);
      continue;
    }
    const paragraphLines: string[] = [];
    while (index < lines.length && lines[index].trim() && !messageLineStartsBlock(lines[index])) {
      paragraphLines.push(lines[index]);
      index += 1;
    }
    if (!paragraphLines.length) {
      paragraphLines.push(line);
      index += 1;
    }
    appendMessageParagraph(body, paragraphLines);
  }
  bubble.appendChild(body);
  if (source.length <= 2200 && lines.length <= 36) return;
  body.classList.add("is-collapsed");
  const toggle = document.createElement("button");
  toggle.type = "button";
  toggle.className = "message-expand";
  toggle.textContent = resolveTr("Show full message", ctx);
  toggle.setAttribute("aria-expanded", "false");
  toggle.addEventListener("click", () => {
    const expanded = body.classList.toggle("is-expanded");
    body.classList.toggle("is-collapsed", !expanded);
    toggle.textContent = resolveTr(expanded ? "Collapse message" : "Show full message", ctx);
    toggle.setAttribute("aria-expanded", expanded ? "true" : "false");
  });
  bubble.appendChild(toggle);
}
