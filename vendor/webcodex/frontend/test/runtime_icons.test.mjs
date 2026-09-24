import test from "node:test";
import assert from "node:assert/strict";
import {
  RUNTIME_ICON_PATHS,
  runtimeIcon,
  createMessageAction,
} from "../dist/runtime_icons.js";

function createMockElement(tagName) {
  const children = [];
  const attributes = new Map();
  const eventListeners = new Map();
  const classListSet = new Set();

  const element = {
    tagName: tagName.toUpperCase(),
    style: {},
    dataset: {},
    childNodes: children,
    children,
    parentElement: null,
    get className() {
      return Array.from(classListSet).join(" ");
    },
    set className(val) {
      classListSet.clear();
      String(val)
        .split(/\s+/)
        .filter(Boolean)
        .forEach((c) => classListSet.add(c));
    },
    textContent: "",

    classList: {
      add(...tokens) {
        tokens.forEach((t) => classListSet.add(t));
      },
      remove(...tokens) {
        tokens.forEach((t) => classListSet.delete(t));
      },
      contains(token) {
        return classListSet.has(token);
      },
    },

    appendChild(child) {
      children.push(child);
      child.parentElement = element;
      return child;
    },

    setAttribute(key, value) {
      attributes.set(key, String(value));
      if (key === "class") {
        element.className = String(value);
      }
    },

    getAttribute(key) {
      return attributes.get(key) ?? null;
    },

    addEventListener(event, handler) {
      const list = eventListeners.get(event) || [];
      list.push(handler);
      eventListeners.set(event, list);
    },

    click() {
      const list = eventListeners.get("click") || [];
      for (const handler of list) handler({ type: "click" });
    },
  };

  return element;
}

function withMockDom(run) {
  const originalDocument = globalThis.document;
  globalThis.document = {
    createElement(tagName) {
      return createMockElement(tagName);
    },
    createElementNS(_ns, tagName) {
      return createMockElement(tagName);
    },
  };
  try {
    return run();
  } finally {
    globalThis.document = originalDocument;
  }
}

test("RUNTIME_ICON_PATHS contains definitions for all expected icon names", () => {
  const expectedNames = ["folder", "monitor", "message", "reply", "edit", "trash", "copy"];
  for (const name of expectedNames) {
    assert.ok(Array.isArray(RUNTIME_ICON_PATHS[name]));
    assert.ok(RUNTIME_ICON_PATHS[name].length > 0);
    for (const path of RUNTIME_ICON_PATHS[name]) {
      assert.equal(typeof path, "string");
      assert.ok(path.length > 0);
    }
  }
});

test("runtimeIcon creates SVG element with correct viewBox, aria-hidden, and paths", () => {
  withMockDom(() => {
    const svg = runtimeIcon("folder", "custom-class");
    assert.equal(svg.tagName, "SVG");
    assert.equal(svg.getAttribute("viewBox"), "0 0 24 24");
    assert.equal(svg.getAttribute("aria-hidden"), "true");
    assert.equal(svg.getAttribute("class"), "custom-class");
    assert.equal(svg.children.length, RUNTIME_ICON_PATHS.folder.length);
    for (let i = 0; i < svg.children.length; i++) {
      assert.equal(svg.children[i].tagName, "PATH");
      assert.equal(svg.children[i].getAttribute("d"), RUNTIME_ICON_PATHS.folder[i]);
    }
  });
});

test("createMessageAction creates accessible action button with click listener and danger tone", () => {
  withMockDom(() => {
    let actionFired = 0;
    const button = createMessageAction("Delete message", "trash", () => {
      actionFired++;
    }, true);

    assert.equal(button.tagName, "BUTTON");
    assert.equal(button.type, "button");
    assert.ok(button.className.includes("message-action"));
    assert.ok(button.className.includes("danger"));
    assert.equal(button.title, "Delete message");
    assert.equal(button.getAttribute("aria-label"), "Delete message");

    // Check SVG child
    assert.equal(button.children.length, 1);
    assert.equal(button.children[0].tagName, "SVG");

    button.click();
    assert.equal(actionFired, 1);

    // Non-danger action
    const replyButton = createMessageAction("Reply to message", "reply", () => {});
    assert.ok(replyButton.className.includes("message-action"));
    assert.equal(replyButton.className.includes("danger"), false);
  });
});
