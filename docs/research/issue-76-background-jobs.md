# Issue #76: background job / Msg queue headless measurement

This is reproducible evidence for Issue #76 from one Linux machine, not a product performance specification. No Agnam GUI was started.

## Purpose and method

Measure whether obsolete Library/Search work survives a new request, whether it prevents the latest work from starting, and whether completion messages accumulate. The ignored test-only harness at `src/app/issue76_measure.rs` invokes the current `scan_directory`, `SearchIndex::build`, thumbnail controllers, Cover generator, `spawn_background`, and `AppSender`. It changes no production path.

Run from this revision with an isolated cache directory:

```sh
XDG_CACHE_HOME=/tmp/agnam-issue76-cache cargo test issue76_headless_measurement -- --ignored --nocapture --test-threads=1
```

The harness creates and removes a `tempfile::TempDir`. Each scan or search root has four sibling shelves. Each shelf contains 40 small `.jpg` files and 100, 200, or 400 valid one-entry `.cbz` archives. The archive contains `page.jpg`; its bytes are deliberately small because listing/probe cost, rather than image decoding, is the scan workload. Distinct paths give each cold archive probe a distinct cache key. Scan loads shelves at 30 ms intervals (4 requests), 2 ms intervals (12), or without deliberate spacing (8). Search builds switch among three distinct roots at 2 ms intervals. An additional 400-archive shelf and 1600-archive root are measured alone and cold. The harness samples the headless scan receiver about every 1 ms, counts `LibraryState::apply_scan` stale results, and records job start/finish and the queue length on each enqueue. This receiver omits GTK dispatch and rendering cost.

For Cover cost, the harness creates an 1800×2400 RGB PNG with a coordinate gradient (3,795,703 bytes), copies it to distinct source paths, and runs the current `generate_and_cache_bookshelf_thumbnail`. Scheduler tests hold old jobs for a controlled 80 ms and call the real controller completion methods. The Search generation test also runs two actual obsolete PNG Cover workers, switches generation, then starts the latest Cover only when a slot is released. The queue test sends `Msg::OpenFile` through the real unbounded `AppSender`, with an immediate or 1 ms-per-message synthetic consumer. It does not call `App::dispatch` or `sync_ui_state`.

Environment: commit `15da46ae1f2ee31cd936240bff67a146cf513809`, Linux `7.2.7-zen1-1-zen`, AMD Ryzen 7 8700G, 8 cores / 16 logical CPUs, about 24 GB RAM, local temporary filesystem. `Instant` times are wall-clock durations. This is one run, not a distribution or SLA. CPU scheduling, page cache, archive cache, and disk state can change the numbers.

## Results

| Scan workload | Requests | Peak live jobs | Jobs still active when latest started | Stale Msg | Peak queue | Latest start after issue | Latest finish after issue | Obsolete survival after next issue |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 100 archives/shelf, 30 ms | 4 | 1 | 0 | 0/4 | 1 | 0.13 ms | 6.12 ms | none overlapping |
| 200 archives/shelf, 2 ms | 12 | 6 | 1 | 11/12 | 2 | 0.03 ms | 3.90 ms | median 1.84 ms, max 10.64 ms |
| 400 archives/shelf, immediate | 8 | 8 | 7 | 7/8 | 2 | 0.06 ms | 23.65 ms | median 23.22 ms, max 24.85 ms |

The cold solo 400-archive shelf took 17.04 ms. The concurrent heavy latest scan took 23.65 ms from issue; the difference suggests possible CPU / filesystem contention but distinct fixture paths and a single run do not establish causation. There is no scheduler lane before `spawn_background`, and latest thread start stayed below 0.13 ms in these conditions. Each scan sent one completion Msg. The visible stale ratio reflects this headless 1 ms receiver, not GTK timing.

