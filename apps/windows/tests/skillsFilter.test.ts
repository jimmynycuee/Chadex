import { describe, expect, it } from 'vitest';
import { filterSkills, matchesQuery } from '../src/skillsFilter';
import type { SkillCenterItem } from '../src/skills';

const base = { definition_revision: 'r', source_scope: 's', name_conflict: false };
const managed = (active: boolean) => ({ skill_id: 'm', skill_key: 'm', state_revision: 's', active_package_revision: active ? 'p' : null, preferred_package_revision: 'p', definition_revision: 'r', name: 'm', description: 'd', total_versions: 1 });
const items: SkillCenterItem[] = [
  { ...base, skill_id: 'a', name: 'Alpha', description: 'Writes reports', trust: 'project_content' },
  { ...base, skill_id: 'b', name: 'Beta', description: 'Reviews code', trust: 'operator_configured_guidance', scripts_allowed: false },
  { ...base, skill_id: 'c', name: 'Gamma', description: 'Deploys', trust: 'operator_installed_guidance', managed: managed(true) },
  { ...base, skill_id: 'd', name: 'Delta', description: 'Off one', trust: 'operator_installed_guidance', managed: managed(false) },
];
const ids = (list: SkillCenterItem[]) => list.map((item) => item.skill_id);

describe('skills filter', () => {
  it('filters by segment', () => {
    expect(ids(filterSkills(items, 'all', ''))).toEqual(['a', 'b', 'c', 'd']);
    expect(ids(filterSkills(items, 'active', ''))).toEqual(['a', 'b', 'c']);
    expect(ids(filterSkills(items, 'external', ''))).toEqual(['b']);
    expect(ids(filterSkills(items, 'installed', ''))).toEqual(['c', 'd']);
  });
  it('searches name, description and source label case-insensitively', () => {
    expect(matchesQuery(items[0], '  REPORT ')).toBe(true);
    expect(ids(filterSkills(items, 'all', 'beta'))).toEqual(['b']);
    expect(ids(filterSkills(items, 'all', '外部'))).toEqual(['b']);
    expect(ids(filterSkills(items, 'installed', 'off'))).toEqual(['d']);
    expect(ids(filterSkills(items, 'external', 'alpha'))).toEqual([]);
  });
});
