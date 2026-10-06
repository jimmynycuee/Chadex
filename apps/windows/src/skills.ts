// Pure Skills-page logic, mirroring the macOS ExternalSkillSourcesView / AppModel behavior.
import type { ExternalSkillSource, ExternalSkillSourceDiscovery, ManagedSkillEntry, SkillCatalog, SkillDescriptor, SkillInventory } from './contracts';
import { safeError } from './state';

/** The bridge reports helper failures as "... (<helper_code>)。..."; the code is the only structured part. */
export function helperErrorCode(error: unknown): string | null {
  const message = typeof error === 'string' ? error : error instanceof Error ? error.message
    : error && typeof error === 'object' && 'message' in error ? String(error.message) : '';
  return /\(([a-z][a-z0-9_]*)\)/.exec(message)?.[1] ?? null;
}
export function externalSkillErrorMessage(error: unknown): string {
  switch (helperErrorCode(error)) {
    case 'external_skill_roots_conflict': return 'Skill 資料夾設定已在其他地方變更，已重新載入，請再試一次。';
    case 'skill_root_invalid': case 'skill_root_not_found': case 'skill_root_is_link':
    case 'skill_root_not_directory': case 'skill_root_not_canonical': return '無法連接此資料夾，請選擇實際存在且不是連結的資料夾。';
    case 'skill_root_sensitive': return '此資料夾受保護，無法連接。';
    case 'runner_config_rejected': return 'Runner 拒絕了新的 Skill 資料夾設定。';
    case 'runner_config_reload_failed': case 'runner_config_reload_unsupported': case 'runtime_unreachable':
      return 'Runner 無法重新載入設定，未做任何變更。';
    case 'runner_config_restart_required': return '請重新啟動本機 runtime 以套用 Skill 資料夾變更。';
    case 'runtime_not_ready': return '本機 runtime 尚未就緒，請先完成設定並等到閒置。';
    case 'external_skill_roots_unverified': return '無法驗證新的 Skill 資料夾，變更未保留。';
    case 'external_skill_roots_state_unknown': return 'Runner 可能仍在使用新的資料夾。請重新載入後再套用一次。';
    case 'skill_management_requires_local_runtime': return 'Skill 與專案記憶只能在本機 runtime 上管理。目前連線的是遠端 Server，請先連回本機 runtime。';
    case 'skill_management_credential_unavailable': return '無法取得本機管理憑證，例如 runtime 尚未就緒。請稍後重試，或重新啟動本機服務。';
    case 'skill_management_credential_rejected': return '本機 runtime 拒絕了管理憑證。請重新啟動本機服務後再試。';
    case 'runner_config_path_is_link': return 'runner.toml 所在的資料夾是透過連結（junction 或 symlink）存取，Chadex 無法安全寫入。請改用不在連結之下的 runner.toml 位置。';
    default: return safeError(error);
  }
}
/** Errors after which the roots must be re-read before another attempt. */
export const rootsNeedReload = (error: unknown) => ['external_skill_roots_conflict', 'external_skill_roots_state_unknown'].includes(helperErrorCode(error) ?? '');

/** Helper paths on Windows are std canonical (\\?\C:\...); show the familiar form but always send the original. */
export function displayPath(path: string): string {
  return path.startsWith('\\\\?\\UNC\\') ? `\\\\${path.slice(8)}` : path.startsWith('\\\\?\\') ? path.slice(4) : path;
}
export const kindLabel = (kind: string) => ({ agents: 'Agents', claude: 'Claude Code', codex: 'Codex' } as Record<string, string>)[kind] ?? kind;
export function statusText(status: string): string {
  return ({ not_found: '找不到資料夾', not_directory: '不是資料夾', unavailable: '資料夾無法使用', scan_limit_exceeded: '項目過多，無法掃描' } as Record<string, string>)[status] ?? status;
}

