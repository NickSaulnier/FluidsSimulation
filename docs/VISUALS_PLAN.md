# Visuals Plan

Status: **phases A to E done** (2026-10-31); decisions 4 and 5 open (§4). Better pictures and a closer
fit to the music, now that visualizing music is the app's main purpose. Grouped into
phases; each phase is committed on its own.

## 1. Goal

What's on screen in full screen should always look good and follow the song:
- Only fluids and scenes that look good when the music drives them; the rest stay for
  simulating by hand.
- Scenes made for music: filling the screen, never settling, kept moving by the sound.
- The picture following the song's shape (build-ups, drops, quiet passages), not only
  its hits.
- Motion locked to the beat, landing on it rather than just after it.
- Colour following the harmony and the timbre, not only the volume.
- A way to measure a look's picture, so weak looks are found without watching them all.

## 2. Where things stand

| Piece | Today |
|---|---|
| Fluids in full screen | The director picks from all 48 built-in looks (32 with a fluid, on 12 solvers); the randomizer picks among 13 fluid choices (`looks/random.rs`, `FLUIDS`), none weighted 0. A look rated one star is weighted 0 and skipped, but there's no way to keep a whole solver or scene out. |
| Scenes | Ten 2D presets (`sim/src/presets.rs`), most of them physics setups: dam breaks, Martin & Moyce, a tank with an obstacle, a wind tunnel. Particle fluids in them slosh and then settle on the floor. A `Scene` is a domain, gravity, a solid mask, fluid blocks, emitters and drains. |
| Loudness | Every band and the level are normalised by an AGC whose peak decays over 10 s (`analyzer.rs`, `AGC_S`). That keeps quiet songs lively, but it also brings a breakdown back up to full within seconds, so quiet and loud parts of a song end up looking alike. Raw loudness (LUFS, momentary and short-term) is there, unused by the reactions. |
| Song shape | `sections`: novelty, `buildup` (0..1), and changes classed as drop, breakdown or other change. The director acts on drops and breakdowns (a cut, a slow blend) and fractal tours on changes; equations can read `buildup`, `drop`, `breakdown`. Nothing else rises through a build-up or calms in a breakdown. |
| Beat | `tempo` gives bpm, confidence, `beat_phase`, `bar_phase` and the next beat. The director lands changes on the beat or the bar; equations can read the phases. The reactions (`audio/reactions.rs`) are all driven by the triggers (detected hits and levels), so their motion starts after a hit is heard. |
| Colour | Palettes are fixed, or taken from the song's cover; the Hue effect shifts the hue on triggers. The analysis has `brightness` (spectral), but no pitch content (chroma, key). |
| Variety | A look is the same from start to end except for its equations; the director changes looks on new songs, drops, breakdowns and at an interval. |
| Picture checks | GPU tests check single passes (`render::grab`), such as a fractal frame being neither black nor blown out. Nothing scores a whole look's picture over time. |

## 3. The work

### 3.1 A picture check

- **A score for a run of frames** (`render/score.rs`, CPU, from grabbed frames at low
  resolution):
  - **Dark:** the share of pixels below a luminance floor.
  - **Blown out:** the share near white, or one hue covering most of the frame.
  - **Still:** the mean change between frames a beat apart; low means the music isn't
    moving it.
  - **Flat:** spatial contrast (the spread of luminance and of local gradients); low
    means a wash.
  - **Following:** how well the motion follows the music (the change from frame to
    frame against the level and the hits).
- **A contact sheet:** `--audit <track>` runs every look (or the randomizer's, with a
  seed) against a track for a set time, writes a grid of frames per look with its
  scores, and a table sorted worst first. Three reference tracks of different kinds
  (loud and dense, sparse and quiet, with a clear build-up and drop), generated or
  chosen with the user.
- **Uses:**
  - Finding the weak looks and fluids (§3.2).
  - The randomizer throws away a random look whose first seconds score badly and
    rolls again (off screen, at a small resolution).
  - A test: every built-in look scores above the floors on a generated track.

