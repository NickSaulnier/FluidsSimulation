# Audio Analyzer Plan

Status: **approved; phases A to C done** (C with generated songs and recorded drums from
the Groove MIDI Dataset, see its notes). Decisions (§8) taken as recommended (2026-10-25):
- `rtrb` for the lock-free hand-over from the audio thread;
- `ebur128` for loudness;
- a second, longer FFT for the low bands;
- a few short public-domain or CC0 clips with hand-marked hits, committed as fixtures;
- phases A and B before MilkDrop phase C.

## 1. Goal

Make the audio analysis (`app/src/audio/`) robust, cheaper and more musical, and give
the visualizer what its next phases need:
- **Robust:** capture that never blocks the audio thread and works with every audio
  device's sample format.
- **More to draw with:** stereo, a fine spectrum and the raw waveform, which MilkDrop
  phase C's scopes and spectrum bars need.
- **Better hearing:** clearer bass, fewer false hits on sustained notes, loudness as
  people hear it.
- **Musical time:** tempo, beats and bars, which MilkDrop phase E needs for on-beat
  motion and bar-aligned transitions.
- **Measured:** tests on real music with hand-marked hits, not only synthetic beats.

This plan details the audio parts of the MilkDrop plan (§4.5 and its phase E) and adds
what phase C needs. Where they overlap, this plan is the more detailed one.

## 2. Where we are

One streaming pipeline (`analyzer.rs`) serves both sources: live capture of the
computer's output (`live.rs`, WASAPI loopback through cpal) and track playback
(`mod.rs`). Every 512 samples a Hann-windowed 2048-sample FFT gives the power spectrum,
from which:
- five display bands (sub, bass, low-mid, mid, high) and the overall level, as
  smoothed envelopes normalised by an automatic gain control (each over its own slowly
  decaying peak);
- kick, snare and hat hits, from the positive rise in log energy of three bands,
  picked against an adaptive threshold (mean plus a sensitivity times the deviation
  over the last second), a floor from the last 10 s, a minimum rise and gap, and a
  loudness gate;
- brightness (the spectral centroid on a log scale) and silence.

Tests cover synthetic beats, chunking (results don't depend on how audio arrives),
quiet and loud tracks, tones landing in their bands, a noise bed, and silence.

What works well: the single pipeline, the onset picker's guards (each was added for a
measured failure), the gain control, and chunk-independent results.

## 3. Problems

| # | Problem | Effect |
|---|---|---|
| 1 | Live capture hands audio over through a `Mutex<Vec>` | The audio thread can wait on the frame loop's lock: a risk of dropped audio |
| 2 | Capture accepts only `f32` and `i16` samples | Devices using 24/32-bit integers or `f64` fail to open |
| 3 | Capture mixes to mono at once | No stereo for a left-versus-right scope |
| 4 | Outputs are summaries only (5 bands, level, brightness, hits) | Nothing for spectrum bars or waveform rings |
| 5 | 2048-sample FFT: 23 Hz per bin at 48 kHz | The sub band spans about 2 bins, the kick band about 5, blurred by window leakage |
| 6 | Allocations every hop: the power spectrum; per candidate hit, a copy of recent values and a sort of 10 s of history | Wasted work, and pauses for the allocator on the frame thread |
| 7 | Hit strength is relative to the strongest hit in the last 10 s | One huge hit makes the next 10 s read weak |
| 8 | A complex FFT on real input | Twice the work it needs |
| 9 | Loudness is raw RMS | Level and silence don't follow perceived loudness (bass-heavy and bright mixes differ) |
| 10 | Hits come from one energy number per band | Sustained notes with wobble (bass lines, vibrato, vocals) can trigger hits |
| 11 | No tempo, beat or bar | Effects can't lock to the beat; transitions can't land on bar lines |
| 12 | Only synthetic test signals | Problems 5, 7 and 10 only show on real mixes |

## 4. Libraries

