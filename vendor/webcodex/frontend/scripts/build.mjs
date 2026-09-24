#!/usr/bin/env node
import {
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  watch,
  writeFileSync,
} from "node:fs";
import { basename, dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Script } from "node:vm";
import ts from "typescript";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

// These modules are transpiled independently for ESM consumers and also inlined,
// in this exact order, into the classic Runtime Console bundle. The classic
// contract below therefore treats this list as the authoritative shared scope.
export const RUNTIME_INLINE_MODULES = Object.freeze([
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
]);

const watchedSources = new Set([
  ...RUNTIME_INLINE_MODULES,
  "runtime.css",
  "runtime.html",
  "admin.ts",
  "admin_controller.ts",
  "admin_mutation_controller.ts",
  "admin_mutation_view.ts",
  "admin_view.ts",
  "admin.css",
  "admin.html",
]);

function readSource(sourceDirectory, fileName) {
  return readFileSync(resolve(sourceDirectory, fileName), "utf8");
}

function normalizeNewline(content) {
  return content.replace(/\r\n/g, "\n").trim() + "\n";
}

const diagnosticHost = {
  getCanonicalFileName: (fileName) => fileName,
  getCurrentDirectory: () => root,
  getNewLine: () => "\n",
};

function transpileTypeScriptSource(source, fileName) {
  const result = ts.transpileModule(source, {
    compilerOptions: {
      target: ts.ScriptTarget.ES2020,
      module: ts.ModuleKind.ES2020,
      newLine: ts.NewLineKind.LineFeed,
      removeComments: false,
      sourceMap: false,
      inlineSourceMap: false,
    },
    fileName,
    reportDiagnostics: true,
  });
  const errors = (result.diagnostics || []).filter(
    (diagnostic) => diagnostic.category === ts.DiagnosticCategory.Error
  );
  if (errors.length) {
    throw new Error(ts.formatDiagnostics(errors, diagnosticHost).trim());
  }
  return normalizeNewline(result.outputText);
}

function transpileTypeScript(sourceDirectory, fileName) {
  const sourcePath = resolve(sourceDirectory, fileName);
  return transpileTypeScriptSource(readSource(sourceDirectory, fileName), sourcePath);
}

function buildJs(source) {
  // Keep generated JS readable and avoid whitespace-sensitive rewrites inside
  // template literals. TypeScript owns syntax erasure.
  return normalizeNewline(source);
}

function assertClassicScript(label, source) {
  try {
    new Script(source, { filename: label });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    throw new Error(`${label} is not valid browser JavaScript: ${message}`);
  }
}

function minifyCss(source) {
  return (
    normalizeNewline(source)
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .replace(/\s+/g, " ")
      .replace(/\s*([{}:;,>])\s*/g, "$1")
      .replace(/;}/g, "}")
      .replace(/0\.([0-9]+)/g, ".$1")
      .trim() + "\n"
  );
}

// Turn an ESM module into classic-script statements for inlining: drop the
// `export {}` marker and the `export` keyword on top-level declarations.
function stripModuleExports(js) {
  return js
    .replace(/^export\s*\{\};\s*\n?/gm, "")
    .replace(/^export\s+((?:async\s+)?(?:function|const|let|var|class))\b/gm, "$1");
}

function collectBindingNames(name, names) {
  if (ts.isIdentifier(name)) {
    names.add(name.text);
    return;
  }
  if (ts.isObjectBindingPattern(name) || ts.isArrayBindingPattern(name)) {
    for (const element of name.elements) {
      if (!ts.isOmittedExpression(element)) collectBindingNames(element.name, names);
    }
  }
}

function emittedTopLevelBindings(fileName, source) {
  const emitted = transpileTypeScriptSource(source, fileName);
  const sourceFile = ts.createSourceFile(
    fileName.replace(/\.tsx?$/, ".js"),
    emitted,
    ts.ScriptTarget.ES2020,
    true,
    ts.ScriptKind.JS
  );
  const names = new Set();
  for (const statement of sourceFile.statements) {
    if (ts.isFunctionDeclaration(statement) || ts.isClassDeclaration(statement)) {
      if (statement.name) names.add(statement.name.text);
      continue;
    }
    if (ts.isVariableStatement(statement)) {
      for (const declaration of statement.declarationList.declarations) {
        collectBindingNames(declaration.name, names);
      }
    }
  }
  return names;
}

