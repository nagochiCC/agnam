# Issue #81: archive storage / resource cost

## Scope and reproduction

The current design in [`../design/viewer.md`](../design/viewer.md) remains authoritative. Sequential memory backing still spills only when the next image crosses 256 MiB; this is a backing threshold, not a resource limit. The 64 MiB single-entry, 512 MiB cumulative read, 512 MiB cumulative temporary write and occupancy, and 4,096 entry/image limits remain unchanged.

Measurements below used `cargo test`'s unoptimized test profile on Darwin 23.1.0 arm64. The test fixtures use in-memory readers and a temporary directory on the same APFS data volume as the checkout. Each time is the median of five runs unless stated otherwise. Files are not synced to durable storage; page cache, filesystem metadata cache, allocator state, and test ordering can move the numbers. These are relative measurements of bounded I/O and storage bookkeeping, not archive codec or end-to-end Viewer latency. Reproduce the final implementation with:

```sh
cargo test archive::resource::tests::measure_resource_cost -- --ignored --nocapture
cargo test archive::formats::tests::measure_sequential_spill -- --ignored --nocapture
```

## Sequential spill

`SequentialImageStorage` was called directly, avoiding archive decode. An injected 16 MiB threshold reproduces the 256 MiB algorithm with less memory and disk use. Input buffers contain deterministic pseudo-random bytes, prepared before timing. The first `count` images fill the threshold exactly; the next image crosses it. `before` is the aggregate Memory insertion time, `crossing` includes temp directory creation, rewriting all retained images, writing the crossing image, and switching backings, and `after` is one File-backed addition. The temporary phase timers were removed after this investigation.

| Fixture | Images before crossing | Image size | Before | Crossing / total spill | Existing rewrite | New image write | Later image write | Bytes written at crossing | Retained bytes before → after |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Few large | 2 | 8 MiB | 0.019 ms | 3.880 ms | 2.074 ms | 1.230 ms | 1.420 ms | 24 MiB | 16 MiB → 0 |
| Many small | 256 | 64 KiB | 0.232 ms | 21.785 ms | 21.036 ms | 0.068 ms | 0.091 ms | 16 MiB + 64 KiB | 16 MiB → 0 |

The backing is Memory before crossing and File from crossing onward, including existing progressive references. The phase medians were: root/temp creation 0.062/0.071 ms, backing replacement 0.450/0.505 ms for few/many. Constructing all existing image paths alone took 0.002/0.042 ms. ResourceBudget's path accounting measured approximately 1.7 µs per tracked file in a separate in-memory microbenchmark, suggesting under 0.5 ms for 256 images; the existing rewrite phase combines path construction, accounting, file creation, and byte writes. The many-image crossing is dominated by creating and writing many files. The full crossing times and independent phase medians need not sum exactly.

`glib::Bytes::from_owned(Vec<u8>)` kept the Vec buffer pointer in a deterministic test, and cloning the Memory `ImageSource` kept the same pointer. Spill writes retained bytes once to files; it does not make another large byte-buffer copy. The fixture is smaller than 256 MiB, and warm-cache APFS writes do not establish cold physical-disk latency. Changes that spill before crossing, use metadata to choose File backing immediately, or change the threshold are separate specification proposals.

## Bounded I/O

The same 64 MiB-per-entry and cumulative checks were used for 8, 32, 64, and 128 KiB stack buffers. `read_all` reads a memory slice, `copy_to_path` writes a temporary file, and `drain` discards bytes. Each includes the final EOF read call. The 64 MiB comparison before the allocation experiment was:

| Chunk | Read calls | `read_all` | `copy_to_path` | `drain` |
| ---: | ---: | ---: | ---: | ---: |
| 8 KiB | 8,193 | 9.785 ms | 52.728 ms | 1.438 ms |
| 32 KiB | 2,049 | 8.068 ms | 24.584 ms | 1.159 ms |
| 64 KiB | 1,025 | 8.139 ms | 16.125 ms | 1.103 ms |
| 128 KiB | 513 | 8.480 ms | 16.422 ms | 1.324 ms |

64 KiB gave a clear `copy_to_path` gain; 128 KiB gave no stable further gain and doubles the stack buffer. The final code uses 64 KiB. The following before/after runs used the unchanged `Vec::new()` allocation policy. Times are milliseconds; `read_all`, `copy_to_path`, and `drain` all use the same deterministic call count for a given input size.

