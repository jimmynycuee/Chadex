// Actual Miniflare Worker -> D1 readback, fixed main port, local-only state.
// No require-cache patches, fake responses, dependency install or broad ports.
const path = require("node:path");
const {Miniflare, convertV4MiniflareOptions} = require("miniflare");
async function main() {
  const mf = new Miniflare(convertV4MiniflareOptions({
    modules: true,
    scriptPath: path.join(__dirname, "worker.mjs"),
    compatibilityDate: "2026-10-01",
    host: "127.0.0.1",
    port: Number(process.argv[2]),
    d1Databases: {DB: "00000000-0000-0000-0000-000000000001"},
    resourcePersistencePath: path.join(__dirname, ".goal-acceptance/state"),
    resourceTmpPath: path.join(__dirname, ".goal-acceptance/tmp"),
    cf: false,
  }));
  try {
    const url = await mf.ready;
    if (url.hostname !== "127.0.0.1" || Number(url.port) !== Number(process.argv[2])) {
      throw new Error("unexpected endpoint");
    }
    const response = await fetch(url);
    const result = await response.json();
    if (!response.ok || result.marker !== "LOCAL_WORKER_D1_REAL_READBACK" ||
        result.row?.value !== "SYNTHETIC_LOCAL_ONLY") throw new Error("D1 readback mismatch");
    console.log(JSON.stringify(result));
  } finally {
    await mf.dispose();
  }
}
main().catch(error => {console.error(error.stack || String(error));process.exitCode = 78;});
