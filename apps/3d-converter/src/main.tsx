import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
// The dashboard's theme first, so the app's own styles build on it.
import './index.css';
import App from './App';

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