| Input | Calls 8 → 64 KiB | `read_all` 8 → 64 KiB | `copy_to_path` 8 → 64 KiB | `drain` 8 → 64 KiB |
| ---: | ---: | ---: | ---: | ---: |
| 64 KiB | 9 → 2 | 0.003 → 0.005 | 0.108 → 0.081 | 0.002 → 0.003 |
| 1 MiB | 129 → 17 | 0.068 → 0.079 | 1.079 → 0.521 | 0.030 → 0.021 |
| 16 MiB | 2,049 → 257 | 2.809 → 1.826 | 14.021 → 5.328 | 0.368 → 0.268 |
| 64 MiB | 8,193 → 1,025 | 9.785 → 8.292 | 52.728 → 16.385 | 1.438 → 1.080 |

For 16/64 MiB, the corresponding in-memory/file-cache throughputs were approximately 5,696/6,541 → 8,762/7,718 MiB/s for `read_all`, 1,141/1,214 → 3,003/3,906 MiB/s for `copy_to_path`, and 43,478/44,506 → 59,701/59,259 MiB/s for `drain`. Small-input times are at microsecond scale and do not support a meaningful throughput claim. The strongest result is the reduced loop/read/write-accounting count for `copy_to_path`, not a claim about durable disk throughput.

`read_all` reservation was tested with metadata as a capacity hint only, while actual reads kept the same bounded checks. An unpaired 1 MiB run suggested 45 vs 77 µs with and without a 1 MiB reservation. A later alternating-order comparison (10 runs per side) gave 40 vs 40 µs. Larger entries showed no reliable benefit from a partial reservation. Reservation was not adopted, avoiding an extra allocation based on potentially overstated metadata. No metadata value is treated as authority for actual byte limits.

## Entry and temporary tracking

The ignored resource test also times entry registration, repeat registration, false→true image upgrade, path cloning, and map lookup. Times below are aggregate medians in milliseconds for the entire count; `1` is below useful timer resolution.

| Entries | Insert | Repeat | Upgrade | Path clone alone | Lookup alone |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | <0.001 | <0.001 | 0.001 | <0.001 | <0.001 |
| 100 | 0.164 | 0.058 | 0.110 | 0.001 | 0.052 |
| 1,000 | 2.017 | 0.576 | 1.099 | 0.018 | 0.527 |
| 4,096 | 8.204 | 2.368 | 4.512 | 0.077 | 2.155 |

| Tracked temp files | Initial `write_temp` | Overwrite | `release_temp_tree` | Path clone alone | Map update alone | In-memory accounting alone |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 0.039 | 0.061 | <0.001 | <0.001 | 0.001 | 0.001 |
| 100 | 4.009 | 3.623 | 0.033 | 0.001 | 0.089 | 0.175 |
| 1,000 | 40.540 | 36.169 | 0.340 | 0.017 | 0.877 | 1.733 |
| 4,096 | 168.180 | 156.768 | 1.423 | 0.084 | 3.709 | 7.304 |

Initial/overwrite temp timings include thousands of filesystem file operations; the isolated columns exclude filesystem I/O. At the 4,096-entry cap, a full temp-tree scan is about 1.4 ms. Path clones are a small share. Replacing `(PathBuf, index)` with an archive ID would require preserving separate identities for the same index in nested archives and changing wider state/API ownership. There is no measured reason to take that architectural change here.

## Outcome and integration

The production change is the bounded I/O chunk increase from 8 to 64 KiB. `copy_to_path`'s loop was extracted into a private writer helper so its writer-error propagation can be tested. Every read still asks for at most the remaining allowance plus one byte, checks the single-entry limit first and cumulative limit second, and refuses the extra byte before accounting or writing it. Exact limits remain accepted. The larger stack buffer adds 56 KiB per active bounded I/O call; `read_all`'s heap allocation behavior is unchanged.

RAR and solid 7z still use Sequential storage and progressive backing updates; TAR and LHA still use Sequential storage without progressive notification. ZIP and non-solid 7z remain lazy Random Access. The shared bounded operations are reached by lazy reads, Cover reads, archive contents extraction/thumbnail, nested extraction, and solid 7z's non-image drain. The nested recursion limit, safe path checks, operation-shared budget, typed resource errors, and temp cleanup were not changed. Archive integration tests passed across these paths. RAR's metadata-preflight/actual-length defense remains as documented in the current design; this change does not add mid-decode enforcement inside the unrar API.

The remaining spill bottleneck is byte and file I/O at crossing, particularly many small files. Early temp backing and staged spill could reduce the crossing pause, but both change the specified backing policy and progressive timing. They require a separate design decision and cold-disk/real-archive evaluation. No GUI was started because production behavior and progressive timing policy did not change.
