import { describe, expect, it } from 'vitest';
import { displayPath, externalSkillErrorMessage, extraRoots, hasValidSource, helperErrorCode, isConnectable, planRoots, rootsNeedReload, skillCenterItems, validSkillKey } from '../src/skills';
import { skillSource } from './fixtures';

describe('skills logic', () => {
  it('extracts helper codes from bridge errors and maps them to messages', () => {
    const err = 'Chadex bridge: Backend (external_skill_roots_conflict)。請查看診斷並重試。';
    expect(helperErrorCode(err)).toBe('external_skill_roots_conflict');
    expect(externalSkillErrorMessage(err)).toContain('已在其他地方變更');
    expect(rootsNeedReload(err)).toBe(true);
    expect(rootsNeedReload('Chadex bridge: Backend (external_skill_roots_state_unknown)')).toBe(true);
    expect(rootsNeedReload('Chadex bridge: Backend (skill_root_sensitive)')).toBe(false);
    expect(externalSkillErrorMessage('x (skill_root_not_canonical)')).toContain('無法連接');
    expect(externalSkillErrorMessage('x (skill_root_sensitive)')).toContain('受保護');
    expect(externalSkillErrorMessage('plain failure')).toBe('plain failure');
  });
  it('strips the extended-length prefix for display only', () => {
    expect(displayPath('\\\\?\\C:\\a')).toBe('C:\\a');
    expect(displayPath('\\\\?\\UNC\\srv\\s')).toBe('\\\\srv\\s');
    expect(displayPath('C:\\a')).toBe('C:\\a');
  });
  it('treats symlink-only and duplicate sources as not connectable', () => {
    expect(isConnectable(skillSource())).toBe(true);
    expect(isConnectable(skillSource({ valid_count: 0, provided_by: ['X'] }))).toBe(false);
    expect(isConnectable(skillSource({ status: 'duplicate_source' }))).toBe(false);
    expect(isConnectable(skillSource({ canonical_path: null }))).toBe(false);
  });
  it('plans full replacement lists with scripts a subset of roots', () => {
    expect(planRoots(['a'], [], { connect: 'b', on: true })).toEqual({ roots: ['a', 'b'], script_roots: [] });
    expect(planRoots(['a', 'b'], ['b'], { connect: 'b', on: false })).toEqual({ roots: ['a'], script_roots: [] });
    expect(planRoots(['a', 'b'], ['a'], { scripts: 'b', on: true })).toEqual({ roots: ['a', 'b'], script_roots: ['a', 'b'] });
    expect(planRoots(['a'], ['a'], { scripts: 'a', on: false }).script_roots).toEqual([]);
  });
  it('lists configured and recommended roots without a discovered row', () => {
    const source = skillSource();
    const discovery = { format: 'f', sources: [source], recommended_roots: [source.canonical_path as string, 'R'] };
    expect(extraRoots(discovery, [source.canonical_path as string, 'C'])).toEqual([{ path: 'C', configured: true }, { path: 'R', configured: false }]);
    expect(hasValidSource(discovery, [])).toBe(true);
    expect(hasValidSource({ format: 'f', sources: [skillSource({ valid_count: 0 })], recommended_roots: [] }, [])).toBe(false);
    expect(hasValidSource(null, ['C'])).toBe(true);
  });
  it('validates skill keys and merges catalog with inventory', () => {
    expect(validSkillKey('my-skill_1.0')).toBe(true);
    for (const bad of ['', '.', '..', 'a b', 'a/b', 'x'.repeat(97)]) expect(validSkillKey(bad)).toBe(false);
    const managed = { skill_id: 'm', skill_key: 'm', state_revision: 's', preferred_package_revision: 'p', definition_revision: 'd', name: 'Zed', description: '', total_versions: 1 };
    const items = skillCenterItems({ project: 'p', catalog_revision: 'c', total_count: 1, returned_count: 1, invalid_count: 0, diagnostics: [], discovery_truncated: false,
      skills: [{ skill_id: 'a', name: 'Alpha', description: '', definition_revision: 'd', source_scope: 'project', trust: 'project_content', name_conflict: false }] },
    { project: 'p', total_count: 1, skills: [managed] });
    expect(items.map((item) => item.name)).toEqual(['Alpha', 'Zed']);
    expect(items[1].trust).toBe('operator_installed_guidance');
  });
});
