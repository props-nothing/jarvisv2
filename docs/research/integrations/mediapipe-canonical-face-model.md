---
integration: mediapipe-canonical-face-model
status: adopted
last_verified: 2026-10-06
owners: []
selected_spec_version: null
selected_sdk: null
---

# MediaPipe canonical face model (the console's head)

## Scope

One static asset: the 468-vertex, 898-triangle canonical face mesh, used as the geometry of the head the browser console
draws (`apps/jarvisd/src/hud/head.js`). No MediaPipe SDK, model, network call or camera is used. Out of scope: face
tracking, textures, any runtime dependency.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| upstream asset | `google-ai-edge/mediapipe`, `mediapipe/modules/face_geometry/data/canonical_face_model.obj` (master) | 2026-10-06 | the geometry |
| licence | `google-ai-edge/mediapipe` `LICENSE` (Apache License 2.0) | 2026-10-06 | redistribution terms |
| `llms.txt` | not found (a static data file, not an API) | 2026-10-06 | n/a |

## Verified Contract

- The file lists `v x y z` positions (468) and `f v/vt v/vt v/vt` triangles (898, 1-based, with texture indices).
- Coordinates are in the model's own units: x to the viewer's right, y up, z out of the face; chin about y = -9.4,
  forehead about y = 8.3, nose tip (index 1) at z = 7.5. The mesh is an open mask with a 36-edge border and 1,365 unique edges.
- Landmark rings used for animation (eye, brow, lip, nose indices) were checked against the geometry: eye rings span
  y 2.3 to 3.0 and are mirror-symmetric in x; the inner lip ring spans y -4.5 to -4.0; the lip line sits at y of about -4.2.
- Licence: Apache License 2.0, Copyright 2019-2023 The MediaPipe Authors. Redistribution is allowed with the licence
  text and attribution retained; the attribution is in the header of `head.js`.

## JARVIS Mapping

A presentation asset only: positions and triangle indices (integers in hundredths of a unit, about 30 KB), served as a
public static script like the rest of the console (`/head.js`, `GET` only). It carries no data and no secret. The cranium,
neck, jaw rig and brow rig are written for JARVIS; the rigs are weights computed from the geometry at load.

## Decisions

- Take the mesh from the upstream repository (Apache-2.0), **not** from `example/`, whose own licence (CC BY-NC 4.0) is
  unconfirmed for this distribution. The example's copy was only compared to confirm the geometry is identical.
- Render with plain 2D canvas (own projection, back-face culling, additive blending) so the console keeps its
  no-library, same-origin content policy.

## Rejected Alternatives

- A WebGL or three.js renderer: a dependency and a larger policy surface for no visible gain at this size.
- A procedural head: reads as an egg with a face drawn on it; real geometry is the point.
- A rendered image or video of a head: not interactive, cannot follow the pointer or open its mouth with speech.

## Verification Plan

- Test that the asset is public, `GET` only, and carries no outside origin or markup insertion (`hud.rs` tests).
- Parse check at build time: 468 vertices and 898 triangles, indices in range (script load test below).
- Live: the page shows the head and the mouth opens while speaking.

## Unresolved Questions

- None blocking. If a different head style is wanted later, only `head.js` changes.