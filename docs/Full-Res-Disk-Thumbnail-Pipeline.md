You should store **Full-Resolution images on disk** while using **256x256 Thumbnails for UI display**.

Storing only thumbnails destroys original image quality when a user pastes an item back from history (e.g., pasting into Figma, Slack, or Word). Conversely, keeping full-res images in memory will cause UI lag and excessive RAM usage. A **Full-Res Disk + Cached Thumbnail UI** hybrid matches user expectations without memory overhead.

---

## Architectural Design: Full-Res Disk + Thumbnail Pipeline

```
Clipboard Event (WM_CLIPBOARDUPDATE)
       │
       ▼
GDI Source Handle (CF_DIB) ──► xxHash3 Stream (Deduplication Check)
       │
       ├──────────────────────────────────────────┐
       ▼                                          ▼
[Path A: Thumbnail Generation]         [Path B: Full-Res Storage]
 GDI StretchBlt (256x256)               Scanline GetDIBits (Chunked)
       │                                          │
 256KB Buffer                             64KB Streaming Buffer
       │                                          │
 png::Encoder                            png::Encoder (Compression::Fast)
       │                                          │
 thumb_{hash}.png                       full_{hash}.png
 (~20–50 KB)                            (~500 KB – 2 MB)
       │                                          │
       ▼                                          ▼
 UI Grid Render                          Copy-Back to Clipboard

```

---

## Technical Component Breakdown

### 1. Zero-Abort Capture & Encoding Pipeline

To solve the `std::process::abort()` issue, avoid allocating large intermediate `Vec<u8>` buffers in Rust heap.

* **GDI Downscaling for Thumbnails:**
Use `CreateDIBSection` to allocate a 256x256 32bpp destination bitmap (~256KB). If memory is exhausted, `CreateDIBSection` returns a `NULL` `HBITMAP`, allowing Nex to return `Err` gracefully instead of aborting. `StretchBlt` performs hardware-accelerated downscaling directly on the GPU/GDI surface.
* **Scanline-Streamed Full-Res PNG Storage:**
Instead of reading the entire full-res bitmap into a multi-megabyte `Vec<u8>`, stream it row-by-row:
1. Open a output file `full_{hash}.png`.
2. Initialize `png::Encoder` using `Compression::Fast` or `Compression::Default`.
3. Query `GetDIBits` row-by-row (or in small 64KB scanline chunks).
4. Pass each scanline chunk directly to `png::Writer::write_image_data`.


* Peak RAM usage drops from **~100MB+** down to **<1MB**, completely eliminating OOM abort risks.

---

### 2. Copy-Back (Paste) Mechanics

When a user selects an image item from Nex's Bento grid:

1. **Load Full-Res File:** Nex reads `full_{hash}.png` from disk (not the thumbnail).
2. **Decode to DIB:** Decode the PNG back into a `CF_DIB` structure or raw `HBITMAP`.
3. **Clipboard Placement:** Call `SetClipboardData(CF_DIB, h_dib)`.

The pasted result retains 100% losslessness and original dimensions.

---

### 3. Deduplication Strategy

Comparing 256x256 thumbnail PNG bytes for deduplication introduces false positives (two slightly different 4K images downscaled to 256x256 might yield identical compressed bytes).

* **Recommended Strategy:** Compute an `xxHash3` or `BLAKE3` hash over the raw image pixel stream during the initial `GetDIBits` pass.
* If the hash matches an existing database entry:
* Do not encode or write new PNG files.
* Update the `last_used_at` timestamp in the database and bump the item to the top of the history list.



---

### 4. Cache Management & Storage Quota

To prevent `%LOCALAPPDATA%\Nex\image_cache\` from consuming unbounded disk space:

* **Limits:** Enforce both an item cap (e.g., max 50 images) and a disk budget cap (e.g., max 500MB total).
* **Eviction Policy:** Least Recently Used (LRU). When limits are exceeded, purge the oldest pair of `full_{hash}.png` and `thumb_{hash}.png` files along with their database entries.

---

### Key Questions Strategy Recommendations

| Decision Area | Recommendation | Rationale |
| --- | --- | --- |
| **GDI vs. WIC** | **GDI + `png` Crate** | Already used in `overlay/icons.rs`, simpler C FFI bindings, no COM initialization overhead (`CoInitializeEx`), zero-abort safety via `NULL` checks. |
| **Deduplication** | **Full Raw Pixel Stream Hash** | Hash computed via `xxhash-rust` on raw scanlines during capture. Prevents thumbnail collision risks. |
| **MAX_CAPTURE_DIM** | **Keep 2560px or Remove Limit** | With GDI scanline streaming, large dimensions no longer cause OOM. If `CreateDIBSection` returns `NULL`, handle the error safely. |
| **PNG Compression** | `png::Compression::Fast` | Reduces capture latency to <15ms while keeping file sizes small. |