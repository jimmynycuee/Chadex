#!/usr/bin/env node

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);

const COMPOSER_SELECTOR = [
  '[data-testid="prompt-textarea"]',
  "#prompt-textarea",
  '[contenteditable="true"][data-lexical-editor="true"]',
].join(", ");
const EFFORT_CONTROL_SELECTOR = [
  'button[aria-haspopup="menu"][data-tone="neutral"]',
  'button[data-testid="model-switcher-dropdown-button"][aria-haspopup="menu"]',
].join(", ");
const EFFORT_SLIDER_CONTAINER = "[data-model-reasoning-effort-slider]";
const EFFORT_OPTION_SELECTOR = [
  '[role="menuitem"]',
  '[role="menuitemradio"]',
  '[role="option"]',
  '[role="radio"]',
  '[data-radix-collection-item]',
  'button',
].join(", ");
const ASSISTANT_TURN_SELECTOR = [
  '[data-testid^="conversation-turn-"][data-turn="assistant"]',
  '[data-testid^="conversation-turn-"][data-message-author-role="assistant"]',
  '[data-testid^="conversation-turn-"]:has([data-message-author-role="assistant"])',
].join(", ");
const USER_TURN_SELECTOR = [
  '[data-testid^="conversation-turn-"][data-turn="user"]',
  '[data-testid^="conversation-turn-"][data-message-author-role="user"]',
  '[data-testid^="conversation-turn-"]:has([data-message-author-role="user"])',
].join(", ");
const STOP_BUTTON_SELECTOR = '[data-testid="stop-button"]';
const COMPLETION_ACTION_SELECTOR = 'button[data-testid="copy-turn-action-button"]';
const CONNECTOR_PICKER_CANDIDATE_SELECTOR = [
  '.popover [role="menuitem"]',
  '.popover [role="option"]',
  '.popover [tabindex="0"]',
  '[role="dialog"] [role="menuitem"]',
  '[role="dialog"] [role="option"]',
  '[role="dialog"] [tabindex="0"]',
  '[cmdk-item]',
  '[data-radix-collection-item]',
  'button[data-testid*="connector" i]',
  'button[data-testid*="plugin" i]',
  '[role="menuitem"][data-testid]',
  '[role="option"][data-testid]',
].join(", ");
const CONNECTOR_SEARCH_INPUT_SELECTOR = [
  '.popover input[type="search"]',
  '.popover input[placeholder]',
  '[role="dialog"] input[type="search"]',
  '[role="dialog"] input[placeholder]',
  '[cmdk-input]',
  '.popover [contenteditable="true"]',
  '[role="dialog"] [contenteditable="true"]',
].join(", ");
const SELECTED_CONNECTOR_STRONG_SELECTOR = [
  '[data-id^="plugin:"][data-keyword]',
  '[data-keyword]',
  'button[data-testid*="connector" i]',
  '[role="button"][data-testid*="connector" i]',
  'button[data-testid*="plugin" i]',
  '[role="button"][data-testid*="plugin" i]',
].join(", ");
const SELECTED_CONNECTOR_SEMANTIC_SELECTOR = [
  SELECTED_CONNECTOR_STRONG_SELECTOR,
  'button[aria-label]',
  '[role="button"][aria-label]',
  'button[title]',
  '[role="button"][title]',
].join(", ");
const BENCH_PAGE_MARKER = "chadex-chatgpt-completion-benchmark";

function fail(message) {
  throw new Error(message);
}

function requiredEnv(name) {
  const value = process.env[name];
  if (!value) fail(name + " is required");
  return value;
}