function runtimeImportTarget(specifier) {
  if (!specifier.startsWith("./")) return null;
  const relativeName = specifier.slice(2);
  if (!relativeName || relativeName.includes("/") || relativeName.includes("\\")) return null;
  if (relativeName.endsWith(".js")) return relativeName.slice(0, -3) + ".ts";
  if (relativeName.endsWith(".ts")) return relativeName;
  return relativeName + ".ts";
}

export function analyzeRuntimeClassicBundleModules(modules) {
  const moduleNames = new Set(modules.map(({ fileName }) => fileName));
  const moduleOrder = new Map(modules.map(({ fileName }, index) => [fileName, index]));
  const bindingsByModule = new Map();
  const declarationSites = new Map();
  for (const { fileName, source } of modules) {
    const bindings = emittedTopLevelBindings(fileName, source);
    bindingsByModule.set(fileName, bindings);
    for (const name of bindings) {
      const sites = declarationSites.get(name) || [];
      sites.push(fileName);
      declarationSites.set(name, sites);
    }
  }

  const aliases = [];
  const unresolvedImports = [];
  for (const { fileName, source } of modules) {
    const sourceFile = ts.createSourceFile(
      fileName,
      source,
      ts.ScriptTarget.Latest,
      true,
      ts.ScriptKind.TS
    );
    for (const statement of sourceFile.statements) {
      if (!ts.isImportDeclaration(statement)) continue;
      const clause = statement.importClause;
      const fromModule = ts.isStringLiteral(statement.moduleSpecifier)
        ? statement.moduleSpecifier.text
        : "";
      const targetFile = runtimeImportTarget(fromModule);
      const line = sourceFile.getLineAndCharacterOfPosition(statement.getStart(sourceFile)).line + 1;
      if (!clause) {
        unresolvedImports.push({
          file: fileName,
          fromModule,
          imported: "(side effect)",
          line,
          reason: "side-effect imports are not supported by the classic Runtime bundle",
        });
        continue;
      }
      if (clause.isTypeOnly) continue;

      if (clause.name) {
        unresolvedImports.push({
          file: fileName,
          fromModule,
          imported: "default",
          line,
          reason: "default value imports are not supported by the classic Runtime bundle",
        });
      }
      if (!clause.namedBindings) continue;
      if (ts.isNamespaceImport(clause.namedBindings)) {
        unresolvedImports.push({
          file: fileName,
          fromModule,
          imported: "*",
          line,
          reason: "namespace value imports are not supported by the classic Runtime bundle",
        });
        continue;
      }
      for (const specifier of clause.namedBindings.elements) {
        if (specifier.isTypeOnly) continue;
        const imported = specifier.propertyName?.text || specifier.name.text;
        const local = specifier.name.text;
        if (specifier.propertyName) {
          aliases.push({ file: fileName, fromModule, imported, local, line });
        }
        if (!targetFile || !moduleNames.has(targetFile)) {
          unresolvedImports.push({
            file: fileName,
            fromModule,
            imported,
            line,
            reason: "value import target is not part of the classic Runtime bundle",
          });
          continue;
        }
        if (moduleOrder.get(targetFile) > moduleOrder.get(fileName)) {
          unresolvedImports.push({
            file: fileName,
            fromModule,
            imported,
            line,
            reason: `value import target ${targetFile} must precede ${fileName} in classic Runtime module order`,
          });
        }
        if (!bindingsByModule.get(targetFile)?.has(imported)) {
          unresolvedImports.push({
            file: fileName,
            fromModule,
            imported,
            line,
            reason: `canonical binding is not emitted by ${targetFile}`,
          });
        }
      }
    }
  }

  const collisions = Array.from(declarationSites.entries())
    .filter(([, files]) => files.length > 1)
    .map(([name, files]) => ({ name, files }))
    .sort((left, right) => left.name.localeCompare(right.name));
  aliases.sort((left, right) => left.file.localeCompare(right.file) || left.line - right.line);
  unresolvedImports.sort(
    (left, right) => left.file.localeCompare(right.file) || left.line - right.line
  );
  return { aliases, collisions, unresolvedImports };
}

