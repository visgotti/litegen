import { convert, type Conversion } from '@litegen/3d-converter';

const c: Conversion = convert(new Uint8Array(0), { to: 'glb' });
void c;