async function liveBrowserDescriptor(candidates) {
  const deadline = Date.now() + 20000;
  const expectedPid = process.env.BENCH_EXPECT_LAUNCHER_PID || null;
  const expectedEndpoint = process.env.BENCH_EXPECT_LAUNCHER_ENDPOINT || null;
  let diagnostics = [];

  while (Date.now() < deadline) {
    diagnostics = [];
    for (const candidate of candidates) {
      if (!fs.existsSync(candidate)) {
        diagnostics.push({ path: candidate, status: "missing" });
        continue;
      }
      try {
        const descriptor = JSON.parse(fs.readFileSync(candidate, "utf8"));
        const endpoint = new URL(descriptor.endpoint);
        if (expectedPid !== null && String(descriptor.pid) !== expectedPid) {
          diagnostics.push({
            path: candidate,
            status: "launcher_identity_changed",
            expected_pid: expectedPid,
            observed_pid: descriptor.pid,
          });
          continue;
        }
        if (expectedEndpoint !== null && endpoint.origin !== expectedEndpoint) {
          diagnostics.push({
            path: candidate,
            status: "launcher_identity_changed",
            expected_endpoint: expectedEndpoint,
            observed_endpoint: endpoint.origin,
          });
          continue;
        }
        if (endpoint.protocol !== "http:" || endpoint.hostname !== "127.0.0.1" || !endpoint.port) {
          diagnostics.push({ path: candidate, status: "invalid_endpoint" });
          continue;
        }
        const response = await fetch(new URL("/json/version", endpoint.origin), {
          signal: AbortSignal.timeout(1500),
        });
        if (!response.ok) {
          diagnostics.push({ path: candidate, status: "unhealthy", http_status: response.status });
          continue;
        }
        await response.text();
        return { descriptorPath: candidate, descriptor, endpoint };
      } catch (error) {
        diagnostics.push({
          path: candidate,
          status: "unhealthy",
          error: String(error).slice(0, 180),
        });
      }
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  fail(
    "fixed launcher browser is unavailable or changed; start Codex Web GPT before the campaign and keep that browser session alive; diagnostics=" +
      JSON.stringify(diagnostics),
  );
}

function integerAttribute(value, label) {
  if (value === null || !/^-?\d+$/.test(value)) {
    fail("invalid " + label + ": " + JSON.stringify(value));
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed)) fail("invalid " + label + ": " + JSON.stringify(value));
  return parsed;
}

function visible(locator) {
  return locator.isVisible().catch(() => false);
}

async function countVisible(locator) {
  const count = await locator.count();
  let result = 0;
  for (let index = 0; index < count; index += 1) {
    if (await visible(locator.nth(index))) result += 1;
  }
  return result;
}

function isTemporaryChat(urlString) {
  try {
    const url = new URL(urlString);
    return (
      url.origin === "https://chatgpt.com" &&
      url.pathname === "/" &&
      url.searchParams.get("temporary-chat") === "true"
    );
  } catch {
    return false;
  }
}

function isChatGPTAuthPage(urlString) {
  try {
    const url = new URL(urlString);
    return url.origin === "https://chatgpt.com" && url.pathname.startsWith("/auth/");
  } catch {
    return false;
  }
}

function failIfChatGPTAuthRequired(page) {
  if (isChatGPTAuthPage(page.url())) {
    fail(
      "ChatGPT launcher session is not authenticated; sign in once in the Codex Web GPT production launcher",
    );
  }
}

async function dismissKnownBlockingModals(page) {
  const historyRateLimit = page.getByTestId("modal-conversation-history-rate-limit");
  if (!(await historyRateLimit.isVisible().catch(() => false))) return null;

  const text = (await historyRateLimit.innerText().catch(() => "")).trim();
  const buttons = historyRateLimit.locator("button").filter({ visible: true });
  const count = await buttons.count().catch(() => 0);
  if (count !== 1) {
    fail(
      "conversation-history rate-limit modal is visible but does not have exactly one dismiss button",
    );
  }
  await buttons.first().click({ timeout: 5000 });
  await historyRateLimit.waitFor({ state: "hidden", timeout: 5000 });
  await page.waitForTimeout(1200);
  if (await historyRateLimit.isVisible().catch(() => false)) {
    fail(
      "ChatGPT conversation-history rate limit is active; benchmark turn was not submitted",
    );
  }
  return {
    kind: "conversation_history_rate_limit",
    text: text.slice(0, 240),
  };
}

async function clickWithBlockingModalRecovery(
  page,
  locator,
  { timeout = 10000, attempts = 3 } = {},
) {
  let lastError = null;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    await dismissKnownBlockingModals(page);
    try {
      await locator.click({ timeout });
      return;
    } catch (error) {
      lastError = error;
      const historyRateLimit = page.getByTestId("modal-conversation-history-rate-limit");
      const modalVisible = await historyRateLimit.isVisible().catch(() => false);
      const blockedByOverlay = /intercepts pointer events|modal-conversation-history-rate-limit/i.test(
        String(error),
      );
      if (!modalVisible && !blockedByOverlay) throw error;
      await dismissKnownBlockingModals(page);
      await page.waitForTimeout(180);
    }
  }
  throw lastError ?? new Error("click failed after blocking-modal recovery");
}

async function gotoTemporaryChat(page, timeoutMs = 60000, attempts = 3, requireClean = true) {
  let lastError = null;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      await page.goto("https://chatgpt.com/?temporary-chat=true", {
        waitUntil: "domcontentloaded",
        timeout: timeoutMs,
      });
    } catch (error) {
      lastError = error;
      const message = String(error);
      if (!/ERR_ABORTED|navigation|interrupted|net::ERR_/i.test(message)) throw error;
    }

    try {
      failIfChatGPTAuthRequired(page);
      await dismissKnownBlockingModals(page);
      await activeComposer(page, Math.min(timeoutMs, 30000));
      if (isTemporaryChat(page.url())) {
        if (!requireClean || await pageIsCleanTemporaryChat(page)) return;
      }
    } catch (error) {
      lastError = error;
    }
    await page.waitForTimeout(250 * (attempt + 1));
  }
  if (lastError) throw lastError;
  fail("ChatGPT Temporary Chat navigation did not reach a usable page");
}

async function activeComposer(page, timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    failIfChatGPTAuthRequired(page);
    const composers = page.locator(COMPOSER_SELECTOR);
    const count = await composers.count().catch(() => 0);
    for (let index = count - 1; index >= 0; index -= 1) {
      const candidate = composers.nth(index);
      if (await visible(candidate)) return candidate;
    }
    await page.waitForTimeout(100);
  }
  fail("ChatGPT composer did not become visible");
}

