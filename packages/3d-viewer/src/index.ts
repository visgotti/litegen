// @litegen/3d-viewer: litegen's 3D preview, shared by the dashboard and the
// 3D converter tool (apps/3d-converter). Design:
// docs/superpowers/specs/2026-11-06-shared-3d-preview-design.md.
export { VIEWER_FORMATS, isViewerFormat } from './types';
export type { StoredFrame, UpAxis, ViewerFile, ViewerFileRole, ViewerFormat } from './types';
export { copyToClipboard, drawnNote, formatBytes, formatDimensions } from './format';
export type { ClipboardLike, Dimensions } from './format';
export { default as MeshViewer } from './viewer/MeshViewer';
export type { MeshViewerHandle, MeshViewerProps, ViewerBackground, ViewerFailure, ViewerLoadInfo } from './viewer/MeshViewer';
export { default as RemoteMeshViewer } from './viewer/RemoteMeshViewer';
export type { RemoteMeshViewerProps } from './viewer/RemoteMeshViewer';
export { fetchModel, ModelFetchError } from './viewer/fetch-model';
export { default as ModelInspector } from './inspector/ModelInspector';
export type { InspectorSource, ModelInspectorProps } from './inspector/ModelInspector';
export { Chip, ClipTable, CopyUrlButton, DataTable, FactList, InspectorError, NoteList, Thumbnails } from './inspector/parts';
export type { ClipRow, TableColumn, TableRow, Thumbnail } from './inspector/parts';
