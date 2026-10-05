import { useEffect, useRef, useState, useSyncExternalStore, type ReactNode } from 'react';
import { desktopApi, type DesktopApi } from './api';
import { defaultPreferences, type Preferences, type ProjectInspection, type RuntimeMethod } from './contracts';
import { DesktopStore, connectionStatus, localStatus, observedRuntime, safeError } from './state';
import { Ferret } from './FerretCompanion';
import { JobActivity, TraceActivity } from './RuntimeActivity';
import { SkillsPage } from './SkillsPage';

type Page = 'home' | 'projects' | 'project' | 'skills' | 'connection' | 'activity' | 'settings' | 'diagnostics' | 'updates';
const titles: Record<Page, string> = { home: '總覽', projects: '專案', project: '專案詳情', skills: 'Skills', connection: '連線', activity: '活動紀錄', settings: '設定', diagnostics: '診斷', updates: '版本與更新' };
const nav: { page: Page; glyph: string; name: string }[] = [
  { page: 'home', glyph: '◫', name: '總覽' }, { page: 'projects', glyph: '▱', name: '專案' }, { page: 'skills', glyph: '✦', name: 'Skills' },
  { page: 'connection', glyph: '↗', name: '連線' }, { page: 'activity', glyph: '≡', name: '活動紀錄' },
  { page: 'settings', glyph: '⚙', name: '設定' },
];
const pageNotes: Record<Page, string> = {
  home: '你的專案與連線，在這裡掌握。', projects: '選擇資料夾，讓工作留在你的電腦。', project: '檢視工作範圍與專案狀態。', skills: '管理 Skills、外部來源與 ZIP 匯入。',
  connection: '把目前的專案連接到 ChatGPT。', activity: '由本地服務回報的實際活動。', settings: '調整桌面體驗與啟動偏好。',
  diagnostics: '檢視本地服務與連線的觀測資訊。', updates: '目前安裝的版本與更新狀態。',
};
const GLOBAL_INSTRUCTIONS_MAX_BYTES = 8 * 1024;
const utf8Bytes = (value: string) => new TextEncoder().encode(value).length;
function basename(path: string) { return path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || path; }
function date(time: number | null | undefined) { return time == null ? '尚無紀錄' : new Date(time).toLocaleString('zh-TW', { hour12: false }); }
function Badge({ children, tone = 'quiet' }: { children: ReactNode; tone?: 'good' | 'quiet' | 'warn' }) {
  return <span className={`badge ${tone}`}><span className={`dot ${tone}`} />{children}</span>;
}
function Panel({ title, children, action, className = '' }: { title: string; children: ReactNode; action?: ReactNode; className?: string }) {
  return <section className={`panel ${className}`}><div className="panel-heading"><h2>{title}</h2>{action}</div>{children}</section>;
}
function Empty({ title, children }: { title: string; children: ReactNode }) {
  return <div className="empty"><span className="empty-glyph" aria-hidden="true">▱</span><h2>{title}</h2><p>{children}</p></div>;
}
function Facts({ items }: { items: [string, ReactNode][] }) {
  return <dl className="facts">{items.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl>;
}
const defaultStore = new DesktopStore(desktopApi);
export function App({ api = desktopApi, store = defaultStore }: { api?: DesktopApi; store?: DesktopStore }) {
  const view = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const [page, setPage] = useState<Page>('home');
  const [busy, setBusy] = useState<string | null>(null);
  const actionGate = useRef(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [inspection, setInspection] = useState<ProjectInspection | null>(null);
  const [activityLevel, setActivityLevel] = useState('all');
  const [activitySearch, setActivitySearch] = useState('');
  const [credential, setCredential] = useState('');
  const [tunnelId, setTunnelId] = useState('');
  const [proxyMode, setProxyMode] = useState<'auto' | 'direct' | 'custom'>('auto');
  const [proxyUrl, setProxyUrl] = useState('');
  const [prefsDraft, setPrefsDraft] = useState<Preferences | null>(null);
  const [globalInstructions, setGlobalInstructions] = useState('');
  const [savedGlobalInstructions, setSavedGlobalInstructions] = useState('');
  const [globalInstructionsLoading, setGlobalInstructionsLoading] = useState(true);
  const [globalInstructionsSaving, setGlobalInstructionsSaving] = useState(false);
  const [globalInstructionsError, setGlobalInstructionsError] = useState<string | null>(null);
  const prefs = view.desktop?.preferences ?? defaultPreferences;
  const snapshot = observedRuntime(view);
  const project = snapshot?.selected_project ?? null;
  const detail = inspection ?? project;
  const connection = connectionStatus(snapshot);
  const local = localStatus(snapshot);
  const helperRunning = view.freshness === 'fresh' && view.desktop?.helper.state === 'running';
  const editable = !busy && helperRunning && Boolean(snapshot);
  const prefsEditable = !busy && view.freshness === 'fresh';
  const activities = snapshot ? view.desktop?.activity ?? [] : [];
  const traces = snapshot ? view.desktop?.traces ?? [] : [];
  const jobs = snapshot?.mascot_jobs ?? [];
  const filteredActivities = activities.filter((entry) => (activityLevel === 'all' || entry.level === activityLevel)
    && `${entry.message} ${entry.source} ${entry.event_kind}`.toLocaleLowerCase().includes(activitySearch.toLocaleLowerCase())).slice().sort((a, b) => b.sequence - a.sequence);

  useEffect(store.retain, [store]);
  useEffect(() => { document.documentElement.dataset.theme = prefs.theme; }, [prefs.theme]);
  useEffect(() => { setTunnelId(prefs.tunnel_id); }, [prefs.tunnel_id]);
  useEffect(() => { if (page !== 'connection') setCredential(''); }, [page]);
  useEffect(() => {
    let active = true;
    setGlobalInstructionsLoading(true);
    void api.getGlobalInstructions().then((value) => {
      if (!active) return;
      setGlobalInstructions(value); setSavedGlobalInstructions(value); setGlobalInstructionsError(null);
    }).catch((error) => { if (active) setGlobalInstructionsError(safeError(error)); })
      .finally(() => { if (active) setGlobalInstructionsLoading(false); });
    return () => { active = false; };
  }, [api]);

  async function run(label: string, work: () => Promise<void>, affectsRuntime = true) {
    if (actionGate.current) return;
    actionGate.current = true; setBusy(label); setActionError(null);
    if (affectsRuntime) store.beginAction();
    try { await work(); }
    catch (error) { setActionError(safeError(error)); }
    finally {
      if (affectsRuntime) { store.endAction(); await store.refreshAfterAction(); }
      actionGate.current = false; setBusy(null);
    }
  }
  function runtime(method: RuntimeMethod, label: string, params: Record<string, unknown> = {}) {
    return run(label, async () => { await api.runtimeAction(method, params); });
  }
  async function saveGlobalInstructions() {
    if (globalInstructionsSaving || utf8Bytes(globalInstructions) > GLOBAL_INSTRUCTIONS_MAX_BYTES) return;
    setGlobalInstructionsSaving(true); setGlobalInstructionsError(null);
    try {
      const saved = await api.saveGlobalInstructions(globalInstructions);
      setGlobalInstructions(saved); setSavedGlobalInstructions(saved);
    } catch (error) { setGlobalInstructionsError(safeError(error)); }
    finally { setGlobalInstructionsSaving(false); }
  }
  async function chooseProject() {
    await run('選擇專案', async () => {
      const path = await api.chooseProject();
      if (path === null) return;
      setInspection(await api.inspectProject(path)); setPage('project');
    }, false);
  }
  function inspect(path: string) {
    void run('檢查專案', async () => { setInspection(await api.inspectProject(path)); setPage('project'); }, false);
  }
  function activate(path: string) {
    void run('切換專案', async () => {
      await api.runtimeAction(project ? 'switchLocalProject' : 'activateProject', { path });
      setInspection(null);
    });
  }
  async function saveCredential(event: React.FormEvent) {
    event.preventDefault();
    if (!credential.trim() || !tunnelId.trim()) return;
    const secret = credential; setCredential('');
    await run('安全儲存憑證', async () => {
      try {
        const latest = store.getSnapshot().desktop?.preferences ?? prefs;
        await api.savePreferences({ ...latest, tunnel_id: tunnelId.trim() });
        await api.storeCredential(secret);
      } catch { throw new Error('憑證儲存失敗，請確認 Tunnel ID 與系統安全儲存區，再重新輸入。'); }
    });
  }
  const statusRows: [string, ReactNode][] = [
    ['Helper', <Badge tone={helperRunning ? 'good' : 'warn'}>{helperRunning ? '執行中' : view.freshness === 'loading' ? '取得狀態中' : '無法確認'}</Badge>],
    ['本地服務', <Badge tone={local.ready ? 'good' : 'quiet'}>{local.label}</Badge>],
    ['Tunnel', <Badge tone={snapshot?.tunnel_ready ? 'good' : 'quiet'}>{snapshot ? snapshot.tunnel_ready ? '可用' : '尚未可用' : '狀態未確認'}</Badge>],
    ['ChatGPT', <Badge tone={connection.tone}>{connection.label}</Badge>],
  ];
  return <div className="app-shell">
    <aside className="sidebar" aria-label="主要導覽">
      <a className="brand" href="#home" onClick={(event) => { event.preventDefault(); setPage('home'); }}><span className="brand-mark" aria-hidden="true">C<span>·</span></span><span>Chadex<small>你的本地工作夥伴</small></span></a>
      <div className="sidebar-label">工作空間</div>
      <nav>{nav.map((item) => <button key={item.page} className={`nav-item ${page === item.page || (page === 'project' && item.page === 'projects') ? 'active' : ''}`}
        aria-current={page === item.page || (page === 'project' && item.page === 'projects') ? 'page' : undefined} onClick={() => setPage(item.page)}><span aria-hidden="true">{item.glyph}</span>{item.name}</button>)}</nav>
      <div className="sidebar-project"><small>目前專案</small><strong>{project ? basename(project.path) : '尚未選擇'}</strong><span title={project?.path}>{project?.path ?? '從「專案」選擇資料夾'}</span></div>
      <div className="sidebar-spacer" />
      <Ferret snapshot={snapshot} activities={activities} visible={prefs.ferret_visible} motion={prefs.ferret_motion} unavailable={view.freshness === 'unavailable' || view.desktop?.helper.state === 'failed'} />
      <button className={`nav-item ${page === 'diagnostics' ? 'active' : ''}`} onClick={() => setPage('diagnostics')}><span aria-hidden="true">⊙</span>診斷</button>
      <button className={`version-link ${page === 'updates' ? 'active' : ''}`} onClick={() => setPage('updates')}>Windows Desktop <span>{view.desktop ? `v${view.desktop.version}` : '版本未確認'} ↗</span></button>
    </aside>
    <div className="workspace">
      <header className="topbar"><span>本地工作空間 <span className="crumb">/ {titles[page]}</span></span><div><Badge tone={connection.tone}>{!snapshot ? '連線狀態未確認' : connection.verified ? 'ChatGPT 已驗證' : !local.ready ? local.label : snapshot.tunnel_ready ? '等待驗證' : '尚未連線'}</Badge><button className="icon-button" title="重新整理狀態" aria-label="重新整理狀態" disabled={Boolean(busy)} onClick={() => { void store.refresh(); }}>↻</button></div></header>
      <main id="main"><div className="page-heading"><div><div className="eyebrow">CHADEX / {page === 'project' ? 'PROJECT' : page.toUpperCase()}</div><h1>{titles[page]}</h1><p>{pageNotes[page]}</p></div>{page === 'projects' && <button className="primary" disabled={!editable} onClick={() => { void chooseProject(); }}>＋ 選擇資料夾</button>}</div>
        {view.error && <div className="notice error" role="alert"><strong>無法取得目前狀態</strong><p>{view.error}</p><button disabled={Boolean(busy)} onClick={() => { void store.refresh(); }}>重試</button></div>}
        {view.desktop && view.desktop.helper.state !== 'running' && <div className="notice error" role="alert"><strong>Helper {view.desktop.helper.state === 'failed' ? '異常結束' : view.desktop.helper.state === 'stopping' ? '正在停止' : '已停止'}</strong><p>{safeError(view.desktop.helper.error ?? '本地服務暫時無法使用。')}</p><button disabled={Boolean(busy) || view.desktop.helper.state === 'stopping'} onClick={() => { void run('重新啟動 Helper', api.restartHelper); }}>重新啟動 Helper</button></div>}
        {snapshot?.error && <div className="notice error" role="alert"><strong>{safeError(snapshot.error.message)}</strong><p>{snapshot.error.recovery && safeError(snapshot.error.recovery)}</p></div>}
        {actionError && <div className="notice error" role="alert"><p>{actionError}</p><button onClick={() => setActionError(null)}>關閉</button></div>}
        {busy && <div className="notice busy" role="status"><span className="spinner" />{busy}…</div>}

        {page === 'home' && <>
          <section className="overview-hero"><div><span className="eyebrow">CURRENT WORKSPACE</span><h2>{project ? basename(project.path) : '從一個專案開始。'}</h2><p>{project ? project.path : '選擇你的本地資料夾，準備服務，再連接 ChatGPT。'}</p><div className="button-row"><button className="primary" disabled={!editable} onClick={() => project ? (setInspection(null), setPage('project')) : void chooseProject()}>{project ? '檢視專案' : '選擇專案'} <span aria-hidden="true">↗</span></button><button onClick={() => setPage('connection')}>連線設定</button></div></div><div className="hero-index" aria-hidden="true">LOCAL<span>01</span></div></section>
          <div className="two-column"><Panel title="服務狀態" action={<button className="text-button" onClick={() => setPage('diagnostics')}>檢視診斷 ↗</button>}><Facts items={statusRows} />{snapshot?.runtime_status?.summary && <p className="panel-note">{safeError(snapshot.runtime_status.summary)}</p>}</Panel>
          <Panel title="接著做"><ol className="steps"><li className={project ? 'done' : ''}><span>01</span><div><strong>選擇專案</strong><p>{project ? '目前工作範圍已指定。' : '指定 ChatGPT 可操作的本地範圍。'}</p></div></li><li className={local.ready ? 'done' : ''}><span>02</span><div><strong>準備本地服務</strong><p>{local.label}</p></div></li><li className={connection.verified ? 'done' : ''}><span>03</span><div><strong>連接 ChatGPT</strong><p>{connection.label}</p></div></li></ol></Panel></div>
          <Panel title="最近活動" action={<button className="text-button" onClick={() => setPage('activity')}>全部紀錄 ↗</button>}>{activities.length ? <ul className="activity-list">{activities.slice().sort((a, b) => b.sequence - a.sequence).slice(0, 4).map((entry) => <li key={entry.sequence}><span className={`dot ${entry.level === 'error' ? 'warn' : 'quiet'}`} /><div><p>{safeError(entry.message)}</p><small>{entry.source} · {date(entry.timestamp_ms)}</small></div></li>)}</ul> : <p className="panel-note">{snapshot ? '尚無活動紀錄。' : '等待取得最新狀態。'}</p>}</Panel>
        </>}

        {page === 'projects' && <Panel title="本地專案"><p className="panel-note">資料夾的實際存取權限由本地服務檢查。</p>{prefs.recent_projects.length ? <div className="project-list">{prefs.recent_projects.map((path) => <button key={path} className="project-row" disabled={!editable} onClick={() => inspect(path)}><span className="folder-icon" aria-hidden="true">▱</span><span><strong>{basename(path)}</strong><small>{path}</small></span>{path === project?.path && <Badge tone="good">目前專案</Badge>}<span aria-hidden="true">→</span></button>)}</div> : <Empty title="還沒有專案">使用右上角「選擇資料夾」加入本地專案。</Empty>}</Panel>}

        {page === 'project' && (detail ? <>
          <Panel title={basename(detail.path)} action={<Badge tone={detail.path === project?.path ? 'good' : 'quiet'}>{detail.path === project?.path ? '目前專案' : '檢查結果'}</Badge>}><p className="path-block">{detail.path}</p><Facts items={[
            ['允許操作範圍', <code>{detail.allowed_root}</code>], ['Git 儲存庫', detail.is_git_repository ? '是' : '否'],
            ['讀取權限', detail.readable ? '可讀取' : '不可讀取'], ['寫入權限', detail.writable ? '可寫入' : '不可寫入'],
          ]} /><div className="button-row"><button className="primary" disabled={!editable || !detail.readable || !detail.writable || detail.path === project?.path} onClick={() => activate(detail.path)}>設為目前專案</button><button disabled={!editable || detail.path !== project?.path} onClick={() => { void run('開啟檔案總管', () => api.openProject(detail.path), false); }}>在檔案總管開啟 ↗</button><button onClick={() => setPage('connection')}>連線設定</button></div></Panel>
          {detail.path === project?.path && <Panel title="本地服務"><Facts items={statusRows.slice(1)} /><div className="button-row"><button disabled={!editable} onClick={() => { void runtime(snapshot?.runtime_status?.runtime_configured ? 'resumeService' : 'configureLocalSetup', '準備本地服務'); }}>準備／恢復服務</button><button className="danger" disabled={!editable} onClick={() => { void runtime('stopLocalService', '停止本地服務'); }}>停止本地服務</button></div></Panel>}
        </> : <Empty title="尚未選擇專案">到「專案」選擇資料夾，查看實際檢查結果。</Empty>)}

        {page === 'skills' && <SkillsPage api={api} project={project?.path ?? null} helperReady={helperRunning} />}

        {page === 'connection' && <>
          <Panel title="ChatGPT 連線" action={<Badge tone={connection.tone}>{connection.label}</Badge>}><Facts items={[
            ['目前專案', project?.path ?? '尚未選擇'], ['本地服務', local.label], ['Tunnel', snapshot?.tunnel_ready ? '可用' : '尚未確認可用'],
            ['連線階段', snapshot?.phase ?? '狀態未確認'], ['最近專案驗證', date(snapshot?.last_verified_at_ms)],
          ]} /><p className="panel-note">Tunnel 可用後，仍需由 ChatGPT 對目前專案進行實際驗證。切換專案後會重新確認。</p><div className="button-row"><button className="primary" disabled={!editable || !project || !view.desktop?.credential_stored || !prefs.tunnel_id || Boolean(snapshot?.current_operation)} onClick={() => { void runtime('connectChatGPT', '連接 ChatGPT'); }}>連接 ChatGPT</button><button disabled={!editable || !project || !view.desktop?.credential_stored || !prefs.tunnel_id} onClick={() => { void runtime('startTunnel', '啟動 Tunnel'); }}>啟動 Tunnel</button><button disabled={!editable || !project} onClick={() => { void runtime('observeChatGPTActivity', '檢查 ChatGPT 驗證'); }}>重新檢查驗證</button><button className="danger" disabled={!editable} onClick={() => { void runtime('disconnectAI', '中斷 AI 連線'); }}>中斷連線</button></div></Panel>
          <div className="two-column"><Panel title="連線憑證" action={<Badge tone={view.freshness === 'fresh' && view.desktop?.credential_stored ? 'good' : 'quiet'}>{view.freshness === 'fresh' && view.desktop?.credential_stored ? '安全儲存區已有憑證' : '尚未確認憑證'}</Badge>}><form onSubmit={(event) => { void saveCredential(event); }}><label className="field">Tunnel ID<input value={tunnelId} disabled={!prefsEditable} onChange={(event) => setTunnelId(event.target.value)} autoComplete="off" spellCheck={false} placeholder="輸入 Tunnel ID" /></label><label className="field">API credential<input type="password" name="credential" value={credential} disabled={!prefsEditable} onChange={(event) => setCredential(event.target.value)} autoComplete="new-password" spellCheck={false} placeholder="輸入後交由系統安全儲存" /></label><p className="panel-note">憑證只交由系統安全儲存，不顯示已儲存的內容。</p><div className="button-row"><button type="submit" disabled={!prefsEditable || !credential.trim() || !tunnelId.trim()}>安全儲存</button><button className="danger" type="button" disabled={!prefsEditable || !view.desktop?.credential_stored} onClick={() => { setCredential(''); void run('移除憑證', api.forgetCredential); }}>移除憑證</button></div></form></Panel>
          <Panel title="網路代理"><form onSubmit={(event) => { event.preventDefault(); void runtime('updateProxySettings', '套用代理設定', { mode: proxyMode, custom_url: proxyMode === 'custom' ? proxyUrl.trim() : null }); }}><label className="field">代理模式<select value={proxyMode} disabled={!editable} onChange={(event) => setProxyMode(event.target.value as typeof proxyMode)}><option value="auto">自動</option><option value="direct">直接連線</option><option value="custom">自訂代理</option></select></label>{proxyMode === 'custom' && <label className="field">代理 URL<input type="url" value={proxyUrl} disabled={!editable} onChange={(event) => setProxyUrl(event.target.value)} placeholder="http://127.0.0.1:8080" required /></label>}<p className="panel-note">套用選擇的代理模式。套用後由服務重新回報連線狀態。</p><button type="submit" disabled={!editable}>套用代理設定</button></form></Panel></div>
        </>}

        {page === 'activity' && <><Panel title="本地 Job"><JobActivity jobs={jobs} available={Boolean(snapshot)} /></Panel><Panel title="工具／命令呼叫"><TraceActivity traces={traces} available={Boolean(snapshot)} /><p className="panel-note">顯示服務實際觀測的工具名稱、完成狀態與耗時。</p></Panel><Panel title="服務活動"><div className="filter-row"><label className="field">等級<select value={activityLevel} onChange={(event) => setActivityLevel(event.target.value)}><option value="all">全部等級</option><option value="info">資訊</option><option value="warning">警告</option><option value="error">錯誤</option></select></label><label className="field grow">搜尋紀錄<input type="search" value={activitySearch} onChange={(event) => setActivitySearch(event.target.value)} placeholder="搜尋訊息、來源或事件" /></label></div>{filteredActivities.length ? <ul className="activity-list">{filteredActivities.map((entry) => <li key={entry.sequence}><Badge tone={entry.level === 'error' || entry.level === 'warning' ? 'warn' : 'quiet'}>{entry.level}</Badge><div><p>{safeError(entry.message)}</p><small>{entry.source} · {entry.event_kind} · #{entry.sequence} · {date(entry.timestamp_ms)}</small></div></li>)}</ul> : <Empty title={snapshot ? '沒有符合的紀錄' : '活動狀態未確認'}>{snapshot ? '試著調整搜尋條件，或等待服務回報新活動。' : '重新取得 helper 狀態後會顯示活動紀錄。'}</Empty>}</Panel></>}

        {page === 'settings' && <>
          <Panel title="Chadex Global Instructions" action={<Badge tone={globalInstructionsError ? 'warn' : 'quiet'}>{globalInstructionsLoading ? '載入中' : globalInstructions === savedGlobalInstructions ? '已儲存' : '尚未儲存'}</Badge>}>
            <p className="panel-note">所有 Chadex 專案共用的可編輯行為偏好。內容由 Chadex 儲存在 App 資料中，不會讀取或修改 repository 或父資料夾的 AGENTS.md。</p>
            <label className="field">Global Instructions
              <textarea aria-label="Chadex Global Instructions" className="global-instructions-editor" value={globalInstructions} disabled={globalInstructionsLoading || globalInstructionsSaving} onChange={(event) => setGlobalInstructions(event.target.value)} placeholder="可以留白；Chadex 內建的安全與行為 baseline 仍會正常運作。" />
            </label>
            <p className="panel-note">行為偏好優先序：目前要求 -&gt; nested／repository AGENTS.md -&gt; Global Instructions -&gt; built-in baseline。System／platform safety、authority、approval 與 sensitive-content 保護永遠不可被覆寫。</p>
            {globalInstructionsError && <div className="notice error">{globalInstructionsError}</div>}
            <div className="button-row"><button className="primary" disabled={globalInstructionsLoading || globalInstructionsSaving || globalInstructions === savedGlobalInstructions || utf8Bytes(globalInstructions) > GLOBAL_INSTRUCTIONS_MAX_BYTES} onClick={() => { void saveGlobalInstructions(); }}>{globalInstructionsSaving ? '儲存中…' : '儲存 Global Instructions'}</button><code>{utf8Bytes(globalInstructions)} / {GLOBAL_INSTRUCTIONS_MAX_BYTES} bytes</code></div>
          </Panel>
          <Panel title="桌面偏好"><form onSubmit={(event) => { event.preventDefault(); if (prefsDraft) void run('儲存偏好', async () => { const latest = store.getSnapshot().desktop?.preferences ?? prefs; await api.savePreferences({ ...latest, restore_project: prefsDraft.restore_project, launch_at_login: prefsDraft.launch_at_login, notifications: prefsDraft.notifications, ferret_visible: prefsDraft.ferret_visible, ferret_motion: prefsDraft.ferret_motion, theme: prefsDraft.theme }); setPrefsDraft(null); }); }}>
            {([['restore_project', '啟動時恢復上次專案', '回到上一次使用的本地工作範圍。'], ['launch_at_login', '登入時啟動 Chadex', '由桌面應用程式管理系統登入設定。'], ['notifications', '桌面通知', '允許桌面應用程式顯示服務通知。'], ['ferret_visible', '顯示 Code Ferret', '在側欄呈現實際任務狀態。'], ['ferret_motion', 'Code Ferret 動畫', '降低動態效果時也會自動停用。']] as const).map(([key, label, hint]) => <label className="setting-row" key={key}><span><strong>{label}</strong><small>{hint}</small></span><input type="checkbox" disabled={!prefsEditable} checked={(prefsDraft ?? prefs)[key]} onChange={(event) => setPrefsDraft({ ...(prefsDraft ?? prefs), [key]: event.target.checked })} /></label>)}
            <label className="setting-row"><span><strong>外觀</strong><small>選擇淺色、深色或跟隨系統。</small></span><select disabled={!prefsEditable} value={(prefsDraft ?? prefs).theme} onChange={(event) => setPrefsDraft({ ...(prefsDraft ?? prefs), theme: event.target.value as Preferences['theme'] })}><option value="system">跟隨系統</option><option value="light">淺色</option><option value="dark">深色</option></select></label><div className="button-row"><button className="primary" type="submit" disabled={!prefsEditable || !prefsDraft}>儲存偏好</button><button type="button" disabled={!prefsDraft || Boolean(busy)} onClick={() => setPrefsDraft(null)}>取消變更</button></div></form></Panel>
          <Panel title="應用程式"><div className="button-row"><button onClick={() => setPage('updates')}>版本與更新 ↗</button><button onClick={() => setPage('diagnostics')}>診斷資訊 ↗</button><button disabled={Boolean(busy)} onClick={() => { void run('結束 Chadex', api.quitApp, false); }}>結束 Chadex</button></div></Panel>
        </>}

        {page === 'diagnostics' && <>
          <Panel title="觀測狀態"><Facts items={[
            ['Snapshot', view.freshness === 'fresh' ? '已取得' : view.freshness === 'loading' ? '取得中' : '不可用'], ['收到時間', date(view.receivedAt)],
            ['啟動錯誤', view.desktop?.startup_error ? safeError(view.desktop.startup_error) : '無'], ['憑證儲存區錯誤', view.desktop?.credential_error ? safeError(view.desktop.credential_error) : '無'],
            ['Runtime version', view.desktop?.runtime_version ?? '未觀測'],
            ['Helper state', view.freshness === 'fresh' ? view.desktop?.helper.state ?? '未知' : '未知'], ['Helper PID', view.freshness === 'fresh' ? view.desktop?.helper.pid ?? '無' : '未知'],
            ['Phase', snapshot?.phase ?? '未知'], ['State revision', snapshot?.state_revision ?? '未知'], ['Activity sequence', snapshot?.activity_sequence ?? '未知'],
            ['Graphify', snapshot?.graphify ? snapshot.graphify.available ? '可用' : '未安裝／不可用' : '未知'],
            ['Tunnel configured', snapshot?.tunnel_status ? snapshot.tunnel_status.configured ? '是' : '否' : '未知'], ['Tunnel state', snapshot?.tunnel_status?.state ?? '未知'],
          ]} /><div className="button-row"><button disabled={!editable} onClick={() => { void runtime('refreshRuntime', '重新檢查 Runtime'); }}>重新檢查 Runtime</button><button disabled={Boolean(busy)} onClick={() => { void run('重新啟動 Helper', api.restartHelper); }}>重新啟動 Helper</button></div></Panel>
          {snapshot?.runtime_status && <Panel title="Runtime readiness"><Facts items={Object.entries(snapshot.runtime_status).map(([key, value]) => [key, typeof value === 'boolean' ? value ? 'true' : 'false' : safeError(value ?? '—')] as [string, ReactNode])} /></Panel>}
          <Panel title="應用程式路徑"><Facts items={Object.entries(view.desktop?.paths ?? { helper: '未知', runtime: '未知', data: '未知' }).map(([key, value]) => [key, <code>{value}</code>])} /></Panel>
        </>}

        {page === 'updates' && <Panel title="版本與更新"><div className="version-display"><div className="brand-mark" aria-hidden="true">C<span>·</span></div><div><h2>Chadex for Windows</h2><p>{view.desktop ? `已安裝版本 ${view.desktop.version}` : '尚未取得版本資訊'}</p></div></div><Facts items={[
          ['更新狀態', '尚未提供更新資訊'], ['更新來源', '桌面後端尚未提供更新檢查介面'],
        ]} /><p className="panel-note">此版本尚無可用的自動更新流程。</p><button onClick={() => setPage('diagnostics')}>檢視診斷</button></Panel>}
      </main><footer><span>Chadex · 本地優先</span><span>{view.freshness === 'fresh' ? `狀態更新 ${date(view.receivedAt)}` : '等待新的服務狀態'}</span></footer>
    </div>
  </div>;
}