async function pageIsCleanTemporaryChat(page) {
  if (!isTemporaryChat(page.url())) return false;
  let composer;
  try {
    composer = await activeComposer(page, 1500);
  } catch {
    return false;
  }
  const users = await page.locator(USER_TURN_SELECTOR).count();
  const assistants = await page.locator(ASSISTANT_TURN_SELECTOR).count();
  const selectedConnectors = await composer.locator(SELECTED_CONNECTOR_STRONG_SELECTOR).count();
  const draft = await composer.evaluate((node) =>
    node instanceof HTMLTextAreaElement || node instanceof HTMLInputElement
      ? node.value
      : node.innerText ?? node.textContent ?? "",
  );
  return users === 0 && assistants === 0 && selectedConnectors === 0 && !draft.trim();
}

async function chooseTemporaryPage(browser) {
  const contexts = browser.contexts();
  if (contexts.length === 0) fail("launcher browser exposed no browser context");

  const clean = [];
  const marked = [];
  for (const context of contexts) {
    for (const page of context.pages()) {
      const isMarked = isTemporaryChat(page.url()) && await page
        .evaluate((marker) => window.name === marker, BENCH_PAGE_MARKER)
        .catch(() => false);
      if (isMarked) {
        marked.push(page);
      }
      if (await pageIsCleanTemporaryChat(page)) clean.push(page);
    }
  }
  if (marked.length > 1) fail("multiple benchmark-owned ChatGPT pages; refusing ambiguous selection");
  if (marked.length === 1) {
    const page = marked[0];
    if (!(await pageIsCleanTemporaryChat(page))) {
      await gotoTemporaryChat(page);
    }
    if (!(await pageIsCleanTemporaryChat(page))) fail("benchmark page did not reset to a clean Temporary Chat");
    return { page, created: false };
  }
  if (clean.length > 0) {
    const page = clean.at(-1);
    await page.evaluate((marker) => { window.name = marker; }, BENCH_PAGE_MARKER);
    return { page, created: false };
  }

  const context = contexts[0];
  const existingPages = context.pages();
  if (existingPages.length === 0) fail("launcher browser exposed no reusable page");
  const page = existingPages.at(-1);
  await gotoTemporaryChat(page);
  if (!(await pageIsCleanTemporaryChat(page))) {
    fail("reused ChatGPT page did not become a clean Temporary Chat");
  }
  await page.evaluate((marker) => { window.name = marker; }, BENCH_PAGE_MARKER);
  return { page, created: false };
}