export function assertRuntimeClassicBundleModules(modules) {
  const analysis = analyzeRuntimeClassicBundleModules(modules);
  const problems = [];
  if (analysis.aliases.length) {
    problems.push(
      `${analysis.aliases.length} named value import alias(es):\n` +
        analysis.aliases
          .map(
            ({ file, fromModule, imported, local, line }) =>
              `  ${file}:${line}: import { ${imported} as ${local} } from "${fromModule}"`
          )
          .join("\n")
    );
  }
  if (analysis.collisions.length) {
    problems.push(
      `${analysis.collisions.length} shared classic-scope binding collision(s):\n` +
        analysis.collisions
          .map(({ name, files }) => `  ${name}: ${files.join(", ")}`)
          .join("\n")
    );
  }
  if (analysis.unresolvedImports.length) {
    problems.push(
      `${analysis.unresolvedImports.length} canonical value import binding error(s):\n` +
        analysis.unresolvedImports
          .map(
            ({ file, fromModule, imported, line, reason }) =>
              `  ${file}:${line}: ${imported} from "${fromModule}" — ${reason}`
          )
          .join("\n")
    );
  }
  if (problems.length) {
    throw new Error(
      "Runtime classic bundle contract violated. Imports are stripped before the Runtime modules " +
        "share one classic-script lexical scope.\n" +
        problems.join("\n")
    );
  }
  return analysis;
}

export function assertRuntimeClassicBundleContract(
  sourceDirectory = resolve(root, "src")
) {
  return assertRuntimeClassicBundleModules(
    RUNTIME_INLINE_MODULES.map((fileName) => ({
      fileName,
      source: readSource(sourceDirectory, fileName),
    }))
  );
}

