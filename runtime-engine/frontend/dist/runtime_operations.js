import { translate, localizedCountLabel, } from "./runtime_i18n.js";
import { communicationTimeLabel, parseAgentIds, } from "./runtime_communication.js";
export function operationKey(prefix) {
    const random = typeof crypto !== "undefined" && typeof crypto.randomUUID === "function"
        ? crypto.randomUUID()
        : Date.now().toString(36) + "-" + Math.random().toString(36).slice(2);
    return prefix + "-" + random;
}
export function idempotencyKeyFor(pending, fingerprint, prefix) {
    return pending && pending.fingerprint === fingerprint
        ? pending
        : { fingerprint, key: operationKey(prefix) };
}
export function formatCommunicationAvailability(readAvailable, manageAvailable, language) {
    const available = readAvailable !== false;
    if (readAvailable === null) {
        return language === "zh-CN" ? "正在检查 communication:read…" : "communication:read checking…";
    }
    if (!available) {
        return language === "zh-CN" ? "communication:read 不可用" : "communication:read unavailable";
    }
    return ("communication:read" +
        (manageAvailable === false
            ? language === "zh-CN"
                ? " · 只读"
                : " · read only"
            : language === "zh-CN"
                ? " · 当前视图每 30 秒刷新 · 端点租约每 30 秒续期"
                : " · 30s refresh while visible · 30s endpoint lease renewal"));
}
export function formatAgentCardRevision(agent, language) {
    return ((language === "zh-CN" ? "配置版本 " : "Profile revision ") +
        String(agent?.profile_revision || 0) +
        (language === "zh-CN" ? " · 控制器代数 " : " · controller generation ") +
        String(agent?.current_controller_generation || 0) +
        (language === "zh-CN" ? " · 更新于 " : " · updated ") +
        communicationTimeLabel(agent?.updated_at_unix_ms, language));
}
export function formatAgentWakeStatus(agent, language) {
    const unresolvedWakeCount = Number(agent?.unresolved_wake_count || 0);
    const latestWakeState = String(agent?.latest_wake_state || "none");
    return (localizedCountLabel(unresolvedWakeCount, "unresolved Wake", "unresolved Wakes", language) +
        (language === "zh-CN" ? " · 最近状态 " : " · latest ") +
        translate(latestWakeState, language) +
        (language === "zh-CN"
            ? " · 收件箱投递与唤醒消费彼此独立"
            : " · Inbox Delivery and Wake consumption remain independent"));
}
export function formatAgentEndpointStatus(endpoint, language) {
    if (!endpoint) {
        return language === "zh-CN"
            ? "此窗口尚未作为该 Agent。Agent 卡片、对话、收件箱投递和唤醒意图仍会持久保留。"
            : "This window is not acting as the Agent. Agent Card, Conversations, Inbox deliveries, and Wake Intents remain durable.";
    }
    return ((language === "zh-CN" ? "浏览器端点 " : "Browser Endpoint ") +
        endpoint.endpoint_id +
        " · " +
        translate(endpoint.lifecycle, language) +
        (language === "zh-CN" ? " · 代数 " : " · generation ") +
        String(endpoint.controller_generation) +
        (language === "zh-CN" ? " · 租约至 " : " · lease ") +
        communicationTimeLabel(endpoint.lease_expires_at_unix_ms, language) +
        (language === "zh-CN"
            ? " · 运行控制台适配器：仅轮询（运行时可唤醒："
            : " · Runtime Console adapter: polling only (runtime wake capable: ") +
        String(endpoint.wake_capable) +
        ")");
}
export function formatConversationSeq(summary, detail, language) {
    return ((language === "zh-CN" ? "序号 " : "seq ") +
        String(summary?.last_seq || 0) +
        " · " +
        localizedCountLabel(summary?.message_count, "message", "messages", language) +
        (Number(detail?.after_seq || 0) > 0 || detail?.truncated
            ? language === "zh-CN"
                ? " · 最近有界页面"
                : " · recent bounded page"
            : ""));
}
export function validateAgentCreateInputs(handle, displayName, description, labelsRaw) {
    const cleanHandle = handle.trim();
    const cleanName = displayName.trim();
    const cleanDescription = description.trim();
    const labels = parseAgentIds(labelsRaw);
    if (!cleanHandle || !cleanName) {
        return { valid: false, error: "Handle and display name are required." };
    }
    const fingerprint = JSON.stringify({
        handle: cleanHandle,
        displayName: cleanName,
        description: cleanDescription,
        labels,
    });
    return {
        valid: true,
        data: {
            handle: cleanHandle,
            displayName: cleanName,
            description: cleanDescription,
            labels,
            fingerprint,
        },
    };
}
export function validateAgentUpdateInputs(handle, displayName, description, labelsRaw) {
    const cleanHandle = handle.trim();
    const cleanName = displayName.trim();
    const cleanDescription = description.trim();
    const specialtyLabels = parseAgentIds(labelsRaw);
    if (!cleanHandle || !cleanName) {
        return { valid: false, error: "Handle and display name are required." };
    }
    return {
        valid: true,
        data: {
            handle: cleanHandle,
            displayName: cleanName,
            description: cleanDescription,
            specialtyLabels,
        },
    };
}
export function validateConversationCreateInputs(title, agentIdsRaw, defaultAgentId = "") {
    const cleanTitle = title.trim();
    const agentIds = parseAgentIds(agentIdsRaw || defaultAgentId);
    if (agentIds.length === 0) {
        return { valid: false, error: "At least one Agent id is required." };
    }
    const fingerprint = JSON.stringify({ title: cleanTitle, agentIds: [...agentIds].sort() });
    return {
        valid: true,
        data: {
            title: cleanTitle,
            agentIds,
            fingerprint,
        },
    };
}