| SearchIndex build | Value |
| --- | ---: |
| Root changes / build requests | 3 |
| Peak concurrent builds; old builds active at latest start | 3; 2 |
| Longest obsolete survival after next root issue | 31.93 ms |
| Latest start / finish after issue | 0.03 / 65.19 ms |
| Completion Msg rejected as stale | 2/3 |
| Cold solo 1600-archive root | 63.04 ms |

`LibrarySearchState::set_query` searches the retained in-memory index; it does not start `SearchIndex::build`. Only beginning a new root/session starts a build. The latest build starts promptly. This run does not prove material latest completion slowdown from the two old builds.

| Visible Cover scope | Cache lane peak | Generation lane peak | Latest start while old lane full | First latest slot after controlled old completion |
| --- | ---: | ---: | ---: | ---: |
| Library thumbnail | 8 | 2 | 0 | about 80 ms |
| Search thumbnail | 8 | 2 | 0 | about 80 ms |
| History Cover | 8 | 2 | 0 | about 80 ms |
| Favorites Cover | 8 | 2 | 0 | about 80 ms |

The 80 ms hold is imposed by the harness to isolate lane behavior; it is not a measured cache or decode time. In every controller, obsolete pending demand is replaced but old in-flight work stays counted until completion. One completion releases one slot. Library/Search/History/Favorites have separate generations and lanes. For the actual PNG generator, eight distinct concurrent Covers finished in 1183.72–1225.20 ms (median 1186.40 ms). With Search's two generation slots occupied by two obsolete PNG Covers, the latest generation waited **1134.37 ms to start**, completed 2248.65 ms after the switch, and both old completions were stale. The latest's own generation took 1114.28 ms. This establishes a same-lane latest-demand blocker for this heavy PNG workload; it is not a claim about typical GUI content.

| Msg queue workload | Producer burst time | Peak depth | Mean / maximum queue age |
| --- | ---: | ---: | ---: |
| 10 messages, immediate consumer | 0.01 ms | 10 | 0.01 / 0.01 ms |
| 100 messages, 1 ms consumer | 0.02 ms | 100 | 52.16 / 104.33 ms |
| 300 messages, 1 ms consumer | 0.12 ms | 300 | 157.59 / 314.98 ms |

`async_channel::unbounded` accepted every synthetic burst Msg. Depth and age grow when a producer burst outruns the consumer. This is a queue capacity demonstration, not evidence that normal Agnam background producers generate 100–300 simultaneous completions. In actual headless scan runs, with the 1 ms receiver, completion queue peak was 1–2. GTK `dispatch + sync_ui_state` cost and its input-message mix remain unmeasured.

The sequential archive contents thumbnail stream and progressive document extraction can each emit one Msg per image while a worker continues processing. Their real producer rate and queue depth were not exercised by this fixture; neither should be inferred from the one-message-per-scan results.

## Background job inventory and interpretation

`spawn_background()` has 20 production call sites. Each call creates one OS thread; it provides no cross-scope lane or cancellation. The `AppSender` unbounded channel carries both UI actions and background results; the GTK main context receives one Msg at a time and calls `dispatch` followed by `sync_ui_state`.

