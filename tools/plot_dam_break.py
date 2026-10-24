"""Plot simulated dam-break surge fronts against Martin & Moyce (1952).

Usage:
    cargo run -p bench --release -- --solver flip --preset martin --frames 30 \
        --set cells_y=100 --front-csv docs/validation/flip_cells_y100.csv
    python tools/plot_dam_break.py docs/validation/*.csv -o docs/validation/dam_break.png

Needs only Pillow (no matplotlib). Each CSV has columns t, T, Z as written by the bench.
"""
import argparse
import csv
import os

from PIL import Image, ImageDraw, ImageFont

# Martin & Moyce (1952), n^2 = 2, ensemble means: (T, Z, corroborated).
# Keep in sync with sim::analysis::MARTIN_MOYCE_N2_2.
MARTIN_MOYCE = [
    (0.41, 1.11, False), (0.84, 1.22, False), (1.19, 1.44, False), (1.43, 1.67, False),
    (1.63, 1.89, False), (1.83, 2.11, True), (1.98, 2.33, False), (2.20, 2.56, False),
    (2.32, 2.78, False), (2.51, 3.00, True), (2.65, 3.22, False), (2.83, 3.44, False),
    (2.98, 3.67, False), (3.11, 3.89, True),
]
COLOURS = [(31, 119, 180), (214, 39, 40), (44, 160, 44), (148, 103, 189), (255, 127, 14), (23, 190, 207)]

W, H = 1100, 760
LEFT, RIGHT, TOP, BOTTOM = 110, 330, 60, 100
T_MAX, Z_MIN, Z_MAX = 3.5, 1.0, 4.5


def to_px(t, z):
    x = LEFT + t / T_MAX * (W - LEFT - RIGHT)
    y = H - BOTTOM - (z - Z_MIN) / (Z_MAX - Z_MIN) * (H - TOP - BOTTOM)
    return x, y


def font(size):
    for name in ("segoeui.ttf", "arial.ttf", "DejaVuSans.ttf"):
        try:
            return ImageFont.truetype(name, size)
        except OSError:
            pass
    return ImageFont.load_default()


def label_for(path):
    name = os.path.splitext(os.path.basename(path))[0]
    solver, _, setting = name.partition("_")
    # Filenames use 'p' for the decimal point: pbf_spacing0p01 -> "PBF (spacing 0.01)".
    setting = setting.replace("cells_y", "cells ").replace("spacing", "spacing ").replace("p", ".")
    setting = setting.replace("s.acing", "spacing")
    return f"{solver.upper()} ({setting})"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("csvs", nargs="+")
    ap.add_argument("-o", "--output", default="dam_break.png")
    args = ap.parse_args()

    img = Image.new("RGB", (W, H), "white")
    d = ImageDraw.Draw(img)
    f_small, f_axis, f_title = font(16), font(19), font(23)

    # Grid and axes.
    for i in range(8):
        t = i * 0.5
        x, _ = to_px(t, Z_MIN)
        d.line([x, TOP, x, H - BOTTOM], fill=(232, 232, 232))
        d.text((x, H - BOTTOM + 8), f"{t:g}", fill="black", font=f_small, anchor="mt")
    for i in range(8):
        z = Z_MIN + i * 0.5
        _, y = to_px(0, z)
        d.line([LEFT, y, W - RIGHT, y], fill=(232, 232, 232))
        d.text((LEFT - 10, y), f"{z:g}", fill="black", font=f_small, anchor="rm")
    d.rectangle([LEFT, TOP, W - RIGHT, H - BOTTOM], outline="black", width=2)
    d.text(((LEFT + W - RIGHT) / 2, H - 40), "T = t √(2g/a)", fill="black", font=f_axis, anchor="mm")
    d.text((30, (TOP + H - BOTTOM) / 2), "Z = x/a", fill="black", font=f_axis, anchor="mm")
    d.text(((LEFT + W - RIGHT) / 2, 28), "Dam-break surge front vs Martin & Moyce (1952), n² = 2",
           fill="black", font=f_title, anchor="mm")

    legend = []

    # Ritter's shallow-water solution, Z = 1 + 2T (dashed).
    t = 0.0
    while t < T_MAX:
        t1 = min(t + 0.06, T_MAX)
        if 1 + 2 * t1 <= Z_MAX:
            d.line([to_px(t, 1 + 2 * t), to_px(t1, 1 + 2 * t1)], fill=(120, 120, 120), width=2)
        t += 0.12
    legend.append(("Ritter (shallow water)", (120, 120, 120), "dash"))

    # Simulations.
    for k, path in enumerate(args.csvs):
        colour = COLOURS[k % len(COLOURS)]
        with open(path) as fh:
            rows = [(float(r["T"]), float(r["Z"])) for r in csv.DictReader(fh)]
        pts = [to_px(t, z) for t, z in rows if t <= T_MAX and Z_MIN <= z <= Z_MAX]
        if len(pts) > 1:
            d.line(pts, fill=colour, width=3, joint="curve")
        legend.append((label_for(path), colour, "line"))

    # Experiment on top: filled = corroborated against published validations.
    for t, z, confirmed in MARTIN_MOYCE:
        x, y = to_px(t, z)
        r = 7
        d.ellipse([x - r, y - r, x + r, y + r], outline="black", width=2, fill="black" if confirmed else "white")
    legend.insert(0, ("Martin & Moyce (filled: corroborated)", "black", "dot"))

    # Legend to the right of the plot.
    lx, ly = W - RIGHT + 25, TOP + 10
    for text, colour, kind in legend:
        if kind == "dot":
            d.ellipse([lx + 10, ly + 3, lx + 24, ly + 17], outline="black", width=2, fill="black")
        elif kind == "dash":
            for s in range(0, 34, 10):
                d.line([lx + s, ly + 10, lx + s + 5, ly + 10], fill=colour, width=2)
        else:
            d.line([lx, ly + 10, lx + 34, ly + 10], fill=colour, width=3)
        d.text((lx + 44, ly + 10), text, fill="black", font=f_small, anchor="lm")
        ly += 30

    img.save(args.output)
    print(f"wrote {args.output}")


if __name__ == "__main__":
    main()
