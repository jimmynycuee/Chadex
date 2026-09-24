import test from "node:test";
import assert from "node:assert/strict";
import {
  translate,
  translateStaticNodeValue,
  localizedCountLabel,
  localizedWorkflowText,
  languagePreference,
  loadLanguagePreference,
  LANGUAGE_STORAGE_KEY,
} from "../dist/runtime_i18n.js";

test("languagePreference defaults to en and accepts zh-CN", () => {
  assert.equal(languagePreference("zh-CN"), "zh-CN");
  assert.equal(languagePreference("en"), "en");
  assert.equal(languagePreference("fr"), "en");
  assert.equal(languagePreference(null), "en");
});

test("translate returns original in English and localized string in Chinese", () => {
  assert.equal(translate("Your workspace", "en"), "Your workspace");
  assert.equal(translate("Your workspace", "zh-CN"), "你的工作空间");
  assert.equal(translate("Live updates", "zh-CN"), "实时更新");
  assert.equal(translate("Non-existent key", "zh-CN"), "Non-existent key");
});

test("translateStaticNodeValue preserves leading and trailing whitespace", () => {
  assert.equal(
    translateStaticNodeValue("  Connect  ", "zh-CN"),
    "  连接  "
  );
  assert.equal(
    translateStaticNodeValue("\n  Local  \n", "zh-CN"),
    "\n  本地  \n"
  );
});

test("localizedCountLabel formats quantities with plurals and Chinese classifiers", () => {
  assert.equal(localizedCountLabel(1, "Project", "Projects", "en"), "1 Project");
  assert.equal(localizedCountLabel(5, "Project", "Projects", "en"), "5 Projects");
  assert.equal(localizedCountLabel(1, "Project", "Projects", "zh-CN"), "1 个项目");
  assert.equal(localizedCountLabel(3, "Runner", "Runners", "zh-CN"), "3 台运行器");
  assert.equal(localizedCountLabel(0, "active Session", "active Sessions", "zh-CN"), "0 个活跃会话");
});

test("localizedWorkflowText translates exact and parameterized workflow status", () => {
  assert.equal(
    localizedWorkflowText("Latest validation passed", "en"),
    "Latest validation passed"
  );
  assert.equal(
    localizedWorkflowText("Latest validation passed", "zh-CN"),
    "最近验证已通过"
  );
  assert.equal(
    localizedWorkflowText("Recent observed work: 3 edits", "zh-CN"),
    "最近观察到的工作：3 次编辑"
  );
  assert.equal(
    localizedWorkflowText("Retained open messages: 2 todos", "zh-CN"),
    "保留的开放消息：2 个待办"
  );
});
