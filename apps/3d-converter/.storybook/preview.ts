import type { Preview } from '@storybook/react-vite';
import '../src/index.css';

const preview: Preview = {
  parameters: {
    layout: 'padded',
    backgrounds: {
      // The tool renders on the dashboard's ground (#0d1117).
      options: {
        app: { name: 'App', value: '#0d1117' },
        light: { name: 'Light', value: '#ffffff' },
      },
    },
  },
  initialGlobals: { backgrounds: { value: 'app' } },
};

export default preview;
