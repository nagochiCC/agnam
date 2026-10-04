# Issue #80: obsolete Cover generation cancellation

This is reproducible headless performance evidence, not a Cover latency specification. No GUI was started. The ignored `issue80_cover_cancel_measurement` test in `src/app/issue76_measure.rs` creates one 1800×2400 gradient PNG and three distinct source paths, starts two old Search generation jobs, then changes generation and requests one latest visible Cover. Both old workers report the checkpoint immediately before PNG decode; the generation switch occurs after both reports. The same generator, fixture, and scheduler are used twice in one process, first with cancellation disabled and then enabled. Neither run reads the disk cache for the generated source.

Run with `cargo test issue80_cover_cancel_measurement -- --ignored --nocapture --test-threads=1`. The result below is one run on Darwin 23.1.0 arm64, starting from repository HEAD `49d774e` plus the Issue #80 working-tree change. Times are wall-clock and can vary with CPU load and storage. The pre-decode checkpoint records that both old jobs will enter a noninterruptible decoder call; it does not interrupt the call.

| Measure | Cancellation disabled | Cancellation enabled |
| --- | ---: | ---: |
| Latest demand to worker start | 842.78 ms | 204.78 ms |
| Latest demand to result | 1706.90 ms | 1093.69 ms |
| First obsolete worker survival after switch | 842.74 ms | 204.74 ms |
| Peak physical generation workers | 2 | 2 |
| Peak stale workers | 2 | 2 |
| Decode starts | 3 | 3 |
| Obsolete jobs cancelled after decode | 0 | 2 |
| Completion events / stale completions | 3 / 2 | 3 / 2 |

The improvement comes from skipping obsolete crop/resize and cache encode/write after PNG decode, then releasing the existing physical slot when the worker's completion is handled. The result does not imply immediate preemption: an old PNG/WebP/JPEG decoder call or archive source read can still hold a slot until that call returns. `image::load_from_memory_with_format`, TurboJPEG `decompress`, and the current archive Cover read APIs have no cancellation parameter in this path. The generator checks cancellation before/after source resolution, metadata, source read, decode, and crop/resize; it does not cancel between cover and metadata writes.

The separate Issue #76 headless harness measured the unmodified path at 1180.48 ms latest start on this machine. That run used a different timing setup and is context only; the table above is the controlled same-fixture comparison.
