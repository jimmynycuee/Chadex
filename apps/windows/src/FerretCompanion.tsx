import { useEffect, useRef, useState } from 'react';
import { canonical, FerretController } from './ferret';
import type { BackendSnapshot, RuntimeActivityEntry } from './contracts';

export function Ferret({ snapshot, activities, visible, motion, unavailable }: {
  snapshot: BackendSnapshot | null; activities: RuntimeActivityEntry[];
  visible: boolean; motion: boolean; unavailable: boolean;
}) {
  const controller = useRef(new FerretController());
  const [presentation, setPresentation] = useState(() => controller.current.update(snapshot, activities, Date.now(), true, unavailable));
  useEffect(() => {
    const update = () => setPresentation(controller.current.update(snapshot, activities, Date.now(), document.visibilityState !== 'hidden', unavailable));
    update();
    // Local presentation clock only; this never requests helper/server state.
    const timer = setInterval(update, 500);
    document.addEventListener('visibilitychange', update);
    return () => { clearInterval(timer); document.removeEventListener('visibilitychange', update); };
  }, [snapshot, activities, unavailable]);
  if (!visible) return null;
  const pose = canonical.poses[presentation.state];
  const layout = canonical.layouts[presentation.state];
  const scale = Math.min(layout.width / pose.width, layout.height / pose.height);
  const width = pose.width * scale; const height = pose.height * scale;
  return <div className={`ferret ${motion ? 'ferret-motion' : ''}`} aria-label={`Code Ferret：${canonical.labels[presentation.state]}`}>
    <div className="ferret-stage" aria-hidden="true">
      <div className="ferret-shadow" style={{ width: layout.shadowWidth }} />
      {presentation.state !== 'sleep' && <img className="ferret-tail" src={`/ferret/ferret-motion-${layout.tailMirrored ? 'tail-left' : 'tail'}.png`} alt="" draggable={false}
        style={{ width: layout.tailWidth, height: layout.tailHeight, left: layout.tailX - layout.tailWidth / 2, top: layout.tailY - layout.tailHeight / 2,
          transform: `rotate(${layout.tailAngle}deg)`, transformOrigin: `${layout.tailMirrored ? 82 : 18}% 86%` }} />}
      <div className="ferret-body" style={{ width, height, left: layout.x - width / 2, top: canonical.baseline - height }}>
        <img src={`/ferret/ferret-motion-${pose.name}.png`} alt="" draggable={false} />
        {pose.eyes.map((eye) => <img key={eye.name} className="ferret-eye" src={`/ferret/ferret-motion-${eye.name}.png`} alt=""
          style={{ left: `${eye.x / pose.width * 100}%`, top: `${eye.y / pose.height * 100}%`, width: `${eye.width / pose.width * 100}%`, height: `${eye.height / pose.height * 100}%` }} />)}
      </div>
    </div>
    <div className="ferret-caption"><span className={`dot ${presentation.state === 'error' ? 'warn' : 'quiet'}`} />{canonical.labels[presentation.state]}</div>
    {presentation.progress !== null && <progress max={1} value={presentation.progress} aria-label="已完成任務步驟" />}
    <small>Code Ferret</small>
  </div>;
}
