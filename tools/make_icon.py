"""Generate assets/tokpaek.ico and assets/tokpaek.png.

Design:
A rounded dark tile with a circle made of two arcs forming a single circular boundary.
Each arc contains 12 cells (segments).
Every 4 cells are colored in traffic-light colors:
- First 4 cells (0..3): Green
- Middle 4 cells (4..7): Yellow (Amber)
- Last 4 cells (8..11): Red (Coral)

Usage:
    python tools/make_icon.py
"""
import math
import os
from PIL import Image, ImageDraw

S = 1024

COLORS = {
    "green": (16, 196, 128, 255),    # #10C480
    "yellow": (250, 185, 25, 255),   # #FAB919
    "red": (244, 63, 94, 255),       # #F43F5E
    "bg": (18, 20, 26, 255),         # #12141A
    "track": (34, 38, 48, 255),      # #222630
    "border": (45, 50, 65, 120),
}

def render_master_image():
    # 2x supersampling for ultra-clean antialiased edges
    SS = S * 2
    img = Image.new("RGBA", (SS, SS), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    pad = int(SS * 0.035)
    radius = int(SS * 0.22)

    # 1. Rounded squircle background tile
    d.rounded_rectangle(
        [pad, pad, SS - 1 - pad, SS - 1 - pad],
        radius=radius,
        fill=COLORS["bg"]
    )
    d.rounded_rectangle(
        [pad, pad, SS - 1 - pad, SS - 1 - pad],
        radius=radius,
        outline=COLORS["border"],
        width=int(SS * 0.006)
    )

    cx, cy = SS / 2.0, SS / 2.0
    r_out = SS * 0.42
    r_in = SS * 0.27

    # Split gap between top and bottom arcs at 3 o'clock (0 rad) and 9 o'clock (pi rad)
    split_gap_deg = 12.0
    split_half_rad = math.radians(split_gap_deg / 2.0)

    # 12 cells per arc
    n_cells = 12
    arc_span_rad = math.pi - 2.0 * split_half_rad
    cell_gap_deg = 2.4
    cell_gap_rad = math.radians(cell_gap_deg)

    total_gaps_rad = (n_cells - 1) * cell_gap_rad
    cell_span_rad = (arc_span_rad - total_gaps_rad) / n_cells

    def draw_sector(a_start, a_end, color):
        n_steps = 24
        pts = []
        for step in range(n_steps + 1):
            t = step / n_steps
            ang = a_start + t * (a_end - a_start)
            pts.append((cx + r_out * math.cos(ang), cy + r_out * math.sin(ang)))
        for step in range(n_steps + 1):
            t = step / n_steps
            ang = a_end - t * (a_end - a_start)
            pts.append((cx + r_in * math.cos(ang), cy + r_in * math.sin(ang)))
        d.polygon(pts, fill=color)

    # Groove track underneath
    draw_sector(math.pi + split_half_rad, 2 * math.pi - split_half_rad, COLORS["track"])
    draw_sector(split_half_rad, math.pi - split_half_rad, COLORS["track"])

    def get_color(idx):
        if idx < 4:
            return COLORS["green"]
        elif idx < 8:
            return COLORS["yellow"]
        else:
            return COLORS["red"]

    # Top arc: Left to Right (9 o'clock to 3 o'clock)
    for i in range(n_cells):
        a1 = (math.pi + split_half_rad) + i * (cell_span_rad + cell_gap_rad)
        a2 = a1 + cell_span_rad
        draw_sector(a1, a2, get_color(i))

    # Bottom arc: Left to Right (9 o'clock to 3 o'clock)
    for i in range(n_cells):
        a1 = (math.pi - split_half_rad) - i * (cell_span_rad + cell_gap_rad)
        a2 = a1 - cell_span_rad
        draw_sector(min(a1, a2), max(a1, a2), get_color(i))

    return img.resize((S, S), Image.LANCZOS)

def main():
    os.makedirs("assets", exist_ok=True)
    master = render_master_image()

    sizes = [16, 24, 32, 48, 64, 128, 256]
    master.save("assets/tokpaek.ico", sizes=[(s, s) for s in sizes])
    master.save("assets/quotty.ico", sizes=[(s, s) for s in sizes])

    master.resize((256, 256), Image.LANCZOS).save("assets/tokpaek.png")
    master.resize((256, 256), Image.LANCZOS).save("assets/quotty.png")

    print("Successfully generated assets/tokpaek.ico, assets/tokpaek.png, assets/quotty.ico, and assets/quotty.png")

if __name__ == "__main__":
    main()

