# Bandwidth Reduction

How `kitweb` reduces the data it streams to the terminal over SSH, what was
implemented, the measured effect, and the options left to explore.

The goal throughout: **reduce bytes on the wire without lowering the browser
resolution or the frame rate.**

---

## Where the bytes come from

`kitweb` captures the Xvfb screen with FFmpeg `x11grab`, converts each frame to
native-size RGBA, and ships it to the terminal using the Kitty Graphics
Protocol. Over SSH the only usable transmission medium is `t=d` (direct,
base64-encoded inline escape codes); the file / shared-memory transports
(`t=f`, `t=t`, `t=s`) are local-only.

At the defaults (1680×1260 @ 30 fps):

```
raw RGBA frame  = 1680 · 1260 · 4            = 8,467,200 B ≈ 8.07 MiB
base64 inflate  = × 4/3                       ≈ 10.76 MiB / frame
full 30 fps     = × 30                        ≈ 323 MiB/s
```

The original pipeline sent **uncompressed RGBA, base64-encoded, every frame**
(`f=32`, no `o=` key, no change detection). 323 MiB/s is far above any SSH
link, so the bounded capture→render channel was dropping most frames: the
observed **70 MB/s** corresponds to only ~6–7 frames/s actually reaching the
wire. Bytes/frame, not the link, was the limiter.

---

## What was implemented

Two lossless changes in the render path. Both keep resolution and fps intact.

### 1. Skip identical frames

`renderer.rs` keeps the previously transmitted RGBA buffer and compares each
new frame to it. If the pixels are unchanged, the image is **not retransmitted
at all**. The status bar is only redrawn when its text changes, so when both the
page and the status line are static the renderer writes **zero bytes**.

This is the largest real-world win for *browsing*, which is mostly static
(reading, idle). It does nothing for full-motion content, where every frame
differs.

### 2. zlib compression (`o=z`)

Changed frames are deflated with zlib (RFC 1950) before base64 and sent with the
`o=z` key in the graphics header (`kitty.rs`). The terminal inflates the payload
and then reads `s·v·4` bytes, so `s`/`v` remain the native pixel dimensions and
`f=32` is unchanged. Supported by both Kitty and Ghostty.

Compression level is **1 (fastest)**. Screen content is dominated by runs of
identical pixels, so level 1 already captures most of the ratio on UI/text;
higher levels cost materially more CPU per frame for little gain, and give
almost nothing on photographic/video frames. The backend is `flate2`'s default
pure-Rust `miniz_oxide` (no new C/cmake build dependency).

Expected ratios:

| Content              | zlib level-1 ratio | ≈ bytes/frame (from 10.76 MiB) |
| -------------------- | ------------------ | ------------------------------ |
| Static text / UI     | 5–15×              | 0.7–2 MiB                      |
| Mixed page + images  | 2–4×               | 2.7–5 MiB                      |
| Full-motion video    | 1.5–2.5×           | 4–7 MiB                        |

---

## Measured result

End to end: **70 MB/s → 30 MB/s** (~57% reduction) on the user's Mac→Linux
Ghostty SSH session.

Interpreting the residual 30 MB/s matters for choosing the next step. Two
non-exclusive explanations:

1. **The measured scene was changing a lot** (scrolling / animation / video).
   There, skip-identical never fires and zlib level-1 only manages ~1.5–2.5×, so
   the floor is high and inherent to lossless coding.
2. **Compression CPU is now the throughput limiter.** Level-1 deflate on an
   8 MiB frame with `miniz_oxide` is non-trivial; if encode time per frame is
   the bottleneck, byte-rate = (achievable fps) × (compressed frame size).