| Crate | Licence | Use |
|---|---|---|
| `rtrb` | MIT/Apache | Lock-free single-producer queue from the audio thread to the frame loop (problem 1) **(?)** |
| `realfft` | MIT | FFTs of real input, on top of `rustfft` (problem 8) |
| `ebur128` | MIT | EBU R128 loudness (K-weighted, momentary and short-term) for the level and silence (problem 9) **(?)** |

No new crate is needed for sample formats: cpal's `Sample` conversion handles them
all (problem 2).

Considered and not recommended:
- **aubio, BTrack** (GPL-3) and **Essentia** (AGPL-3, a large C++ library): strong onset
  and tempo detection, but their licences would bind the whole app.
- **bliss-audio**: whole-file analysis, offline, not live.
- **`spectrum-analyzer`**: convenience over what we already do.
- **`dasp`**: sample conversion, ring buffers, envelopes; a small gain over cpal's
  conversion and our own code.
- The existing Rust beat-detection crates are simpler than what we have.

Tempo and beat tracking is written here, following published methods (§5.6).

## 5. Design

### 5.1 Capture and hand-over

- The capture callback writes stereo frames into an `rtrb` ring (two seconds long),
  and never locks or allocates. The audio thread can't remove old frames from the
  ring, so after a stall the frame loop keeps only the newest second when it drains
  (dropping the oldest, as before); a stall longer than two seconds fills the ring, and
  new frames are dropped until it's drained.
- One generic callback over cpal's sample types converts every format to `f32`.
- Devices with more than two channels mix down to stereo (front left and right, plus
  the centre at -3 dB); mono devices duplicate their channel.
- Track playback feeds the same stereo frames, so both sources look alike from here on.

### 5.2 The analysis core

- **No allocations per hop:** the power spectrum, band sums and the picker's statistics
  live in buffers made once.
- **`realfft`** for the transforms.
- **Two resolutions:** the 2048-sample FFT every 512 samples for everything above
  150 Hz, and an 8192-sample FFT (about 6 Hz bins) every 1024 samples for the sub, bass
  and kick bands **(?)**. The long window adds latency to the low bands (its centre is
  85 ms back), so the kick detector keeps using the short FFT's onset timing and uses
  the long one for how much of the rise is really low.
- **A log-spaced filter bank:** about 64 triangular bands from 20 Hz to 16 kHz, computed
  once per hop. The five display bands, the onset bands and the spectrum output are all
  sums of these, so band edges are consistent and smooth instead of hard bin cut-offs.
- **Running statistics in the picker:** the threshold's mean and variance as running
  sums over the ring, constant time; strength relative to a decaying maximum (a few
  seconds' fall) instead of the 10 s maximum (problem 7).

### 5.3 New outputs

`AudioFeatures` gains, alongside what it has:
- `spectrum`: the 64 filter-bank bands, normalised by the gain control, with a
  smoothed version for drawing;
- a waveform view: the last 1024 frames of each channel (and the mono mix), read from
  a ring at draw time;
- stereo width and balance (from the left-right correlation and levels), for effects;
- `bass`, `mid`, `treb` in MilkDrop's sense (each over its own running average, so 1 is
  typical for this song) and their smoothed `_att` versions.

### 5.4 Loudness

`ebur128` gives momentary (400 ms) and short-term (3 s) K-weighted loudness in LUFS.
The level output follows the momentary loudness mapped through the gain control;
silence becomes "momentary loudness below -50 LUFS for half a second". Raw RMS stays
available.

### 5.5 Hit detection

- **SuperFlux** (Böck and Widmer, 2013) on the filter-bank spectrum: per band, the rise
  in log magnitude since a frame or two ago, measured against the maximum of the
  neighbouring bands in the earlier frame, which ignores pitch wobble. Summed over a
  hit kind's bands it gives that kind's onset function, then the existing picker.
- **Kinds by shape, not just band:** a kick is a low rise with little high energy, a
  snare a broad mid rise, a hat a high rise with little low energy, which separates
  hits that spill across bands.
