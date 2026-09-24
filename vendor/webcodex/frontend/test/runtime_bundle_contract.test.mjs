import test from "node:test";
import assert from "node:assert/strict";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  RUNTIME_INLINE_MODULES,
  analyzeRuntimeClassicBundleModules,
  assertRuntimeClassicBundleContract,
  assertRuntimeClassicBundleModules,
} from "../scripts/build.mjs";

const frontendRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const srcDir = resolve(frontendRoot, "src");

const EXPECTED_RUNTIME_INLINE_MODULES = [
  "workflow_session_state.ts",
  "runtime_collaboration_state.ts",
  "runtime_communication_state.ts",
  "runtime_context_state.ts",
  "runtime_console_state.ts",
  "runtime_i18n.ts",
  "runtime_rich_text.ts",
  "runtime_api.ts",
  "runtime_window.ts",
  "runtime_communication.ts",
  "runtime_activity.ts",
  "runtime_storage.ts",
  "runtime_overview.ts",
  "runtime_operations.ts",
  "runtime_icons.ts",
  "runtime_navigation.ts",
  "runtime_collaboration.ts",
  "runtime.ts",
];

test("Runtime classic bundle uses one explicit ordered module contract", () => {
  assert.deepEqual([...RUNTIME_INLINE_MODULES], EXPECTED_RUNTIME_INLINE_MODULES);
  const analysis = assertRuntimeClassicBundleContract(srcDir);
  assert.deepEqual(analysis.aliases, []);
  assert.deepEqual(analysis.collisions, []);
  assert.deepEqual(analysis.unresolvedImports, []);
});

test("the pre-fix named-import alias shape fails the classic bundle contract", () => {
  const modules = [
    {
      fileName: "runtime_storage.ts",
      source: "export function loadAppearancePreference() { return 'system'; }\n",
    },
    {
      fileName: "runtime.ts",
      source:
        "import { loadAppearancePreference as loadAppearanceFromStorage } from './runtime_storage.js';\n" +
        "export function boot() { return loadAppearanceFromStorage(); }\n",
    },
  ];
  const analysis = analyzeRuntimeClassicBundleModules(modules);
  assert.deepEqual(analysis.aliases, [
    {
      file: "runtime.ts",
      fromModule: "./runtime_storage.js",
      imported: "loadAppearancePreference",
      local: "loadAppearanceFromStorage",
      line: 1,
    },
  ]);
  assert.throws(
    () => assertRuntimeClassicBundleModules(modules),
    /named value import alias\(es\)/,
  );
});

test("type-only named-import aliases do not create classic runtime bindings", () => {
  const modules = [
    {
      fileName: "types.ts",
      source: "export type RuntimeThing = { id: string };\n",
    },
    {
      fileName: "runtime.ts",
      source:
        "import { type RuntimeThing as Thing } from './types.js';\n" +
        "export function identity(value: Thing) { return value; }\n",
    },
  ];
  const analysis = assertRuntimeClassicBundleModules(modules);
  assert.deepEqual(analysis.aliases, []);
  assert.deepEqual(analysis.unresolvedImports, []);
});

test("duplicate emitted top-level bindings fail before classic concatenation", () => {
  const modules = [
    { fileName: "first.ts", source: "export function sharedBinding() { return 1; }\n" },
    { fileName: "second.ts", source: "const sharedBinding = 2;\n" },
  ];
  const analysis = analyzeRuntimeClassicBundleModules(modules);
  assert.deepEqual(analysis.collisions, [
    { name: "sharedBinding", files: ["first.ts", "second.ts"] },
  ]);
  assert.throws(
    () => assertRuntimeClassicBundleModules(modules),
    /shared classic-scope binding collision\(s\)/,
  );
});

test("every stripped value import must have a canonical binding in an inlined module", () => {
  const modules = [
    { fileName: "helpers.ts", source: "export function canonicalHelper() { return 1; }\n" },
    {
      fileName: "runtime.ts",
      source:
        "import { missingHelper } from './helpers.js';\n" +
        "export function boot() { return missingHelper(); }\n",
    },
  ];
  const analysis = analyzeRuntimeClassicBundleModules(modules);
  assert.equal(analysis.unresolvedImports.length, 1);
  assert.equal(analysis.unresolvedImports[0].imported, "missingHelper");
  assert.match(analysis.unresolvedImports[0].reason, /canonical binding is not emitted/);
  assert.throws(
    () => assertRuntimeClassicBundleModules(modules),
    /canonical value import binding error\(s\)/,
  );
});

test("classic Runtime module order places every value-import provider before its consumer", () => {
  const modules = [
    {
      fileName: "consumer.ts",
      source:
        "import { value } from './provider.js';\n" +
        "export const result = value + 1;\n",
    },
    { fileName: "provider.ts", source: "export const value = 1;\n" },
  ];
  const analysis = analyzeRuntimeClassicBundleModules(modules);
  assert.equal(analysis.unresolvedImports.length, 1);
  assert.equal(analysis.unresolvedImports[0].imported, "value");
  assert.match(analysis.unresolvedImports[0].reason, /must precede consumer\.ts/);
  assert.throws(
    () => assertRuntimeClassicBundleModules(modules),
    /canonical value import binding error\(s\)/,
  );
});