async function setExtraHigh(page) {
  await dismissKnownBlockingModals(page);
  const composer = await activeComposer(page);
  const form = composer.locator("xpath=ancestor::form[1]");
  const knownEffortLabel = /^(即時|思考|低|中|高|極高|instant|thinking|low|medium|high|extra\s*high)$/i;
  const isExtraHighLabel = (label) => /^(極高|extra\s*high)$/i.test(label.trim());
  const chooseEffortControl = async (scope) => {
    const controls = scope.locator(EFFORT_CONTROL_SELECTOR).filter({ visible: true });
    const count = await controls.count().catch(() => 0);
    const matches = [];
    for (let index = 0; index < count; index += 1) {
      const candidate = controls.nth(index);
      const label = (await candidate.innerText().catch(() => "")).trim();
      if (knownEffortLabel.test(label)) matches.push({ index, label });
    }
    if (matches.length === 1) return controls.nth(matches[0].index);
    return null;
  };

  let control = await chooseEffortControl(form);
  const effortDeadline = Date.now() + 10000;
  while (!control && Date.now() < effortDeadline) {
    control = await chooseEffortControl(page);
    if (control && await visible(control)) break;
    await page.waitForTimeout(100);
  }
  if (!control || !(await visible(control))) fail("ChatGPT effort control is unavailable");
  const currentLabel = (await control.innerText().catch(() => "")).trim();
  if (isExtraHighLabel(currentLabel)) {
    return { requested: "extra-high", observed_label: currentLabel, verified_by: "button_text" };
  }

  await clickWithBlockingModalRecovery(page, control, { timeout: 10000 });
  await page.waitForTimeout(150);

  const optionRows = page.locator(EFFORT_OPTION_SELECTOR).filter({ visible: true });
  const optionMatches = await optionRows.evaluateAll((nodes) => {
    const normalize = (value) =>
      String(value ?? "")
        .normalize("NFKC")
        .replace(/\s+/g, " ")
        .trim()
        .toLocaleLowerCase("en-US");
    const targets = new Set(["極高", "extra high"]);
    return nodes
      .map((node, index) => {
        const text = normalize(node.innerText ?? node.textContent ?? "");
        if (!text) return null;
        if (targets.has(text)) return { index, score: 1000, text };
        if (
          text.length <= 80 &&
          (text.startsWith("極高 ") || text.startsWith("extra high "))
        ) {
          return { index, score: 900, text };
        }
        return null;
      })
      .filter(Boolean)
      .sort((a, b) => b.score - a.score || a.index - b.index);
  }).catch(() => []);

  if (optionMatches.length > 0) {
    const best = optionMatches[0];
    const tied = optionMatches.filter((candidate) => candidate.score === best.score);
    if (tied.length !== 1) {
      fail("Extra High effort option is ambiguous: " + JSON.stringify(tied.slice(0, 8)));
    }
    await clickWithBlockingModalRecovery(page, optionRows.nth(best.index), { timeout: 10000 });
    const verifyDeadline = Date.now() + 5000;
    while (Date.now() < verifyDeadline) {
      const observed = (await control.innerText().catch(() => "")).trim();
      if (isExtraHighLabel(observed)) {
        await page.keyboard.press("Escape").catch(() => {});
        await page.waitForTimeout(200);
        await activeComposer(page, 3000);
        return {
          requested: "extra-high",
          observed_label: observed,
          verified_by: "semantic_menu_option",
        };
      }
      await page.waitForTimeout(100);
    }
    fail("Extra High effort option was selected but the control did not update");
  }

  const readSliderState = async (timeout = 1500) => {
    const container = page.locator(EFFORT_SLIDER_CONTAINER).filter({ visible: true }).last();
    if ((await container.count().catch(() => 0)) !== 1) return null;
    const slider = container.locator('[role="slider"]').last();
    if ((await slider.count().catch(() => 0)) !== 1) return null;
    const [minimumRaw, maximumRaw, currentRaw] = await Promise.all([
      slider.getAttribute("aria-valuemin", { timeout }).catch(() => null),
      slider.getAttribute("aria-valuemax", { timeout }).catch(() => null),
      slider.getAttribute("aria-valuenow", { timeout }).catch(() => null),
    ]);
    if (minimumRaw === null || maximumRaw === null || currentRaw === null) return null;
    return {
      slider,
      minimum: integerAttribute(minimumRaw, "effort minimum"),
      maximum: integerAttribute(maximumRaw, "effort maximum"),
      current: integerAttribute(currentRaw, "effort value"),
    };
  };

  let sliderState = await readSliderState();
  const sliderDeadline = Date.now() + 2500;
  while (!sliderState && Date.now() < sliderDeadline) {
    await page.waitForTimeout(100);
    sliderState = await readSliderState(500);
  }
  if (!sliderState) {
    const observed = (await control.innerText().catch(() => "")).trim();
    await page.keyboard.press("Escape").catch(() => {});
    if (isExtraHighLabel(observed)) {
      return {
        requested: "extra-high",
        observed_label: observed,
        verified_by: "label_after_menu_interaction",
      };
    }
    fail("Extra High effort option is unavailable: no semantic option or slider state");
  }

  const minimum = sliderState.minimum;
  const maximum = sliderState.maximum;
  let current = sliderState.current;
  const target = minimum + 3;
  if (target > maximum) {
    fail("Extra High is unavailable (effort range " + minimum + ".." + maximum + ")");
  }

  for (let attempts = 0; current !== target && attempts < 8; attempts += 1) {
    let keyTarget = sliderState.slider.locator('xpath=ancestor::*[@role="menuitem"][1]');
    if ((await keyTarget.count()) !== 1) keyTarget = sliderState.slider;
    await keyTarget.press(current < target ? "ArrowRight" : "ArrowLeft");
    await page.waitForTimeout(100);
    let refreshed = await readSliderState(500);
    const refreshDeadline = Date.now() + 1500;
    while (!refreshed && Date.now() < refreshDeadline) {
      const observed = (await control.innerText().catch(() => "")).trim();
      if (isExtraHighLabel(observed)) {
        current = target;
        break;
      }
      await page.waitForTimeout(100);
      refreshed = await readSliderState(500);
    }
    if (current === target) break;
    if (!refreshed) {
      fail("effort slider disappeared before Extra High could be verified");
    }
    if (refreshed.minimum !== minimum || refreshed.maximum !== maximum) {
      fail(
        "effort slider range changed while selecting Extra High (" +
          minimum + ".." + maximum + " -> " +
          refreshed.minimum + ".." + refreshed.maximum + ")",
      );
    }
    sliderState = refreshed;
    current = refreshed.current;
  }
  if (current !== target) {
    fail("could not set Extra High effort (target=" + target + ", current=" + current + ")");
  }
  await page.keyboard.press("Escape").catch(() => {});
  await page.waitForTimeout(150);
  const observed = (await control.innerText().catch(() => "")).trim();
  return {
    min: minimum,
    max: maximum,
    value: current,
    index: target - minimum,
    requested: "extra-high",
    observed_label: observed,
    verified_by: isExtraHighLabel(observed) ? "slider_and_label" : "slider_value",
  };
}

async function clearComposerText(composer) {
  await composer.fill("");
  await composer.focus();
  await composer.press(process.platform === "darwin" ? "Meta+A" : "Control+A").catch(() => {});
  await composer.press("Backspace").catch(() => {});
}

function normalizedConnectorText(value) {
  return String(value ?? "")
    .normalize("NFKC")
    .replace(/\s+/g, " ")
    .trim()
    .toLocaleLowerCase("en-US");
}

