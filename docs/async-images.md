# Late and async images

Most image vendors answer in one call. Seven run a job instead and are polled
until it finishes: BFL, Replicate, Kling, Hunyuan, Leonardo, Luma and Runway.
For those, litegen never loses an image to its own deadline, and a caller can
choose not to wait at all.

## A job that outlasts the vendor timeout

litegen waits for an image call as long as the vendor timeout
(`vendor_timeout_seconds`, [gateway-settings.md](gateway-settings.md)). A
job still running then isn't lost: the vendor keeps it, and litegen delivers
it when it finishes.

- The call answers `504 vendor_timeout`, as it always did, so a client that
  treats a 504 as a failure behaves as before. The error now also carries
  `generation_id` and `status: "processing"`:

  ```json
  {"error": {"code": "vendor_timeout", "retryable": false,
             "vendor_job_id": "…", "generation_id": "litegen-img-…",
             "status": "processing"}}
  ```

  Don't send the request again: the vendor is still running, and billing,
  the first one.
- The background poller asks the vendor about the job. When it finishes,
  litegen keeps the image where any image of the app is kept (the app's own
  bucket, or litegen's store), the generation becomes `completed`, and the
  key's webhook fires. `GET /v1/generations/{id}` then serves it, its links
  handed out in the app's access mode ([storage-access.md](storage-access.md)).
- A job the vendor fails ends `failed` with the vendor's reason, refunded,
  with its webhook.

## Asking not to wait

`Prefer` (RFC 7240) on `POST /v1/images/generations`:

| Header | litegen answers |
|---|---|
| `Prefer: respond-async` | `202 Accepted` as soon as the vendor has the job. |
| `Prefer: wait=10` | the image, if the job finishes within 10 s; otherwise `202`. |
| `Prefer: respond-async, wait=10` | the same as `wait=10`. |

The `202` answer is the generation to come:

```json
{"id": "litegen-img-…", "object": "image.generation", "status": "processing",
 "model": "bfl/flux-pro-1.1", "created": 1793712202}
```

with `Location: /v1/generations/{id}` and `Preference-Applied` naming what
was honoured. Poll `GET /v1/generations/{id}`, or wait for the key's webhook.

litegen honours the preference only when the image can be delivered later:

- the model's vendor runs jobs (the seven above);
- the request keeps its output (a request that keeps nothing has nowhere to
  deliver it from);
- the request isn't an edit that litegen finishes itself (`preserve`, which
  pastes the original back outside the edit area).

Otherwise the call is synchronous, as without the header, and
`Preference-Applied` is absent. The RFC lets a server ignore a preference.

## Billing

A job litegen will deliver later is charged when the vendor accepts it, as a
video is at submit. A job that then fails is refunded. A finished one is
billed once.

## In the SDKs

```ts
import { isAccepted } from "@litegen/sdk";

const r = await client.images.submit({ model: "bfl/flux-pro-1.1", prompt: "a lighthouse" }, { waitSeconds: 5 });
const image = isAccepted(r) ? await client.generations.waitForCompletion(r.id) : r;
```

```python
from litegen import is_accepted

r = client.images.submit(model="bfl/flux-pro-1.1", prompt="a lighthouse", wait_seconds=5)
image = client.generations.wait_for_completion(r["id"]) if is_accepted(r) else r
```

`images.generate` is unchanged: it never asks for a `202`.
