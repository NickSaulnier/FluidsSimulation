# Music Visualizer Plan

Status: **approved; Phases A–C done** (see §9). Decisions (§8):
- live system audio first, track files second;
- an automatic director, with a lock;
- `rustfft` for the FFT;
- no "now playing" (GSMTC dropped);
- Windows only;
- phases in order, A→G.

## 1. Goal

Turn the fluid toy into a music visualizer. You play music (Spotify, a file, anything)
and the fluids move, glow and change colour with it, full screen, hands-off, for as
long as the music lasts. The toy stays available: the visualizer is a mode, not a
replacement.

What "good" means here:

- **It reads as the music.** Kicks hit, bass swells, hi-hats sparkle, quiet passages
  calm down, drops explode. You can tell what's playing from the picture.
- **Hands-off.** It starts listening, picks a look, and runs. No panel, no cursor, no
  tuning per song: quiet and loud tracks both work.
- **Runs for hours.** Nothing fills up, drains away, burns out or settles into stillness.
- **Looks finished.** It's full screen at a steady 60 fps, with glow and colour that
  move with the music.

## 2. What already exists

| Piece         | State                                                                                                                  | Gaps for a visualizer                                                                                                    |
| ------------- | ---------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| Audio sources | Track files (decoded, played by the app) and live loopback capture of the system output (Windows).                     | Live captures _all_ system audio; Windows only.                                                                          |
| Analysis      | Kick detection only: offline for tracks, streaming for live.                                                           | No continuous features (loudness, bass/mid/high energy, brightness), no tempo or beat phase, no onsets outside the bass. |
| Reactions     | Kicks fire a 90 ms pulse in one of 3 fixed lanes: Splash (push up) or Jet (emitter). Selectivity and strength sliders. | Two reactions, one input, fixed placement; nothing continuous; colour and parameters never react.                        |
| Visuals       | 7 solvers incl. liquid FLIP, dye (stable, LBM), smoke & fire; 5 colour palettes; liquid surface shader.                | No glow or bloom, no trails, static colours, no full screen.                                                             |
| UI            | Panel, top/status bars, tool strip, settings saved.                                                                    | Always on screen; no presentation mode.                                                                                  |
| Long runs     | Never tested past a few minutes.                                                                                       | Jets fill tanks to the particle cap; fires burn out; FLIP settles flat; no soak test.                                    |

## 3. Architecture

Three new layers in the `app` crate. The `sim` crate stays audio-agnostic, as now:
reactions only produce what the app already feeds it (`StepInput` brushes and emitters,
gravity, parameter values by key, and the palette).

```
 audio source ──► Analyzer ──► AudioFeatures ──► Reactions ──► StepInput, gravity,
 (track / live)   (FFT, bands,   (per frame)     (per preset)   params, palette
                   onsets, beat)                       ▲
                                     Director ─────────┘  (picks presets, transitions)
```

### 3.1 Analyzer → `AudioFeatures`

One streaming pipeline for both sources. Track mode feeds the decoded samples at the
playback position into the same analyzer as live mode, so the offline kick path can
retire and both sources behave identically. Per frame it produces:

- **Loudness:** RMS, smoothed attack/release envelopes, and an automatic gain control
  (AGC) that normalises to the last ~20 s, so a quiet track drives as hard as a loud
  one.
- **Bands:** energy in 5 bands (sub, bass, low-mid, mid, high) from an FFT (2048-point,
  ~50% overlap), each normalised by its own AGC.
- **Onsets per band:** the existing kick detector generalised: bass onset = kick,
  mid = snare and claps, high = hi-hats. Each comes with a strength.
- **Beat grid:** a tempo estimate (onset autocorrelation) and a beat phase 0..1, so
  reactions can land _on_ the beat, plus a bar counter for phrase-level changes.
- **Character:** spectral centroid (brightness), energy trend (build-up, drop
  detection: a sharp rise after a sustained build), and a silence flag.

### 3.2 Reactions

A visualizer preset holds a list of reactions. Each maps one feature to one effect:

- a **source**: a band envelope, an onset, the beat phase or a trend;
- a **response curve**: threshold, gain, smoothing;
- a **placement**: lanes, random, centre, mirrored, or circling.

Reaction types:

