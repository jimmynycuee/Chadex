/// <reference types="vite/client" />
declare module 'virtual:ferret-canonical' {
  interface Pose { name: string; width: number; height: number; eyes: { name: string; x: number; y: number; width: number; height: number }[]; light?: number[] }
  const canonical: {
    states: string[];
    labels: Record<string, string>;
    poses: Record<string, Pose>;
    layouts: Record<string, { width: number; height: number; x: number; shadowWidth: number; tailWidth: number; tailHeight: number; tailX: number; tailY: number; tailAngle: number; tailMirrored: boolean }>;
    baseline: number;
    toolStates: Record<string, string>;
    taskSteps: Record<string, string>;
    activeTasks: string[];
    activeJobs: string[];
    waitingJobs: string[];
    ignoredTools: string[];
  };
  export default canonical;
}
