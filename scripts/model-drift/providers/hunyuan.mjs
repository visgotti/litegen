// Tencent Hunyuan (Tencent Cloud) — the official Go SDK on GitHub, which is
// generated from the API definitions: client.go has one method per action,
// models.go the request structs with the parameter docs as comments. Tencent's
// generation APIs take no model name, so the "ids" are the async Submit…Job
// action names of the three services that host image/video generation:
//   hunyuan-client  hunyuan 2023-09-01 (Hunyuan image generation)
//   vclm-client     vclm 2024-05-23 (video generation: Hunyuan, Kling, Vidu…)
//   aiart-client    aiart 2022-12-29 (also a Hunyuan text-to-image action)
//   vclm-models     the `Model` values SubmitImageToVideoJob accepts (it is a
//                   Kling-branded resale action we skip) + that request struct
//                   as the snapshot
//   vclm-hunyuan-video  index: the SubmitHunyuanToVideoJob request struct and
//                   the DescribeHunyuanToVideoJob response struct (our video
//                   action's contract)
//   vclm-image-to-video-general  index: the SubmitImageToVideoGeneralJob request
//                   struct and the DescribeImageToVideoGeneralJob response
//                   struct (hunyuan-video-i2v's contract)
//   hunyuan-models  index: the SubmitHunyuanImageJob request struct
//   aiart-hunyuan-image-3  index: the SubmitTextToImageJob request struct and
//                   the QueryTextToImageJob response struct (Hunyuan Image 3.0)
// Mapping mirrors litegen-core/src/providers/image/hunyuan.rs (action
// SubmitHunyuanImageJob on hunyuan, or SubmitTextToImageJob on aiart for
// hunyuan-image-3; no model field) and video/hunyuan.rs (action
// SubmitHunyuanToVideoJob, Tencent's native Hunyuan video model, or
// SubmitImageToVideoGeneralJob for hunyuan-video-i2v; no model field). Until
// 2026-09-11 hunyuan-video called SubmitImageToVideoJob with
// `Model: "Kling-V1-6"`, i.e. Kling 1.6 resold through Tencent Cloud.

const SDK = 'https://raw.githubusercontent.com/TencentCloud/tencentcloud-sdk-go/master/tencentcloud';

/** Acknowledge a list of upstream ids with one reason (ids compare case-insensitively). */
const skip = (reason, ids) => ids.map((id) => ({ id, reason: `skipped 2026-09-11: ${reason}` }));

