// TokenHub's definition reads Tencent's server-rendered docs pages: the
// model list's generation tables and the price page's Tencent rows.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import def from '../providers/tokenhub.mjs';

const source = (key) => def.sources.find((s) => s.key === key);
const table = (head, rows) =>
  `<table><tr>${head.map((h) => `<th>${h}</th>`).join('')}</tr>${rows.map((r) => `<tr>${r.map((c) => `<td>${c}</td>`).join('')}</tr>`).join('')}</table>`;

const MODEL_LIST = `<h2>语言模型</h2>${table(['模型名称', 'model（调用参数）', '能力支持'], [['Hy-T1', 'hunyuan-t1', '深度思考']])}
<h2>视觉模型</h2><h3>图像生成</h3>${table(['模型名称', 'model（调用参数）', '模型介绍', '任务类型', '默认并发数'], [
  ['Hy-Image-3.5-preview', 'hy-image-v3.5-preview', '…', '文生图 图生图', '5'],
  ['Hy-Image-3.0 （2026-11-12 下线）', 'hy-image-v3', '…', '文生图', '5'],
])}<h3>视频生成</h3>${table(['模型名称', 'model（调用参数）', '模型介绍', '任务类型', '默认并发数'], [['Kling-Video-V3', 'kling-video-v3', '…', '文生视频', '5']])}
<h3>3D 生成</h3>${table(['模型名称', 'model（调用参数）', '模型介绍', '任务类型', '默认并发数'], [['HY-3D-3.1', 'hy-3d-3.1', '…', '文生3D', '3']])}`;

test('the model list yields every generation id, and only those', () => {
  assert.deepEqual(source('models').extract(MODEL_LIST), ['hy-image-v3.5-preview', 'hy-image-v3', 'kling-video-v3', 'hy-3d-3.1']);
  assert.match(source('models').snapshot(MODEL_LIST), /图像生成 \| Hy-Image-3\.0 （2026-11-12 下线） \| hy-image-v3 \| 文生图/, 'a retirement date shows as a change');
});

test('a model list that lost a generation section is a broken source', () => {
  assert.throws(() => source('models').extract(MODEL_LIST.replace(/<h3>3D 生成<\/h3>[\s\S]*$/, '')), /3D 生成/);
});

test('the price snapshot keeps every row, with its rowspan continuations', () => {
  const html = `<h3>视觉模型</h3><h4>图片生成</h4>${table(['模型名称', '功能', '分辨率', 'Token 单价', 'Token 用量', '折合参考费用'], [
    ['Hy-Image-3.5-preview', '文生图', '1K', '10 元/百万 tokens', '15,000 tokens/张', '0.15 元/张'],
    ['﻿', '﻿', '4K', '﻿', '20,000 tokens/张', '0.2 元/张'],
    ['Kling-Image-v3', '文生图', '1K / 2K', '10 元/百万 tokens', '20,000 tokens/张', '0.2 元/张'],
  ])}<h4>视频生成</h4>${table(['模型名称', '生成功能', '分辨率', 'Token 单价', 'Token 用量', '折合参考费用'], [
    ['WAND-Vega-Video-1.0-Lite', '文 / 图生视频', '768P', '10元/百万 tokens', '37500 tokens/秒', '0.375元/秒'],
  ])}`;
  const snap = source('prices').snapshot(html);
  assert.match(snap, /0\.15 元\/张/);
  assert.match(snap, /20,000 tokens\/张 \| 0\.2 元\/张/, 'the 4K continuation row');
  assert.match(snap, /0\.375元\/秒/);
  assert.match(snap, /Kling-Image-v3 \| 文生图/, 'resold models are carried as TokenHub routes, so their prices are watched too');
  assert.deepEqual(source('prices').extract(html), ['Hy-Image-3.5-preview', 'Kling-Image-v3', 'WAND-Vega-Video-1.0-Lite']);
});

test('only retired TokenHub versions and its 3D resales are skipped by pattern', () => {
  const patterns = def.acknowledged.filter((a) => a.pattern);
  const skipped = (id) => patterns.some((a) => a.pattern.test(id));
  for (const id of ['kl-video-v3', 'vd-video-q2-pro', 'tripo-3d-p2', 'rodin-gen-3d-2.5', 'hi3d-3.0']) assert.ok(skipped(id), id);
  for (const id of ['kling-video-v4', 'vidu-video-q4-preview', 'seedream-image-v5.0-pro', 'minimax-video-h3', 'pixverse-video-c1', 'yt-video-2.0']) {
    assert.ok(!skipped(id), `${id} is a route to carry, not a skip`);
  }
});
