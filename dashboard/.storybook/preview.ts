import type { Preview } from '@storybook/react-vite';
// Both global stylesheets, in the app's order (main.tsx imports index.css,
// App.tsx imports App.css): without them `.btn`, `.input` and every `.pg-*`
// class renders unstyled.
import '../src/index.css';
import '../src/App.css';
import './preview.css';

const preview: Preview = {
  parameters: {
    layout: 'padded',
    backgrounds: {
      // The dashboard renders on this ground throughout (Generations.tsx,
      // the inspector's dark background), so stories must too or every
      // component reads wrong. `light` is for checking the viewer's contrast.
      options: {
        dashboard: { name: 'Dashboard', value: '#0d1117' },
        light: { name: 'Light', value: '#ffffff' },
      },
    },
  },
  initialGlobals: {
    backgrounds: { value: 'dashboard' },
  },
};

export default preview;