These have *different* fixes (lossy/dirty-rects for case 1; faster backend for
case 2), so the first recommended action is to **measure per scenario** (see
[Measurement](#measurement)) before investing in a specific lever.

---

## What more can be explored

Ordered roughly by value-for-effort toward the "keep res + fps" goal. Each notes
benefit, effort, and risk.

### A. Drop the alpha channel — `f=24`  *(easy, modest)*

Browser frames are opaque (alpha = 255 everywhere). Capturing `AV_PIX_FMT_RGB24`
and sending `f=24` removes 25% of the raw bytes *before* compression. After
zlib the marginal gain is small (the constant alpha plane compresses to almost
nothing), but it also reduces the number of bytes fed to the compressor, which
helps the CPU-limited case. Low risk.

### B. Tune the deflate stage  *(easy–medium)*

- **Higher level (6–9):** better ratio on UI, diminishing returns, more CPU;
  near-useless on video. Only worth it if *not* CPU-limited.
- **`zlib-ng` backend** (`flate2 = { features = ["zlib-ng"] }`, needs `cmake`):
  several× faster encode at the same level. The single best lever if case 2
  (CPU-limited) is true — it directly raises achievable fps and lets you afford
  a higher level. Adds a C build dependency.
- **Adaptive level** based on a cheap entropy estimate of the frame.

### C. Dirty-rectangle / tile updates  *(medium–high, biggest lossless win for localized change)*

Tile the frame (e.g. 64×64), compare each tile to the previous frame, and
transmit only changed tiles as separate Kitty placements at their pixel offset.
Huge for *localized* changes — typing, a blinking cursor, a small widget,
hover states. Limited for full-frame scroll/video, where most tiles change and
per-placement overhead (header + base64 padding per tile) can erase the win.

Implementation notes: choose tile size to balance granularity vs. per-image
overhead; cap the changed-tile count and fall back to a single full-frame
transmit when most of the screen changed; reuse the existing compression path
per tile.

### D. Native Kitty animation / compositing  *(medium–high, risky)*

The protocol's `a=f` frame transmission + `a=c` compose let the terminal hold a
base image and apply region updates server-side — the "proper" delta mechanism.
In practice Ghostty's animation support has lagged Kitty's, so portability is a
risk. Option C achieves most of the same benefit using plain placements that are
universally supported, so this is lower priority.

### E. Lossy pre-quantization  *(medium — the main lever for video)*

The protocol has **no JPEG/lossy format**, so lossless coding has a hard ceiling
on full-motion content. We can introduce *our own* lossy step that still emits
24/32-bit pixels but lowers entropy so zlib compresses far better, all without
touching resolution or fps:

- **Bit-depth reduction / posterize:** mask the low bits of each channel
  (e.g. 8→5 bits) → many more repeated values → 2–4× better zlib ratio.
- **Optional ordered dithering** to mask the resulting banding.
- Expose a quality knob (`--quality`) so the user trades sharpness for
  bandwidth on demand.

This is the highest-leverage remaining option specifically for video, where A–D
help little.

### F. Capture from Chrome instead of x11grab (CDP screencast)  *(high effort, architectural)*

`Page.startScreencast` (Chrome DevTools Protocol) delivers frames **only when
the page changes**, already encoded (JPEG/PNG), with damage metadata — Chrome
does change detection and encoding for us. Two uses:

- Forward CDP **PNG** frames directly as Kitty `f=100` with no re-encode.
- Or use CDP purely as a change/damage signal feeding options C/E.

Mostly a CPU and capture-efficiency win; the link still carries RGBA/PNG to the
terminal, and our skip-identical already removes static frames from the wire.
Worth it only if we also adopt JPEG-on-the-source ideas, and it's a significant
rewrite of the capture stage.

### G. Adaptive, backpressure-aware quality  *(medium, good UX)*

Watch the capture→render channel depth (proxy for link saturation) and degrade
gracefully under load — raise quantization (E), coarsen tiles (C), or lower the
deflate level only while saturated, restoring full quality when idle. Keeps
nominal resolution/fps while smoothing the worst case.

### Hard constraints (not levers)

- **base64 +33% is unavoidable over SSH** — `t=d` requires it, and the local
  transports don't cross the link. Compressing before base64 already mitigates
  this; there is no further win here.
- **No native lossy/video codec** in the protocol — anything lossy must be done
  by us before transmission (option E).

---

## Measurement

The residual byte-rate should be characterized per scenario before picking a
lever.

1. **Built-in wire counter (`KITWEB_STATS=1`).** The renderer wraps its stdout
   in a `StatWriter` that tallies every byte written — image payload, status
   bar, and cursor moves — and once per second shows the rate as a
   `[xx.xx MB/s]` prefix on the status bar. This isolates `kitweb`'s own output
   from other SSH traffic and ticks down to `0.00 MB/s` on a static page,
   validating skip-identical. Run with `KITWEB_STATS=1 cargo run -- <url>`.
2. **Or measure the link** with `iftop` / `nload` on the SSH connection, or pipe
   through `pv` in a local repro.
3. **Run controlled scenes:** blank page, static text article (should approach
   0 B/s), slow scroll, fast scroll, and a 1080p video. Record bytes/s and CPU
   for each.

The blank/static numbers validate skip-identical; the scroll/video numbers tell
you whether to pursue lossless dirty-rects (C), faster deflate (B), or lossy
quantization (E).

---

## Current state (summary)

| Lever                         | Status        | Helps most with        |
| ----------------------------- | ------------- | ---------------------- |
| Skip identical frames         | **Done**      | Reading / idle         |
| zlib `o=z` (level 1)          | **Done**      | UI / text              |
| `f=24` drop alpha             | Not started   | Everything (small)     |
| Faster/zlib-ng + level tuning | Not started   | CPU-limited case       |
| Dirty-rectangle tiles         | Not started   | Localized change       |
| Lossy pre-quantization        | Not started   | Full-motion video      |
| CDP screencast capture        | Not started   | Source CPU / capture   |
| Adaptive quality              | Not started   | Worst-case smoothness  |
