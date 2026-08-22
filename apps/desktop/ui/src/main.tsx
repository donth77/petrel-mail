import { createRoot } from 'react-dom/client';
import './styles/fonts.css';
import './styles/tokens.css';
import './styles/app.css';
import { App } from './App';
import { ComposeWindow } from './ComposeWindow';
import { SettingsProvider } from './lib/settings';

// A popped-out composer is the same bundle with one query parameter, not a
// second app. It still wants the settings provider — theme, accent and reading
// size are the user's, not the main window's.
const draftId = Number(new URLSearchParams(window.location.search).get('compose'));
const popout = Number.isFinite(draftId) && draftId > 0;

createRoot(document.getElementById('root')!).render(
  <SettingsProvider>{popout ? <ComposeWindow draftId={draftId} /> : <App />}</SettingsProvider>,
);