async function rankedConnectorCandidates(scope, selector, connectorName) {
  const target = normalizedConnectorText(connectorName);
  return scope.locator(selector).evaluateAll((nodes, expected) => {
    const normalize = (value) =>
      String(value ?? "")
        .normalize("NFKC")
        .replace(/\s+/g, " ")
        .trim()
        .toLocaleLowerCase("en-US");
    const isVisible = (node) => {
      const style = getComputedStyle(node);
      const rect = node.getBoundingClientRect();
      return (
        style.visibility !== "hidden" &&
        style.display !== "none" &&
        Number(style.opacity || "1") !== 0 &&
        rect.width > 0 &&
        rect.height > 0
      );
    };
    return nodes
      .map((node, index) => {
        if (!isVisible(node)) return null;
        const text = normalize(node.innerText ?? node.textContent ?? "");
        const keyword = normalize(node.getAttribute("data-keyword"));
        const aria = normalize(node.getAttribute("aria-label"));
        const title = normalize(node.getAttribute("title"));
        const testid = normalize(node.getAttribute("data-testid"));
        const role = normalize(node.getAttribute("role"));
        const tabindex = node.getAttribute("tabindex");
        const fields = [keyword, aria, title, text].filter(Boolean);
        if (!fields.some((value) => value === expected || value.includes(expected))) return null;

        let score = 0;
        let matchedBy = "contains";
        if (keyword === expected) {
          score = 1200;
          matchedBy = "data-keyword";
        } else if (aria === expected) {
          score = 1100;
          matchedBy = "aria-label";
        } else if (title === expected) {
          score = 1050;
          matchedBy = "title";
        } else if (text === expected) {
          score = 1000;
          matchedBy = "text";
        } else if (keyword.includes(expected)) {
          score = 900;
          matchedBy = "data-keyword-contains";
        } else if (aria.includes(expected)) {
          score = 850;
          matchedBy = "aria-label-contains";
        } else if (title.includes(expected)) {
          score = 825;
          matchedBy = "title-contains";
        } else if (text.includes(expected)) {
          score = 800;
          matchedBy = "text-contains";
        }
        if (role === "menuitem" || role === "option") score += 80;
        if (tabindex === "0") score += 40;
        if (node.tagName === "BUTTON") score += 30;
        if (testid.includes("connector") || testid.includes("plugin")) score += 25;
        if (text.length > expected.length + 100) score -= 150;

        return {
          index,
          score,
          matchedBy,
          text: (node.innerText ?? node.textContent ?? "").trim().slice(0, 180),
          keyword: node.getAttribute("data-keyword"),
          ariaLabel: node.getAttribute("aria-label"),
          title: node.getAttribute("title"),
          testid: node.getAttribute("data-testid"),
          role: node.getAttribute("role"),
        };
      })
      .filter(Boolean)
      .sort((a, b) => b.score - a.score || a.index - b.index);
  }, target).catch(() => []);
}

async function selectedConnectorEvidence(page, connectorName) {
  const composer = await activeComposer(page);
  const form = composer.locator("xpath=ancestor::form[1]");
  const scope = (await form.count()) === 1 ? form : composer;
  const candidates = await rankedConnectorCandidates(
    scope,
    SELECTED_CONNECTOR_SEMANTIC_SELECTOR,
    connectorName,
  );
  const strong = candidates.filter((candidate) => candidate.score >= 800);
  if (strong.length === 0) return null;

  const best = strong[0];
  const tied = strong.filter((candidate) => candidate.score === best.score);
  if (tied.length > 1) {
    const exactKeyword = tied.filter(
      (candidate) =>
        normalizedConnectorText(candidate.keyword) === normalizedConnectorText(connectorName),
    );
    if (exactKeyword.length !== 1) return null;
    return { ...exactKeyword[0], verifiedBy: "exact-data-keyword" };
  }
  return { ...best, verifiedBy: best.matchedBy };
}

async function verifySelectedConnector(page, connectorName, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const evidence = await selectedConnectorEvidence(page, connectorName);
    if (evidence) return evidence;
    await page.waitForTimeout(100);
  }
  return null;
}

async function visibleConnectorPickerLabels(page) {
  return page.locator(CONNECTOR_PICKER_CANDIDATE_SELECTOR).evaluateAll((nodes) =>
    nodes
      .filter((node) => {
        const style = getComputedStyle(node);
        const rect = node.getBoundingClientRect();
        return style.visibility !== "hidden" && style.display !== "none" && rect.width > 0 && rect.height > 0;
      })
      .map((node) => (node.innerText ?? node.textContent ?? "").trim())
      .filter(Boolean)
      .slice(0, 30),
  ).catch(() => []);
}

async function connectorPickerOpeners(page) {
  return [
    { strategy: "testid-exact", locator: page.getByTestId("composer-plus-btn") },
    {
      strategy: "testid-fuzzy",
      locator: page.locator('button[data-testid*="composer-plus" i]'),
    },
    {
      strategy: "semantic-role",
      locator: page.getByRole("button", {
        name: /add|attach|tools?|plugins?|connectors?|新增|加入|附件|附加|工具|外掛|連接器/i,
      }),
    },
    {
      strategy: "semantic-aria",
      locator: page.locator(
        'button[aria-label*="add" i], button[aria-label*="attach" i], button[aria-label*="tool" i], button[aria-label*="plugin" i], button[aria-label*="connector" i], button[aria-label*="新增"], button[aria-label*="附件"], button[aria-label*="工具"], button[aria-label*="外掛"], button[aria-label*="連接器"]',
      ),
    },
  ];
}