export function createOutputs(
  outputDirectory,
  sourceDirectory = resolve(root, "src")
) {
  assertRuntimeClassicBundleContract(sourceDirectory);
  const workflowSessionStateModule = buildJs(
    transpileTypeScript(sourceDirectory, "workflow_session_state.ts")
  );
  const workflowSessionStateClassic = stripModuleExports(workflowSessionStateModule);
  const runtimeCollaborationStateModule = buildJs(
    transpileTypeScript(sourceDirectory, "runtime_collaboration_state.ts")
  );
  const runtimeCollaborationStateClassic = stripModuleExports(
    runtimeCollaborationStateModule
  );
  const runtimeCommunicationStateModule = buildJs(
    transpileTypeScript(sourceDirectory, "runtime_communication_state.ts")
  );
  const runtimeCommunicationStateClassic = stripModuleExports(
    runtimeCommunicationStateModule
  );
  const runtimeContextStateModule = buildJs(
    transpileTypeScript(sourceDirectory, "runtime_context_state.ts")
  );
  const runtimeContextStateClassic = stripModuleExports(
    runtimeContextStateModule
  );
  const runtimeConsoleStateModule = buildJs(
    transpileTypeScript(sourceDirectory, "runtime_console_state.ts")
  );
  const runtimeConsoleStateClassic = stripModuleExports(
    runtimeConsoleStateModule
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/workflow_session_state(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_collaboration_state(?:\.js)?["'];?\s*\n/m,
        ""
      )
  );
  const runtimeI18nModule = buildJs(
    transpileTypeScript(sourceDirectory, "runtime_i18n.ts")
  );
  const runtimeI18nClassic = stripModuleExports(runtimeI18nModule);
  const runtimeRichTextModule = buildJs(
    transpileTypeScript(sourceDirectory, "runtime_rich_text.ts")
  );
  const runtimeRichTextClassic = stripModuleExports(runtimeRichTextModule);
  const runtimeApiModule = buildJs(
    transpileTypeScript(sourceDirectory, "runtime_api.ts")
  );
  const runtimeApiClassic = stripModuleExports(runtimeApiModule);
  const runtimeWindowModule = buildJs(
    transpileTypeScript(sourceDirectory, "runtime_window.ts")
  );
  const runtimeWindowClassic = stripModuleExports(
    runtimeWindowModule
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_i18n(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_console_state(?:\.js)?["'];?\s*\n/m,
        ""
      )
  );
  const runtimeCommunicationModule = buildJs(
    transpileTypeScript(sourceDirectory, "runtime_communication.ts")
  );
  const runtimeCommunicationClassic = stripModuleExports(
    runtimeCommunicationModule
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_i18n(?:\.js)?["'];?\s*\n/m,
        ""
      )
  );
  const runtimeActivityModule = buildJs(
    transpileTypeScript(sourceDirectory, "runtime_activity.ts")
  );
  const runtimeActivityClassic = stripModuleExports(
    runtimeActivityModule
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_i18n(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/workflow_session_state(?:\.js)?["'];?\s*\n/m,
        ""
      )
  );
  const runtimeStorageModule = buildJs(
    transpileTypeScript(sourceDirectory, "runtime_storage.ts")
  );
  const runtimeStorageClassic = stripModuleExports(runtimeStorageModule);
  const runtimeOverviewModule = buildJs(
    transpileTypeScript(sourceDirectory, "runtime_overview.ts")
  );
  const runtimeOverviewClassic = stripModuleExports(
    runtimeOverviewModule
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_i18n(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_console_state(?:\.js)?["'];?\s*\n/m,
        ""
      )
  );
  const runtimeOperationsModule = buildJs(
    transpileTypeScript(sourceDirectory, "runtime_operations.ts")
  );
  const runtimeOperationsClassic = stripModuleExports(
    runtimeOperationsModule
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_i18n(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_communication(?:\.js)?["'];?\s*\n/m,
        ""
      )
  );
  const runtimeIconsModule = buildJs(
    transpileTypeScript(sourceDirectory, "runtime_icons.ts")
  );
  const runtimeIconsClassic = stripModuleExports(runtimeIconsModule);
  const runtimeNavigationModule = buildJs(
    transpileTypeScript(sourceDirectory, "runtime_navigation.ts")
  );
  const runtimeNavigationClassic = stripModuleExports(
    runtimeNavigationModule
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_i18n(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_console_state(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_overview(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_activity(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_icons(?:\.js)?["'];?\s*\n/m,
        ""
      )
  );
  const runtimeCollaborationModule = buildJs(
    transpileTypeScript(sourceDirectory, "runtime_collaboration.ts")
  );
  const runtimeCollaborationClassic = stripModuleExports(
    runtimeCollaborationModule
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_i18n(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_activity(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_collaboration_state(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_rich_text(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_icons(?:\.js)?["'];?\s*\n/m,
        ""
      )
  );
  const runtimeModule = transpileTypeScript(sourceDirectory, "runtime.ts");
  const runtimeScript = stripModuleExports(
    runtimeModule
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/workflow_session_state(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_communication_state(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_context_state(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_collaboration_state(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_console_state(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_navigation(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_collaboration(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_i18n(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_rich_text(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_api(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_window(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_communication(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_activity(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_storage(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_overview(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_icons(?:\.js)?["'];?\s*\n/m,
        ""
      )
      .replace(
        /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/runtime_operations(?:\.js)?["'];?\s*\n/m,
        ""
      )
  );
  const runtimeClassicModules = new Map([
    ["workflow_session_state.ts", workflowSessionStateClassic],
    ["runtime_collaboration_state.ts", runtimeCollaborationStateClassic],
    ["runtime_communication_state.ts", runtimeCommunicationStateClassic],
    ["runtime_context_state.ts", runtimeContextStateClassic],
    ["runtime_console_state.ts", runtimeConsoleStateClassic],
    ["runtime_i18n.ts", runtimeI18nClassic],
    ["runtime_rich_text.ts", runtimeRichTextClassic],
    ["runtime_api.ts", runtimeApiClassic],
    ["runtime_window.ts", runtimeWindowClassic],
    ["runtime_communication.ts", runtimeCommunicationClassic],
    ["runtime_activity.ts", runtimeActivityClassic],
    ["runtime_storage.ts", runtimeStorageClassic],
    ["runtime_overview.ts", runtimeOverviewClassic],
    ["runtime_operations.ts", runtimeOperationsClassic],
    ["runtime_icons.ts", runtimeIconsClassic],
    ["runtime_navigation.ts", runtimeNavigationClassic],
    ["runtime_collaboration.ts", runtimeCollaborationClassic],
    ["runtime.ts", runtimeScript],
  ]);
  const runtimeInlined = buildJs(
    RUNTIME_INLINE_MODULES.map((fileName) => {
      const classicModule = runtimeClassicModules.get(fileName);
      if (classicModule === undefined) {
        throw new Error(`Runtime inline module has no classic build output: ${fileName}`);
      }
      return classicModule;
    }).join("\n")
  );
  assertClassicScript(resolve(outputDirectory, "runtime.js"), runtimeInlined);
  const adminControllerModule = buildJs(
    transpileTypeScript(sourceDirectory, "admin_controller.ts")
  );
  const adminControllerClassic = stripModuleExports(adminControllerModule);
  const adminMutationControllerModule = buildJs(
    transpileTypeScript(sourceDirectory, "admin_mutation_controller.ts")
  );
  const adminMutationControllerClassic = stripModuleExports(adminMutationControllerModule);
  const adminMutationViewModule = buildJs(
    transpileTypeScript(sourceDirectory, "admin_mutation_view.ts")
  );
  const adminMutationViewClassic = stripModuleExports(adminMutationViewModule);
  const adminViewModule = buildJs(
    transpileTypeScript(sourceDirectory, "admin_view.ts")
  );
  const adminViewClassic = stripModuleExports(adminViewModule);
  const adminModule = transpileTypeScript(sourceDirectory, "admin.ts");
  const adminScript = buildJs(
    adminControllerClassic +
      "\n" +
      adminMutationControllerClassic +
      "\n" +
      adminMutationViewClassic +
      "\n" +
      adminViewClassic +
      "\n" +
      stripModuleExports(
        adminModule
          .replace(
            /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/admin_controller(?:\.js)?["'];?\s*\n/m,
            ""
          )
          .replace(
            /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/admin_mutation_controller(?:\.js)?["'];?\s*\n/m,
            ""
          )
          .replace(
            /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/admin_mutation_view(?:\.js)?["'];?\s*\n/m,
            ""
          )
          .replace(
            /^import\s*\{[\s\S]*?\}\s*from\s*["']\.\/admin_view(?:\.js)?["'];?\s*\n/m,
            ""
          )
      )
  );
  assertClassicScript(resolve(outputDirectory, "admin.js"), adminScript);

  return new Map([
    ["workflow_session_state.js", workflowSessionStateModule],
    ["runtime_collaboration_state.js", runtimeCollaborationStateModule],
    ["runtime_communication_state.js", runtimeCommunicationStateModule],
    ["runtime_context_state.js", runtimeContextStateModule],
    ["runtime_console_state.js", runtimeConsoleStateModule],
    ["runtime_i18n.js", runtimeI18nModule],
    ["runtime_rich_text.js", runtimeRichTextModule],
    ["runtime_api.js", runtimeApiModule],
    ["runtime_window.js", runtimeWindowModule],
    ["runtime_communication.js", runtimeCommunicationModule],
    ["runtime_activity.js", runtimeActivityModule],
    ["runtime_storage.js", runtimeStorageModule],
    ["runtime_overview.js", runtimeOverviewModule],
    ["runtime_operations.js", runtimeOperationsModule],
    ["runtime_icons.js", runtimeIconsModule],
    ["runtime_navigation.js", runtimeNavigationModule],
    ["runtime_collaboration.js", runtimeCollaborationModule],
    ["admin_controller.js", adminControllerModule],
    ["admin_mutation_controller.js", adminMutationControllerModule],
    ["admin_mutation_view.js", adminMutationViewModule],
    ["admin_view.js", adminViewModule],
    ["runtime.js", runtimeInlined],
    ["runtime.css", minifyCss(readSource(sourceDirectory, "runtime.css"))],
    ["admin.js", adminScript],
    ["admin.css", minifyCss(readSource(sourceDirectory, "admin.css"))],
    ["runtime.html", normalizeNewline(readSource(sourceDirectory, "runtime.html"))],
    ["admin.html", normalizeNewline(readSource(sourceDirectory, "admin.html"))],
  ]);
}

function atomicWriteOutputs(outputDirectory, outputs) {
  mkdirSync(outputDirectory, { recursive: true });
  const nonce = `${process.pid}-${Date.now()}`;
  const staged = [];
  try {
    let index = 0;
    for (const [name, content] of outputs) {
      const finalPath = resolve(outputDirectory, name);
      const temporaryPath = resolve(
        outputDirectory,
        `.${basename(name)}.${nonce}-${index}.tmp`
      );
      index += 1;
      writeFileSync(temporaryPath, content);
      staged.push({ finalPath, temporaryPath });
    }
    for (const entry of staged) {
      renameSync(entry.temporaryPath, entry.finalPath);
    }
  } finally {
    for (const entry of staged) {
      rmSync(entry.temporaryPath, { force: true });
    }
  }
}

function checkOutputs(outputDirectory, outputs) {
  const drift = [];
  for (const [name, expected] of outputs) {
    const fullPath = resolve(outputDirectory, name);
    const actual = existsSync(fullPath) ? readFileSync(fullPath, "utf8") : "";
    if (actual !== expected) drift.push(name);
  }
  if (drift.length) {
    throw new Error(
      `${drift.join(", ")} out of date; run: npm --prefix frontend run build`
    );
  }
}

export function runBuild({
  outputDirectory,
  sourceDirectory = resolve(root, "src"),
  checkOnly = false,
}) {
  const startedAt = Date.now();
  // Generate and validate every output before touching any final file. A
  // TypeScript or JS parse failure therefore preserves the previous build.
  const outputs = createOutputs(outputDirectory, sourceDirectory);
  if (checkOnly) {
    checkOutputs(outputDirectory, outputs);
  } else {
    atomicWriteOutputs(outputDirectory, outputs);
  }
  const displayDirectory =
    relative(root, outputDirectory) || basename(outputDirectory);
  console.log(
    `[console] ${checkOnly ? "checked" : "built"} ${displayDirectory} (${
      outputs.size
    } files, ${Date.now() - startedAt}ms)`
  );
}

function parseArguments(argv) {
  let outputDirectory = resolve(root, "dist");
  let sourceDirectory = resolve(root, "src");
  let checkOnly = false;
  let watchMode = false;
  for (let index = 0; index < argv.length; index += 1) {
    switch (argv[index]) {
      case "--check":
        checkOnly = true;
        break;
      case "--watch":
        watchMode = true;
        break;
      case "--out-dir": {
        const value = argv[index + 1];
        if (!value || value.startsWith("--")) {
          throw new Error("--out-dir requires a path");
        }
        outputDirectory = resolve(root, value);
        index += 1;
        break;
      }
      case "--source-dir": {
        const value = argv[index + 1];
        if (!value || value.startsWith("--")) {
          throw new Error("--source-dir requires a path");
        }
        sourceDirectory = resolve(root, value);
        index += 1;
        break;
      }
      default:
        throw new Error(`unknown option: ${argv[index]}`);
    }
  }
  if (checkOnly && watchMode) {
    throw new Error("--check and --watch cannot be used together");
  }
  return { outputDirectory, sourceDirectory, checkOnly, watchMode };
}

function startWatcher(outputDirectory, sourceDirectory) {
  let debounceTimer;
  let closed = false;

  const rebuild = () => {
    debounceTimer = undefined;
    try {
      runBuild({ outputDirectory, sourceDirectory });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      console.error(`[console] build failed: ${message}`);
    }
  };
  const schedule = () => {
    if (closed) return;
    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(rebuild, 100);
  };
  const sourceWatcher = watch(sourceDirectory, (_event, fileName) => {
    const name = fileName === null ? null : fileName.toString();
    if (name === null || watchedSources.has(name)) schedule();
  });
  const displaySourceDirectory =
    relative(root, sourceDirectory) || basename(sourceDirectory);
  const watched = [...watchedSources]
    .sort()
    .map((name) => `${displaySourceDirectory}/${name}`)
    .join(", ");
  console.log(`[console] watching ${watched}`);

  const close = () => {
    if (closed) return;
    closed = true;
    if (debounceTimer) clearTimeout(debounceTimer);
    sourceWatcher.close();
  };
  process.once("SIGINT", close);
  process.once("SIGTERM", close);
}

function main() {
  const options = parseArguments(process.argv.slice(2));
  try {
    runBuild(options);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (!options.watchMode) throw error;
    console.error(`[console] initial build failed: ${message}`);
  }
  if (options.watchMode) {
    startWatcher(options.outputDirectory, options.sourceDirectory);
  }
}

const invokedPath = process.argv[1] ? resolve(process.argv[1]) : "";
if (invokedPath === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.error(`[console] build failed: ${message}`);
    process.exitCode = 1;
  }
}
