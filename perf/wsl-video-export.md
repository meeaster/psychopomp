# WSL video export performance

The retained alpha-blend fast path reduces CPU-heavy window entrance time without
changing the rendered pixels. It does not deliver the proposed one-to-three-minute
deck improvement. Repeated pairs support about 15 seconds of savings across the
three main window entrances in the OpenCode deck. This is a sum of measured clip
savings, not a repeated full-deck result.

## Comparison conditions

The base renderer is commit `65dd4e606237b18adae13b678838ed263c329657`, with timing
instrumentation but no rendering optimization. The candidate adds the fast path
in `render/text/raster.rs`. Both binaries use the release profile, the same Scene
Plans, 1920×1080 at 60 fps, and unchanged shutter sampling. Stage exposures use 24
samples. Window entrances use the existing 16-sample and 8-sample schedule.

The machine runs WSL with Mesa Dozen 26.2.4 on an NVIDIA Quadro T2000. Encoding
uses the existing libx264 settings. Rayon keeps its default 12 threads. Timing
reports are off for the full-deck comparison and its confirmation runs. Renders
run sequentially, with the order of baseline and candidate alternating.

## Retained change

`blend_pixel` copies straight RGB when an opaque source replaces its destination
or when a source paints a transparent destination. It computes the weighted alpha
as before. Other blends retain the original arithmetic, including very small
alpha values below the fast path's guard.

A 0.75-second `start` entrance window took a median 23.9 seconds before the change
and 21.2 seconds after it in three pairs, an 11.3% reduction in reported export
time. The complete entrance confirmation runs gave these results:

| Clip | Measured pairs | Median paired wall-time saving | Median paired reduction |
| --- | ---: | ---: | ---: |
| `session-intro` | 3 | 4.70 s | 11.6% |
| `reveal-intro` | 4 | 6.83 s | 13.8% |
| `start-intro` | 3 | 4.24 s | 9.2% |

The pairs include each clip's full-deck comparison and its subsequent confirmation
runs. Machine activity affects absolute time. For example, one `reveal` baseline
took 78.01 seconds, while its later baselines took 48.10 to 49.66 seconds. The paired
medians limit the influence of that run. They do not establish an exact saving
for every future deck export.

## Full-deck check

All 14 clips rendered once per binary, alternating order per clip. Summing process
wall times gave 634.99 seconds for the baseline and 574.35 seconds for the candidate.
Reported export times summed to 612.7 and 552.7 seconds. Build time, the deck
script's progress polling, and share-folder copies are outside these measurements.

The apparent 60.64-second wall-time saving overstates the repeatable gain. An
`extend-loop` baseline took 98.82 seconds in that run, but later baseline and
candidate pairs both took about 74 seconds. The `reveal` baseline also ran unusually
slowly. No reliable Stage gain remains after confirmation.

## Rejected experiments

Each short Stage comparison used three pairs on `title`, `gateway`, and `extend`.
All changes below were removed:

| Experiment | Observation |
| --- | --- |
| One command submission per Stage exposure using a staging belt | Median export time rose 1.4% to 6.4%. Less submission time moved into the final GPU wait. |
| Reject distant polyline segments by bounds | Mixed results, including a 3.2% regression on `extend`. |
| Compare squared polyline distances before one square root | Median differences stayed within 0.7%, with no consistent gain. |
| Skip exposure lookup tables that map bytes to themselves | No added gain on Stage or the alpha-optimized terminal window. |
| Special-case axis-aligned card unprojection | No added gain beyond alpha blending. |
| Parallelize exposure blocks with Rayon | No repeatable added gain and higher system CPU time. |

Earlier matching-window tests also found no wall-time improvement from NVENC or
fewer Rayon threads. Encoder handoff was not the main constraint.

## GPU attribution

DXGI adapter descriptions mapped Windows GPU-counter LUIDs to the physical cards.
During an eight-second Stage window, the Quadro's 3D engine attributed to the WSL
VM worker had a 71.1% median among active samples and a 72.9% peak. Intel UHD
activity attributed to Windows Terminal stayed near 2%, comparable with idle.
The CPU-heavy terminal entrance showed no sustained Quadro 3D activity.

The earlier reported 70% Intel spike did not recur. These measurements establish
that the controlled Stage export used the Quadro. They do not identify the cause
of that earlier spike.

## Verification

- All 6,738 decoded video frames across the 14 baseline and candidate clip pairs
  have identical SHA-256 hashes. ffprobe reports matching frame counts, durations,
  1920×1080 dimensions, and 60 fps.
- `verify compare` reports identical plans and all 240 available frames across 31
  manifest scenes. `high-council` and `shape-of-openness` compare plans only because
  their generated image inputs are absent.
- The alpha invariant test covers every source channel byte, every nonzero source
  alpha byte, several opacity weights, and opaque replacement over every destination
  alpha byte.
- Workspace tests, formatting, and Clippy pass. A full-scale entrance frame was
  inspected for text, border, shadow, and compositing defects.
- The existing ignored terminal and caption pixel tests pass with lavapipe. The
  terminal test crashes during Dozen adapter initialization, before the `GPU:`
  log line, in both dev and release test builds. Disabling wgpu debug and
  validation flags did not change that result. The cause is undiagnosed. Dozen
  CLI exports and verification frames completed successfully.

Raw benchmark scripts, logs, results, GPU counters, and videos are in
`/tmp/opencode/psychopomp-optimize-ses_eceae7ed1ffeGXrVyuAFR4FKon/`. This is temporary
machine-local evidence, not a portable benchmark fixture. Existing presentation
clips were not overwritten.
