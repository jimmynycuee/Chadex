import { useState, type ReactNode } from 'react';
import type { DesktopApi, GuideLink } from './api';
import { safeError } from './state';
import { Icon } from './icons';

// The same steps, order and links as the macOS guide (Sources/ChadexApp/GuideView.swift),
// with the Windows app's own button names and window behavior.
export const GUIDE_TEST_PROMPT = 'List Chadex projects, find my selected project and list its top-level files. If it is empty, report to me.';

function Step({ number, title, complete, children }: { number: number; title: string; complete: boolean; children: ReactNode }) {
  return <div className="guide-step">
    <span className={`guide-marker${complete ? ' done' : ''}`} aria-label={complete ? '已完成' : `步驟 ${number}`}>{complete ? <Icon name="check" size={16} /> : number}</span>
    <div className="guide-step-body"><h3>{title}</h3>{children}</div>
  </div>;
}

function Group({ id, title, children }: { id: string; title: string; children: ReactNode }) {
  return <section className="panel guide-group" id={id} aria-labelledby={`${id}-title`}>
    <div className="panel-heading"><h2 id={`${id}-title`}>{title}</h2></div>{children}
  </section>;
}

export interface GuidePageProps {
  api: DesktopApi;
  projectReady: boolean;
  credentialsReady: boolean;
  tunnelReady: boolean;
  verified: boolean;
  canConnect: boolean;
  canChooseProject: boolean;
  /** The two steps only the user can confirm; stored in the app's preferences. */
  openAIProjectConfirmed: boolean;
  pluginConfirmed: boolean;
  onConfirm(step: 'openai_project' | 'chatgpt_plugin', value: boolean): void;
  onChooseProject(): void;
  onOpenConnection(): void;
  onConnect(): void;
}

