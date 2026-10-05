import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import type { DesktopApi } from './api';
import type { ExternalSkillRootsState, ExternalSkillSource, ExternalSkillSourceDiscovery, SkillCatalog, SkillInventory } from './contracts';
import { safeError } from './state';
import {
  displayPath, externalSkillErrorMessage, extraRoots, hasValidSource, isProvidedElsewhere, isSkillActive, kindLabel, planRoots,
  providedByPaths, rootsNeedReload, skillCenterItems, sourceLabel, statusText, validSkillKey, type SkillCenterItem,
} from './skills';

function Chip({ children, title }: { children: ReactNode; title?: string }) { return <span className="skill-chip" title={title}>{children}</span>; }

export function SkillsPage({ api, project, helperReady }: { api: DesktopApi; project: string | null; helperReady: boolean }) {
  const [discovery, setDiscovery] = useState<ExternalSkillSourceDiscovery | null>(null);
  const [rootsState, setRootsState] = useState<ExternalSkillRootsState | null>(null);
  const [externalError, setExternalError] = useState<string | null>(null);
  const [externalLoading, setExternalLoading] = useState(false);
  const [applying, setApplying] = useState(false);
  const [catalog, setCatalog] = useState<SkillCatalog | null>(null);
  const [inventory, setInventory] = useState<SkillInventory | null>(null);
  const [skillsError, setSkillsError] = useState<string | null>(null);
  const [skillsLoading, setSkillsLoading] = useState(false);
  const [mutating, setMutating] = useState<string | null>(null);
  const [importing, setImporting] = useState(false);
  const [skillKey, setSkillKey] = useState('');
  const [archive, setArchive] = useState('');
  const [installing, setInstalling] = useState(false);
  const [importError, setImportError] = useState<string | null>(null);
  const [expandedInvalid, setExpandedInvalid] = useState<Set<string>>(new Set());
  const epoch = useRef(0);

  const roots = rootsState?.roots ?? [];
  const scriptRoots = new Set(rootsState?.script_roots ?? []);
  const sources = discovery?.sources ?? [];
  const extras = extraRoots(discovery, roots);
  const busy = applying || externalLoading;
  const canEdit = helperReady && rootsState !== null && !busy;
  const validSource = hasValidSource(discovery, roots);
  const items = skillCenterItems(catalog, inventory);

  const loadSkills = useCallback(async () => {
    if (!project || !helperReady) { setCatalog(null); setInventory(null); setSkillsError(null); return; }
    const mine = epoch.current; setSkillsLoading(true);
    try {
      const nextCatalog = await api.getSkillCatalog(project);
      let nextInventory: SkillInventory | null = null; let warning: string | null = null;
      try { nextInventory = await api.getSkillInventory(project); } catch (error) { warning = safeError(error); }
      if (mine !== epoch.current) return;
      setCatalog(nextCatalog); setInventory(nextInventory); setSkillsError(warning);
    } catch (error) {
      if (mine !== epoch.current) return;
      setCatalog(null); setInventory(null); setSkillsError(safeError(error));
    } finally { if (mine === epoch.current) setSkillsLoading(false); }
  }, [api, project, helperReady]);

  const loadExternal = useCallback(async () => {
    if (!helperReady) return;
    const mine = epoch.current; setExternalLoading(true);
    let failure: string | null = null;
    try { const found = await api.discoverExternalSkillSources(); if (mine === epoch.current) setDiscovery(found); }
    catch (error) { if (mine === epoch.current) setDiscovery(null); failure = externalSkillErrorMessage(error); }
    try { const state = await api.getExternalSkillRoots(); if (mine === epoch.current) setRootsState(state); }
    catch (error) { if (mine === epoch.current) setRootsState(null); failure = externalSkillErrorMessage(error); }
    if (mine !== epoch.current) return;
    setExternalError(failure); setExternalLoading(false);
  }, [api, helperReady]);

  useEffect(() => { epoch.current += 1; setSkillsLoading(false); void loadSkills(); }, [loadSkills]);
  useEffect(() => { void loadExternal(); }, [loadExternal]);
  useEffect(() => () => { epoch.current += 1; }, []);

  async function applyRoots(next: { roots: string[]; script_roots: string[] }) {
    if (applying || !rootsState) return;
    setApplying(true);
    try {
      setRootsState(await api.setExternalSkillRoots({ ...next, expected_revision: rootsState.revision, ...(project ? { verify_project_path: project } : {}) }));
      setExternalError(null);
      await loadSkills();
    } catch (error) {
      setExternalError(externalSkillErrorMessage(error));
      if (rootsNeedReload(error)) { try { setRootsState(await api.getExternalSkillRoots()); } catch { /* keep the error above */ } }
    } finally { setApplying(false); }
  }
  const setConnected = (path: string, on: boolean) => applyRoots(planRoots(roots, [...scriptRoots], { connect: path, on }));
  const setScripts = (path: string, on: boolean) => applyRoots(planRoots(roots, [...scriptRoots], { scripts: path, on }));
  async function chooseFolder() {
    try {
      const path = await api.chooseSkillFolder();
      if (path === null || roots.includes(path)) return;
      await applyRoots(planRoots(roots, [...scriptRoots], { connect: path, on: true }));
    } catch (error) { setExternalError(externalSkillErrorMessage(error)); }
  }
  async function chooseArchive() {
    if (!project) return;
    try {
      const chosen = await api.chooseSkillArchive(project);
      if (chosen === null) return;
      setArchive(chosen); setImportError(null);
    } catch (error) { setArchive(''); setImportError(/skill_archive_outside_project/.test(safeError(error)) ? '請選擇位於目前專案資料夾內的 ZIP。' : safeError(error)); }
  }
  async function install() {
    if (!project || installing || !validSkillKey(skillKey) || !archive) return;
    setInstalling(true); setImportError(null);
    try {
      await api.installSkill(project, skillKey.trim(), archive);
      setImporting(false); setSkillKey(''); setArchive('');
      await loadSkills();
    } catch (error) { setImportError(safeError(error)); await loadSkills(); }
    finally { setInstalling(false); }
  }
  async function toggleManaged(item: SkillCenterItem) {
    if (!project || !item.managed || mutating) return;
    setMutating(item.skill_id);
    try {
      await (isSkillActive(item) ? api.deactivateSkill(project, item.managed) : api.activateSkill(project, item.managed));
      setSkillsError(null);
    } catch (error) { setSkillsError(safeError(error)); }
    finally { setMutating(null); await loadSkills(); }
  }

  const importButton = <button className={validSource ? '' : 'primary'} disabled={!project || !helperReady || installing} onClick={() => setImporting(true)}>匯入 Skill（ZIP）…</button>;
  const toggles = (path: string) => {
    const connected = roots.includes(path);
    return <div className="skill-toggles">
      <label><input type="checkbox" checked={connected} disabled={!canEdit} onChange={(event) => { void setConnected(path, event.target.checked); }} />連接</label>
      <label><input type="checkbox" checked={connected && scriptRoots.has(path)} disabled={!canEdit || !connected} onChange={(event) => { void setScripts(path, event.target.checked); }} />允許腳本</label>
    </div>;
  };
  function sourceRow(source: ExternalSkillSource) {
    const key = `${source.kind}|${source.path}`; const invalid = source.packages.filter((pkg) => pkg.state === 'invalid');
    return <li className="skill-row" key={key}>
      <div className="skill-title"><strong>{kindLabel(source.kind)}</strong><code title={source.path}>{displayPath(source.path)}</code></div>
      {isProvidedElsewhere(source) ? <p className="skill-meta">由 {providedByPaths(source, sources).join('、')} 提供{source.root_is_link ? '（此資料夾為連結）' : ''}</p>
        : source.status === 'available' && source.canonical_path ? <>
          <p className="skill-meta">{source.valid_count} 個 Skill · {source.script_count} 個含腳本{source.invalid_count > 0 ? ` · ${source.invalid_count} 個無效` : ''}{source.symlink_count > 0 ? ` · ${source.symlink_count} 個符號連結` : ''}</p>
          {source.root_is_link && <p className="skill-meta">此資料夾為連結，實際路徑：<code>{displayPath(source.canonical_path)}</code></p>}
          {!source.root_is_link && source.canonical_path !== source.path && <p className="skill-meta">實際路徑：<code>{displayPath(source.canonical_path)}</code></p>}
          {source.truncated && <p className="skill-meta">掃描已達上限，部分 Skill 可能未列出。</p>}
          {toggles(source.canonical_path)}
          {invalid.length > 0 && <div className="skill-invalid"><button className="text-button" aria-expanded={expandedInvalid.has(key)} onClick={() => setExpandedInvalid((current) => { const next = new Set(current); if (!next.delete(key)) next.add(key); return next; })}>{expandedInvalid.has(key) ? '收合' : '檢視'} {invalid.length} 個無效套件</button>
            {expandedInvalid.has(key) && <ul>{invalid.map((pkg) => <li key={pkg.package}><code>{pkg.package} — {pkg.invalid_reason ?? 'invalid'}</code></li>)}</ul>}</div>}
        </> : <p className="skill-meta">{statusText(source.status)}</p>}
    </li>;
  }
  const skillRow = (item: SkillCenterItem) => {
    const active = isSkillActive(item);
    return <li className="skill-row" key={item.skill_id}>
      <div className="skill-title"><strong>{item.name}</strong><Chip>{sourceLabel(item)}</Chip><Chip>{active ? '已啟用' : '已停用'}</Chip>
        {item.name_conflict && <Chip title="有多個 Skill 解析成相同名稱；在衝突移除前，名稱比對會安全失敗。">名稱衝突</Chip>}
        {item.trust === 'operator_configured_guidance' && item.scripts_allowed === false && <Chip title="此 Skill 的腳本預設不允許執行。請在其資料夾開啟「允許腳本」。">腳本關閉</Chip>}
        {item.managed && <button disabled={mutating !== null || !helperReady} onClick={() => { void toggleManaged(item); }} aria-label={`${active ? '停用' : '啟用'} ${item.name}`}>{active ? '停用' : '啟用'}</button>}</div>
      <p className="skill-meta">{item.description}</p>
      <small className="skill-rev" title={item.definition_revision}>{item.definition_revision.slice(0, 12)}</small>
    </li>;
  };

  return <>
    <section className="panel"><div className="panel-heading"><h2>Skills</h2>
      <div className="skill-actions"><button className="text-button" disabled={skillsLoading || !helperReady} onClick={() => { void loadSkills(); }}>重新整理 ↻</button>{validSource && importButton}</div></div>
      <p className="panel-note">目前專案可用的重複使用流程。Chadex 只探索與比對 metadata，被選中使用時才讀取 SKILL.md。</p>
      {!project && <p className="panel-note">請先到「專案」選擇資料夾，才能查看 Skills 與匯入 ZIP。</p>}
      {skillsError && <div className="notice error" role="alert">{skillsError}</div>}
      {catalog && (catalog.invalid_count > 0 || catalog.discovery_truncated) && <p className="panel-note">{catalog.discovery_truncated ? `Skill 探索已達上限；另有 ${catalog.invalid_count} 個無效套件未納入 catalog。` : `有 ${catalog.invalid_count} 個無效 Skill 套件未納入 catalog。`}</p>}
      {skillsLoading && !catalog ? <p className="panel-note">正在探索 Skills…</p>
        : project && items.length === 0 ? <p className="panel-note">目前沒有可用的 Skill。Project Skill 放在 .agents/skills/&lt;skill&gt;/SKILL.md。</p>
          : items.length > 0 && <ul className="skill-list">{items.map(skillRow)}</ul>}
      {importing && <form className="skill-import" onSubmit={(event) => { event.preventDefault(); void install(); }} aria-label="匯入 Skill">
        <h3>安裝 Managed Skill</h3><p className="panel-note">將 immutable Skill ZIP 安裝到本機 Runner store，接著啟用該版本。ZIP 必須位於目前專案資料夾內。</p>
        <label className="field">Skill key<input value={skillKey} onChange={(event) => setSkillKey(event.target.value)} placeholder="my-skill" /></label>
        <div className="skill-archive"><code>{archive || '尚未選擇 ZIP'}</code><button type="button" onClick={() => { void chooseArchive(); }}>選擇 ZIP…</button></div>
        {importError && <div className="notice error" role="alert">{importError}</div>}
        <div className="button-row"><button type="button" onClick={() => { setImporting(false); setImportError(null); }}>取消</button>
          <button className="primary" type="submit" disabled={!validSkillKey(skillKey) || !archive || installing}>安裝並啟用</button></div>
      </form>}
    </section>
    <section className="panel"><div className="panel-heading"><h2>外部 Skill 來源</h2>
      <div className="skill-actions">{externalLoading && <span className="spinner" aria-label="載入中" />}<button disabled={!canEdit} onClick={() => { void chooseFolder(); }}>選擇資料夾…</button></div></div>
      <p className="panel-note">連接你已經在 Codex、Claude Code 或共用 agent 使用的 Skill 資料夾。已連接的資料夾會即時沿用，修改原檔不需要重新匯入。</p>
      <p className="panel-note">匯入 ZIP 會複製到 Chadex 自己的儲存區，與原檔無關；連接則持續沿用原本的資料夾。</p>
      <p className="panel-note">腳本預設關閉。這是 run_skill_resource 的預設政策，不是沙盒：啟用 shell 工具時，ChatGPT 仍可執行指令。</p>
      {externalError && <div className="notice error" role="alert">{externalError}</div>}
      {!validSource && !externalLoading && <div className="skill-import-cta"><p>找不到可連接的外部 Skill 資料夾。可用「選擇資料夾…」手動連接，或直接匯入 Skill ZIP。</p>{importButton}</div>}
      {(sources.length > 0 || extras.length > 0) && <ul className="skill-list">
        {sources.map(sourceRow)}
        {extras.map(({ path, configured }) => <li className="skill-row" key={`extra|${path}`}>
          <div className="skill-title"><strong>{configured ? '自訂資料夾' : '連結的資料夾'}</strong><code title={path}>{displayPath(path)}</code>
            {configured && <button className="text-button" disabled={!canEdit} aria-label={`移除 ${displayPath(path)}`} onClick={() => { void setConnected(path, false); }}>移除</button>}</div>
          {toggles(path)}</li>)}
      </ul>}
    </section>
    <p className="panel-note">Skills 是流程，不是權限來源；Project Instructions 與 Chadex 的授權邊界仍然有效。</p>
  </>;
}