- A general onset strength (all bands) feeds the tempo tracking.

### 5.6 Tempo, beats and bars

- **Tempo:** autocorrelation of the onset strength over the last 8 seconds, weighted
  toward 120 BPM by a log-normal prior (as in Ellis 2007 and librosa), searched over
  60–200 BPM, with octave checks (half and double tempo compared by their own scores).
  Updated every half second, smoothed, with a confidence.
- **Beats:** a phase-locked clock at that tempo, nudged toward onsets that fall near its
  predicted beats. It keeps time through quiet passages and drops out (confidence falls)
  when the music stops having a beat. It gives `beat` (a pulse each beat),
  `beat_phase` (0 to 1 between beats) and the next beat's time.
- **Bars:** the downbeat as the beat with the strongest low-frequency onsets over the
  last few bars, assuming four beats to a bar; `bar_phase` from it.
- **Latency offset:** a setting (milliseconds) that shifts the analysis to match what
  is heard, for systems whose loopback runs ahead of or behind the speakers, with a
  calibration screen that flashes on detected beats.

### 5.7 Measuring

- **Real-music fixtures:** a handful of short clips (10–20 s each, public domain or
  CC0: electronic, rock, acoustic, quiet ambient) with hand-marked hits and beats,
  stored in the repository.
- **A scoring harness:** precision, recall and F-measure per hit kind with a 50 ms
  tolerance (the usual measure, as in mir_eval); tempo within 4% (octave errors counted
  separately); beat F-measure within 70 ms. A test asserts each fixture's scores don't
  fall below a recorded baseline, and prints them so improvements are visible.
- The existing synthetic tests stay, extended to stereo and the new outputs.

## 6. Phases

Each phase leaves the app working and is committed on its own.

| Phase | Scope | Result |
|---|---|---|
| **A. Capture** | `rtrb` hand-over; every sample format; stereo frames from both sources. | No lock on the audio thread; any device opens; stereo available. |
| **B. Core and outputs** | No per-hop allocations; `realfft`; log filter bank; running picker statistics and decaying strength scale; spectrum, waveform and stereo outputs; MilkDrop bass/mid/treb. | Everything MilkDrop phase C needs; cheaper analysis. |
| **C. Measuring** | Real-music fixtures with marked hits; the scoring harness and baseline test. | Changes to detection can be judged by numbers, starting from today's. |
| **D. Hearing** | Long low FFT; SuperFlux; hit kinds by shape; EBU R128 loudness and silence. | Clearer kicks, fewer false hits, loudness as heard; scores up against phase C's baseline. |
| **E. Musical time** | Tempo, beat clock, bars; latency offset and calibration. | Beat and bar outputs for MilkDrop phase E. |

A and B come before MilkDrop phase C; C before D, so D's changes can be measured; E
before or with MilkDrop phase E.

## 7. Testing

- Capture: the generic conversion for each sample format; downmixing of 1, 2 and 6
  channels; a full ring drops the oldest audio.
- Core: the filter bank's bands tile the range and sum to the total energy; results
  unchanged by chunking (stereo too); no allocation per hop (a counting allocator in a
  test); the running statistics match a direct computation.
- Outputs: a left-only tone shows in the balance; the waveform view returns the last
  frames in order; MilkDrop's `bass` is about 1 on a steady signal and rises on a
  louder passage.
- Hearing: on the fixtures, hit F-measures at or above the baseline; a sustained bass
  line with vibrato fires no kicks; loudness of a calibrated tone matches EBU R128's
  reference values.
- Musical time: synthetic beats at 70–180 BPM, with swing, dropped beats and a tempo
  change: tempo within 2%, beat phase within 30 ms; on the fixtures, beat F-measure at
  or above baseline.
- The 30-minute soak keeps running on synthetic music, now with the new outputs.

## 8. Decisions for you

1. **Lock-free queue:** adopt `rtrb` (recommended: small, focused on exactly this,
   MIT/Apache), or write a minimal single-producer ring ourselves (no dependency, but
   lock-free code is easy to get subtly wrong).