export function GuidePage(props: GuidePageProps) {
  const { api, projectReady, credentialsReady, tunnelReady, verified } = props;
  const { openAIProjectConfirmed, pluginConfirmed } = props;
  const [linkError, setLinkError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  function open(link: GuideLink) {
    setLinkError(null);
    api.openGuideLink(link).catch((error) => setLinkError(`無法開啟瀏覽器：${safeError(error)}`));
  }
  const linkButton = (link: GuideLink, label: string) =>
    <button type="button" className="text-button" onClick={() => open(link)}>{label}<Icon name="external" size={14} /></button>;
  async function copyPrompt() {
    try { await navigator.clipboard.writeText(GUIDE_TEST_PROMPT); setCopied(true); window.setTimeout(() => setCopied(false), 2000); }
    catch { setCopied(false); }
  }

  const progress: [string, string, boolean][] = [
    ['guide-openai', 'OpenAI Project', openAIProjectConfirmed], ['guide-openai', 'Tunnel / API key', credentialsReady],
    ['guide-chadex', '本機專案', projectReady], ['guide-chadex', '已連線', tunnelReady],
    ['guide-chatgpt', 'ChatGPT 外掛', pluginConfirmed], ['guide-test', '最後測試', verified],
  ];

  return <div className="guide">
    <section className="panel guide-intro">
      <h2>第一次使用 Chadex</h2>
      <p>不需要先了解 MCP。完成下面 5 個設定步驟，再跑一次最後測試，就能開始使用 Chadex。</p>
      <p className="panel-note">第一次設定大約只需要做一次；之後開啟 Chadex、連線，再回 ChatGPT 使用即可。</p>
      <nav className="guide-progress" aria-label="目前進度">{progress.map(([target, label, done]) =>
        <a key={label} href={`#${target}`} className={done ? 'done' : undefined}
          onClick={(event) => { event.preventDefault(); document.getElementById(target)?.scrollIntoView({ behavior: 'smooth', block: 'start' }); }}>
          <Icon name={done ? 'check' : 'dot'} size={14} />{label}</a>)}</nav>
    </section>
    {linkError && <div className="notice error" role="alert">{linkError}</div>}

    <Group id="guide-openai" title="先在 OpenAI Platform 準備">
      <Step number={1} title="建立一個 OpenAI Project" complete={openAIProjectConfirmed}>
        <p>先到 OpenAI Platform 建立一個 Project，這樣 Chadex 使用的 API key 可以獨立管理。</p>
        <ol><li>建立新的 Project。</li><li>Project 名稱建議直接填 Chadex。</li></ol>
        <div className="guide-links">{linkButton('openai_projects', '開啟 OpenAI Projects')}</div>
        <label className="guide-check"><input type="checkbox" checked={openAIProjectConfirmed} onChange={(event) => props.onConfirm('openai_project', event.target.checked)} />我已建立 Chadex Project</label>
      </Step>
      <Step number={2} title="建立 Tunnel ID 和 API key" complete={credentialsReady}>
        <p>接著在 OpenAI Platform 建立一組 Secure MCP Tunnel 與 API key。</p>
        <ol>
          <li>建立 Tunnel：Name、Description 可自由填；Organization 選自己的 Organization；Workspace 選你要使用 Chadex 的 Business / Enterprise workspace。建立後複製 Tunnel ID。</li>
          <li>建立 API key：Project 選剛剛建立的 Chadex；Permissions 第一次設定建議選 All。</li>
        </ol>
        <p className="guide-warning">重要：API key 建立後請立刻複製。關閉建立視窗後，完整金鑰不會再顯示。</p>
        <div className="guide-links">{linkButton('openai_tunnels', '開啟 Tunnels')}{linkButton('openai_api_keys', '開啟 API keys')}</div>
      </Step>
    </Group>

    <Group id="guide-chadex" title="回到 Chadex">
      <Step number={3} title="新增你要讓 ChatGPT 讀取的本機專案" complete={projectReady}>
        <p>按下面的「選擇資料夾…」（或到「專案」頁右上角），選擇你希望 ChatGPT 網頁版可以讀取與操作的資料夾，再按「設為目前專案」。Chadex 不會因此開放整台電腦。</p>
        <div className="button-row"><button type="button" disabled={!props.canChooseProject} onClick={props.onChooseProject}>選擇資料夾…</button></div>
      </Step>
      <Step number={4} title="填入憑證並連接 ChatGPT" complete={tunnelReady}>
        <p>把剛才複製的 Tunnel ID 和 API key 填到「連線」頁的「連線憑證」（API key 填在 API credential 欄位），按「安全儲存」後再按「連接 ChatGPT」。看到「已連線，等待 ChatGPT」就代表 Chadex 這邊完成了。</p>
        <div className="button-row">
          <button type="button" onClick={props.onOpenConnection}>填入 Tunnel ID 與 API key</button>
          {!tunnelReady && credentialsReady && projectReady && <button type="button" className="primary" disabled={!props.canConnect} onClick={props.onConnect}>連接 ChatGPT</button>}
        </div>
        {tunnelReady ? <p className="guide-ok"><Icon name="check" size={16} />已連線，可以到 ChatGPT 使用</p>
          : !(credentialsReady && projectReady) && <p className="panel-note">請先新增本機專案，並填好 Tunnel ID 與 API key。</p>}
      </Step>
    </Group>

    <Group id="guide-chatgpt" title="設定 ChatGPT 網頁版">
      <Step number={5} title="在 ChatGPT 網頁版新增 Chadex 外掛程式" complete={pluginConfirmed}>
        <p>保持 Chadex 顯示「已連線」，然後到 ChatGPT 網頁版新增外掛程式。</p>
        <ol>
          <li>在 ChatGPT 點「外掛程式」後按「新增」。</li>
          <li>名稱填 Chadex。</li>
          <li>連線選「通道」，再選「改用通道 ID」，貼上剛才建立的 Tunnel ID。</li>
          <li>驗證選「無驗證」。</li>
          <li>按下建立外掛程式。如果畫面要求 Scan Tools，等工具掃描完成後再建立。</li>
        </ol>
        <p className="panel-note">如果找不到「新增」或無法建立外掛程式，通常是 ChatGPT 方案、workspace 權限或 Developer Mode 尚未開放。</p>
        <div className="guide-links">{linkButton('chatgpt', '開啟 ChatGPT')}{linkButton('mcp_guide', 'OpenAI MCP 官方說明')}</div>
        <label className="guide-check"><input type="checkbox" checked={pluginConfirmed} onChange={(event) => props.onConfirm('chatgpt_plugin', event.target.checked)} />我已建立 Chadex 外掛程式</label>
      </Step>
    </Group>

    <Group id="guide-test" title="最後測試">
      <h3>在 ChatGPT 執行測試</h3>
      <p>回到 ChatGPT，先選擇 Chadex 外掛程式，再貼上下面這句測試指令。</p>
      <div className="guide-prompt"><code>{GUIDE_TEST_PROMPT}</code>
        <button type="button" className="icon-button small" title="複製" aria-label="複製測試指令" onClick={() => { void copyPrompt(); }}><Icon name={copied ? 'check' : 'copy'} size={16} /></button></div>
      <p className="panel-note">這句會一次測試 ChatGPT → Tunnel → Chadex → 目前選取專案 → 本機檔案讀取是否全部正常。</p>
      {verified ? <div className="guide-ok"><Icon name="check" size={16} /><span><strong>看到檔案就完成了</strong><br />如果 ChatGPT 能列出你目前專案的頂層檔案，代表 Chadex 已經設定成功。之後只要 Chadex 顯示已連線，就可以回 ChatGPT 使用 Chadex。</span></div>
        : tunnelReady && <p className="panel-note">等待 ChatGPT 執行第一次測試…</p>}
    </Group>

    <Group id="guide-trouble" title="遇到問題？">
      <dl className="guide-trouble">
        <dt>ChatGPT 找不到「新增外掛程式」</dt>
        <dd>先確認你使用 ChatGPT 網頁版，以及自己的 Business / Enterprise workspace 有權使用自訂外掛程式。若仍找不到，請檢查 Developer Mode 或詢問 workspace 管理員。</dd>
        <dt>建立外掛程式時找不到 Chadex</dt>
        <dd>先確認 Chadex 顯示「已連線」，再確認 ChatGPT 裡填的是同一個 Tunnel ID、連線選「通道」、驗證選「無驗證」，然後重試。</dd>
        <dt>Chadex 一直顯示「等待驗證」</dt>
        <dd>這不是錯誤。回到 ChatGPT 選擇 Chadex 外掛程式，執行上面的測試指令；成功後 Chadex 會自動更新狀態。</dd>
        <dt>關閉視窗後 Chadex 還在執行</dt>
        <dd>這是正常的。關閉視窗只會把 Chadex 收到系統匣，背景服務與連線仍會繼續運作；點系統匣的 Chadex 圖示並選「Show Chadex」可重新顯示視窗。若要完全結束，請在系統匣圖示選「Quit」。</dd>
      </dl>
    </Group>
    <p className="panel-note">ChatGPT 與 OpenAI Platform 的介面可能更新；如果按鈕名稱略有不同，請找意思相同的「外掛程式 / Apps」、「新增 / Create」、「通道 / Tunnel」選項。</p>
  </div>;
}