### 3.2 Only what looks good in full screen

- **A flag** on each solver and scene: shown in the visualizer or not. The director, the
  randomizer and the look browser in full screen keep to looks whose fluid and scene
  are flagged; the Simulate tab still offers everything.
- **Which ones:** from the contact sheet and the user's eye (§4, decision 1). My guess at
  the weak ones: FLIP, PBF and the MPM fluids in the tank scenes (they settle on the
  floor), Lattice Boltzmann's wind tunnel, and the Martin & Moyce column.
- **The built-in looks on them** move to a fluid that's kept (the same colours,
  feedback and reactions over a better fluid), go fluid-free, or are dropped (§4,
  decision 2).
- **The randomizer's** `FLUIDS` table keeps to flagged fluids and scenes.

### 3.3 Scenes made for music

New presets for the grid fluids (stable fluids CPU and GPU, smoke & fire, shallow water),
with no floor to settle on, filling the window:
- **Band emitters:** five sources round the screen, one per band, each pouring dye and
  pushing inward with its own band's level; gravity off.
- **Tempo vortex:** a swirl at the middle turning once a bar, kicked on the downbeat,
  with dye fed in at its edge.
- **Rain:** shallow water with no walls in view, a drop on each hit (snare and hat
  stronger, kicks a wide swell), the rings crossing.
- **Two jets:** opposed jets of two colours meeting in the middle, their strength
  following bass and treble, curling where they meet.

Each gets a built-in look or two, and joins the randomizer's scenes.

### 3.4 Following the song's shape

- **An intensity, 0..1, from the song itself:** the short-term loudness against the
  song's own range so far (a slow minimum and maximum over a minute or more), lifted
  through a build-up (`buildup`), at its highest right after a drop, and low in a
  breakdown. Unlike the AGC'd levels, it stays low through a whole quiet passage.
- **What it moves,** for every look, scaled by a look's own `intensity` amount (default
  1; 0 for looks that want to stay as they are):
  - The reactions' gains (a breakdown's hits are gentler).
  - Brightness and bloom; saturation; the feedback's motion (zoom, turn, wobble).
  - Fractal dive speed, wave thickness.
- **Through a build-up:** a rising tension the look can show (feedback zoom creeping in,
  the colours brightening, the fluid quickening), released at the drop with a flash and
  a burst, which the director's cut on drops already lands on.
- **An equation input,** `intensity`, so looks can use it their own way.

### 3.5 Motion on the beat

- **Phase-locked reactions:** reactions that move on the beat's phase instead of after a
  detected hit, when the tempo is confident:
  - A pulse (zoom, push, brightness) shaped to rise into the beat and peak on it.
  - A step a bar (a turn, a palette step, a gust changing direction) on the downbeat.
- **Hits still accent** on top of them, and the tempo's confidence fades between the two:
  with no steady beat (ambient, a breakdown), only hits.
- **New triggers** `Pulse` (beat phase) and `Bar` (bar phase) in `Trigger`, usable with
  any effect; the default reactions and the randomizer use them.
- **Latency:** the phase already includes the latency setting; the pulse's peak lands on
  the beat heard.

### 3.6 Colour from the harmony and timbre

- **Chroma:** twelve pitch classes from the spectrum (log-spaced bins folded to octaves,
  smoothed over a beat or two), and the strongest pitch class, held over a bar.
- **Colour:** an optional hue drift following the pitch class around a colour wheel (a
  fifth apart is a small step, so related chords stay close), changing on chord changes
  rather than on every note; saturation and glow following `brightness`.
- **In looks:** a `harmony` amount in the colours (0, off, by default for the built-ins
  that keep their palette); equation inputs `chroma_0` to `chroma_11` and `key`.

### 3.7 Variants within a look

