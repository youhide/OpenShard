# Client LOD follow-up plan

The LOD1 producer/cache/restore path is now a single-owner flat-ground path.
This document records the remaining work and, more importantly, the evidence
required before each change is accepted.

## 1. Live soak acceptance

- Run the normally connected client at default zoom, zoom all the way out,
  then pan slowly in both axes and leave it still for several minutes.
- Keep NPC animation and packet traffic enabled; this is specifically not a
  synthetic scene test.
- Take F12 frame dumps before, during and after the wait.  A healthy dump has
  `world_pass_ready_blocks > 0`, `live_ground_quads < full_ground_quads`, no
  LOD oracle mismatch and no quarantined block.
- Record the producer GPU time, frame time, queue depth and retained cache
  bytes for the run.

Acceptance: no visible churn, gaps, wrong sprite/roof, black triangle or
quarantine during the complete pan-and-wait sequence.

## 2. Make load shedding evidence-led

- If the soak shows producer-related frame spikes, add a small adaptive
  dispatcher budget based on the preceding GPU frame timing.
- The dispatcher may defer work only. It must never capture from the camera
  frame, reuse its mutable instance buffers or evict visible atlas content.
- Add a deterministic queue test for the overloaded case and retain the
  existing one-job upper bound as the default safe policy.

Acceptance: overload changes a new block to temporary LOD0, never to a hole;
the producer count remains bounded and frame-time telemetry explains the
deferral.

## 3. Improve field diagnostics

- Add the number of quarantined LOD blocks and the latest quarantine reason to
  the Frames HUD and F12 report.
- Keep the existing opt-in byte audit and full LOD0 oracle separate: one
  establishes producer/cache identity, the other establishes final scene
  equivalence.
- Preserve a compact block/key/transform record in each report so a user dump
  identifies the owner without a screenshot.

Acceptance: a single F12 report distinguishes LOD disabled, LOD not ready,
safe LOD0 fallback, quarantine and an actual compositor mismatch.

## 4. Consider LOD2 only after LOD1 passes soak

- Derive LOD2 only from the canonical LOD1 source, retaining colour, IDs,
  position, normal and depth as one atomic sample contract.
- Add GPU tests for downsample edge ownership and depth interaction with a
  later dynamic draw.
- Gate rollout behind the same live oracle; LOD1 remains the fallback when an
  LOD2 entry is absent or invalid.

Acceptance: LOD2 has no extra ownership path, no black seam at cache block
edges and no regression against the LOD1 soak baseline.

## 5. Keep the implementation small and explicit

- Keep producer code outside the camera-frame presentation method.
- Keep `FlatGroundBlock` as the sole proof passed from map inspection through
  queue dispatch, producer camera and cached restore.
- Remove retired screen-capture helpers rather than retaining a dormant second
  producer path.

Acceptance: there is exactly one visible cache producer and it has private,
fixed-size attachments and a distinct ground-renderer stream.
