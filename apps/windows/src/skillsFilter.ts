// Pure list filtering for the Skills page (search box + segmented filter).
import { isSkillActive, sourceLabel, type SkillCenterItem } from './skills';

export type SkillFilter = 'all' | 'active' | 'external' | 'installed';
export const SKILL_FILTERS: { id: SkillFilter; label: string }[] = [
  { id: 'all', label: '全部' }, { id: 'active', label: '已啟用' }, { id: 'external', label: '外部來源' }, { id: 'installed', label: '已安裝' },
];

export const isExternalSkill = (item: SkillCenterItem) => item.trust === 'operator_configured_guidance';
export const isInstalledSkill = (item: SkillCenterItem) => Boolean(item.managed) || item.trust === 'operator_installed_guidance';

export function matchesFilter(item: SkillCenterItem, filter: SkillFilter): boolean {
  switch (filter) {
    case 'active': return isSkillActive(item);
    case 'external': return isExternalSkill(item);
    case 'installed': return isInstalledSkill(item);
    default: return true;
  }
}
export function matchesQuery(item: SkillCenterItem, query: string): boolean {
  const needle = query.trim().toLocaleLowerCase();
  if (!needle) return true;
  return [item.name, item.description, sourceLabel(item)].some((field) => field.toLocaleLowerCase().includes(needle));
}
export const filterSkills = (items: SkillCenterItem[], filter: SkillFilter, query: string) =>
  items.filter((item) => matchesFilter(item, filter) && matchesQuery(item, query));
