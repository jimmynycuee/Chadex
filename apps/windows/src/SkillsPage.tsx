import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import type { DesktopApi } from './api';
import type { ExternalSkillRootsState, SkillDefinitionPreview, ExternalSkillSource, ExternalSkillSourceDiscovery, SkillCatalog, SkillInventory } from './contracts';
import { safeError } from './state';
import { filterSkills, SKILL_FILTERS, type SkillFilter } from './skillsFilter';
import {
  helperErrorCode, displayPath, externalSkillErrorMessage, extraRoots, hasValidSource, isProvidedElsewhere, isSkillActive, kindLabel, planRoots,
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
  const [removing, setRemoving] = useState<string | null>(null);
  const [importing, setImporting] = useState(false);
  const [skillKey, setSkillKey] = useState('');
  const [archive, setArchive] = useState('');
  const [installing, setInstalling] = useState(false);
  const [importError, setImportError] = useState<string | null>(null);
  const [expandedInvalid, setExpandedInvalid] = useState<Set<string>>(new Set());
  const [expandedSkills, setExpandedSkills] = useState<Set<string>>(new Set());
  const [catalogInvalidOpen, setCatalogInvalidOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState<SkillFilter>('all');
  const [definitions, setDefinitions] = useState<Record<string, SkillDefinitionPreview>>({});
  const [definitionLoading, setDefinitionLoading] = useState<Set<string>>(new Set());
  const [definitionErrors, setDefinitionErrors] = useState<Record<string, string>>({});
  const [remoteBlocked, setRemoteBlocked] = useState(false);
  const epoch = useRef(0);
  const loadingRef = useRef(new Set<string>());
  // The desktop has no topology field; a remote runtime is recognised by the helper's own refusal.
  const noteError = (error: unknown) => { if (helperErrorCode(error) === 'skill_management_requires_local_runtime') setRemoteBlocked(true); };

  const roots = rootsState?.roots ?? [];
  const scriptRoots = new Set(rootsState?.script_roots ?? []);
  const sources = discovery?.sources ?? [];
  const extras = extraRoots(discovery, roots);
  const busy = applying || externalLoading;
  const canEdit = helperReady && rootsState !== null && !busy && !remoteBlocked;
  const validSource = hasValidSource(discovery, roots);
  const items = skillCenterItems(catalog, inventory);
  const visible = filterSkills(items, filter, query);

  const loadSkills = useCallback(async () => {
    if (!project || !helperReady) { setCatalog(null); setInventory(null); setSkillsError(null); return; }
    const mine = epoch.current; setSkillsLoading(true);
    try {
      const nextCatalog = await api.getSkillCatalog(project);
      let nextInventory: SkillInventory | null = null; let warning: string | null = null;
      try { nextInventory = await api.getSkillInventory(project); } catch (error) { noteError(error); warning = externalSkillErrorMessage(error); }
      if (mine !== epoch.current) return;
      setCatalog(nextCatalog); setInventory(nextInventory); setSkillsError(warning); if (!warning) setRemoteBlocked(false);
    } catch (error) {
      if (mine !== epoch.current) return;
      noteError(error); setCatalog(null); setInventory(null); setSkillsError(externalSkillErrorMessage(error));
    } finally { if (mine === epoch.current) setSkillsLoading(false); }
  }, [api, project, helperReady]);

  const loadExternal = useCallback(async () => {
    if (!helperReady) return;
    const mine = epoch.current; setExternalLoading(true);
    let failure: string | null = null;
    try { const found = await api.discoverExternalSkillSources(); if (mine === epoch.current) setDiscovery(found); }
    catch (error) { noteError(error); if (mine === epoch.current) setDiscovery(null); failure = externalSkillErrorMessage(error); }
    try { const state = await api.getExternalSkillRoots(); if (mine === epoch.current) setRootsState(state); }
    catch (error) { noteError(error); if (mine === epoch.current) setRootsState(null); failure = externalSkillErrorMessage(error); }
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
      noteError(error); setExternalError(externalSkillErrorMessage(error));
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
    } catch (error) { noteError(error); setExternalError(externalSkillErrorMessage(error)); }
  }
  async function chooseArchive() {
    if (!project) return;
    try {
      const chosen = await api.chooseSkillArchive(project);
      if (chosen === null) return;
      setArchive(chosen); setImportError(null);
    } catch (error) { setArchive(''); setImportError(safeError(error)); }
  }
  async function install() {
    if (!project || installing || !validSkillKey(skillKey) || !archive) return;
    setInstalling(true); setImportError(null);
    try {
      await api.installSkill(project, skillKey.trim(), archive);
      setImporting(false); setSkillKey(''); setArchive('');
      await loadSkills();
    } catch (error) { noteError(error); setImportError(externalSkillErrorMessage(error)); await loadSkills(); }
    finally { setInstalling(false); }
  }
  async function loadDefinition(item: SkillCenterItem) {
    if (!project || !isSkillActive(item)) return;
    const have = definitions[item.skill_id];
    if (have && have.definition_revision === item.definition_revision && (have.package_revision ?? null) === (item.package_revision ?? null)) return;
    if (loadingRef.current.has(item.skill_id)) return;
    loadingRef.current.add(item.skill_id);
    setDefinitionLoading((current) => new Set(current).add(item.skill_id));
    setDefinitionErrors((current) => { const next = { ...current }; delete next[item.skill_id]; return next; });
    try {
      const preview = await api.getSkillDefinition(project, item.skill_id, item.definition_revision, item.package_revision);
      if (preview.definition_revision !== item.definition_revision || (preview.package_revision ?? null) !== (item.package_revision ?? null)) { await loadSkills(); return; }
      setDefinitions((current) => ({ ...current, [item.skill_id]: preview }));
    } catch (error) {
      noteError(error);
      setDefinitionErrors((current) => ({ ...current, [item.skill_id]: externalSkillErrorMessage(error) }));
      await loadSkills();
    } finally {
      loadingRef.current.delete(item.skill_id);
      setDefinitionLoading((current) => { const next = new Set(current); next.delete(item.skill_id); return next; });
    }
  }
  async function toggleManaged(item: SkillCenterItem) {
    if (!project || !item.managed || mutating) return;
    setMutating(item.skill_id);
    let failure: string | null = null;
    try {
      await (isSkillActive(item) ? api.deactivateSkill(project, item.managed) : api.activateSkill(project, item.managed));
      setSkillsError(null);
    } catch (error) { failure = externalSkillErrorMessage(error); noteError(error); }
    finally { setMutating(null); await loadSkills(); }
    // Refresh first: loadSkills resets the error, which would hide this failure.
    if (failure) setSkillsError(failure);
  }
  /** Removes every stored version of an installed Skill after the user confirmed in the inline dialog. */
  async function removeManaged(item: SkillCenterItem) {
    if (!project || !item.managed || mutating) return;
    setMutating(item.skill_id); setRemoving(null);
    let failure: string | null = null;
    try {
      await api.removeSkill(project, item.managed);
      setSkillsError(null);
      setDefinitions((current) => { const next = { ...current }; delete next[item.skill_id]; return next; });
    } catch (error) { failure = externalSkillErrorMessage(error); noteError(error); }
    finally { setMutating(null); await loadSkills(); }
    if (failure) setSkillsError(failure);
  }

  const toggleIn = (setter: typeof setExpandedSkills, key: string) => setter((current) => { const next = new Set(current); if (!next.delete(key)) next.add(key); return next; });
  const importButton = <button className={validSource ? '' : 'primary'} disabled={!project || !helperReady || installing || remoteBlocked} onClick={() => setImporting(true)}>匯入 Skill（ZIP）…</button>;
  const toggles = (path: string) => {
    const connected = roots.includes(path);
    return <div className="skill-toggles">
      <label><input type="checkbox" checked={connected} disabled={!canEdit} onChange={(event) => { void setConnected(path, event.target.checked); }} />連接</label>
      <label><input type="checkbox" checked={connected && scriptRoots.has(path)} disabled={!canEdit || !connected} onChange={(event) => { void setScripts(path, event.target.checked); }} />允許腳本</label>
    </div>;
  };
  function sourceCard(source: ExternalSkillSource) {
    const key = `${source.kind}|${source.path}`; const invalid = source.packages.filter((pkg) => pkg.state === 'invalid');
    return <li className="skill-card" key={key}>
      <div className="skill-title"><strong>{kindLabel(source.kind)}</strong></div>
      <code className="skill-path" title={source.path}>{displayPath(source.path)}</code>
      {isProvidedElsewhere(source) ? <p className="skill-meta">由 {providedByPaths(source, sources).join('、')} 提供{source.root_is_link ? '（此資料夾為連結）' : ''}</p>
        : source.status === 'available' && source.canonical_path ? <>
          <p className="skill-meta">{source.valid_count} 個 Skill · {source.script_count} 個含腳本{source.invalid_count > 0 ? ` · ${source.invalid_count} 個無效` : ''}{source.symlink_count > 0 ? ` · ${source.symlink_count} 個符號連結` : ''}</p>
          {source.root_is_link && <p className="skill-meta">此資料夾為連結，實際路徑：<code>{displayPath(source.canonical_path)}</code></p>}
          {!source.root_is_link && source.canonical_path !== source.path && <p className="skill-meta">實際路徑：<code>{displayPath(source.canonical_path)}</code></p>}
          {source.truncated && <p className="skill-meta">掃描已達上限，部分 Skill 可能未列出。</p>}
          {toggles(source.canonical_path)}
          {invalid.length > 0 && <div className="skill-invalid"><button className="text-button" aria-expanded={expandedInvalid.has(key)} onClick={() => toggleIn(setExpandedInvalid, key)}>{expandedInvalid.has(key) ? '收合' : '檢視'} {invalid.length} 個無效套件</button>
            {expandedInvalid.has(key) && <ul>{invalid.map((pkg) => <li key={pkg.package}><code>{pkg.package} — {pkg.invalid_reason ?? 'invalid'}</code></li>)}</ul>}</div>}
        </> : <p className="skill-meta">{statusText(source.status)}</p>}
    </li>;
  }
  const skillRow = (item: SkillCenterItem) => {
    const active = isSkillActive(item); const open = expandedSkills.has(item.skill_id);
    return <li className={`skill-item${open ? ' open' : ''}`} key={item.skill_id}>
      <div className="skill-line">
        <button className="skill-toggle" aria-expanded={open} aria-label={`${open ? '收合' : '展開'} ${item.name}`} onClick={() => { if (!open) void loadDefinition(item); toggleIn(setExpandedSkills, item.skill_id); }}>
          <span className="skill-caret" aria-hidden="true">{open ? '▾' : '▸'}</span>
          <strong className="skill-name">{item.name}</strong>
          <Chip>{sourceLabel(item)}</Chip><Chip>{active ? '已啟用' : '已停用'}</Chip>
          {item.name_conflict && <Chip title="有多個 Skill 解析成相同名稱；在衝突移除前，名稱比對會安全失敗。">名稱衝突</Chip>}
          {item.trust === 'operator_configured_guidance' && item.scripts_allowed === false && <Chip title="此 Skill 的腳本預設不允許執行。請在其資料夾開啟「允許腳本」。">腳本關閉</Chip>}
          {!open && <span className="skill-summary">{item.description}</span>}
        </button>
        {item.managed && <>
          <label className="skill-switch"><input type="checkbox" role="switch" checked={active} disabled={mutating !== null || !helperReady} aria-label={`啟用 ${item.name}`} onChange={() => { void toggleManaged(item); }} />啟用</label>
          <button className="danger" disabled={mutating !== null || !helperReady} aria-label={`移除 ${item.name}…`} title={`移除已安裝的 Skill ${item.name}`} onClick={() => setRemoving(item.skill_id)}>移除…</button>
        </>}
      </div>
      {item.managed && removing === item.skill_id && <div className="skill-remove-confirm" role="alertdialog" aria-label={`移除 ${item.name}`}>
        <p>要移除「{item.name}」嗎？這會刪除 Chadex 儲存的這個 Skill 的所有版本，且無法復原；使用同一個本機 runtime 的所有專案都會一起失去它。原本的 ZIP 檔不受影響；專案 Skill 與外部 Skill 資料夾也不會被更動。</p>
        <div className="button-row"><button autoFocus onClick={() => setRemoving(null)} onKeyDown={(event) => { if (event.key === 'Escape') setRemoving(null); }}>取消</button>
          <button className="danger" disabled={mutating !== null || !helperReady} onClick={() => { void removeManaged(item); }}>移除</button></div>
      </div>}
      {open && <div className="skill-detail">
        <p>{item.description}</p>
        <p className="skill-meta">來源：{sourceLabel(item)}（{item.source_scope}）· ID <code>{item.skill_id}</code></p>
        <small className="skill-rev" title={item.definition_revision}>{item.definition_revision.slice(0, 12)}</small>
        {!active ? <p className="skill-meta">啟用後才能預覽 SKILL.md。</p>
          : definitionLoading.has(item.skill_id) ? <p className="skill-meta" role="status">正在載入 SKILL.md…</p>
            : definitionErrors[item.skill_id] ? <p className="skill-meta" role="alert">無法載入 SKILL.md：{definitionErrors[item.skill_id]}</p>
              : definitions[item.skill_id] && <><pre className="skill-definition" aria-label={`${item.name} SKILL.md`}>{definitions[item.skill_id].text}</pre>
                {definitions[item.skill_id].has_more && <p className="skill-meta">內容過長，僅顯示前段。</p>}</>}
      </div>}
    </li>;
  };
  const invalidWarning = catalog && (catalog.invalid_count > 0 || catalog.discovery_truncated)
    ? (catalog.discovery_truncated ? `Skill 探索已達上限；另有 ${catalog.invalid_count} 個無效套件未納入 catalog。` : `有 ${catalog.invalid_count} 個無效 Skill 套件未納入 catalog。`) : null;

  return <div className="skills-page">
    {/* The page heading already says "Skills"; this row only carries the explanation and refresh. */}
    <div className="skills-header"><p className="panel-note">目前專案可用的重複使用流程。Chadex 只探索與比對 metadata，被選中使用時才讀取 SKILL.md。Skills 是流程，不是權限來源；Project Instructions 與 Chadex 的授權邊界仍然有效。</p>
      <div className="skill-actions"><button className="text-button" disabled={skillsLoading || !helperReady} onClick={() => { void loadSkills(); }}>重新整理</button></div></div>
    {!project && <p className="panel-note">請先到「專案」選擇資料夾，才能查看 Skills 與匯入 ZIP。</p>}

    <section className="panel"><div className="panel-heading"><h2>外部 Skill 來源</h2>
      <div className="skill-actions">{externalLoading && <span className="spinner" aria-label="載入中" />}<button disabled={!canEdit} onClick={() => { void chooseFolder(); }}>選擇資料夾…</button>{importButton}</div></div>
      <p className="panel-note">連接你已經在 Codex、Claude Code 或共用 agent 使用的 Skill 資料夾，修改原檔不需要重新匯入。匯入 ZIP 則會複製到 Chadex 自己的儲存區。腳本預設關閉，需逐一開啟；這是 run_skill_resource 的預設政策，不是沙盒：啟用 shell 工具時，ChatGPT 仍可執行指令。</p>
      {remoteBlocked && <div className="notice error" role="status">遠端 runtime 不支援匯入或管理 Skill，請在 runtime 所在的機器上操作。</div>}
      {externalError && !remoteBlocked && <div className="notice error" role="alert">{externalError}</div>}
      {!validSource && !externalLoading && <div className="skill-import-cta"><p>找不到可連接的外部 Skill 資料夾。可用「選擇資料夾…」手動連接，或直接匯入 Skill ZIP。</p></div>}
      {(sources.length > 0 || extras.length > 0) && <ul className="skill-cards">
        {sources.map(sourceCard)}
        {extras.map(({ path, configured }) => <li className="skill-card" key={`extra|${path}`}>
          <div className="skill-title"><strong>{configured ? '自訂資料夾' : '連結的資料夾'}</strong>
            {configured && <button className="text-button" disabled={!canEdit} aria-label={`移除 ${displayPath(path)}`} onClick={() => { void setConnected(path, false); }}>移除</button>}</div>
          <code className="skill-path" title={path}>{displayPath(path)}</code>
          {toggles(path)}</li>)}
      </ul>}
      {importing && <form className="skill-import" onSubmit={(event) => { event.preventDefault(); void install(); }} aria-label="匯入 Skill">
        <h3>安裝 Managed Skill</h3><p className="panel-note">將 immutable Skill ZIP 安裝到本機 Runner store，接著啟用該版本。可選擇任何位置的 ZIP，安裝時會暫時複製到專案的 .chadex/skill-imports/。</p>
        <label className="field">Skill key<input value={skillKey} onChange={(event) => setSkillKey(event.target.value)} placeholder="my-skill" /></label>
        <div className="skill-archive"><code>{archive || '尚未選擇 ZIP'}</code><button type="button" onClick={() => { void chooseArchive(); }}>選擇 ZIP…</button></div>
        {importError && <div className="notice error" role="alert">{importError}</div>}
        <div className="button-row"><button type="button" onClick={() => { setImporting(false); setImportError(null); }}>取消</button>
          <button className="primary" type="submit" disabled={!validSkillKey(skillKey) || !archive || installing}>安裝並啟用</button></div>
      </form>}
    </section>

    <section className="panel"><div className="panel-heading"><h2>Skill 列表{items.length > 0 ? `（${visible.length}/${items.length}）` : ''}</h2></div>
      {skillsError && !remoteBlocked && <div className="notice error" role="alert">{skillsError}</div>}
      {invalidWarning && <div className="skill-warning"><button className="text-button" aria-expanded={catalogInvalidOpen} onClick={() => setCatalogInvalidOpen((open) => !open)}>⚠ 有 {catalog?.invalid_count ?? 0} 個無效套件 {catalogInvalidOpen ? '▾' : '▸'}</button>
        {catalogInvalidOpen && <p className="skill-meta">{invalidWarning}</p>}</div>}
      {items.length > 0 && <div className="skill-filterbar">
        <input type="search" aria-label="搜尋 Skill" placeholder="搜尋名稱或描述…" value={query} onChange={(event) => setQuery(event.target.value)} />
        <div className="skill-segments" role="group" aria-label="篩選">{SKILL_FILTERS.map((entry) => <button key={entry.id} className={filter === entry.id ? 'selected' : ''} aria-pressed={filter === entry.id} onClick={() => setFilter(entry.id)}>{entry.label}</button>)}</div>
      </div>}
      {skillsLoading && !catalog ? <p className="panel-note">正在探索 Skills…</p>
        : project && items.length === 0 ? <p className="panel-note">目前沒有可用的 Skill。Project Skill 放在 .agents/skills/&lt;skill&gt;/SKILL.md。</p>
          : items.length > 0 && (visible.length === 0 ? <p className="panel-note">沒有符合的 Skill。</p> : <ul className="skill-list">{visible.map(skillRow)}</ul>)}
    </section>
  </div>;
}
