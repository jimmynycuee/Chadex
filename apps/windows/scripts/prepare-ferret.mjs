// Canonical files remain the only editable asset/config source. No generated
// manifest or image copy is checked into the frontend; Vite emits them at build.
import { readFile, readdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

export const resourceRoot = fileURLToPath(new URL('../../../Sources/ChadexApp/Resources/', import.meta.url));
const swiftRoot = fileURLToPath(new URL('../../../Sources/ChadexApp/', import.meta.url));
const required = (value, name) => {
  if (!value) throw new Error(`Canonical ferret contract changed: ${name}`);
  return value;
};
const quoted = (source) => [...source.matchAll(/"([^"]+)"/g)].map((match) => match[1]);

export async function loadCanonicalFerret() {
  const [swift, models, strings, poseText, files, view] = await Promise.all([
    readFile(path.join(swiftRoot, 'CodeFerretState.swift'), 'utf8'),
    readFile(path.join(swiftRoot, 'Models.swift'), 'utf8'),
    readFile(path.join(resourceRoot, 'zh-Hant.lproj/Localizable.strings'), 'utf8'),
    readFile(path.join(resourceRoot, 'ferret-motion-poses.json'), 'utf8'),
    readdir(resourceRoot),
    readFile(path.join(swiftRoot, 'CodeFerretView.swift'), 'utf8'),
  ]);
  const states = required(swift.match(/enum FerretState[^\{]+\{\s*case ([^\n]+)/)?.[1], 'states').split(',').map((s) => s.trim());
  const poses = JSON.parse(poseText);
  const labels = {};
  for (const state of states) {
    labels[state] = required(strings.match(new RegExp(`"ferret.state.${state}" = "([^"]+)";`))?.[1], `label ${state}`);
    required(poses[state], `pose ${state}`);
  }
  const rules = (fn) => {
    const block = required(swift.match(new RegExp(`static func ${fn}[\\s\\S]*?switch [\\s\\S]*?\\{([\\s\\S]*?)default:`))?.[1], fn);
    return Object.fromEntries([...block.matchAll(/case ([\s\S]*?): return \.(\w+)/g)].flatMap((m) => quoted(m[1]).map((name) => [name, m[2]])));
  };
  const statuses = (fn, source = swift) => quoted(required(source.match(new RegExp(`${fn.startsWith('var ') ? fn : `static func ${fn}`}[\\s\\S]*?\\[([^\\]]+)\\]\\.contains`))?.[1], fn));
  const ignoredBlock = required(swift.match(/static func isMeaningfulTool[\s\S]*?!\[([\s\S]*?)\]\.contains/)?.[1], 'ignored tools');
  const images = files.filter((file) => /^ferret-motion-[\w-]+\.png$/.test(file));
  const layoutBlock = required(view.match(/private struct FerretPoseLayout \{([\s\S]*?)\n\}/)?.[1], 'pose layout');
  const assignments = (source) => Object.fromEntries([...source.matchAll(/(\w+)\s*=\s*(-?[\d.]+|false|true)/g)]
    .map(([, key, value]) => [key, value === 'true' ? true : value === 'false' ? false : Number(value)]));
  const defaults = assignments(layoutBlock.split('init(state:')[0]);
  const layouts = Object.fromEntries(states.map((state) => [state, { ...defaults }]));
  for (const [, cases, body] of layoutBlock.matchAll(/case ([^:]+):([\s\S]*?)(?=case |default:)/g)) {
    for (const state of cases.split(',').map((s) => s.trim().replace(/^\./, ''))) {
      required(layouts[state], `layout ${state}`);
      Object.assign(layouts[state], assignments(body));
    }
  }
  const baseline = Number(required(view.match(/position\(x: layout.x, y: (\d+) - height \/ 2\)/)?.[1], 'body baseline'));
  for (const pose of Object.values(poses)) {
    for (const name of [pose.name, ...pose.eyes.map((eye) => eye.name)]) {
      required(images.includes(`ferret-motion-${name}.png`), `image ${name}`);
    }
  }
  return {
    data: { states, labels, poses, layouts, baseline, toolStates: rules('toolState'), taskSteps: rules('taskState'),
      activeTasks: statuses('taskIsActive'), activeJobs: statuses('var isActive', models.slice(models.indexOf('struct FerretJobSnapshot'))),
      waitingJobs: statuses('var isWaiting', models.slice(models.indexOf('struct FerretJobSnapshot'))), ignoredTools: quoted(ignoredBlock) },
    images,
  };
}

export function canonicalFerretPlugin() {
  let canonical;
  return {
    name: 'chadex-canonical-ferret',
    async buildStart() { canonical = await loadCanonicalFerret(); },
    resolveId(id) { if (id === 'virtual:ferret-canonical') return '\0ferret-canonical'; },
    async load(id) {
      if (id !== '\0ferret-canonical') return;
      canonical ??= await loadCanonicalFerret();
      return `export default ${JSON.stringify(canonical.data)};`;
    },
    configureServer(server) {
      server.middlewares.use(async (req, res, next) => {
        const file = req.url?.split('?')[0]?.match(/^\/ferret\/(ferret-motion-[\w-]+\.png)$/)?.[1];
        if (!file) return next();
        try {
          canonical ??= await loadCanonicalFerret();
          if (!canonical.images.includes(file)) return next();
          res.setHeader('Content-Type', 'image/png');
          res.end(await readFile(path.join(resourceRoot, file)));
        } catch (error) { next(error); }
      });
    },
    async generateBundle() {
      for (const file of canonical.images) {
        this.emitFile({ type: 'asset', fileName: `ferret/${file}`, source: await readFile(path.join(resourceRoot, file)) });
      }
    },
  };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const canonical = await loadCanonicalFerret();
  console.log(`Canonical ferret: ${canonical.data.states.length} states / ${canonical.images.length} PNG assets validated.`);
}
