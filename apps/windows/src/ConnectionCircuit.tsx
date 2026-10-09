import type { BackendSnapshot } from './contracts';
import { Icon, type IconName } from './icons';

type NodeState = 'complete' | 'active' | 'waiting' | 'idle' | 'error';
const stateText: Record<NodeState, string> = { complete: '已完成', active: '進行中', waiting: '就緒', idle: '尚未開始', error: '錯誤' };

function basename(path: string) { return path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || path; }

/** The project → secure tunnel → ChatGPT path, the same signature graphic as the macOS app. */
export function ConnectionCircuit({ snapshot, verified }: { snapshot: BackendSnapshot | null; verified: boolean }) {
  const failed = snapshot?.phase === 'error' || Boolean(snapshot?.error);
  const project: NodeState = snapshot?.selected_project ? 'complete' : 'idle';
  const tunnel: NodeState = failed && !snapshot?.tunnel_ready ? 'error'
    : snapshot?.tunnel_ready ? 'complete'
    : snapshot?.phase === 'preparing' || snapshot?.current_operation ? 'active' : 'idle';
  // A tunnel that never came up never reached ChatGPT: only the failed hop is marked.
  const chatGPT: NodeState = verified ? 'complete'
    : failed && snapshot?.tunnel_ready ? 'error'
    : snapshot?.tunnel_ready ? 'waiting' : 'idle';
  const accent = verified ? 'good' : 'signal';
  return <ol className={`circuit accent-${accent}`} aria-label="連線路徑">
    <CircuitNode icon="laptop" title="本機專案" detail={snapshot?.selected_project ? basename(snapshot.selected_project.path) : '尚未選擇'} state={project} />
    <CircuitLink filled={tunnel === 'complete'} live={tunnel === 'active'} />
    <CircuitNode icon="shield" title="安全 Tunnel" detail={tunnel === 'complete' ? '已就緒' : tunnel === 'active' ? '啟動中…' : tunnel === 'error' ? '錯誤' : '未啟動'} state={tunnel} />
    <CircuitLink filled={chatGPT === 'complete'} live={chatGPT === 'waiting'} />
    <CircuitNode icon="spark" title="ChatGPT" detail={chatGPT === 'complete' ? '已驗證' : chatGPT === 'waiting' ? '等待首次操作' : chatGPT === 'error' ? '錯誤' : '尚未連線'} state={chatGPT} />
  </ol>;
}

function CircuitNode({ icon, title, detail, state }: { icon: IconName; title: string; detail: string; state: NodeState }) {
  return <li className={`circuit-node ${state}`} aria-label={`${title}：${detail}，${stateText[state]}`}>
    <span className="circuit-dot" aria-hidden="true">{state === 'active' ? <span className="spinner" /> : <Icon name={state === 'error' ? 'warning' : icon} size={18} />}</span>
    <strong aria-hidden="true">{title}</strong>
    <small aria-hidden="true">{detail}</small>
  </li>;
}

function CircuitLink({ filled, live }: { filled: boolean; live: boolean }) {
  return <li className={`circuit-link${filled ? ' filled' : live ? ' live' : ''}`} aria-hidden="true"><span /></li>;
}
