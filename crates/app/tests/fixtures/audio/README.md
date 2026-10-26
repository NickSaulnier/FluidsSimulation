# Analyzer fixtures

Audio with known hit and beat times, for measuring the analyzer's hit detection and
beat tracking (`crates/app/src/audio/eval.rs`). The test `detection_holds_its_baseline`
scores every clip here, plus the generated songs (`audio/songs.rs`), and fails if a score
falls more than 0.02 below `baseline.txt`.

## Adding a clip

1. Put the audio here: `name.wav` (or `.flac`, `.ogg`, `.mp3`). Keep it short (10–20 s)
   and only commit audio you may redistribute: public domain, CC0, or CC BY (then add
   the attribution below).
2. Beside it, `name.txt` with one hit per line: the time in seconds, then `kick`,
   `snare` or `hat`. If the music has a steady beat, add its beats too: `beat`, or
   `downbeat` for the first beat of a bar. `#` starts a comment.

   ```
   # name.wav: 120 BPM, hand-marked
   0.500 downbeat
   0.512 kick
   1.000 beat
   0.998 snare
   1.250 hat
   ```

   Mark every hit you can hear, including quiet ones (ghost notes). The first 1.5 s
   aren't scored for hits, so the detector can warm up, nor the first 5 s for beats.
3. Record its scores into the baseline:

   ```
   UPDATE_ANALYZER_BASELINE=1 cargo test --release -p app detection -- --nocapture
   ```

## Attribution

### Groove MIDI Dataset: `gmd_*.wav`

The six `gmd_*` clips are from the [Groove MIDI Dataset](https://magenta.tensorflow.org/datasets/groove)
by Google Magenta, licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/):

> Jon Gillick, Adam Roberts, Jesse Engel, Douglas Eck, and David Bamman.
> "Learning to Groove with Inverse Sequence Transformations."
> International Conference on Machine Learning (ICML), 2019.

| Fixture | Dataset file (`groove/…`) |
|---|---|
| `gmd_funk_138` | `drummer1/eval_session/1_funk-groove1_138_beat_4-4` |
| `gmd_soul_102` | `drummer1/eval_session/10_soul-groove10_102_beat_4-4` |
| `gmd_hiphop_87` | `drummer1/eval_session/6_hiphop-groove6_87_beat_4-4` |
| `gmd_rock_135` | `drummer1/session3/8_rock_135_beat_4-4` |
| `gmd_samba_116` | `drummer1/session1/38_latin-samba_116_beat_4-4` |
| `gmd_breakbeat_170` | `drummer1/session1/81_dance-breakbeat_170_beat_4-4` |

Changes: each recording is cut to its first 20 s and mixed to mono. Its hit list
comes from the dataset's MIDI for the same performance: kick from note 36; snare from
38, 40 and 37 (head, rim, cross-stick); hat from the hi-hats (42, 22, 44, 46, 26), ride
(51, 59, 53) and crashes (49, 55, 57, 52). Toms are left out, and notes of one kind
within 30 ms of each other count as one hit. The beats are the MIDI's quarter-note grid
(the drummers played to a click), a downbeat every four; the samba has none, as its
playing sits off that grid.