async function typeConnectorSearch(page, connectorName) {
  const searchInputs = page.locator(CONNECTOR_SEARCH_INPUT_SELECTOR).filter({ visible: true });
  const count = await searchInputs.count().catch(() => 0);
  if (count > 0) {
    const search = searchInputs.last();
    await search.fill(connectorName).catch(async () => {
      await search.focus();
      await page.keyboard.insertText(connectorName);
    });
    return "search-input";
  }
  await page.keyboard.insertText(connectorName);
  return "keyboard";
}

async function selectConnector(page, connectorName) {
  await dismissKnownBlockingModals(page);
  let composer = await activeComposer(page);
  await clearComposerText(composer);

  const alreadySelected = await verifySelectedConnector(page, connectorName, 1200);
  if (alreadySelected) {
    return {
      alreadySelected: true,
      openerStrategy: null,
      searchStrategy: null,
      pickerMatch: null,
      verification: alreadySelected,
    };
  }

  await page.keyboard.press("Escape").catch(() => {});
  await page.waitForTimeout(200);
  composer = await activeComposer(page);
  await clearComposerText(composer);

  const openers = await connectorPickerOpeners(page);
  const diagnostics = [];
  for (const opener of openers) {
    const count = await opener.locator.count().catch(() => 0);
    for (let index = 0; index < count; index += 1) {
      const control = opener.locator.nth(index);
      if (!(await visible(control))) continue;

      await page.keyboard.press("Escape").catch(() => {});
      await page.waitForTimeout(120);
      composer = await activeComposer(page);
      await clearComposerText(composer);
      try {
        await clickWithBlockingModalRecovery(page, control, { timeout: 5000 });
      } catch (error) {
        diagnostics.push({
          opener: opener.strategy,
          outcome: "click-failed",
          error: String(error).slice(0, 180),
        });
        await page.keyboard.press("Escape").catch(() => {});
        await page.waitForTimeout(120);
        continue;
      }
      await page.waitForTimeout(180);

      const searchStrategy = await typeConnectorSearch(page, connectorName);
      const deadline = Date.now() + 5000;
      let ranked = [];
      while (Date.now() < deadline) {
        ranked = await rankedConnectorCandidates(
          page,
          CONNECTOR_PICKER_CANDIDATE_SELECTOR,
          connectorName,
        );
        if (ranked.length > 0 && ranked[0].score >= 800) break;
        await page.waitForTimeout(100);
      }
      if (ranked.length === 0 || ranked[0].score < 800) {
        diagnostics.push({
          opener: opener.strategy,
          search: searchStrategy,
          outcome: "no-semantic-match",
          visibleLabels: await visibleConnectorPickerLabels(page),
        });
        continue;
      }

      const best = ranked[0];
      const tied = ranked.filter((candidate) => candidate.score === best.score);
      if (tied.length > 1) {
        diagnostics.push({
          opener: opener.strategy,
          search: searchStrategy,
          outcome: "ambiguous-top-match",
          matches: tied.slice(0, 5),
        });
        continue;
      }

      await clickWithBlockingModalRecovery(
        page,
        page.locator(CONNECTOR_PICKER_CANDIDATE_SELECTOR).nth(best.index),
        { timeout: 10000 },
      );
      const verification = await verifySelectedConnector(page, connectorName, 10000);
      if (verification) {
        return {
          alreadySelected: false,
          openerStrategy: opener.strategy,
          searchStrategy,
          pickerMatch: best,
          verification,
        };
      }
      diagnostics.push({
        opener: opener.strategy,
        search: searchStrategy,
        outcome: "post-selection-verification-failed",
        pickerMatch: best,
      });
    }
  }

  fail(
    "ChatGPT connector " +
      JSON.stringify(connectorName) +
      " could not be selected semantically; diagnostics=" +
      JSON.stringify(diagnostics.slice(-8)),
  );
}

async function attachPrompt(page, prompt) {
  const composer = await activeComposer(page);
  await composer.focus();
  await composer.press("End").catch(() => {});
  await page.keyboard.insertText(prompt);
}

async function sendPrompt(page, prompt) {
  const composer = await activeComposer(page);
  const form = composer.locator("xpath=ancestor::form[1]");
  const send = form.getByTestId("send-button");
  await send.waitFor({ state: "visible", timeout: 10000 });
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    if (await send.isEnabled().catch(() => false)) break;
    await page.waitForTimeout(100);
  }
  if (!(await send.isEnabled().catch(() => false))) {
    fail("ChatGPT send button remained disabled");
  }
  const submittedAtMs = Date.now();
  await send.press("Enter", { noWaitAfter: true });

  const acceptedDeadline = Date.now() + 30000;
  const normalizedPrompt = prompt.replace(/\r\n/g, "\n").trim();
  while (Date.now() < acceptedDeadline) {
    const users = page.locator(USER_TURN_SELECTOR);
    const count = await users.count();
    if (count >= 1) {
      const rendered = (await users.nth(count - 1).innerText().catch(() => ""))
        .replace(/\r\n/g, "\n")
        .trim();
      if (rendered.includes(normalizedPrompt)) {
        return { submittedAtMs, acceptedAtMs: Date.now() };
      }
    }
    await page.waitForTimeout(100);
  }
  fail("ChatGPT did not preserve the benchmark prompt in the submitted user turn");
}

