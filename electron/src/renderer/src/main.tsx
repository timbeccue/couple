import React from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './views/App';
import { installOverlayScrollbars } from './scroll-overlay';
import './styles.css';

installOverlayScrollbars();

createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
