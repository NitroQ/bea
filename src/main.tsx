import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import { UpdateBanner } from './UpdateBanner';
import './styles.css';

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <UpdateBanner />
    <App />
  </StrictMode>,
);
