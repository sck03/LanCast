"""Regenerate the Windows icon from the existing LanCast Android mark.

Development-only dependency: Pillow. The packaged program only needs lancast.ico.
"""
from pathlib import Path
from PIL import Image, ImageDraw

canvas = Image.new("RGBA", (768, 768), "#175CD3")
draw = ImageDraw.Draw(canvas)
factor = 16
draw.polygon([(x * factor, y * factor) for x, y in [
    (8, 10), (40, 10), (40, 33), (25, 33), (25, 30), (37, 30),
    (37, 13), (11, 13), (11, 22), (8, 22)]], fill="white")
for radius in (15, 9, 3):
    box = tuple(v * factor for v in (8-radius, 40-radius, 8+radius, 40+radius))
    draw.arc(box, 270, 360, fill="white", width=3 * factor)
canvas.save(Path(__file__).with_name("lancast.ico"), sizes=[(n, n) for n in (16, 20, 24, 32, 48, 64, 128, 256)])