| Reaction      | Effect                                                                       | Typical source         |
| ------------- | ---------------------------------------------------------------------------- | ---------------------- |
| Splash        | Upward push (exists)                                                         | Kick onset             |
| Jet           | Emitter burst (exists)                                                       | Kick onset             |
| Vortex        | A rotating brush: stirs a swirl                                              | Snare onset            |
| Gust          | Sideways wind across the domain                                              | Mid envelope           |
| Slosh         | Gravity tilt oscillating with the beat                                       | Beat phase × bass      |
| Pulse gravity | Gravity strength breathing with loudness                                     | Loudness               |
| Ignite        | Fire burst / fuel puff (fire solver)                                         | Kick or drop           |
| Colour        | Palette hue rotation, brightness flash, palette switch                       | Centroid, onsets, drop |
| Param         | Modulate a solver parameter by key (e.g. vorticity ← highs, buoyancy ← bass) | Any envelope           |

New sim-side work is small:

- a rotational brush (vortex) in `ForceBrush`, or a `SwirlBrush` in `StepInput`;
- a general "set parameter by key" helper (the bench's `--set` already does this
  through `ParamVisitor`).

### 3.3 Visualizer presets and the director

A **visualizer preset** bundles a solver, a scene, a palette, look settings and its
reactions. A starter library of about 6, each tuned by hand:

| Preset        | Solver / scene                 | Character                                                                    |
| ------------- | ------------------------------ | ---------------------------------------------------------------------------- |
| Ocean pulse   | FLIP, liquid look, tank        | Kicks splash, bass sloshes, hi-hats spray.                                   |
| Ink storm     | Stable fluids, dye             | Snares stir vortices, highs add swirl detail, colour follows brightness.     |
| Firestorm     | Smoke & fire, campfire         | Kicks puff fuel, drops ignite, smoke drifts on mids.                         |
| Wind tunnel   | LBM, wind tunnel               | Inlet speed follows loudness, kicks shed vortices, stripes recolour on bars. |
| Lava lamp     | Stable fluids, heavy/light dye | Slow: bass drives buoyancy; for ambient music.                               |
| Particle rain | PBF or FLIP, discs             | Kicks drop bursts from the ceiling.                                          |

The **director** chooses and changes presets:

- by energy (ambient vs dance);
- on phrase boundaries (every N bars), on detected drops, or on song changes (long
  silence or a big shift in character);
- with transitions: fade through dark, or a dye/smoke wipe, since solvers can't
  crossfade directly.

The user can lock a preset or let it roam **(?)**.

## 4. Presentation mode

- **Entering and leaving:** full screen (borderless) via F11 or `--visualizer`. All UI
  hides: bars, panel, tool strip and cursor. Moving the mouse shows a small translucent
  overlay with preset, next/previous, lock, and exit; it fades after 3 s.
- **Steady frame rate:** a frame-time budget (sim under ~8 ms at 1080p). If the sim
  can't keep up it drops resolution rather than going into slow motion, which would
  desync from the music.
- **Multi-monitor:** choose the display; a remembered setting.

## 5. Making it run for hours

- **Volume balance:** jets and emitters are paired with drains or a particle budget.
  When fluid exceeds a target, the director drains; when it's low, it refills between
  phrases.
- **Fire:** fuel replenishes on bass, so the fire never burns out; a smouldering base
  stays lit.
- **Stagnation guard:** if nothing has moved for a while (silence), drift gently.
  After long silence, settle to a calm idle look.
- **Soak test:** the bench gains a `--music <file>` mode that runs the analyzer and a
  preset for 30+ simulated minutes, with invariants and volume/fuel bounds checked
  throughout. It runs on synthetic beats in CI-style tests and on real tracks by hand.

## 6. Visual polish

- **Bloom/glow:** an HDR offscreen target and a downsampled blur, added back. Flames,
  foam, fast dye and beat flashes glow. This is the single biggest step toward
  looking like a visualizer.
- **Trails:** an optional feedback buffer (previous frame faded and composited), for
  ink and particle presets.
- **Colour motion:** palettes interpolate (hue rotation, beat flashes), with smooth
  blends between palettes on preset changes.
- **Resolution:** GPU LBM (the earlier suggestion) would allow 4× the grid for the
  dye/wind presets at 60 fps. Optional, later.

## 7. Phases

Each phase leaves the app working and is committed on its own. The plan reorders
cleanly if you want visible results sooner (e.g. C before B).

