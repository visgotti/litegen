import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, userEvent, waitFor, within } from 'storybook/test';
import ConverterPage from './ConverterPage';
import '../App.css';

/** The whole converter, with its real worker and package. */
const meta = { title: 'Converter/Page', component: ConverterPage, parameters: { layout: 'fullscreen' } } satisfies Meta<typeof ConverterPage>;
export default meta;
type Story = StoryObj<typeof meta>;

/** Waiting for a model. */
export const Empty: Story = {};

/** A sample read, then converted to OBJ: both viewers draw. */
export const SampleToObj: Story = {
  tags: ['page'],
  play: async ({ canvasElement }) => {
    const page = within(canvasElement);
    await userEvent.click(await page.findByTestId('sample-skinned-glb', {}, { timeout: 20_000 }));
    await waitFor(() => expect(page.getByTestId('input-viewer')).toHaveAttribute('data-state', 'loaded'), { timeout: 30_000 });
    await userEvent.click(page.getByTestId('target-obj'));
    await waitFor(() => expect(page.getByTestId('output-viewer')).toHaveAttribute('data-format', 'obj'), { timeout: 30_000 });
    await waitFor(() => expect(page.getByTestId('output-viewer')).toHaveAttribute('data-state', 'loaded'), { timeout: 30_000 });
  },
};