export const isProvidedElsewhere = (source: ExternalSkillSource) =>
  source.status === 'duplicate_source' || (source.provided_by.length > 0 && source.valid_count === 0);
export const isConnectable = (source: ExternalSkillSource) =>
  source.status === 'available' && Boolean(source.canonical_path) && !isProvidedElsewhere(source);
export function providedByPaths(source: ExternalSkillSource, sources: ExternalSkillSource[]): string[] {
  if (source.provided_by.length) return source.provided_by.map(displayPath);
  const target = source.same_as ? sources.find((candidate) => candidate.kind === source.same_as) : undefined;
  return target ? [displayPath(target.path)] : [];
}
/** Configured or recommended roots that have no discovered row of their own. */
export function extraRoots(discovery: ExternalSkillSourceDiscovery | null, roots: string[]): { path: string; configured: boolean }[] {
  const seen = new Set((discovery?.sources ?? []).filter(isConnectable).map((source) => source.canonical_path as string));
  const result: { path: string; configured: boolean }[] = [];
  for (const path of roots) if (!seen.has(path)) { seen.add(path); result.push({ path, configured: true }); }
  for (const path of discovery?.recommended_roots ?? []) if (!seen.has(path)) { seen.add(path); result.push({ path, configured: false }); }
  return result;
}
/** A usable source exists when a discovered root holds Skills of its own, or any root is already connected. */
export const hasValidSource = (discovery: ExternalSkillSourceDiscovery | null, roots: string[]) =>
  roots.length > 0 || (discovery?.sources ?? []).some((source) => isConnectable(source) && source.valid_count > 0);

/** Full replacement lists; script roots stay a subset of roots (scripts default off). */
export function planRoots(roots: string[], scriptRoots: string[], change: { connect: string; on: boolean } | { scripts: string; on: boolean }) {
  const scripts = new Set(scriptRoots);
  let next = roots;
  if ('connect' in change) {
    next = roots.filter((path) => path !== change.connect);
    if (change.on) next = [...next, change.connect]; else scripts.delete(change.connect);
  } else if (change.on) scripts.add(change.scripts); else scripts.delete(change.scripts);
  return { roots: next, script_roots: next.filter((path) => scripts.has(path)) };
}

export const validSkillKey = (key: string) => key.length > 0 && key.length <= 96 && key !== '.' && key !== '..' && /^[A-Za-z0-9._-]+$/.test(key);

export interface SkillCenterItem {
  skill_id: string; name: string; description: string; definition_revision: string; package_revision?: string | null;
  source_scope: string; trust: string; name_conflict: boolean; scripts_allowed?: boolean | null; managed?: ManagedSkillEntry;
}
export const isSkillActive = (item: SkillCenterItem) => item.managed ? Boolean(item.managed.active_package_revision) : true;
export function skillCenterItems(catalog: SkillCatalog | null, inventory: SkillInventory | null): SkillCenterItem[] {
  const managed = new Map((inventory?.skills ?? []).map((entry) => [entry.skill_id, entry]));
  const items: SkillCenterItem[] = (catalog?.skills ?? []).map((descriptor: SkillDescriptor) => ({ ...descriptor, managed: managed.get(descriptor.skill_id) }));
  const known = new Set(items.map((item) => item.skill_id));
  for (const entry of managed.values()) if (!known.has(entry.skill_id)) items.push({
    skill_id: entry.skill_id, name: entry.name, description: entry.description, definition_revision: entry.definition_revision,
    package_revision: entry.preferred_package_revision, source_scope: 'runner', trust: 'operator_installed_guidance', name_conflict: false, scripts_allowed: true, managed: entry });
  return items.sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: 'accent' }) || a.skill_id.localeCompare(b.skill_id));
}
export const sourceLabel = (item: SkillCenterItem) => item.trust === 'project_content' ? 'Project'
  : item.trust === 'operator_configured_guidance' ? '外部' : item.trust === 'operator_installed_guidance' ? '已安裝' : item.source_scope;
