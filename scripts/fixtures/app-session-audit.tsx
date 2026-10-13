// Private Vite test entry: production main.tsx never imports this module.
import { StrictMode, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import '@radix-ui/themes/styles.css';
import 'react-day-picker/style.css';
import 'bootstrap-icons/font/bootstrap-icons.css';
import '../../src/styles.css';
import '../../src/i18n/index';
import { AppTheme } from '../../src/AppTheme';
import { App } from '../../src/App';

declare global { interface Window { auditRemount?: () => void } }
function Session() {
  const [generation, setGeneration] = useState(0);
  useEffect(() => {
    window.auditRemount = () => setGeneration(current => current + 1);
    return () => { delete window.auditRemount; };
  }, []);
  return <AppTheme><App key={generation}/></AppTheme>;
}
createRoot(document.getElementById('root')!).render(<StrictMode><Session/></StrictMode>);
