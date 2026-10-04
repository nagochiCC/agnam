# Issue 88 Slider thumbnail disk cache measurement

`cargo test thumbnail::disk_cache::tests::measure_jpeg_png_webp_first_generation_write_and_second_hit -- --ignored --nocapture`

Fixture: deterministic 900×1300 RGB gradient, one file per format. Miss measures source metadata and absent cache lookup; first generation measures source read plus existing thumbnail decode/resize; write measures raw RGB serialization, checksum, atomic write and file sync; second load measures cache file read, checksum and RGB reconstruction. Timings below are one local run and are not performance thresholds.

| Source | Miss lookup | First generation | Persist write | Second cache hit | Cache bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| JPEG | 0.18 ms | 22.27 ms | 38.62 ms | 0.67 ms | 83,114 |
| PNG | 0.02 ms | 117.20 ms | 3.80 ms | 0.62 ms | 83,114 |
| WebP | 0.02 ms | 136.42 ms | 3.79 ms | 0.55 ms | 83,116 |

Each first load reads the source once and decodes once; each second load reads one cache file and does zero source reads or image decodes. The write runs in a bounded single background writer after the generated result is emitted, so it does not delay that result. A separate deterministic ZIP/non-solid 7z test checks that the archive reader is not opened on a cache hit.

Raw RGB was selected over PNG encoding: thumbnail pixels already have this form, so writes avoid a second image encode and hits need no image codec. A single versioned file holds both Spread halves, its exact key and a checksum; an incomplete or corrupt file is a miss. The 1 GiB soft limit bounds the larger on-disk representation.