| Scope | Jobs per initiating action; limit | Stale / cancel | Worker retains until exit |
| --- | --- | --- | --- |
| Library filesystem navigation | 1 scan; no lane | Request ID + path; no cancel | Path, scan/probe data, then directory result and sender |
| Archive contents navigation | 1 new archive location; in-memory directory navigation starts none | Request ID; no listing cancel | Location and archive listing/reader/result |
| Sequential archive contents thumbnails | 1 stream per accepted content level; serial images | Session ID; cooperative archive token | Archive paths, image list, backing/reader, image being processed |
| Library thumbnail | Up to 8 cache loads + 2 generations across visible demand | Generation/cohort; no in-flight cancel | Source, optional archive reader/backing, result |
| SearchIndex | 1 per new root session, none per query change; no lane | Build request ID + root; no cancel | Root, traversal data, completed index |
| Search thumbnail | Up to 8 cache loads + 2 generations | Generation; no in-flight cancel | Source and thumbnail/result |
| History / Favorites Cover | Each has its own 8 cache + 2 generation lanes | Generation and current demand; no in-flight cancel | Source or identity, resolved entry, thumbnail/result |
| Document / archive load | 1 per load request; no lane | Load request ID; progressive archive has cooperative token, ordinary load does not | Path, Document/archive resources and possible temp backing |
| Sibling / boundary lookup | 1 per lookup action; no lane | Dedicated request IDs (sibling also checks document generation); no cancel | Current path and result |
| External Cover preparation | 1 per selected file; no lane | Identity request ID; no worker cancel | Input path and prepared Cover data |
| Dropped path resolution / Favorite folder open | 1 per action; no lane | No independent generation on these result messages | Path or favorite identity and resolution result |
| Progressive Viewer hover thumbnail | At most 1 in-flight in a session; session reset can leave an older thread finishing | Session/preview generation; no cooperative worker cancel | Image source/bytes and optional temp-dir lifetime guard |
| Normal Viewer background scheduler | One persistent named `viewer-background` thread per scheduler, shared preload/near/hover/distributed lane | Document and thumbnail generations, replaceable demand and thumbnail cancel | Current command inputs, preload bytes, thumbnail worker/cache state |
| Smart crop preparation | One persistent named `viewer-smart-crop` thread per scheduler | Document/target checks and cooperative preparation checks | Queued bytes, decoded/prepared asset and shared state |

Viewer's two persistent workers are structurally different from App's per-request threads. The Viewer background scheduler prioritizes and replaces pending work in one lane, and smart crop checks cancellation while preparing; neither creates a fresh thread for each UI event. Their own command queues and per-step latency were not timed in this investigation. Progressive archive and hover paths are distinct App jobs and retain their separate lifecycle.

## Assessment and next measurement

| Scope | Current assessment |
| --- | --- |
| Library scan | Measurable duplicate work and stale messages, without demonstrated latest-start blockage or GUI-visible delay |
| SearchIndex build | Measurable duplicate work after root switches, without demonstrated latest-start blockage or GUI-visible delay |
| Search thumbnail generation | Latest demand clearly blocked by obsolete in-flight work in the measured heavy PNG case |
| Library / History / Favorites Covers, both lanes; Search cache lane | Controlled test confirms same-lane blocking while old work is active; real user-visible delay is unmeasured |
| Msg queue | Backlog confirmed only under a synthetic slower consumer; ordinary headless scan bursts did not build a large queue |
| Viewer dedicated schedulers | Per-request thread growth from Issue #76 does not apply; no runtime latency conclusion here |

Before a production fix, measure on a GUI session the distribution of `dispatch + sync_ui_state` time, peak `AppSender` depth/age during real navigation and visible Cover changes, and the time from latest visible demand to first worker start and thumbnail paint. The task explicitly prohibited starting the GUI in this turn. Whether the observed heavy PNG wait warrants cancellation or priority policy is a product decision; no performance threshold is established in current source of truth. The narrow candidates supported by this evidence are scope-local cooperative cancellation or obsolete in-flight replacement for thumbnail generation, plus queue depth/age instrumentation if GUI data shows backlog. A global thread pool or blindly bounded Msg queue is not supported by these measurements.

## Verification notes

The ignored measurement test passed. `cargo check`, direct rustfmt checks of both changed Rust files, and `git diff --check` passed. Full `cargo test -- --test-threads=1` reported 665 passed, 1 failed, 2 ignored: `viewer::render::tests::cpu_decode_keeps_crop_decisions_equal_to_the_previous_gtk_download_path` could not start the Gdk pixbuf loader through `bwrap`. The same exact test failed with the same loader error from a `/tmp` export of baseline commit `15da46ae`. `cargo fmt --check` reports only the existing `src/main.rs` formatting difference; it also fails identically on that baseline export. Neither failure is caused by this measurement harness.