async function waitForCompletion(page, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let stableSince = null;
  let lastText = "";
  let visibleFinalAtMs = null;
  while (Date.now() < deadline) {
    if (!isTemporaryChat(page.url())) {
      fail("ChatGPT left Temporary Chat during benchmark (" + page.url() + ")");
    }
    const assistants = page.locator(ASSISTANT_TURN_SELECTOR);
    const count = await assistants.count();
    if (count > 0) {
      const last = assistants.nth(count - 1);
      const text = await last.innerText().catch(() => "");
      const running = (await countVisible(page.locator(STOP_BUTTON_SELECTOR))) > 0;
      const completionAction =
        (await countVisible(last.locator(COMPLETION_ACTION_SELECTOR))) > 0;
      if (!running && completionAction && text.trim().length > 0) {
        if (visibleFinalAtMs === null) visibleFinalAtMs = Date.now();
        if (text === lastText) {
          if (stableSince === null) stableSince = Date.now();
          if (Date.now() - stableSince >= 1500) {
            return {
              assistantTurns: count,
              finalTextChars: text.length,
              visibleFinalAtMs,
              confirmedFinalAtMs: Date.now(),
            };
          }
        } else {
          lastText = text;
          stableSince = Date.now();
        }
      } else {
        stableSince = null;
        lastText = text;
        visibleFinalAtMs = null;
      }
    }
    await page.waitForTimeout(200);
  }
  fail("ChatGPT response did not reach a confirmed completion state within " + timeoutMs + " ms");
}

function writeResult(resultPath, payload) {
  fs.mkdirSync(path.dirname(resultPath), { recursive: true });
  const temporaryPath = resultPath + ".tmp-" + process.pid;
  fs.writeFileSync(temporaryPath, JSON.stringify(payload, null, 2) + "\n", { mode: 0o600 });
  fs.renameSync(temporaryPath, resultPath);
}