2. **Loudness:** adopt `ebur128` for loudness as people hear it (recommended), or keep
   raw RMS with a simple bass-cut filter.
3. **Low-frequency resolution:** a second, longer FFT for the low bands (recommended:
   simple, clearly better bass), or a constant-Q transform (more elegant, more code and
   cost).
4. **Real-music fixtures:** commit a few short public-domain or CC0 clips with
   hand-marked hits (recommended), or keep to synthetic signals only.
5. **Order:** phases A and B before MilkDrop phase C (recommended, since C needs B's
   outputs), or MilkDrop phase C first with the outputs added as needed.

## 9. Progress

- **Phase A (done):** capture (`app/src/audio/live.rs`).
  - The capture callback converts samples, mixes to stereo and pushes frames into an
    `rtrb` ring, without locking or allocating; the frame loop drains it, keeping at
    most the newest second (see §5.1 for what happens on long stalls).
  - Every cpal sample format opens: 8-, 16-, 24-, 32- and 64-bit integers, signed and
    unsigned, and 32- and 64-bit floats, through one generic callback and cpal's sample
    conversion. Only DSD bitstreams are refused (they aren't samples).
  - Channels mix to stereo: mono to both sides; centre and surrounds at -3 dB, the LFE
    at -6 dB (kept for the kick detector). Track playback produces the same stereo
    frames, so both sources look alike from there; the analyzer still takes their mono
    mix until phase B gives it stereo outputs.
  - Tests: every format lands on -1..1 (unsigned centred on its midpoint); mono, stereo
    and 5.1 mix down as described; a full ring drops frames instead of waiting, and a
    drain after a stall keeps the newest.
  - Checked in the app: `--live` opens the headphones at 48 kHz, 2 channels, `f32`, with
    no warnings. Audio flowing through it wasn't checked (nothing was playing).
- **Phase B (done):** the core and outputs (`app/src/audio/analyzer.rs`).
  - The analyzer takes stereo frames (`process`); its mono mix feeds the FFT. A mono
    ring replaces the per-sample queue, `realfft` replaces the complex FFT (and the
    `rustfft` dependency), and every buffer is made once: after warming up, analysing
    allocates nothing (a test counts allocations on its thread with a test-only
    allocator). Onsets come out through `drain_onsets`, into the caller's buffer.
  - The onset picker keeps its statistics as it goes: running sums for the threshold
    window, a 256-bin histogram for the 10 s floor's 98th percentile (accurate to a
    bin, 0.03 in log units), and a maximum falling with a 3 s time constant for the
    strength scale, so one huge hit no longer makes the next 10 s read weak.
  - New outputs in `AudioFeatures`: `spectrum` and `spectrum_smooth` (64 log-spaced
    triangular bands, 20 Hz to 16 kHz, tilted +3 dB per octave, shown over the 60 dB
    below their shared recent peak); `balance` and `width` (from the channels' levels and
    correlation); `milk` (bass, mid and treble over 3 s running averages, capped at 4,
    and their 0.25 s smoothed `_att` versions). `waveform` returns the last 1024 stereo
    frames.
  - Changed from §5.2: the five display bands and the onset bands still sum FFT bins
    directly, not the filter bank. At 23 Hz per bin the filter bank's lowest bands are
    narrower than a bin (they interpolate between the two nearest), so summing them
    would distort the kick band's energy and with it the loudness gate. They're unified
    in phase D, with the long low FFT and the scoring harness to measure the change.
  - The Music tab shows the new outputs under the band meters: the spectrum as bars, a
    two-trace scope (left and right), stereo balance and width, and MilkDrop's levels.
  - Tests: the spectrum's bands tile the range (weights sum to 1 from 500 Hz up); tones
    peak in their own band (80 Hz, 1 kHz, 6 kHz); the waveform is the latest frames in
    order; left-only audio reads left, identical channels read mono, opposite ones wide;
    MilkDrop's bass is about 1 when steady and over 2 when it gets louder; the running
    statistics match a direct computation; analysing allocates nothing. The existing
    tests pass unchanged, including identical onsets for any chunking.
  - Checked in the app with a generated stereo track (a beat, plus a tone on the right
    only), muted: the spectrum spikes at the tone, the scope shows it on the right
    trace only, balance reads +0.57, MilkDrop's levels sit near 1, and hits count.