| Phase                       | Scope                                                                                                                                                                                                                                    | Visible result                                                                             |
| --------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------ |
| **A. Audio features**       | Streaming analyzer (FFT, 5 bands, AGC envelopes, per-band onsets, silence) for both sources; track mode moves onto it; a debug spectrum/envelope view in the Music tab; unit tests on synthetic tones and beats. Adds `rustfft` **(?)**. | Music tab shows live bands and onsets. Existing kick reactions keep working, now with AGC. |
| **B. Reactions**            | Reaction model and evaluator; Vortex brush in sim; Gust, Slosh, Pulse gravity, Colour, Param; existing Splash/Jet become reactions. Editable per preset (basic panel).                                                                   | One hand-built preset already feels like a visualizer.                                     |
| **C. Presentation mode**    | Full screen, hidden UI with reveal overlay, `--visualizer`, fixed frame budget with adaptive resolution.                                                                                                                                 | Press F11 and it's a visualizer.                                                           |
| **D. Presets and director** | The starter library; preset bundles; director with phrase/drop/song-change triggers and transitions; lock/next.                                                                                                                          | Hands-off: it changes looks with the music.                                                |
| **E. Long-run stability**   | Volume balance, fire refuel, stagnation guard, idle look; bench `--music` soak test.                                                                                                                                                     | Leave it on all evening.                                                                   |
| **F. Visual polish**        | Bloom, trails, palette motion and blends.                                                                                                                                                                                                | Looks finished.                                                                            |
| **G. Beat grid and extras** | Tempo/beat phase for on-beat effects; per-app capture (Spotify only); optional GPU LBM.                                                                                                                             | Tighter sync, nicer touches.                                                               |

A–D is the core visualizer; E–G make it good enough to leave running.

## 8. Decisions for you

1. **Primary source:** live system audio (Spotify) first, with track files second
   (recommended), or equal weight?
   Answer: live system audio (Spotify) first, with track files second (recommended)
2. **Hands-off vs hands-on:** a fully automatic director (recommended default, with a
   lock), or mostly manual preset picking?
   Answer: a fully automatic director (recommended default, with a
   lock)
3. **Which looks matter most:** liquid (FLIP), ink/dye, fire, or wind tunnel? This
   decides which presets get tuned first.
   Answer: no preference
4. **New dependency:** `rustfft` (pure Rust, widely used) for the FFT (recommended),
   or a hand-written FFT (no dependency, more code)?
   Answer: User `rustfft`--no need to reinvent the wheel
5. **"Now playing" via the Windows media session (GSMTC):** worth it? It needs
   Windows API bindings (the `windows` crate is already pulled in by wgpu/cpal).
   Answer: Not worth it
6. **Platforms:** Windows only for now (recommended; loopback is Windows-only today),
   or plan macOS/Linux capture early?
   Answer: Windows only for now
7. **Order:** keep A→G, or jump to presentation mode (C) right after A for an early
   full-screen demo?
   Answer: A->G

## 9. Progress

- **Phase A, audio features (done).**
  - **Analyzer** (`app/src/audio/analyzer.rs`, `rustfft` 6):
    - a 2048-point Hann FFT every 512 samples;
    - five band envelopes and overall level, each normalised by an automatic gain
      control (its own peak, decaying over 10 s);
    - brightness, from the spectral centroid;
    - a silence flag;
    - kick, snare and hat onsets from three detection bands.
  - **One pipeline:** live capture and track playback both feed it. Track mode passes
    the decoded samples at the playback position, so the offline kick analysis
    (`kicks.rs`) is gone. Kicks drive the existing Splash/Jet pulses.
  - **Music tab:** an Analysis group with band meters, level, brightness and
    flashing hit counters.
  - **Tests:**
    - kicks found and hats on the off-beats;
    - chunking doesn't matter;
    - a beat at a tenth of the volume gives the same kicks and band levels;
    - tones land in their bands;
    - steady hiss and silence fire nothing.
  - **Findings:**
    - *Gate timing:* the loudness gate has to accept the hop that confirms a peak. A
      hit's biggest log-energy rise comes as the window first catches it, and in a
      quiet track that moment can still be under the gate (5 of 11 kicks found at
      −20 dB).
    - *Minimum rise:* wide bands need one. In steady hiss the adaptive threshold alone
      fired about 10 snare/hat hits a second, so a hit must now raise its band's
      energy at least 1.5× in one hop.
  - **Open:**
    - The synthetic test beat's broadband hi-hats also register as snares.
    - Live, about 6 kicks a second were counted on whatever was playing.
    - Telling hit kinds apart and the thresholds need checking against real music
      early in Phase B, where hits drive different reactions.