async function main() {
  const promptFile = requiredEnv("BENCH_PROMPT_FILE");
  const resultPath = requiredEnv("BENCH_DRIVER_RESULT");
  const progressPath = process.env.BENCH_DRIVER_PROGRESS || null;
  const connectorName = process.env.BENCH_WEBCODEX_CONNECTOR_NAME || "WebCodex Benchmark";
  const allowDevDescriptor =
    process.env.BENCH_ALLOW_DEV_BROWSER_DESCRIPTOR === "1";
  const descriptorCandidates = [
    process.env.BENCH_BROWSER_DESCRIPTOR,
    path.join(os.homedir(), ".codex-chatgpt-web", "runtime", "launcher-browser.json"),
    ...(allowDevDescriptor
      ? [path.join(os.homedir(), ".codex-chatgpt-web-dev", "runtime", "launcher-browser.json")]
      : []),
  ].filter(Boolean);
  const playwrightPath =
    process.env.BENCH_PLAYWRIGHT_PATH ||
    path.join(
      os.homedir(),
      ".cache",
      "codex-runtimes",
      "codex-primary-runtime",
      "dependencies",
      "node",
      "node_modules",
      "playwright",
    );
  const timeoutMs = Number(process.env.BENCH_DRIVER_TIMEOUT_MS || "1200000");
  const requireConnectorAtCompletion =
    process.env.BENCH_REQUIRE_CONNECTOR_AT_COMPLETION !== "0";
  const returnAfterAccepted =
    process.env.BENCH_RETURN_AFTER_ACCEPTED === "1";
  const postAcceptLingerMs = Number(process.env.BENCH_POST_ACCEPT_LINGER_MS || "0");
  if (!Number.isFinite(timeoutMs) || timeoutMs < 1000) {
    fail("BENCH_DRIVER_TIMEOUT_MS is invalid");
  }
  if (!Number.isFinite(postAcceptLingerMs) || postAcceptLingerMs < 0 || postAcceptLingerMs > 60000) {
    fail("BENCH_POST_ACCEPT_LINGER_MS is invalid");
  }

  const prompt = fs.readFileSync(promptFile, "utf8");
  const { endpoint } = await liveBrowserDescriptor(descriptorCandidates);

  const { chromium } = require(playwrightPath);
  let browser;
  let page;
  let submission;
  try {
    browser = await chromium.connectOverCDP(endpoint.origin, { timeout: 20000 });
    const selected = await chooseTemporaryPage(browser);
    page = selected.page;
    const viewport = await page.evaluate(() => ({ width: innerWidth, height: innerHeight }));
    if (viewport.width < 640 || viewport.height < 480) {
      await page.setViewportSize({ width: 1120, height: 720 });
    }
    await page.bringToFront();
    const effort = await setExtraHigh(page);
    const connectorSelection = await selectConnector(page, connectorName);
    if (process.env.BENCH_PREFLIGHT_ONLY === "1") {
      await gotoTemporaryChat(page);
      if (!(await pageIsCleanTemporaryChat(page))) fail("preflight could not restore clean Temporary Chat");
      writeResult(resultPath, {
        ready: true,
        completed: false,
        temporary_chat: true,
        connector: connectorName,
        effort: { requested: "extra-high", observed: effort },
        connector_selection: connectorSelection,
        submission_count: 0,
      });
      return;
    }
    await attachPrompt(page, prompt);
    const connectorBeforeSubmit = await verifySelectedConnector(page, connectorName, 3000);
    if (!connectorBeforeSubmit) {
      fail(
        "connector " +
          JSON.stringify(connectorName) +
          " did not survive prompt attachment before submit",
      );
    }
    submission = await sendPrompt(page, prompt);
    if (progressPath) {
      writeResult(progressPath, {
        accepted: true,
        connector: connectorName,
        temporary_chat: true,
        effort: { requested: "extra-high", observed: effort },
        connector_selection: connectorSelection,
        connector_before_submit: connectorBeforeSubmit,
        submitted_at_ms: submission.submittedAtMs,
        accepted_at_ms: submission.acceptedAtMs,
      });
    }
    if (returnAfterAccepted) {
      writeResult(resultPath, {
        completed: false,
        accepted: true,
        completion_mode: "submission-only",
        submission_checkpoint_version: 1,
        submission_committed: true,
        driver_self_reported_stage: "accepted",
        context_continuation_failure: false,
        temporary_chat: true,
        connector: connectorName,
        effort: { requested: "extra-high", observed: effort },
        connector_selected_at_completion: null,
        connector_selection: connectorSelection,
        connector_before_submit: connectorBeforeSubmit,
        connector_completion_evidence: null,
        submission_count: 1,
        assistant_turns: null,
        final_text_chars: null,
        submitted_at_ms: submission.submittedAtMs,
        accepted_at_ms: submission.acceptedAtMs,
        visible_final_at_ms: null,
        confirmed_final_at_ms: null,
        submit_to_visible_final_ms: null,
        submit_to_confirmed_final_ms: null,
        token_context_usage: null,
        retries: null,
      });
      if (postAcceptLingerMs > 0) {
        await page.waitForTimeout(postAcceptLingerMs);
      }
      return;
    }
    const completion = await waitForCompletion(page, timeoutMs);
    const connectorCompletionEvidence = requireConnectorAtCompletion
      ? await verifySelectedConnector(page, connectorName, 3000)
      : await selectedConnectorEvidence(page, connectorName);
    const connectorSelectedAtCompletion = Boolean(connectorCompletionEvidence);
    if (requireConnectorAtCompletion && !connectorSelectedAtCompletion) {
      fail("connector " + JSON.stringify(connectorName) + " was not selected at completion");
    }
    writeResult(resultPath, {
      completed: true,
      context_continuation_failure: false,
      temporary_chat: true,
      connector: connectorName,
      effort: { requested: "extra-high", observed: effort },
      connector_selected_at_completion: connectorSelectedAtCompletion,
      connector_selection: connectorSelection,
      connector_before_submit: connectorBeforeSubmit,
      connector_completion_evidence: connectorCompletionEvidence,
      submission_count: 1,
      assistant_turns: completion.assistantTurns,
      final_text_chars: completion.finalTextChars,
      submitted_at_ms: submission.submittedAtMs,
      accepted_at_ms: submission.acceptedAtMs,
      visible_final_at_ms: completion.visibleFinalAtMs,
      confirmed_final_at_ms: completion.confirmedFinalAtMs,
      submit_to_visible_final_ms: completion.visibleFinalAtMs - submission.submittedAtMs,
      submit_to_confirmed_final_ms: completion.confirmedFinalAtMs - submission.submittedAtMs,
      token_context_usage: null,
      retries: null,
    });
  } catch (error) {
    if (page && isTemporaryChat(page.url())) {
      await gotoTemporaryChat(page, 10000, 2, false).catch(() => {});
    }
    writeResult(resultPath, {
      completed: false,
      context_continuation_failure:
        page && !isTemporaryChat(page.url()) ? true : null,
      temporary_chat: page ? isTemporaryChat(page.url()) : null,
      connector: connectorName,
      submitted_at_ms: submission?.submittedAtMs ?? null,
      accepted_at_ms: submission?.acceptedAtMs ?? null,
      token_context_usage: null,
      retries: null,
      error: error instanceof Error ? error.message : String(error),
    });
    throw error;
  } finally {
    // This browser is launcher-owned. Closing the Browser returned by
    // connectOverCDP would terminate the remote launcher browser rather than
    // merely detach this benchmark client. Let process exit close the CDP
    // transport while the launcher keeps owning the browser across turns.
    browser = null;
  }
}

main()
  .then(() => {
    process.exit(0);
  })
  .catch((error) => {
    console.error(error instanceof Error ? error.stack || error.message : String(error));
    process.exit(1);
  });
