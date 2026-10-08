import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { DesktopStore } from './state';
import './styles.css';

const root = createRoot(document.getElementById('root')!);
const scenario = import.meta.env.DEV ? new URLSearchParams(location.search).get('preview') : null;
if (scenario) {
  void import('./devPreview').then(({ previewApi }) => {
    const api = previewApi(scenario);
    root.render(<StrictMode><App api={api} store={new DesktopStore(api)} /></StrictMode>);
  });
} else {
  root.render(<StrictMode><App /></StrictMode>);
}