/** Async generation actions of a client.go (`func (c *Client) SubmitXxxJob(request …`), minus model training. */
const submitJobs = (go) =>
  [...go.matchAll(/^func \(c \*Client\) (Submit\w+Job)\(request \*/gm)].map((m) => m[1]).filter((a) => !/Train/.test(a));

/** `type <name> struct { … }` of a models.go, verbatim. */
function goStruct(go, name) {
  const start = go.indexOf(`type ${name} struct {`);
  if (start < 0) throw new Error(`struct ${name} not found — the action was renamed or removed upstream`);
  return go.slice(start, go.indexOf('\n}', start) + 2) + '\n';
}

/** Values listed in the `Model` field comment ("v1.6：Kling-V1-6<br>v2.0：Kling-V2-Master…"). */
function modelValues(go, struct) {
  const comment = goStruct(go, struct).match(/((?:\t\/\/[^\n]*\n)+)\tModel \*string/)?.[1];
  if (!comment) throw new Error(`${struct} has no documented Model field`);
  return [...comment.matchAll(/[：:]\s*([A-Za-z][\w.-]*)/g)].map((m) => m[1]);
}

export default {
  provider: 'hunyuan',
  models: {
    'hunyuan/hunyuan-image': 'SubmitHunyuanImageJob',
    'hunyuan/hunyuan-image-3': 'SubmitTextToImageJob',
    'hunyuan/hunyuan-video': 'SubmitHunyuanToVideoJob',
    'hunyuan/hunyuan-video-i2v': 'SubmitImageToVideoGeneralJob',
  },
  sources: [
    { key: 'hunyuan-client', url: `${SDK}/hunyuan/v20230901/client.go`, expect: 'text', extract: submitJobs },
    { key: 'vclm-client', url: `${SDK}/vclm/v20240523/client.go`, expect: 'text', extract: submitJobs },
    { key: 'aiart-client', url: `${SDK}/aiart/v20221229/client.go`, expect: 'text', extract: submitJobs },
    {
      key: 'vclm-models',
      url: `${SDK}/vclm/v20240523/models.go`,
      expect: 'text',
      extract: (go) => modelValues(go, 'SubmitImageToVideoJobRequestParams'),
      snapshot: (go) => goStruct(go, 'SubmitImageToVideoJobRequestParams'),
    },
    {
      key: 'vclm-hunyuan-video',
      index: true,
      url: `${SDK}/vclm/v20240523/models.go`,
      expect: 'text',
      extract: (go) => [...goStruct(go, 'SubmitHunyuanToVideoJobRequestParams').matchAll(/^\t(\w+) /gm)].map((m) => m[1]),
      snapshot: (go) =>
        goStruct(go, 'SubmitHunyuanToVideoJobRequestParams') + '\n' + goStruct(go, 'DescribeHunyuanToVideoJobResponseParams'),
    },
    {
      key: 'vclm-image-to-video-general',
      index: true,
      url: `${SDK}/vclm/v20240523/models.go`,
      expect: 'text',
      extract: (go) => [...goStruct(go, 'SubmitImageToVideoGeneralJobRequestParams').matchAll(/^\t(\w+) /gm)].map((m) => m[1]),
      snapshot: (go) =>
        goStruct(go, 'SubmitImageToVideoGeneralJobRequestParams') + '\n' + goStruct(go, 'DescribeImageToVideoGeneralJobResponseParams'),
    },
    {
      key: 'hunyuan-models',
      index: true,
      url: `${SDK}/hunyuan/v20230901/models.go`,
      expect: 'text',
      extract: (go) => [...goStruct(go, 'SubmitHunyuanImageJobRequestParams').matchAll(/^\t(\w+) /gm)].map((m) => m[1]),
      snapshot: (go) => goStruct(go, 'SubmitHunyuanImageJobRequestParams'),
    },
    {
      key: 'aiart-hunyuan-image-3',
      index: true,
      url: `${SDK}/aiart/v20221229/models.go`,
      expect: 'text',
      extract: (go) => [...goStruct(go, 'SubmitTextToImageJobRequestParams').matchAll(/^\t(\w+) /gm)].map((m) => m[1]),
      snapshot: (go) => goStruct(go, 'SubmitTextToImageJobRequestParams') + '\n' + goStruct(go, 'QueryTextToImageJobResponseParams'),
    },
  ],
  acknowledged: [
    ...skip('Kling/Vidu resale action on vclm; we integrate Kling and Vidu directly', [
      'SubmitImageToVideoJob',
      'SubmitTextToVideoJob',
      'SubmitMotionControlKlingJob',
      'SubmitVideoEditKlingJob',
      'SubmitVideoExtendKlingJob',
      'SubmitImageToVideoViduJob',
      'SubmitReferenceToVideoViduJob',
      'SubmitTextToVideoViduJob',
    ]),
    // The `Model` values of SubmitImageToVideoJob (vclm-models source).
    ...skip('Kling model resold through SubmitImageToVideoJob; we integrate Kling directly', [
      'Kling-V1-6',
      'Kling-V2-Master',
      'Kling-V2-1',
      'Kling-V2-5-Turbo',
      'Kling-V2-6',
      'kling-v3',
    ]),
    ...skip('template/app action, not a general generation model', [
      'SubmitDrawPortraitJob',
      'SubmitGlamPicJob',
      'SubmitMemeJob',
      'SubmitHumanActorJob',
      'SubmitPortraitSingJob',
      'SubmitTemplateToVideoJob',
      'SubmitVideoFaceFusionJob',
    ]),
    ...skip('multi-turn chat variant of SubmitHunyuanImageJob (same model, needs a conversation-id workflow)', [
      'SubmitHunyuanImageChatJob',
    ]),
    ...skip('aiart text-to-image (advanced) action; the SDK marks it migrated to SubmitHunyuanImageJob', [
      'SubmitTextToImageProJob',
    ]),
  ],
};
