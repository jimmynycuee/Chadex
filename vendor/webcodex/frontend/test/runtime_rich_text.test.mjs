import test from "node:test";
import assert from "node:assert/strict";
import {
  messageLineStartsBlock,
  appendLinkifiedText,
  appendMessageParagraph,
  appendRichMessage,
} from "../dist/runtime_rich_text.js";

function createMockElement(tag = "div") {
  return {
    tagName: tag.toUpperCase(),
    className: "",
    textContent: "",
    dataset: {},
    children: [],
    childNodes: [],
    attributes: {},
    classList: {
      classes: new Set(),
      add(cls) { this.classes.add(cls); },
      remove(cls) { this.classes.delete(cls); },
      toggle(cls, force) {
        if (force === undefined) {
          if (this.classes.has(cls)) { this.classes.delete(cls); return false; }
          this.classes.add(cls); return true;
        }
        if (force) { this.classes.add(cls); return true; }
        this.classes.delete(cls); return false;
      },
      has(cls) { return this.classes.has(cls); },
    },
    appendChild(child) {
      this.children.push(child);
      this.childNodes.push(child);
      return child;
    },
    setAttribute(key, value) { this.attributes[key] = value; },
    getAttribute(key) { return this.attributes[key] ?? null; },
    addEventListener() {},
  };
}

function withMockDom(fn) {
  const originalDocument = globalThis.document;
  globalThis.document = {
    createElement(tag) { return createMockElement(tag); },
    createElementNS(_ns, tag) { return createMockElement(tag); },
    createTextNode(text) { return { nodeType: 3, textContent: text }; },
  };
  try {
    return fn();
  } finally {
    globalThis.document = originalDocument;
  }
}

test("messageLineStartsBlock detects markdown block boundaries", () => {
  assert.equal(messageLineStartsBlock("```rust"), true);
  assert.equal(messageLineStartsBlock("# Heading 1"), true);
  assert.equal(messageLineStartsBlock("## Heading 2"), true);
  assert.equal(messageLineStartsBlock("### Heading 3"), true);
  assert.equal(messageLineStartsBlock("> quote line"), true);
  assert.equal(messageLineStartsBlock("- bullet item"), true);
  assert.equal(messageLineStartsBlock("* bullet item"), true);
  assert.equal(messageLineStartsBlock("1. ordered item"), true);
  assert.equal(messageLineStartsBlock("normal plain text"), false);
  assert.equal(messageLineStartsBlock(""), false);
});

test("appendLinkifiedText parses URLs into anchor tags", () => {
  withMockDom(() => {
    const parent = createMockElement();
    appendLinkifiedText(parent, "Visit https://example.com for more information.");
    assert.equal(parent.childNodes.length, 3);
    assert.equal(parent.childNodes[0].textContent, "Visit ");
    assert.equal(parent.childNodes[1].tagName, "A");
    assert.equal(parent.childNodes[1].href, "https://example.com");
    assert.equal(parent.childNodes[2].textContent, " for more information.");
  });
});

test("appendRichMessage generates structured DOM without innerHTML", () => {
  withMockDom(() => {
    const bubble = createMockElement();
    const markdown = `# Title\n\nSome paragraph text\n\n\`\`\`ts\nconst x = 1;\n\`\`\`\n\n> quote`;
    appendRichMessage(bubble, markdown, {
      tr: (s) => s,
      runtimeIcon: () => createMockElement("svg"),
    });
    assert.equal(bubble.children.length, 1);
    const body = bubble.children[0];
    assert.equal(body.className, "message-body");
    assert.equal(body.children.length >= 4, true);
  });
});
