# PP-OCRv6 Windows extension measurements

2026-10-16, Windows 11 x64 build 26300, Ryzen 7 9850X3D (8C/16T), Rust 1.96.0
release, ort 2.0.0-rc.14 and source-built ONNX Runtime 1.31.0. Same **41 images /
2,294 normalized ground-truth characters**, unchanged prototype harness, 3 full
passes per tier; table entries are medians of the three run-level metrics.
Four intra-op / one inter-op threads, spinning off, below-normal/background
priority, batch 1, no orientation classifiers. Disk caches were not flushed.

## Final single-file engines

ORT is CPU-only, reduced operators, static /MT; Rust uses +crt-static. No minimal
build: the embedded ONNX files still load normally. All three PE dependency audits
contain only Windows OS DLLs: **no onnxruntime.dll, DirectML.dll, VCRUNTIME,
MSVCP or dynamic UCRT**. Each EXE embeds its own det/rec ONNX, YAML dictionary and
license notices; no extraction, model sidecar, DLL or separate VC++ runtime install.
The store/catalog downloads the raw EXE, not ZIP: **download bytes = EXE bytes**.
The updater/release cap is 256 MiB; medium is below it.

| Engine | CER | Median / p90 ms | Process-cold ms | Peak working set MiB | Peak private commit MiB | CPU s/pass | ONNX bytes | EXE = download bytes (MiB) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| tiny | 1.13% | 194.01 / 210.53 | 280.07 | 32.19 | 209.77 | 20.203 | 6,243,229 | 20,123,136 (19.19) |
| small | 0.52% | 430.83 / 456.70 | 582.77 | 32.19 | 318.16 | 46.141 | 31,039,890 | 45,014,528 (42.93) |
| medium | 0.31% | 1293.94 / 1385.04 | 1801.88 | 32.20 | 709.63 | 166.797 | 138,587,816 | 152,562,688 (145.50) |

For comparison, the unchanged prototype Windows.Media.Ocr results are **22.32%
CER, 14.48 ms median, 47.00 ms cold, 18.74 MiB peak private commit** (reused, not
remeasured). Windows used `zh-Hans-CN`; these PP-OCR helpers return `zh-Hans`.

Cold = spawn through Probe and first recognition. Private commit is the spawned
PID's lifetime `PeakPagefileUsage`, not on-disk paging; working set is lifetime
`PeakWorkingSetSize`, small here because background-mode Windows trims residency.
CPU = helper user + kernel time per 41-image pass. Synthetic horizontal text only;
whitespace-normalized CER does not measure indentation preservation or rotated text.
These are representative peaks, not a memory bound for arbitrary 40 MP images.

| Category | Tiny CER | Small CER | Medium CER |
|---|---:|---:|---:|
| UI (8) | 0.00% | 0.00% | 0.00% |
| Chinese/mixed documents (8) | 0.50% | 0.25% | 0.00% |
| Code/terminal (8) | 0.98% | 0.16% | 0.33% |
| URLs/emails/numbers (8) | 0.39% | 0.39% | 0.19% |
| Hard cases (8) | 3.85% | 1.92% | 0.96% |
| Supplied fixture (1) | 0.00% | 0.00% | 0.00% |

All 41 raw texts match the prior Rust prototype **exactly**, in every final pass.
Against the existing official PaddleOCR 3.7.0 / PaddleX 3.7.2 ONNX Runtime reference,
normalized divergence is tiny **5/2,287 (0.219%)**, small **3/2,292 (0.131%)**,
medium **0/2,294**; line counts match and every image is below 10% divergence.
All satisfy the <=1% aggregate / <=10% per-image parity gate. Ground-truth CER has
26 / 12 / 7 edits respectively; parity and accuracy are different measurements.

## Memory experiment (one complete pass per setting)

Both settings used the same reduced static ORT and borrowed embedded model bytes
(`commit_from_memory`, no extra Rust model Vec). ONNX Runtime still deserializes
and allocates internally; this is not a claim of fully zero-copy ONNX execution.

| Tier | Arena/pattern ON median ms / peak commit MiB | OFF median ms / peak commit MiB | Latency change |
|---|---:|---:|---:|
| tiny | 197.29 / 337.91 | 182.40 / 209.98 | -7.55% |
| small | 417.20 / 679.68 | 399.10 / 316.29 | -4.34% |
| medium | 1263.76 / 1312.34 | 1294.12 / 710.28 | +2.40% |

**Keep both OFF**: substantial commit reduction for all tiers, no >10% latency
regression, identical output. The temporary experiment feature was removed; final
binaries were rebuilt and then measured for all three passes above.

## Sources, failures, recommendation

Official PaddlePaddle Hugging Face ONNX revisions, SHA-256 and individual sizes are
pinned in [models.json](models.json); no model conversion. Dictionary/config bytes
are also pinned and embedded. ORT official tag `v1.31.0`, commit
`402979ac1bef89dcd55add41eaa7277c407599eb`; see [build_ort.py](build_ort.py) for exact
flags and [required_operators.config](required_operators.config). The first reduced
build omitted optimizer-generated Gelu; adding the observed level-3 fusion kernels
fixed loading. The retained reduced build passed the full parity gate.

- Tiny: `code04` reads `v0.1.0` as `ve.1.0` and `0 failed` as `e failed`;
  `hard07` damages the URL; `hard08` misreads blurred Chinese characters.
- Small: final `}` becomes a Chinese character in `code02`; `error log` becomes
  `error 1og` in `doc07`; `a?b=2` becomes `a7b=2` in `hard07`.
- Medium: `code07` turns braces into `Y`/`1`; `field07` confuses `l` with `I` in
  a serial; `hard08` still substitutes Chinese characters.

**Keep system OCR the default; recommend tiny for the first optional download**.
Small is a useful accuracy upgrade; medium buys only five fewer edits than small
at about 3x latency and >2x active commit. The 10-second idle exit frees memory,
but does not eliminate that peak or the next burst's cold-start cost.

## Verification and remaining limits

- Root fmt and locked all-target workspace clippy passed. Extension clippy/tests,
  all three release builds and Probe + fixture Recognize passed (Available / Done,
  EOF exits 0); the idle lifetime was checked separately.
- Root full workspace tests hit three unrelated environment-sensitive failures:
  `clipboard::read::tests::user_file_with_image_data_stays_files` (home resolution),
  `paths::tests::native_layout_matches_tauri_resolution` (local-data resolution),
  `win::autostart::tests::enable_writes_the_quoted_path_and_disable_removes_it`
  (access denied). With exactly those three excluded, the whole workspace passes,
  including both new core tests and the expanded updater dev-catalog test.
- Actual debug app selftest installed sibling EXEs as `0.0.0-dev`, selected tiny,
  recognized the fixture, then switched to small and medium. Each switch removed
  the previous tier, recognized one new image and preserved all existing OCR DB
  row values, including timestamps and attempt counts. All three app processes exited 0 with acceptance passed;
  both the CLI flag and `KWIKPASTE_SELFTEST=1` were set, using isolated portable data.
- **x64 only**. ARM64 recipe attempt stopped at the absent x64_arm64 MSVC toolset;
  the release matrix/register list excludes ARM64 and macOS for these three IDs.
- Workflow prepare/registration branches were exercised locally for all five IDs; no GitHub
  release/publication was run. No clean-machine/low-end-PC/ARM64 validation.

Rebuild/re-run instructions: [README.md](README.md); `measure.py` writes raw results to the ignored `results/`.
UI files/locales and the installed KwikPaste app were left unchanged. No commits,
tags, pushes or publishing.
