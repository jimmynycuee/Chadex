function record(value) { return value && typeof value === "object" && !Array.isArray(value) ? value : {}; }
function list(value) { return Array.isArray(value) ? value : []; }
function display(value) { if (value == null || value === "")
    return "—"; if (typeof value === "object") {
    try {
        return JSON.stringify(value);
    }
    catch {
        return "—";
    }
} return String(value); }
export function capabilityLabels(value) { if (Array.isArray(value))
    return value.filter((item) => typeof item === "string"); if (value && typeof value === "object")
    return Object.entries(value).filter(([, enabled]) => enabled === true).map(([name]) => name).sort(); return []; }
function statusFor(data, section) { return record(record(data.section_status)[section]); }
function sectionOk(data, section) { return statusFor(data, section).status !== "error"; }
function sectionError(doc, section, data) { const node = doc.getElementById(`${section}-error`); if (!node)
    return; const status = statusFor(data, section); const failed = status.status === "error"; node.hidden = !failed; node.textContent = failed ? display(status.error || `${section} unavailable`) : ""; }
function clear(node) { while (node?.firstChild)
    node.removeChild(node.firstChild); }
function cell(doc, row, value, code = false) { const td = doc.createElement("td"); const child = doc.createElement(code ? "code" : "span"); child.textContent = display(value); td.appendChild(child); row.appendChild(td); }
function card(doc, label, value) { const box = doc.createElement("article"); box.className = "card"; const name = doc.createElement("span"); name.textContent = label; const content = doc.createElement("strong"); content.textContent = display(value); box.append(name, content); return box; }
function setVisible(doc, id, visible) { const node = doc.getElementById(id); if (node)
    node.hidden = !visible; }
function actionCell(doc, row, project) { const td = doc.createElement("td"); const group = doc.createElement("div"); group.className = "project-actions"; const actions = record(project.actions); for (const kind of ["enable", "disable", "unregister"]) {
    const button = doc.createElement("button");
    button.type = "button";
    button.textContent = kind[0].toUpperCase() + kind.slice(1);
    button.disabled = actions[kind] !== true;
    button.setAttribute("data-project-action", kind);
    button.setAttribute("data-project-id", String(project.id || ""));
    button.addEventListener("click", () => doc.dispatchEvent(new CustomEvent("admin-project-action", { detail: { kind, project: { ...project } } })));
    group.appendChild(button);
} td.appendChild(group); row.appendChild(td); }
export function renderAdminDashboard(doc, raw) {
    const data = record(raw);
    sectionError(doc, "overview", data);
    if (sectionOk(data, "overview")) {
        const overview = doc.getElementById("overview");
        clear(overview);
        const value = record(data.overview);
        const cards = [["Server", `${display(value.version)} · ${display(value.build_commit)}`], ["Authority", value.authority_mode], ["Agents", `${display(value.agents_online || 0)} / ${display(value.agents_total || 0)} online`], ["Projects", `${display(value.projects_online || 0)} / ${display(value.projects_total || 0)} online`], ["Active jobs", value.active_jobs || 0], ["Compatibility", value.version_compatibility || "unknown"]];
        for (const [label, content] of cards)
            overview?.appendChild(card(doc, label, content));
        const diagnostics = doc.getElementById("diagnostics");
        clear(diagnostics);
        for (const [key, content] of Object.entries(record(data.diagnostics))) {
            const dt = doc.createElement("dt");
            dt.textContent = key.replace(/_/g, " ");
            const dd = doc.createElement("dd");
            dd.textContent = display(content);
            diagnostics?.append(dt, dd);
        }
    }
    sectionError(doc, "devices", data);
    if (sectionOk(data, "devices")) {
        const devices = doc.getElementById("devices");
        clear(devices);
        const rows = list(data.devices);
        for (const item of rows) {
            const device = record(item);
            const row = doc.createElement("tr");
            const values = [[device.display_name], [device.client_id, true], [device.status], [device.transport], [device.hostname], [device.last_seen], [capabilityLabels(device.capabilities).join(", ")], [device.project_count], [device.active_jobs], [device.compatibility]];
            for (const [value, code] of values)
                cell(doc, row, value, Boolean(code));
            devices?.appendChild(row);
        }
        setVisible(doc, "devices-empty", rows.length === 0);
    }
    sectionError(doc, "projects", data);
    if (sectionOk(data, "projects")) {
        const projects = doc.getElementById("projects");
        clear(projects);
        const rows = list(data.projects);
        for (const item of rows) {
            const project = record(item);
            const row = doc.createElement("tr");
            const values = [[project.id, true], [project.name], [project.client_id], [project.path], [project.lifecycle_status || project.readiness], [project.active_jobs], [project.git_available], [project.allow_patch], [project.shell_profile_status], [project.compatibility], [project.console_hint]];
            for (const [value, code] of values)
                cell(doc, row, value, Boolean(code));
            actionCell(doc, row, project);
            projects?.appendChild(row);
        }
        setVisible(doc, "projects-empty", rows.length === 0);
    }
    sectionError(doc, "activity", data);
    if (sectionOk(data, "activity")) {
        const activity = doc.getElementById("activity");
        clear(activity);
        const rows = list(data.activity);
        for (const item of rows) {
            const entry = record(item);
            const li = doc.createElement("li");
            li.textContent = [entry.created_at, entry.kind, entry.project_id, entry.status].filter(Boolean).map(String).join(" · ");
            activity?.appendChild(li);
        }
        setVisible(doc, "activity-empty", rows.length === 0);
    }
}