- **Phase C (done):** measuring (`app/src/audio/eval.rs`,
  `songs.rs`, `tests/fixtures/audio/`).
  - The harness scores hit detection per kind with a 50 ms tolerance (each hit matched
    at most once; precision, recall, F-measure; the first 1.5 s not scored), feeding
    the audio in frame-loop-sized chunks. The test `hit_detection_holds_its_baseline`
    prints the table and fails if any score falls more than 0.02 below
    `tests/fixtures/audio/baseline.txt`; `UPDATE_ANALYZER_BASELINE=1` records new
    scores.
  - Changed from decision 4: the hits aren't hand-marked, and the clips are drums
    alone, not full music. Five generated songs (`songs.rs`)
    mix drum kits, bass lines, pads and leads in stereo, with exact hit times: house
    (124 BPM), a swung breakbeat with ghost notes and a gliding bass with vibrato
    (92), distorted rock (140), ambient with hardly any drums (70), and a loud hit
    followed by quiet bars (110). Beside them, six 20 s clips of a real drummer from
    the Groove MIDI Dataset (CC BY 4.0, attributed in the fixtures README): funk 138,
    soul 102, hip-hop 87, rock 135, samba 116, breakbeat 170. They were played on an
    electronic kit, so the MIDI it recorded gives the hit times (aligned within a few
    ms; checked against the audio). Kick is note 36, snare 38/40/37, hat the hi-hats,
    ride and crashes; toms are left out. The clips were cut from the dataset's zip with
    range requests (about 37 MB of its 4.8 GB), trimmed and mixed to mono (1.7 MB each).
    The STAR Drums preview (full songs with exact annotations) was downloaded too but
    none of its four songs may be used: all are CC BY-NC-SA.
  - Today's baseline (F-measure: kick, snare, hat):

    | Song | Kick | Snare | Hat |
    |---|---|---|---|
    | house 124 | 1.00 | 0.56 | 0.67 |
    | breakbeat 92 | 0.96 | 0.78 | 0.90 |
    | rock 140 | 0.48 | 0.57 | 0.91 |
    | ambient 70 | 0.00 | 0.00 | 1.00 |
    | dynamics 110 | 1.00 | 1.00 | 0.42 |
    | Groove funk 138 | 0.82 | 0.53 | 0.70 |
    | Groove soul 102 | 0.78 | 0.42 | 0.99 |
    | Groove hip-hop 87 | 0.99 | 0.28 | 0.67 |
    | Groove rock 135 | 0.85 | 0.38 | 0.78 |
    | Groove samba 116 | 0.96 | 0.49 | 0.92 |
    | Groove breakbeat 170 | 0.82 | 0.62 | 0.55 |

    The weak spots: snares are heard where there are none (house 49 found for 19,
    rock 50 for 20, ambient 16 for none), from kick clicks, hats and the lead's attacks
    reaching the snare band; rock's kicks are unreliable (precision 0.36, recall 0.73)
    with the distorted bass and chords in the same band; quiet hats after a loud hit
    are lost (dynamics). These are phase D's targets. (The ambient zeros are false hits
    where there were none to find: 2 kicks and 16 snares.) The recorded drums agree:
    snares are the weakest everywhere, mostly false hits (hip-hop 64 found for 14, rock
    92 for 30); quick hats are under-counted (breakbeat finds 84 of 155, hip-hop 52 of
    91); kicks hold up (0.78 to 0.99).
  - Tests: scoring matches each hit once within the tolerance; hit lists parse; the
    generated songs are well formed; the baseline holds.
