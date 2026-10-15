// Isolated synthetic local fixture. No production G1 code, data, auth or API.
export default {
  async fetch(request, env) {
    await env.DB.prepare("CREATE TABLE IF NOT EXISTS sandbox_probe (id INTEGER PRIMARY KEY, value TEXT NOT NULL)").run();
    await env.DB.prepare("INSERT OR REPLACE INTO sandbox_probe(id, value) VALUES (1, ?)").bind("SYNTHETIC_LOCAL_ONLY").run();
    const row = await env.DB.prepare("SELECT id, value FROM sandbox_probe WHERE id = 1").first();
    if (row?.value !== "SYNTHETIC_LOCAL_ONLY") return new Response("readback failed", {status: 500});
    return Response.json({marker: "LOCAL_WORKER_D1_REAL_READBACK", row});
  }
};