- **A look can carry two or three variants:** each a small set of changes (palette,
  symmetry, feedback, wave mode, fractal kind or spot).
- **Section changes** move to another variant, blended over a bar, instead of the
  director always changing the whole look; a drop still changes the look.
- **The randomizer** makes variants of its own looks (as Shift+D does, kept with the
  look), and the editor lists them.

## 4. Decisions (1 to 3 made)

1. **Which solvers and scenes go:** decided from the contact sheet (phase A) and the
   user's eye. Gone from the visualizer: the MPM fluids, FLIP outside the fountain, the
   Martin & Moyce column and the falling blocks. PBF and PBF (GPU) stay (the user has
   chaotic looks of their own on them).
2. **Their built-in looks:** moved where the look is good apart from its fluid, dropped
   otherwise (see phase B in §6).
3. **Intensity for every look:** on for every look (the user's choice); a look can turn
   it down or off with its `dynamics`.
4. **Harmony (§3.6):** in scope? It's new analysis (chroma), and the most uncertain to
   look good. Recommended: yes, as a late phase, off by default.
5. **Variants (§3.7):** in scope? Recommended: yes, last.

## 5. Phases

| Phase | Scope | Visible result |
|---|---|---|
| **A. Picture check** | Scores, the contact sheet, reference tracks, the randomizer's re-roll, a test (§3.1). | A sheet of every look, worst first; fewer bad random looks. |
| **B. Curation** | The flag on solvers and scenes, the built-in looks moved or dropped, the randomizer's table (§3.2). | No settled puddles or test rigs in full screen. |
| **C. Music scenes** | Four presets and their looks (§3.3). | Fluids that fill the screen and keep moving with the sound. |
| **D. Song shape** | Intensity, what it moves, build-up tension and release, the equation input (§3.4). | Quiet parts look quiet; build-ups rise; drops hit. |
| **E. On the beat** | Pulse and Bar triggers, phase-shaped reactions, the fade with tempo confidence (§3.5). | Motion that lands on the beat. |
| **F. Harmony** (optional) | Chroma, the hue drift, the inputs (§3.6). | Colour that changes with the chords. |
| **G. Variants** (optional) | Variants in looks, section changes, the randomizer and editor (§3.7). | A look that changes with the song without being replaced. |

Suggested order: A first (it measures everything after), then B and C, then D and E;
F and G if chosen. Each phase ends with the contact sheet run again, against the one
before.

## 6. Progress

- **Phase A, the picture check (done):** `render/score.rs` and `audit.rs`.
  - **The measures,** from small frames (RGBA, as grabbed): the share dark and near
    white, the detail (the mean step between neighbouring pixels), the coverage (the
    share away from the background, the median colour), the contrast (the luma between
    the darkest and brightest 2 %); between frames the motion; over the run, the
    correlation between each interval's motion and the music's activity (the level and
    the hits), and the share of dead frames. Each becomes a subscore between a floor and
    plenty; the quality, 0 to 100, is their weighted geometric mean, so one failing sinks
    it. Following the music and the frame rate count for less, and have a floor (a steady
    dive or a slow 3D look isn't dead).
  - **Coverage and contrast came from the first run:** with only the planned measures,
    all but a few looks scored 90 to 100, Jelly drop (a few blocks on an empty floor)
    and Ember drift (dark and grainy: the grain read as detail) among them, because a
    dark grey floor isn't "dark". Measuring coverage against the frame's own background,
    and the contrast, moved them down.
  - **The audit,** `fluid-toy --audit [TRACK]`: the app as the visualizer in a 1280 × 720
    window, muted, its settings not saved; each look in turn (the director held, a cut),
    the music played from the same point for each (`Music::seek`), the frames grabbed at
    256 × 144 every 0.25 s after 4 s of warm-up. It writes `index.html` (worst first: the
    quality and its change since the last audit in the folder, what's wrong, a strip of
    six frames, the measures), `scores.json` (the last one kept as
    `scores.previous.json`) and the strips, then quits. `--audit-looks=TEXT`,
    `--audit-random=N` (seeded 1 to N), `--audit-from=S`, `--audit-seconds=S`,
    `--audit-out=FOLDER`; `--audit-sheet` rewrites the page from the saved measures as the
    scoring now stands, for tuning it without running every look again.
  - **The reference track:** the generated anthem (`audio::songs`, now built into the app,
    not only the tests), from 4 s before its first build-up for 36 s: a build-up, the
    whole drop and 10 s of the breakdown. Real tracks work too (from 30 s in). The plan's
    three reference tracks are down to this one for now; a quiet track would test the
    looks in a breakdown better.
  - **Random looks** are now scored in their first seconds (five frames at 128 × 72 from
    2.5 s, while the music plays) and replaced by another under 40, three in a row at
    most, with a notice outside full screen. The plan had them re-rolled off screen
    before showing, but that would need a second copy of the whole picture running
    hidden; this way a weak look shows for about 4 s. 40 is under the weakest built-in
    look (68); of 16 seeded random looks one scored under it (17: dark, murky smoke), one
    61, the rest 76 to 100.
  - **The test** the plan wanted (every built-in look above the floors, rendered) needs
    the whole app headless, which it isn't; the scoring has unit tests, and the audit is
    run by hand (about 30 minutes for every look).
  - **The first audit,** every built-in look against the anthem (mean 94): the weakest
    were Neon scope (68: a white blob in the middle, still), Jelly drop (70: empty),
    Smoke dance (72: low contrast), Dye tornado (76: a washed-out purple kaleidoscope),
    Ember drift (83: dark, grainy), Blue torch (86: a thin flame in a dull field), Deep
    current (89: dim). Sixteen looks scored 100, nine of them on fluids the plan guessed
    were weak (FLIP, PBF, MPM in tank scenes): the scores catch dead, empty, murky and
    still pictures, not a settled puddle or a test rig that's well lit and moving. Phase
    B's choice stays with the user's eye; the sheet shows them all side by side.
  - **Checked in the app:** the audit over all 48 looks twice (no errors; 230 to 240 fps
    for looks without fluid, 66 to 230 with); 16 random looks; pressing D ten times on a
    drum loop, each look scored about 4 s in (all kept, 69 to 100). A look actually
    replaced wasn't seen on screen.
- **Phase B, curation (done):** `looks/random.rs` (`FLUIDS`) and the built-in looks.
  - **What the strips showed:** the particle fluids (FLIP, PBF, MPM) fill the screen
    only in the drop; in the build-up and the breakdown they're a puddle on the floor.
    The scores average over the run, so they missed it. The grid fluids are driven by
    an inflow, a burner or stirring, and never settle.
  - **No flag:** the plan had a flag on each solver and scene, but FLIP stays in one
    scene only (the fountain, which keeps pouring), which separate flags can't say.
    The randomizer's `FLUIDS` table (solver → the scenes it's shown in) is now also the
    list of what the visualizer shows; a test checks every built-in look keeps to it.
    The user's own looks aren't filtered (they chose their fluid), and the Simulate tab
    still offers everything. Ballistic (debug) and PBF (GPU) were never in the table.
  - **Out of the table:** MPM jelly, snow and sand; FLIP in the dam breaks, the tank and
    the column (its weight down from 1.5 to 0.8, with one scene left); the Martin &
    Moyce column and the falling blocks from every solver. The MPM ranges stay in
    `PHYSICS`, for varying the user's own looks (Shift+D).
  - **The built-in looks** (48 to 44): Ocean pulse moved from FLIP in the tank to FLIP
    in the fountain (100 in the audit; a moving mound when quiet instead of a flat
    pool); Dye tornado from the column to the double dam break (77 to 81: colour from
    the first frame, still washing out to purple later). Dropped: Jelly drop, Snow
    globe, Shifting dunes (MPM) and Deep current (FLIP in the column).
  - **Checked in the app:** 231 tests pass; the audit of the two moved looks, and of 16
    seeded random looks (none on a fluid or scene left out; 14 from 81 to 100, two weak
    ones on Smoke & fire at 15 and 58, which the phase A check would replace).
  - **Left for later** (the looks, not their fluids): Ember drift (dark noise, the fire
    barely shows), Smoke dance and Ghost smoke (dim, grainy), Neon scope (a still
    blob), Cover drift and Dye tornado (washing out to one colour).
- **Phase C, music scenes (done):** `sim::presets` (the scenes), `sim::scene::Feed`,
  `ReactionEngine::feed` (driving them), four looks.
  - **Feeds:** a scene can carry emitters the music drives (`Scene::feeds`): each frame
    the app adds them to the step's emitters at their speed times one band's level
    (and the intensity), turned round the middle `turns_per_bar` times a bar (locked to
    the bar phase while a tempo is found, a 2 s bar otherwise). Off while its band is
    quiet or the music silent; a gentle 0.3 with no music at all, so the scenes show
    something in the simulator. Solvers never see feeds, so every solver runs them
    (through the temporary emitters the faucet already uses), and they pause with the
    jets when particles are over budget.
  - **Two colours:** an emitter now carries its dye, 0 to 1 (`Emitter::dye`, 1 for
    every emitter before), which the dye solvers (CPU and GPU) release and the fire
    solver takes as fuel; less is a darker, fainter shade of the palette's ramp. Each
    band's feed has its own shade.
  - **The scenes,** weightless and open on every side (drains in the border, as the
    campfire's top), so what pours in leaves again: **Five bands** (a feed per band on
    an ellipse round the window, the lowest at the bottom, aimed past the middle so the
    streams curl round each other), **Tempo vortex** (two feeds on a circle round the
    middle, bass and mids, pouring along it and going round once a bar), **Two jets**
    (bass from the left, treble from the right, a little apart so they curl where they
    meet) and **Rain** (open water, nothing in it; the drops are the look's reactions,
    so a kick isn't a wider swell than a snare as the plan had it, only stronger).
  - **The looks** (44 to 48): Band light (GPU dye, five bands), Bar spiral (CPU dye,
    tempo vortex), Crossfire (GPU dye, two jets), Rainfall (shallow water, rain, a drop
    on every kick, snare and hat). All four scored 100 against the anthem and against a
    funk drum loop, and stay full of moving colour through the build-up and the
    breakdown, where the tank scenes' strips go quiet. The two GPU ones run at 78 to 90
    fps in the audit window (the other GPU dye looks 92 to 192).
  - **The randomizer:** both dye solvers get the three feed scenes, shallow water gets
    rain, and smoke & fire gets five bands (84 in the audit: five torches in grey smoke).
    Fire on the tempo vortex (17) and the two jets (63) came out sparse and grey, so it
    keeps out of them.
  - **Checked in the app:** 232 app tests (a new one: a feed follows its band and the
    bar) and the workspace's pass, clippy clean; the four looks audited on the anthem and
    the drum loop; 16 seeded random looks (three on the new scenes, 99 to 100; the two
    weak ones are on the campfire, as before).
- **Phase D, the song's shape (done):** `audio/dynamics.rs` (the energy) and
  `looks/dynamics.rs` (what it moves).
  - **Names:** the plan's song intensity is the **energy** (the equation input
    `energy`, and `AudioFeatures::energy`), and a look's own amount is **`dynamics`**
    (in `[meta]`, 0 to 1, default 1), because `intensity` already names the equation
    output that scales the reactions.
  - **The energy,** worked out by the analyzer each hop: the momentary loudness through
    an envelope (rising over 0.3 s, falling over 1.5 s), against the song's own quiet and
    loud (the 10 % and 95 % quantiles of that envelope over the last 90 s, at least 8 dB
    apart). For the first 30 s of a song the range means little (the intro is the
    loudest part so far), so it starts at the neutral 0.7 and eases over. On top: a
    build-up adds up to 0.35, a drop fills it (fading over 8 s), a breakdown halves it
    (recovering over 8 s). Silence lets it fall and forgets the range, so each song is
    measured on its own. On the generated anthem it rises through the build-up, is 1
    just after each drop and about 0.9 through it, about 0 through the whole breakdown,
    and climbs into the second drop; steady clips sit at 0.75 to 0.9. Analysing still
    allocates nothing (the range is sorted into a kept buffer, once per sample).
  - **What it moves,** after the look's equations, every frame, scaled by the look's
    `dynamics`; at 0.7 a look is exactly as set, and without music nothing changes:
    the reactions' strength (0.4 to 1.25 times), the feedback's motion (zoom growth,
    turn, shift, wobble: 0.35 to 1.3), exposure (0.75 to 1.1), saturation (0.7 to 1.15),
    wave thickness (0.6 to 1.3) and a fractal's dive speed (0.4 to 1.3). Through a
    build-up the zoom creeps in (up to 12 % more growth a second), the picture brightens
    and the reactions strengthen; at a drop, for about half a second, a flash (exposure
    up to 1.6 times) and a burst (the reactions 2.5 times), on whichever look the drop
    cuts to. Bloom isn't moved: it's the user's setting, not the look's (exposure does
    the brightening).
  - **Not done:** the plan's "the fluid quickening" through a build-up is the stronger
    reactions, not a faster simulation (changing the simulation's speed would change its
    stability).
  - **Checked:** 237 app tests (new: a breakdown stays low, the song's shape moves the
    energy, silence forgets the song; the energy calms and livens a look, build-ups and
    drops) and clippy pass; the energy printed over the generated songs and the drum
    clips. The contact sheet wasn't run again for this phase (at the user's word).
