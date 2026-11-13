// Tencent Hunyuan (Tencent Cloud). Two kinds of source:
//
// 1. The official Go SDK on GitHub, generated from the TC3 API definitions:
//    client.go has one method per action, models.go the request/response
//    structs with the parameter docs as comments. These generation APIs take
//    no model name, so the "ids" are the async Submit…Job action names of the
//    services that host image/video generation:
//      hunyuan-client  hunyuan 2023-09-01 (Hunyuan image generation)
//      vclm-client     vclm 2024-05-23 (video generation: Hunyuan, Kling, Vidu…)
//      aiart-client    aiart 2022-12-29 (hosted SubmitTextToImageJob, Hunyuan
//                      Image 3.0, until Tencent deleted it on 2026-09-22)
//      vclm-models     the `Model` values SubmitImageToVideoJob accepts (it is
//                      a Kling-branded resale action we skip) + that request
//                      struct as the snapshot
//      vclm-hunyuan-video  index: the SubmitHunyuanToVideoJob request struct,
//                      the vclm `Image` struct it nests, and the
//                      DescribeHunyuanToVideoJob response struct (our video
//                      action's contract)
//      vclm-image-to-video-general  index: the same three for
//                      SubmitImageToVideoGeneralJob / DescribeImageToVideoGeneralJob
//                      (hunyuan-video-i2v's contract)
//      hunyuan-models  index: the SubmitHunyuanImageJob request struct, the
//                      hunyuan `Image` struct it nests (ContentImage) and the
//                      QueryHunyuanImageJob response struct
//
// TokenHub (大模型服务平台 TokenHub), where Tencent moves every Hunyuan model
// ("迁移后，原平台将不再新增模型能力，并停止支持新购模型服务。用户已购买的模型服务可继续
// 使用" — the hunyuan / vclm / aiart billing pages, 2026), is a provider of its
// own since 2026-11-04: `tokenhub` (providers/tokenhub.mjs), a Bearer-key
// API. This definition keeps the old platform's TC3 actions, which existing
// buyers still call.
//
// Mapping mirrors litegen-core/src/providers/image/hunyuan.rs (action
// SubmitHunyuanImageJob on hunyuan; no model field) and video/hunyuan.rs (action
// SubmitHunyuanToVideoJob, Tencent's native Hunyuan video model, or
// SubmitImageToVideoGeneralJob for hunyuan-video-i2v; no model field). Until
// 2026-09-11 hunyuan-video called SubmitImageToVideoJob with
// `Model: "Kling-V1-6"`, i.e. Kling 1.6 resold through Tencent Cloud.
//
// aiart release 31 (2026-09-22, https://cloud.tencent.com/document/product/1668/88078)
// deleted SubmitTextToImageJob and QueryTextToImageJob, the two actions
// hunyuan-image-3 ran on; the row was retired 2026-10-30 (its successor on
// TokenHub, hy-image-v3, retires there on 2026-11-12; tokenhub carries
// hy-image-v3.5-preview).

const SDK = 'https://raw.githubusercontent.com/TencentCloud/tencentcloud-sdk-go/master/tencentcloud';

/** Acknowledge a list of upstream ids with one reason (ids compare case-insensitively). */
const skip = (reason, ids) => ids.map((id) => ({ id, reason: `skipped 2026-09-11: ${reason}` }));

/** Async generation actions of a client.go (`func (c *Client) SubmitXxxJob(request …`), minus model training. */
const submitJobs = (go) =>
  [...go.matchAll(/^func \(c \*Client\) (Submit\w+Job)\(request \*/gm)].map((m) => m[1]).filter((a) => !/Train/.test(a));

/** `type <name> struct { … }` of a models.go, verbatim. */
function goStruct(go, name) {
  const start = go.indexOf(`\ntype ${name} struct {`);
  if (start < 0) throw new Error(`struct ${name} not found — the action was renamed or removed upstream`);
  return go.slice(start + 1, go.indexOf('\n}', start) + 2) + '\n';
}

/** Several structs of a models.go, in order, blank-line separated. */
const goStructs = (go, names) => names.map((n) => goStruct(go, n)).join('\n');
/** An action's request struct plus the structs it references, headed by the action name the adapter sends. */
const actionStructs = (go, action, more) => `// action ${action}\n${goStructs(go, [`${action}RequestParams`, ...more])}`;

/** Top-level field names of a struct (the index id list of a contract source). */
const goFields = (go, name) => [...goStruct(go, name).matchAll(/^\t(\w+) /gm)].map((m) => m[1]);

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
      typings: true,
      url: `${SDK}/vclm/v20240523/models.go`,
      expect: 'text',
      extract: (go) => goFields(go, 'SubmitHunyuanToVideoJobRequestParams'),
      snapshot: (go) => actionStructs(go, 'SubmitHunyuanToVideoJob', ['Image', 'DescribeHunyuanToVideoJobResponseParams']),
    },
    {
      key: 'vclm-image-to-video-general',
      index: true,
      typings: true,
      url: `${SDK}/vclm/v20240523/models.go`,
      expect: 'text',
      extract: (go) => goFields(go, 'SubmitImageToVideoGeneralJobRequestParams'),
      snapshot: (go) => actionStructs(go, 'SubmitImageToVideoGeneralJob', ['Image', 'DescribeImageToVideoGeneralJobResponseParams']),
    },
    {
      key: 'hunyuan-models',
      index: true,
      typings: true,
      url: `${SDK}/hunyuan/v20230901/models.go`,
      expect: 'text',
      extract: (go) => goFields(go, 'SubmitHunyuanImageJobRequestParams'),
      snapshot: (go) => actionStructs(go, 'SubmitHunyuanImageJob', ['Image', 'QueryHunyuanImageJobResponseParams']),
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