- **Phase B, reactions (done).**
  - **Sim:** `ForceBrush` gains `spin` (rad/s) and a `target(p)` helper. All three
    brush paths use it: particle `apply_brushes`, stable-fluid faces, LBM forcing. A
    test checks the swirl's direction.
  - **Engine** (`app/src/audio/reactions.rs`): reactions map a trigger to an effect at
    a gain.
    - *Triggers:* the kick, snare and hat hits, the 5 bands, level and brightness.
    - *One-shot effects:* Splash, Jet, Vortex, Flash.
    - *Continuous effects:* Gust, Slosh, Breathe, Hue, Param.
    - *Any trigger drives any effect:* hits drive continuous effects through a
      decaying pulse; envelopes fire one-shot effects on surges, with hysteresis.
  - **Applied each frame:**
    - brushes and emitters go into the step;
    - gravity is set on the scene, never on the sliders;
    - parameters are multiples of a base value, remembered when modulation starts and
      restored when it stops or the solver changes;
    - the palette is modulated (hue rotation, lightening) for drawing only.
  - **Music tab:** a Reactions editor (on/off, trigger → effect, gain, remove, add,
    defaults, intensity), saved with the settings. Selectivity moves into Analysis.
  - **Defaults:** kick → Splash, snare → Vortex, hat → Flash, bass → Slosh,
    brightness → Hue; Gust and Breathe present but off.
  - **Tests:**
    - no audio does nothing;
    - splashes are brief;
    - vortices alternate;
    - slosh tilts without changing gravity's strength;
    - surges fire once until they re-arm;
    - hue and flash;
    - disabled reactions do nothing;
    - palette hue rotation and lightening.
  - **Checked in the app** with the test beat: FLIP splashing and sloshing with a hue
    shift, and stable fluids stirred into swirls.
  - **Music tab rebuilt so nothing moves while music plays.** Six things used to shift:
    1. The Music tab's title gained a dot, pushing the Info tab.
    2. Play/Pause changed width, sliding Restart and Mute under the cursor.
    3. A "Silent" hint appeared and vanished in Live input.
    4. A second "Silent" line did the same in Analysis.
    5. Loading and error lines pushed the track controls down.
    6. Hit counters and reaction rows changed width.

    Now:
    - the tab titles are fixed, and a ♫ indicator in the status bar shows when audio
      is active;
    - the source is a two-button switch, Track | System audio;
    - both source cards are one fixed height, with a reserved status line;
    - the transport buttons are fixed-size icons;
    - Analysis is always shown, with dimmed meters and a status line instead of
      appearing lines;
    - the reactions are a table of fixed-width slots. Combo boxes treat their width
      as a minimum, so each sits in a capped slot.

    Checked by pixel-diffing snapshots during playback: only the readouts, meters and
    lamps change; the Reactions table is identical frame to frame.
- **Phase C, presentation mode (done).**
  - **Entering and leaving:** F11 (or `--visualizer`, which also listens to system
    audio unless a track is given) switches to borderless full screen. All controls
    hide and the scene fits the whole screen with no margin. Esc closes the cheat sheet
    first, then leaves full screen, and only then quits.
  - **Overlay:** moving the mouse shows the cursor and a bottom overlay for 3 s: exit,
    scene, solver, what's playing (or "Listen to system audio"), shortcuts. It stays
    while hovered.
  - **Governor** (`app/src/governor.rs`): in presentation mode, slow motion above 10%
    for 3 s lowers the solver's resolution a step and resets the scene, with a notice:
    grid height ×0.8 (`cells_y`) or particle spacing ×1.25 (`spacing`). A 10 s
    cooldown follows each step, and it stops at the parameter's limit.
  - **Tests:** the governor acts only on sustained slowness and respects the cooldown;
    brief slowdowns don't add up; FLIP steps down to its minimum and stops.
  - **Checked in the app** at 3840×2160: full screen, the overlay, and Esc back to the
    window. FLIP kept up at 4K (simulation cost doesn't depend on screen size), so the
    governor didn't step in.
  - **Not done:** choosing a monitor. Full screen uses the monitor the window is on.