- **Phase E, motion on the beat (done):** `audio/reactions.rs`.
  - **Two triggers on the beat's own clock** (the tracker's phases, which already
    include the latency setting, so the peak lands on the beat heard), usable with any
    effect: **Pulse** (high on the beat, falling with a time constant of 0.15 of a beat,
    rising again over the last 0.3 of it into the next; one-shot effects fire as the
    beat's phase wraps round, so on the beat rather than a hop or two after it's heard)
    and **Bar** (a step between two levels each downbeat, eased over the first 8 % of the
    bar: a hue that changes each bar, a gust or a breath that turns on and off; one-shot
    effects fire on the downbeat).
  - **The tempo's confidence fades between them and the hits:** both are scaled by a
    weight that's 0 without a steady beat (or in silence) and rises from confidence 0.3
    to full at 0.6; the hit triggers carry on regardless, so with no steady beat
    (ambient, a breakdown) only the hits move the picture, and with one the hits accent
    the pulse.
  - **A new effect, Zoom:** the picture's zoom (the feedback's growth) pushes in with the
    trigger, up to 30 % more a second; with Pulse it breathes in on every beat, for any
    look with feedback, fluid or not. It's added to the frame's motion after the look's
    equations and the song's shape.
  - **Using them:** the default reactions gain Pulse → Zoom (0.4) and Bar → Vortex (0.6);
    every kind of fluid in the randomizer gets a Pulse → Zoom pick and a Bar pick (a
    vortex, a jet for fire, a drop for shallow water, a hue step without fluid); Bar
    spiral (the tempo vortex's look) gets Pulse → Zoom. The other built-in looks are as
    they were.
  - **Checked:** 238 app tests (new: the pulse peaks on the beat, one-shot effects fire
    once a beat on the beat and once a bar on the downbeat, the bar steps, and nothing
    moves without a confident tempo) and clippy pass; Bar spiral run in the app on the
    funk drum loop (98, no errors). The contact sheet wasn't run again.
