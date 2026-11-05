"""Generates the level demo's office: game/levels/office.bbmodel (a Blockbench model of boxes with its own textures) and
game/levels/office.mis (its map, with a spot light under each ceiling light).

An office floor of 36 x 24 m under a flat roof: a lobby with a reception desk, sofas and plants; an open plan of cubicle pods
with desks, monitors and chairs; a kitchen, a glass-walled meeting room, a print room and a private office along the north
side, off a corridor. The outer walls have real windows (openings with glass panes in them); ceiling lights hang in every
room. Sizes suit the level demo's figure (1 m wide, 2 m tall): doors 2 m wide and 2.6 m tall, walls 3.2 m, aisles 1.4 m or
more.

Textures are pixel art drawn here from code, 32 texels a meter (2 a Blockbench unit) everywhere, in flat areas of a
five-step ramp (dark to light) with a few deliberate shading steps and lit or shaded edges, and nothing random:
- the building's large surfaces (floors, walls, ceiling, roof, glass) repeat a seamless tile;
- every face of the furniture gets its own rectangle in its material's atlas, sized to the face, with a lit top-left edge and
  a shaded bottom-right one, and its material's few lines (wood grain, a seam, a band of sheen).
Glass is the only texture with partly see-through texels, so it is the only translucent material. Textures whose name ends
in `_glow` are drawn unlit: the ceiling lights and the screens (monitors, laptops and the meeting room's chart).

Every piece of furniture stands on the floor, on a rug or on another piece: an office chair's base lies on the floor.

Run: python -I tools/office.py  (from examples/level-demo).
"""

import base64
import json
import math
import os
import random
import struct
import sys
import uuid
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "..", "game", "levels", "office.bbmodel")
MIS = os.path.join(HERE, "..", "game", "levels", "office.mis")

RNG = random.Random(20261105)        # the furniture's small turns and offsets
UUID_RNG = random.Random(20261106)   # the elements' ids (apart, so a new element does not move the furniture)
W, D = 36.0, 24.0          # the floor, meters
H = 3.2                    # wall height, the ceiling's underside
ROOF = 0.3                 # the roof slab's thickness
TPM = 32                   # texels a meter
CORNER = (-18.0, -12.0)    # where the office's (0, 0) is in the map


# ---- palette: five-step ramps, dark to light ------------------------------------------------------------------------

RAMPS = {
    "navy": [(30, 38, 62), (40, 52, 84), (52, 68, 104), (66, 84, 124), (86, 106, 148)],
    "teal": [(24, 70, 74), (32, 90, 94), (42, 110, 114), (58, 132, 134), (84, 158, 156)],
    "oak": [(98, 62, 36), (126, 84, 50), (152, 106, 64), (178, 130, 82), (204, 158, 106)],
    "walnut": [(48, 31, 21), (66, 43, 28), (86, 57, 37), (108, 74, 49), (134, 96, 64)],
    "plaster": [(192, 186, 174), (210, 204, 192), (224, 219, 208), (236, 232, 222), (246, 243, 236)],
    "plaster_teal": [(30, 82, 86), (38, 98, 102), (48, 114, 118), (62, 132, 136), (84, 154, 156)],
    "ceiling": [(170, 172, 172), (198, 200, 198), (218, 219, 216), (232, 233, 230), (244, 245, 242)],
    "marble": [(150, 150, 156), (184, 184, 190), (212, 212, 216), (230, 230, 234), (246, 246, 248)],
    "metal": [(112, 116, 124), (140, 144, 152), (168, 172, 180), (192, 196, 202), (218, 222, 228)],
    "dark_metal": [(26, 28, 34), (38, 41, 48), (52, 56, 64), (70, 75, 84), (94, 100, 110)],
    "laminate": [(56, 86, 128), (76, 112, 160), (100, 138, 186), (130, 166, 208), (168, 198, 230)],
    "charcoal": [(32, 34, 40), (46, 49, 57), (62, 66, 76), (82, 87, 98), (110, 116, 128)],
    "beige": [(150, 128, 100), (178, 156, 126), (202, 182, 152), (222, 204, 176), (238, 224, 200)],
    "orange": [(140, 64, 36), (172, 84, 48), (200, 108, 64), (222, 134, 88), (238, 164, 120)],
    "terracotta": [(112, 50, 30), (148, 70, 40), (180, 92, 54), (204, 118, 76), (226, 148, 104)],
    "leaves": [(24, 58, 32), (36, 84, 44), (52, 112, 56), (76, 142, 70), (112, 172, 92)],
    "soil": [(42, 30, 22), (56, 40, 28), (70, 50, 36), (86, 62, 44), (104, 76, 56)],
    "paper": [(196, 196, 192), (218, 218, 214), (234, 234, 230), (244, 244, 241), (252, 252, 250)],
    "granite": [(54, 56, 60), (74, 76, 82), (96, 98, 104), (120, 122, 128), (148, 150, 156)],
    "steel": [(128, 134, 144), (158, 164, 174), (184, 190, 198), (206, 210, 218), (230, 234, 240)],
    "ceramic": [(194, 196, 202), (214, 216, 222), (230, 232, 236), (242, 243, 245), (252, 252, 252)],
    "water": [(68, 118, 168), (88, 144, 196), (114, 170, 218), (144, 194, 232), (184, 218, 242)],
    "facade": [(118, 112, 104), (144, 138, 128), (168, 162, 152), (190, 184, 174), (210, 205, 196)],
    "plinth": [(70, 66, 64), (88, 84, 80), (106, 102, 98), (126, 122, 116), (146, 142, 136)],
    "gravel": [(68, 70, 72), (90, 92, 94), (110, 112, 114), (132, 134, 136), (154, 156, 158)],
    "frame": [(46, 50, 56), (62, 66, 74), (80, 85, 94), (100, 106, 116), (128, 134, 144)],
    "light": [(206, 212, 220), (226, 230, 236), (240, 243, 247), (249, 250, 252), (255, 255, 255)],
    "gold": [(110, 80, 30), (150, 112, 44), (186, 146, 60), (214, 180, 90), (238, 212, 132)],
    "rug_red": [(92, 30, 34), (122, 40, 44), (150, 54, 52), (176, 72, 64), (204, 100, 86)],
}


def ramp(name, k):
    r = RAMPS[name]
    return r[max(0, min(len(r) - 1, k))]


# ---- PNG -------------------------------------------------------------------------------------------------------------

def png(rows, w, h):
    """An RGBA PNG of `rows` (h lists of w (r, g, b, a))."""
    raw = b"".join(b"\x00" + bytes(c for p in row for c in p) for row in rows)

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def solid(c):
    return (c[0], c[1], c[2], 255)


# ---- tiled textures: (side in texels, painter(x, y) -> rgba) ---------------------------------------------------------
#
# Flat areas, a few deliberate shading steps and lit or shaded edges; nothing random.

def flat(name, k):
    return solid(ramp(name, k))


def carpet(name):
    """Carpet tiles half a meter across, laid with their pile turned a quarter each: a dark seam, a lit row and column inside
    each tile's top-left edge, and a few pile lines across each tile (along x in one, along y in the next)."""
    def f(x, y):
        tx, ty, u, v = x // 16, y // 16, x % 16, y % 16
        if u == 0 or v == 0:
            return flat(name, 1)
        if u == 1 or v == 1:
            return flat(name, 3)
        along = v if (tx + ty) % 2 == 0 else u
        return flat(name, 3 if along % 5 == 3 else 2)
    return f


PLANK_LENGTHS = [40, 56, 48, 64, 36, 52, 44, 60]


def planks(x, y):
    """Oak boards a quarter meter wide in staggered lengths: a dark gap between them, a lit edge along each board's top, and a
    grain line along its middle; each board one of three shades."""
    row = y // 8
    length = PLANK_LENGTHS[row % len(PLANK_LENGTHS)]
    offset = (row * 23) % 64
    along = (x + offset) % length
    if y % 8 == 0 or along == 0:
        return flat("oak", 0)
    shade = ((x + offset) // length * 7 + row * 3) % 3 + 1
    if y % 8 == 1:
        return flat("oak", shade + 1)
    if y % 8 == 5 and 4 < along < length - 6:
        return flat("oak", shade - 1)
    return flat("oak", shade)


def kitchen_tiles(x, y):
    """Black and white tiles half a meter across, a gray grout line, each tile's top-left edge a step lighter."""
    u, v = x % 16, y % 16
    if u == 0 or v == 0:
        return flat("granite", 3)
    light = (x // 16 + y // 16) % 2 == 0
    name, k = ("ceramic", 3) if light else ("granite", 1)
    if u == 1 or v == 1:
        k += 1
    elif u == 15 or v == 15:
        k -= 1
    return flat(name, k)


def marble_tiles(x, y):
    """Marble slabs a meter across in two shades, a lit top-left edge and a shaded bottom-right one, and one drawn vein across
    each light slab (a stepped diagonal)."""
    u, v = x % 32, y % 32
    if u == 0 or v == 0:
        return flat("marble", 1)
    light = (x // 32 + y // 32) % 2 == 0
    k = 3 if light else 2
    if u == 1 or v == 1:
        return flat("marble", k + 1)
    if u == 31 or v == 31:
        return flat("marble", k - 1)
    if light and (u - v // 2) == 9 and 4 < v < 28:
        return flat("marble", 2)
    return flat("marble", k)


def lino(x, y):
    """Sheet flooring: flat, with a faint seam every meter."""
    if x % 32 == 0 or y % 32 == 0:
        return flat("beige", 2)
    return flat("beige", 3)


def wall(name):
    return lambda x, y: flat(name, 3)


def ceiling_tiles(x, y):
    u, v = x % 16, y % 16
    if u == 0 or v == 0:
        return flat("ceiling", 1)
    if u == 15 or v == 15:
        return flat("ceiling", 2)
    if (u, v) in ((4, 5), (11, 9), (7, 12), (13, 3)):
        return flat("ceiling", 2)
    return flat("ceiling", 3)


def facade(x, y):
    # Concrete panels 2 m wide, 1 m tall: a lit joint above and left of each, a shadowed one below and right.
    if x % 64 == 63 or y % 32 == 31:
        return flat("facade", 0)
    if x % 64 == 0 or y % 32 == 0:
        return flat("facade", 3)
    return flat("facade", 2)


def plinth(x, y):
    if y % 16 == 15 or (x + (y // 16) * 16) % 32 == 0:
        return flat("plinth", 0)
    if y % 16 == 0:
        return flat("plinth", 3)
    return flat("plinth", 2)


def roof_membrane(x, y):
    """A flat roof's membrane in sheets a meter wide: a raised seam (lit, then shaded) between them."""
    u = x % 32
    if u == 0:
        return flat("gravel", 3)
    if u == 1:
        return flat("gravel", 1)
    return flat("gravel", 2)


def glass(x, y):
    # See-through: a pale blue tint, a few diagonal glints a little less clear.
    if (x + y) % 23 in (0, 1) or (x + y) % 23 == 5:
        return (222, 238, 246, 120)
    return (150, 196, 218, 64)


TILED = {
    "carpet": (64, carpet("navy")),
    "carpet_teal": (64, carpet("teal")),
    "wood_floor": (64, planks),
    "tile_floor": (32, kitchen_tiles),
    "marble": (64, marble_tiles),
    "linoleum": (32, lino),
    "plaster": (32, wall("plaster")),
    "plaster_teal": (32, wall("plaster_teal")),
    "ceiling": (64, ceiling_tiles),
    "facade": (64, facade),
    "plinth": (32, plinth),
    "roof": (32, roof_membrane),
    "glass": (32, glass),
}


# ---- panel textures: each face its own rectangle (atlas per material) -----------------------------------------------
#
# A panel is flat (step 2 of its ramp) with a lit top-left edge and a shaded bottom-right one; a pattern adds a few deliberate
# lines over that (pattern(w, h, x, y, k) -> the step).

def grain_lines(w, h, x, y, k):
    """Wood: a grain line every six texels, broken in a fixed rhythm."""
    return 1 if (y % 6 == 3 and (x + 2 * (y // 6)) % 9 not in (0, 1)) else k


def stitched(w, h, x, y, k):
    """Upholstery: a dashed seam two texels in from the edge."""
    if min(w, h) < 8:
        return k
    edge = min(x, y, w - 1 - x, h - 1 - y)
    return 1 if edge == 2 and (x + y) % 3 != 0 else k


def sheen(w, h, x, y, k):
    """Brushed metal: one lit band a third of the way across."""
    if w < 8:
        return k
    return 3 if x in (w // 3, w // 3 + 1) else k


LEAF = [[2, 3, 3, 2], [3, 4, 3, 1], [2, 3, 2, 1], [1, 2, 1, 1]]


def foliage(w, h, x, y, k):
    """Leaves: a four-texel leaf stamped in rows, each row shifted by two."""
    return LEAF[y % 4][(x + 2 * (y // 4)) % 4]


def clods(w, h, x, y, k):
    """Potting soil: dark, with a lighter clod in a fixed scatter."""
    return 2 if (x % 5, y % 4) in ((1, 1), (3, 3)) else 1


def counter(w, h, x, y, k):
    """A stone top: flat, a lit row along its front edge."""
    return k


PANELS = {
    "wood": ("oak", grain_lines),
    "dark_wood": ("walnut", grain_lines),
    "metal": ("metal", sheen),
    "dark_metal": ("dark_metal", None),
    "desk_blue": ("laminate", None),
    "fabric_black": ("charcoal", stitched),
    "fabric_beige": ("beige", stitched),
    "fabric_orange": ("orange", stitched),
    "leaves": ("leaves", foliage),
    "trunk": ("walnut", grain_lines),
    "terracotta": ("terracotta", None),
    "soil": ("soil", clods),
    "paper": ("paper", None),
    "granite": ("granite", counter),
    "steel": ("steel", sheen),
    "ceramic": ("ceramic", None),
    "water": ("water", None),
    "marble_top": ("marble", None),
    "frame": ("frame", None),
    "trim": ("plaster", None),
    "gold": ("gold", None),
    "light_glow": ("light", None),
    # Drawn whole (their own pictures), each over a ramp for the bevel. The screens' names end in `_glow`: drawn unlit.
    "screen_glow": ("dark_metal", "screen"),
    "chart_glow": ("dark_metal", "chart"),
    "keyboard": ("dark_metal", "keyboard"),
    "books": ("walnut", "books"),
    "whiteboard": ("metal", "whiteboard"),
    "art": ("gold", "art"),
    "rug": ("rug_red", "rug"),
}


def picture(kind, w, h, x, y):
    """The color of texel (x, y) of a `kind` picture w x h, or None for the panel's own."""
    edge = min(x, y, w - 1 - x, h - 1 - y)
    # The screens are drawn unlit (their own light, which the view's exposure lifts with the room's): the bezel nearly black
    # so it stays dark, the picture a little dim so it does not wash out.
    if kind == "screen":
        # A desktop: a bezel, a title bar, and a few rows of text in a window.
        if edge < 3:
            return (8, 9, 12) if edge < 2 else (2, 3, 5)
        if y < 5:
            return (44, 70, 120)
        rows = (y - 6) % 3
        lengths = [0.8, 0.55, 0.7, 0.4, 0.65]
        if rows == 0 and 5 <= x < 5 + (w - 10) * lengths[(y // 3) % len(lengths)]:
            return (120, 160, 200)
        return (16, 26, 48)
    if kind == "chart":
        # A presentation: a pale slide with a title and a bar chart.
        if edge < 3:
            return (8, 9, 12) if edge < 2 else (2, 3, 5)
        bars = [(64, 126, 200), (230, 150, 60), (90, 170, 110), (200, 80, 90)]
        base = h - 6
        if 4 <= y <= 5 and 6 <= x < w // 2:
            return (40, 52, 76)
        if y == base and 5 <= x < w - 5:
            return (110, 118, 130)
        slot = (x - 8) // 6
        if 0 <= slot < 6 and (x - 8) % 6 < 4 and y < base:
            top = base - (4 + (slot * 5 + 3) % 9) * max(1, (h - 14) // 12)
            if y >= max(8, top):
                return bars[slot % len(bars)]
        return (200, 204, 212)
    if kind == "keyboard":
        if edge < 1:
            return None
        return ramp("dark_metal", 0) if x % 3 == 0 or y % 3 == 0 else ramp("dark_metal", 3)
    if kind == "books":
        if y % 16 in (0, 15) or edge < 1:
            return None
        shelf = y // 16
        spine = (x // 3 + shelf * 5) % 7
        colors = [(160, 50, 46), (46, 90, 150), (60, 130, 80), (200, 160, 70), (110, 60, 120), (220, 220, 210),
                  (46, 46, 54)]
        c = colors[spine]
        top = 16 * shelf + 2 + (x * 13 + shelf) % 4
        if y < top:
            return ramp("walnut", 0)
        k = -34 if x % 3 == 2 else 18 if x % 3 == 0 else 0
        return tuple(max(0, min(255, v + k)) for v in c)
    if kind == "whiteboard":
        if edge < 1:
            return None
        if edge == 1:
            return ramp("metal", 2)
        s1 = abs(math.sin(x * 0.45) * 3 + h * 0.3 - y) < 0.7 and x < w * 0.6
        s2 = abs(x * 0.4 + h * 0.15 - y) < 0.6 and w * 0.55 < x < w * 0.85
        if s1:
            return (54, 84, 176)
        if s2:
            return (190, 60, 60)
        return (244, 244, 240)
    if kind == "art":
        if edge < 2:
            return None
        if y < h * 0.5:
            sun = (x - w * 0.7) ** 2 + (y - h * 0.25) ** 2 < (w * 0.1) ** 2
            return (250, 210, 90) if sun else (140, 190, 230)
        hill = h * 0.55 + 3 * math.sin(x * 0.4)
        return (120, 170, 110) if y < hill else (76, 130, 80)
    if kind == "rug":
        if edge < 2:
            return ramp("rug_red", 0)
        if edge < 4:
            return ramp("gold", 3)
        return ramp("rug_red", 3 if (x // 4 + y // 4) % 2 else 2)
    return None


def draw_panel(material, w, h):
    name, pattern = PANELS[material]
    rows = []
    for y in range(h):
        row = []
        for x in range(w):
            k = 2
            if callable(pattern):
                k = pattern(w, h, x, y, k)
            if w >= 4 and h >= 4:
                if x == 0 and y == 0:
                    k = 4
                elif x == w - 1 and y == h - 1:
                    k = 0
                elif y == 0 or x == 0:
                    k = max(k, 3)
                elif y == h - 1 or x == w - 1:
                    k = min(k, 1)
            c = ramp(name, k)
            if isinstance(pattern, str):
                p = picture(pattern, w, h, x, y)
                if p is not None:
                    c = p
            row.append(solid(c))
        rows.append(row)
    return rows


class Atlas:
    """A material's rectangles, packed in shelves."""

    def __init__(self, material):
        self.material = material
        self.rects = {}      # (w, h) -> (x, y)
        self.order = []

    def rect(self, w, h):
        key = (w, h)
        if key not in self.rects:
            self.rects[key] = None
            self.order.append(key)
        return key

    def pack(self):
        width = 256
        while any(w > width for w, _ in self.order):
            width *= 2
        x = y = shelf = 0
        for w, h in sorted(self.order, key=lambda k: (-k[1], -k[0])):
            if x + w > width:
                x, y, shelf = 0, y + shelf, 0
            self.rects[(w, h)] = (x, y)
            x += w
            shelf = max(shelf, h)
        height = 16
        while height < y + shelf:
            height *= 2
        self.size = (width, height)

    def image(self):
        w, h = self.size
        rows = [[(0, 0, 0, 255)] * w for _ in range(h)]
        for (rw, rh), (x0, y0) in sorted(self.rects.items()):
            panel = draw_panel(self.material, rw, rh)
            for y in range(rh):
                rows[y0 + y][x0:x0 + rw] = panel[y]
        return rows


ATLASES = {m: Atlas(m) for m in PANELS}
MATERIALS = list(TILED) + list(PANELS)
TEX_INDEX = {name: i for i, name in enumerate(MATERIALS)}


# ---- elements --------------------------------------------------------------------------------------------------------

elements = []
groups = []          # (name, [uuids])
current = None
fixtures = []        # ceiling lights: (x, y) in the office's numbers


def new_uuid():
    return str(uuid.UUID(int=UUID_RNG.getrandbits(128), version=4))


def group(name):
    global current
    current = (name, [])
    groups.append(current)


def bb(p):
    """Torque (x, y, z) in meters, the office's corner at the origin, into Blockbench units (16 a meter, y up)."""
    x, y, z = p
    return [round((x - W / 2) * 16, 4), round(z * 16, 4), round(-(y - D / 2) * 16, 4)]


def box(name, lo, hi, material, faces=None, turn=None, skip=()):
    """A box from lo to hi (meters) of `material` (per-face materials in `faces`, by Blockbench face name: north is +y,
    south -y, east +x, west -x, up +z), turned by `turn` = (yaw degrees, about (x, y)) as Blockbench turns. A tiled
    material repeats at 32 texels a meter; a panel material gives the face its own rectangle of its size."""
    a, b = bb(lo), bb(hi)
    frm = [min(a[k], b[k]) for k in range(3)]
    to = [max(a[k], b[k]) for k in range(3)]
    size = [(to[k] - frm[k]) / 16 for k in range(3)]
    spans = {"north": (size[0], size[1]), "south": (size[0], size[1]), "east": (size[2], size[1]),
             "west": (size[2], size[1]), "up": (size[0], size[2]), "down": (size[0], size[2])}
    fs = {}
    for face, (u, v) in spans.items():
        if face in skip:
            continue
        m = (faces or {}).get(face, material)
        if m in TILED:
            fs[face] = {"uv": [0, 0, round(u * TPM, 4), round(v * TPM, 4)], "texture": TEX_INDEX[m]}
        else:
            w, h = max(2, round(u * TPM)), max(2, round(v * TPM))
            fs[face] = {"rect": ATLASES[m].rect(w, h), "material": m, "texture": TEX_INDEX[m]}
    e = {"name": name, "box_uv": False, "rescale": False, "locked": False, "render_order": "default",
         "allow_mirror_modeling": True, "from": frm, "to": to, "autouv": 0, "color": 0, "inflate": 0,
         "origin": [0, 0, 0], "faces": fs, "type": "cube", "uuid": new_uuid()}
    if turn:
        yaw, (cx, cy) = turn
        e["origin"] = bb((cx, cy, 0))
        e["rotation"] = [0, round(yaw, 3), 0]
    elements.append(e)
    current[1].append(e["uuid"])


def finish_uvs():
    for a in ATLASES.values():
        if a.order:
            a.pack()
    for e in elements:
        for f in e["faces"].values():
            if "rect" in f:
                w, h = f.pop("rect")
                x, y = ATLASES[f.pop("material")].rects[(w, h)]
                f["uv"] = [x, y, x + w, y + h]


def lifted(dz):
    """`box`, everything raised by dz (furniture standing on a rug: its underside off the rug's, so no two faces meet)."""
    def put(name, lo, hi, material, faces=None, turn=None, skip=()):
        return box(name, (lo[0], lo[1], lo[2] + dz), (hi[0], hi[1], hi[2] + dz), material, faces, turn, skip)
    return put


# ---- furniture (each built facing +y, then turned) ---------------------------------------------------------------------

def chair(name, cx, cy, yaw, fabric="fabric_black", office=True, lift=0.0):
    """An office chair (or a four-legged one) at (cx, cy), its sitter facing yaw; `lift` stands it on a rug."""
    t = (yaw, (cx, cy))
    put = lifted(lift)
    if office:
        # The base's arms lie on the floor (casters, four more boxes a chair, would cut the office into two interiors).
        put(name + "baseA", (cx - 0.3, cy - 0.03, 0), (cx + 0.3, cy + 0.03, 0.06), "dark_metal", turn=t)
        put(name + "baseB", (cx - 0.03, cy - 0.3, 0), (cx + 0.03, cy - 0.03, 0.06), "dark_metal", turn=t)
        put(name + "baseC", (cx - 0.03, cy + 0.03, 0), (cx + 0.03, cy + 0.3, 0.06), "dark_metal", turn=t)
        put(name + "post", (cx - 0.035, cy - 0.035, 0.06), (cx + 0.035, cy + 0.035, 0.42), "dark_metal", turn=t)
    else:
        for i, (dx, dy) in enumerate([(-0.2, -0.2), (0.2, -0.2), (-0.2, 0.2), (0.2, 0.2)]):
            put(name + "leg" + "abcd"[i], (cx + dx - 0.025, cy + dy - 0.025, 0), (cx + dx + 0.025, cy + dy + 0.025, 0.42),
                "dark_metal", turn=t)
    put(name + "seat", (cx - 0.25, cy - 0.25, 0.42), (cx + 0.25, cy + 0.25, 0.5), fabric, turn=t)
    put(name + "back", (cx - 0.23, cy - 0.3, 0.5), (cx + 0.23, cy - 0.25, 1.0), fabric, turn=t)


def plant(name, cx, cy, size=1.0):
    s = size
    box(name + "pot", (cx - 0.25 * s, cy - 0.25 * s, 0), (cx + 0.25 * s, cy + 0.25 * s, 0.45 * s), "terracotta")
    box(name + "soil", (cx - 0.21 * s, cy - 0.21 * s, 0.45 * s), (cx + 0.21 * s, cy + 0.21 * s, 0.48 * s), "soil")
    box(name + "trunk", (cx - 0.04, cy - 0.04, 0.48 * s), (cx + 0.04, cy + 0.04, 1.0 * s), "trunk")
    box(name + "crown", (cx - 0.35 * s, cy - 0.35 * s, 1.0 * s), (cx + 0.35 * s, cy + 0.35 * s, 1.5 * s), "leaves")
    box(name + "top", (cx - 0.22 * s, cy - 0.22 * s, 1.5 * s), (cx + 0.22 * s, cy + 0.22 * s, 1.8 * s), "leaves")
    box(name + "tuftA", (cx + 0.35 * s, cy - 0.15 * s, 1.1 * s), (cx + 0.5 * s, cy + 0.15 * s, 1.35 * s), "leaves")
    box(name + "tuftB", (cx - 0.5 * s, cy - 0.1 * s, 1.2 * s), (cx - 0.35 * s, cy + 0.2 * s, 1.4 * s), "leaves")


def monitor(name, cx, cy, yaw):
    t = (yaw, (cx, cy))
    box(name + "foot", (cx - 0.12, cy - 0.08, 0.76), (cx + 0.12, cy + 0.08, 0.78), "dark_metal", turn=t)
    box(name + "neck", (cx - 0.025, cy + 0.02, 0.78), (cx + 0.025, cy + 0.06, 0.95), "dark_metal", turn=t)
    box(name + "screen", (cx - 0.3, cy + 0.06, 0.9), (cx + 0.3, cy + 0.1, 1.25), "dark_metal",
        faces={"north": "screen_glow"}, turn=t)


def laptop(name, cx, cy, yaw, top=0.78):
    """A laptop open on a table whose top is at `top`, its screen toward its user (facing yaw)."""
    t = (yaw, (cx, cy))
    box(name + "base", (cx - 0.17, cy - 0.12, top), (cx + 0.17, cy + 0.12, top + 0.02), "dark_metal",
        faces={"up": "keyboard"}, turn=t)
    box(name + "lid", (cx - 0.17, cy + 0.1, top + 0.02), (cx + 0.17, cy + 0.12, top + 0.25), "dark_metal",
        faces={"south": "screen_glow"}, turn=t)


def desk(name, x0, y0, x1, y1, top="desk_blue", sides=True):
    box(name + "top", (x0, y0, 0.72), (x1, y1, 0.76), top)
    if sides:
        box(name + "sideA", (x0 + 0.02, y0 + 0.05, 0), (x0 + 0.06, y1 - 0.05, 0.72), "metal")
        box(name + "sideB", (x1 - 0.06, y0 + 0.05, 0), (x1 - 0.02, y1 - 0.05, 0.72), "metal")


def bookshelf(name, x0, y0, x1, y1, h=2.0):
    box(name, (x0, y0, 0), (x1, y1, h), "wood", faces={"south" if y1 - y0 < x1 - x0 else "west": "books"})


def sofa(name, x0, y0, x1, fabric="fabric_beige", depth=0.9, facing_north=True, lift=0.0):
    """A sofa along x from x0 to x1, its seat from y0 (front edge, facing north) to y0 - depth or back again."""
    put = lifted(lift)
    if facing_north:
        ya, yb, back = y0 - depth, y0, (y0 - depth, y0 - depth + 0.25)
    else:
        ya, yb, back = y0, y0 + depth, (y0 + depth - 0.25, y0 + depth)
    put(name + "base", (x0, ya, 0), (x1, yb, 0.42), "dark_wood", faces={"up": fabric})
    put(name + "back", (x0 + 0.22, back[0], 0.42), (x1 - 0.22, back[1], 0.9), fabric)
    put(name + "armA", (x0, ya, 0.42), (x0 + 0.22, yb, 0.65), fabric)
    put(name + "armB", (x1 - 0.22, ya, 0.42), (x1, yb, 0.65), fabric)


def ceiling_light(name, cx, cy, along_x=True):
    """A light panel 1.2 x 0.6 m under the ceiling, its spot light below it (in the map)."""
    hx, hy = (0.6, 0.3) if along_x else (0.3, 0.6)
    box(name, (cx - hx, cy - hy, H - 0.05), (cx + hx, cy + hy, H), "frame", faces={"down": "light_glow"})
    fixtures.append((cx, cy))


# ---- the building ------------------------------------------------------------------------------------------------------

T = 0.3            # outer wall thickness
SILL, HEAD = 0.9, 2.6
PANE = (-0.17, -0.13)   # the glass's depth in a wall (from its inner face, outwards negative)


def floors():
    group("floor")
    zones = [("lobby floor", 0, 0, 12, 15, "marble"), ("open plan floor", 12, 0, 36, 13, "carpet"),
             ("corridor floor", 12, 13, 36, 15, "carpet"), ("kitchen floor", 0, 15, 10, 24, "tile_floor"),
             ("meeting floor", 10, 15, 20, 24, "wood_floor"), ("print floor", 20, 15, 26, 24, "linoleum"),
             ("private floor", 26, 15, 36, 24, "carpet_teal")]
    for name, x0, y0, x1, y1, m in zones:
        box(name, (x0, y0, -0.2), (x1, y1, 0), m)


def wall_run(name, axis, fixed, inside, a0, a1, openings, outside_face, inside_face):
    """An outer wall along `axis` ('x' or 'y') from a0 to a1, its inner face at `fixed` (`inside` +1 when the room is on the
    + side, -1 when on the - side; the wall is T thick outward), with window openings (center, width) and doors (center,
    width, 'door'; a window over a counter 'bare', without its sill): each window a glass pane with a mullion in the middle of the wall and a sill inside."""
    def put(n, s0, s1, z0, z1, d0=0.0, d1=None, material="plaster", faces=None):
        # d: from the wall's inner face (0), outward negative, inward positive; the whole thickness by default
        d1 = -T if d1 is None else d1
        lo_d, hi_d = sorted((fixed + inside * d0, fixed + inside * d1))
        if axis == "x":
            box(n, (s0, lo_d, z0), (s1, hi_d, z1), material, faces=faces)
        else:
            box(n, (lo_d, s0, z0), (hi_d, s1, z1), material, faces=faces)

    full = {outside_face: "facade", inside_face: "plaster"}
    cuts = sorted(openings)
    at = a0
    for o in cuts:
        c, w = o[0], o[1]
        s0, s1 = c - w / 2, c + w / 2
        if s0 > at:
            put(name + " pier", at, s0, -0.2, H, faces=full)
        if len(o) > 2 and o[2] == "door":      # a door: only a lintel
            put(name + " lintel", s0, s1, HEAD, H, faces=full)
        else:
            put(name + " under", s0, s1, -0.2, SILL, faces=full)
            put(name + " over", s0, s1, HEAD, H, faces=full)
            m = (c - 0.03, c + 0.03)
            put(name + " glass", s0, m[0], SILL, HEAD, PANE[0], PANE[1], "glass")
            put(name + " glass", m[1], s1, SILL, HEAD, PANE[0], PANE[1], "glass")
            put(name + " mullion", m[0], m[1], SILL, HEAD, -0.2, -0.1, "frame")
            if len(o) < 3:      # 'bare': over a counter, no sill
                put(name + " sill", s0 - 0.05, s1 + 0.05, SILL - 0.04, SILL, 0.12, 0.0, "trim")
        at = s1
    if a1 > at:
        put(name + " pier", at, a1, -0.2, H, faces=full)


def outer_walls():
    group("outer walls")
    wins = lambda cs, w=1.6: [(c, w) for c in cs]
    # South (y 0, room to +y): the entrance (x 3.5 to 6.5) and five windows.
    wall_run("south wall", "x", 0.0, 1, -T, W + T, [(5.0, 3.0, "door")] + wins([9.0, 15.0, 21.0, 27.0, 33.0]),
             "south", "north")
    # North (y D, room to -y).
    wall_run("north wall", "x", D, -1, -T, W + T, [(3.0, 1.6, "bare")] + wins([18.6, 23.0]) + wins([29.0], 1.4),
             "north", "south")
    # West (x 0, room to +x).
    wall_run("west wall", "y", 0.0, 1, 0, D, wins([4.0, 8.0, 21.0]), "west", "east")
    # East (x W, room to -x).
    wall_run("east wall", "y", W, -1, 0, D, wins([4.0, 9.0, 19.5]), "east", "west")
    # A dark plinth around the foot of the facade.
    box("plinth south", (-T - 0.02, -T - 0.02, -0.2), (3.5, -T, 0.4), "plinth")
    box("plinth south", (6.5, -T - 0.02, -0.2), (W + T + 0.02, -T, 0.4), "plinth")
    box("plinth north", (-T - 0.02, D + T, -0.2), (W + T + 0.02, D + T + 0.02, 0.4), "plinth")
    box("plinth west", (-T - 0.02, -T, -0.2), (-T, D + T, 0.4), "plinth")
    box("plinth east", (W + T, -T, -0.2), (W + T + 0.02, D + T, 0.4), "plinth")


def roof():
    group("roof")
    box("roof", (-T, -T, H), (W + T, D + T, H + ROOF), "facade", faces={"down": "ceiling", "up": "roof"})
    top = H + ROOF
    p = 0.25
    box("parapet south", (-T, -T, top), (W + T, -T + p, top + 0.5), "facade", faces={"up": "trim"})
    box("parapet north", (-T, D + T - p, top), (W + T, D + T, top + 0.5), "facade", faces={"up": "trim"})
    box("parapet west", (-T, -T + p, top), (-T + p, D + T - p, top + 0.5), "facade", faces={"up": "trim"})
    box("parapet east", (W + T - p, -T + p, top), (W + T, D + T - p, top + 0.5), "facade", faces={"up": "trim"})
    for i, (x, y) in enumerate([(8.0, 18.0), (24.0, 6.0), (28.0, 18.0)]):
        box("roof unit %d" % i, (x, y, top), (x + 2.4, y + 1.6, top + 1.1), "steel", faces={"up": "metal"})
        box("roof unit fan %d" % i, (x + 0.4, y + 0.3, top + 1.1), (x + 1.4, y + 1.3, top + 1.2), "dark_metal")
    box("roof hatch", (4.0, 4.0, top), (5.2, 5.2, top + 0.4), "metal")
    # The entrance canopy.
    box("canopy", (3.0, -1.6, 2.7), (7.0, -T, 2.9), "frame", faces={"down": "light_glow"})


def inner_walls():
    group("inner walls")
    y0, y1 = 14.9, 15.1
    door = 2.0
    # The corridor wall: doors into the kitchen, meeting room, print room and private office; the meeting room's glass.
    spans = [(0, 6, "plaster"), (8, 10, "plaster"), (20, 22, "plaster"), (24, 26, "plaster"), (26, 30, "plaster"),
             (32, W, "plaster")]
    for x0, x1, m in spans:
        faces = {"north": "plaster_teal"} if x0 >= 26 else None
        box("corridor wall", (x0, y0, 0), (x1, y1, H), m, faces=faces)
    for x0, x1 in [(10.0, 14.0), (16.0, 20.0)]:
        glass_wall("meeting glass", "x", (y0 + y1) / 2, x0, x1)
    for x0 in [6, 14, 22, 30]:
        box("door head", (x0, y0, 2.6), (x0 + door, y1, H), "plaster")
    # Room walls north of the corridor.
    box("kitchen wall", (9.9, y1, 0), (10.1, D, H), "plaster")
    glass_wall("meeting glass east", "y", 20.0, y1, D)
    box("private wall", (25.9, y1, 0), (26.1, D, H), "plaster", faces={"east": "plaster_teal"})
    # Between the lobby and the open plan, open toward the corridor.
    box("lobby wall", (11.9, 0, 0), (12.1, 9.0, H), "plaster")


def glass_wall(name, axis, at, a0, a1):
    """A glass partition from floor to ceiling along `axis` at `at`, from a0 to a1: posts every 2 m or less, a rail at the
    floor and the ceiling, glass between (all 0.1 m deep but the glass, 0.04 m in the middle)."""
    n = max(1, round((a1 - a0) / 2.0))
    step = (a1 - a0) / n
    posts = [a0 + k * step for k in range(n + 1)]

    def put(nm, s0, s1, z0, z1, d, material):
        if axis == "x":
            box(nm, (s0, at - d, z0), (s1, at + d, z1), material)
        else:
            box(nm, (at - d, s0, z0), (at + d, s1, z1), material)

    for k, p in enumerate(posts):
        s0, s1 = max(a0, p - 0.04), min(a1, p + 0.04)
        put(name + " post", s0, s1, 0, H, 0.05, "frame")
    for k in range(n):
        s0, s1 = min(a1, posts[k] + 0.04), max(a0, posts[k + 1] - 0.04)
        put(name + " rail", s0, s1, 0, 0.1, 0.05, "frame")
        put(name + " head", s0, s1, H - 0.1, H, 0.05, "frame")
        put(name + " pane", s0, s1, 0.1, H - 0.1, 0.02, "glass")


def lobby():
    group("lobby")
    box("reception counter", (3.5, 9.0, 0), (8.5, 9.6, 1.05), "dark_wood")
    box("reception top", (3.4, 8.95, 1.05), (8.6, 9.7, 1.1), "marble_top")
    box("reception wing", (8.0, 9.6, 0), (8.6, 11.0, 0.76), "dark_wood", faces={"up": "marble_top"})
    monitor("reception monitor", 5.5, 9.9, 0)
    box("reception desk", (3.5, 9.6, 0.72), (8.0, 10.4, 0.76), "wood")
    chair("reception chair", 5.5, 10.9, 180)
    box("lobby rug", (0.6, 2.4, 0), (5.0, 6.4, 0.02), "rug")
    sofa("lobby sofa south", 0.9, 3.2, 4.6, facing_north=True, depth=0.9, lift=0.02)
    sofa("lobby sofa north", 0.9, 5.7, 4.6, facing_north=False, depth=0.9, lift=0.02)
    box("coffee table", (1.8, 4.0, 0.3), (3.8, 4.9, 0.36), "wood")
    box("coffee table base", (2.1, 4.2, 0.02), (3.5, 4.7, 0.3), "dark_wood")
    box("magazines", (2.3, 4.3, 0.36), (2.7, 4.6, 0.39), "paper")
    plant("lobby plant west", 0.8, 14.0, 1.2)
    plant("lobby plant east", 11.2, 1.0, 1.2)
    plant("lobby plant reception", 2.6, 9.4, 0.9)
    box("cooler stand", (11.3, 6.5, 0), (11.8, 7.0, 0.9), "ceramic")
    box("cooler bottle", (11.37, 6.57, 0.9), (11.73, 6.93, 1.35), "water")
    box("lobby art", (11.86, 3.0, 1.2), (11.9, 5.0, 2.2), "art", faces={"west": "art"})
    box("bench", (8.0, 0.6, 0), (10.6, 1.1, 0.45), "wood")
    ceiling_light("lobby light a", 2.8, 4.4)
    ceiling_light("lobby light b", 8.0, 4.4)
    ceiling_light("lobby light c", 6.0, 12.4)


def open_plan():
    group("open plan")
    pod = 0
    for x0 in [13.0, 18.5, 24.0, 29.5]:
        for y0 in [2.2, 8.4]:
            pod += 1
            n = "pod%s " % "abcdefgh"[pod - 1]
            box(n + "middle", (x0, y0 + 1.17, 0), (x0 + 4.0, y0 + 1.23, 1.3), "fabric_black")
            for side, (ya, yb) in [("s", (y0, y0 + 1.17)), ("n", (y0 + 1.23, y0 + 2.4))]:
                box(n + "divider " + side, (x0 + 1.97, ya, 0), (x0 + 2.03, yb, 1.3), "fabric_black")
                for half, (xa, xb) in [("w", (x0, x0 + 1.97)), ("e", (x0 + 2.03, x0 + 4.0))]:
                    dn = n + side + half + " "
                    if side == "s":
                        desk(dn + "desk ", xa + 0.05, y0 + 0.2, xb - 0.05, y0 + 1.15)
                        cx = (xa + xb) / 2
                        monitor(dn, cx, y0 + 0.85, 180)
                        box(dn + "keys", (cx - 0.22, y0 + 0.35, 0.76), (cx + 0.22, y0 + 0.5, 0.78), "dark_metal",
                            faces={"up": "keyboard"})
                        chair(dn + "chair ", cx + RNG.uniform(-0.2, 0.2), y0 - 0.45, RNG.uniform(-25, 25))
                    else:
                        desk(dn + "desk ", xa + 0.05, y0 + 1.25, xb - 0.05, y0 + 2.2)
                        cx = (xa + xb) / 2
                        monitor(dn, cx, y0 + 1.55, 0)
                        box(dn + "keys", (cx - 0.22, y0 + 1.9, 0.76), (cx + 0.22, y0 + 2.05, 0.78), "dark_metal",
                            faces={"up": "keyboard"})
                        chair(dn + "chair ", cx + RNG.uniform(-0.2, 0.2), y0 + 2.85, 180 + RNG.uniform(-25, 25))
                    if RNG.random() < 0.5:
                        mx = xa + 0.25
                        my = y0 + 0.4 if side == "s" else y0 + 1.95
                        box(dn + "mug", (mx - 0.05, my - 0.05, 0.76), (mx + 0.05, my + 0.05, 0.88), "ceramic")
            ceiling_light(n + "light", x0 + 2.0, y0 + 1.2)
    for i, y in enumerate([1.5, 4.5, 7.5]):
        box("cabinet %s" % "abc"[i], (35.3, y, 0), (35.95, y + 1.2, 1.3), "metal")
    bookshelf("open shelf", 35.55, 10.0, 35.95, 12.4, 2.0)
    plant("open plant south", 34.8, 0.8, 1.1)
    plant("open plant corridor", 12.8, 12.4, 1.0)
    box("open whiteboard", (12.1, 3.0, 1.0), (12.14, 6.0, 2.2), "metal", faces={"east": "whiteboard"})


def kitchen():
    group("kitchen")
    box("counter base", (0.3, 23.3, 0), (6.5, 24.0, 0.9), "dark_wood")
    box("counter top", (0.25, 23.25, 0.9), (6.55, 24.0, 0.95), "granite")
    box("sink", (2.0, 23.4, 0.95), (2.8, 23.85, 1.0), "steel")
    box("coffee machine", (4.5, 23.5, 0.95), (4.9, 23.9, 1.4), "dark_metal")
    box("upper cabinets", (0.3, 23.6, 1.6), (2.0, 24.0, 2.3), "dark_wood")
    box("upper cabinets east", (4.0, 23.6, 1.6), (6.5, 24.0, 2.3), "dark_wood")
    box("fridge", (7.0, 23.1, 0), (8.0, 24.0, 2.0), "steel")
    box("kitchen table", (2.8, 18.3, 0.72), (4.4, 19.9, 0.76), "wood")
    box("kitchen table post", (3.55, 19.05, 0), (3.65, 19.15, 0.72), "dark_metal")
    for i, (cx, cy, yaw) in enumerate([(3.6, 17.6, 0), (3.6, 20.6, 180), (2.1, 19.1, -90), (5.1, 19.1, 90)]):
        chair("bistro chair %s " % "abcd"[i], cx, cy, yaw + RNG.uniform(-10, 10), fabric="fabric_orange", office=False)
    box("kitchen cooler stand", (9.2, 16.0, 0), (9.7, 16.5, 0.9), "ceramic")
    box("kitchen cooler bottle", (9.27, 16.07, 0.9), (9.63, 16.43, 1.35), "water")
    box("bin", (8.5, 15.3, 0), (8.9, 15.7, 0.6), "metal")
    plant("kitchen plant", 0.8, 16.0, 1.0)
    box("kitchen art", (9.86, 18.0, 1.3), (9.9, 19.6, 2.2), "gold", faces={"west": "art"})
    ceiling_light("kitchen light a", 3.6, 19.1)
    ceiling_light("kitchen light b", 6.5, 22.0)


def meeting_room():
    group("meeting room")
    box("meeting table", (12.5, 18.7, 0.72), (17.5, 20.3, 0.78), "dark_wood")
    box("meeting table leg west", (13.0, 19.2, 0), (13.2, 19.8, 0.72), "dark_metal")
    box("meeting table leg east", (16.8, 19.2, 0), (17.0, 19.8, 0.72), "dark_metal")
    for i, x in enumerate([13.3, 15.0, 16.7]):
        chair("meeting chair s%s " % "abc"[i], x, 18.1, RNG.uniform(-12, 12))
        chair("meeting chair n%s " % "abc"[i], x, 20.9, 180 + RNG.uniform(-12, 12))
    chair("meeting chair head ", 18.1, 19.5, 90)
    box("meeting screen", (13.0, 23.8, 1.2), (17.0, 23.9, 2.4), "dark_metal", faces={"south": "chart_glow"})
    laptop("meeting laptop a ", 13.9, 18.95, 0)
    laptop("meeting laptop b ", 16.1, 20.05, 180)
    box("meeting whiteboard", (10.1, 17.0, 1.0), (10.14, 21.0, 2.2), "metal", faces={"east": "whiteboard"})
    plant("meeting plant", 11.0, 23.2, 1.1)
    ceiling_light("meeting light", 15.0, 19.5)


def print_room():
    group("print room")
    box("copier", (21.0, 22.8, 0), (22.4, 23.9, 1.05), "metal", faces={"up": "dark_metal"})
    box("copier tray", (22.4, 23.2, 0.7), (22.7, 23.7, 0.75), "paper")
    for i, y in enumerate([16.0, 17.3, 18.6, 19.9]):
        box("file cabinet %s" % "abcd"[i], (25.2, y, 0), (25.9, y + 1.2, 1.4), "metal", faces={"west": "steel"})
    bookshelf("print shelf", 23.0, 23.55, 25.0, 23.95, 2.1)
    box("paper boxes", (20.3, 15.4, 0), (21.1, 16.0, 0.6), "paper")
    plant("print plant", 20.7, 20.5, 0.9)
    ceiling_light("print light", 23.0, 19.5, along_x=False)


def private_office():
    group("private office")
    desk("exec desk ", 29.0, 20.6, 33.0, 21.6, top="wood", sides=False)
    box("exec desk front", (29.0, 20.6, 0), (33.0, 20.7, 0.72), "dark_wood")
    box("exec desk end west", (29.0, 20.7, 0), (29.1, 21.6, 0.72), "dark_wood")
    box("exec desk end east", (32.9, 20.7, 0), (33.0, 21.6, 0.72), "dark_wood")
    monitor("exec ", 31.0, 21.2, 0)
    chair("exec chair ", 31.0, 22.3, 180)
    chair("visitor chair a ", 30.2, 19.6, 0, fabric="fabric_beige", office=False, lift=0.02)
    chair("visitor chair b ", 31.8, 19.6, 0, fabric="fabric_beige", office=False, lift=0.02)
    bookshelf("exec shelf", 33.6, 23.5, 35.6, 23.94, 2.2)
    box("exec rug", (28.0, 17.0, 0), (34.0, 20.0, 0.02), "rug")
    box("armchair", (26.6, 15.6, 0), (27.6, 16.6, 0.45), "fabric_beige")
    box("armchair back", (26.6, 15.6, 0.45), (26.85, 16.6, 1.0), "fabric_beige")
    box("floor lamp", (27.9, 15.5, 0), (28.0, 15.6, 1.6), "dark_metal")
    box("floor lamp shade", (27.75, 15.35, 1.6), (28.15, 15.75, 1.9), "paper")
    plant("private plant", 35.2, 15.8, 1.2)
    box("private art", (26.1, 17.5, 1.3), (26.14, 19.0, 2.2), "gold", faces={"east": "art"})
    ceiling_light("private light", 31.0, 18.5)


def corridor():
    group("corridor")
    plant("corridor plant", 35.3, 14.2, 1.0)
    box("corridor art", (26.8, 14.86, 1.3), (28.8, 14.9, 2.3), "gold", faces={"south": "art"})
    for x in [16.0, 24.0, 32.0]:
        ceiling_light("corridor light", x, 14.0)


def mission():
    lights = []
    for (x, y) in fixtures:
        lights.append('   new SpotLight() { position = "%g %g %g"; rotation = "1 0 0 90"; range = 7; innerAngle = 80; '
                      'outerAngle = 150; brightness = 1.6; color = "1 0.95 0.86"; castShadows = 1; };'
                      % (round(x + CORNER[0], 3), round(y + CORNER[1], 3), round(H - 0.08, 3)))
    return """// Office
// An office floor to walk around: a lobby, rows of desks, a kitchen, a meeting room and a private office.

new SimGroup(MissionGroup)
{
   new ScriptObject(MapInfo) { title = "Office"; spawn = "-12.5 -15 0.2"; spawnYaw = 0; };
   // The sky: a light haze from 1,500 units out, nothing drawn past 3,000.
   new Sky(WorldSky) { visibleDistance = 3000; fogDistance = 1500; fogColor = "0.72 0.8 0.9 1"; };
   // The ground around the building, a hand below its floor.
   new StaticShape() { dataBlock = WorldCubeData; position = "0 0 -0.55"; scale = "80 80 1"; };
   // The office: built from levels/office.bbmodel when the example starts.
%interiors%
   // The ceiling lights: a spot light under each panel, pointing down.
""" + "\n".join(lights) + "\n};\n"


def main():
    floors()
    outer_walls()
    roof()
    inner_walls()
    lobby()
    open_plan()
    kitchen()
    meeting_room()
    print_room()
    private_office()
    corridor()
    finish_uvs()
    textures = []
    for i, name in enumerate(MATERIALS):
        if name in TILED:
            side, fn = TILED[name]
            rows = [[fn(x, y) for x in range(side)] for y in range(side)]
            w = h = side
        else:
            a = ATLASES[name]
            if not a.order:
                a.rect(2, 2)
                a.pack()
            rows = a.image()
            w, h = a.size
        data = png(rows, w, h)
        textures.append({"path": "", "name": "office_%s.png" % name, "folder": "block", "namespace": "", "id": str(i),
                         "width": w, "height": h, "uv_width": w, "uv_height": h, "particle": False,
                         "render_mode": "default", "render_sides": "auto", "frame_time": 1, "frame_order_type": "loop",
                         "frame_order": "", "frame_interpolate": False, "visible": True, "internal": True,
                         "saved": False, "uuid": new_uuid(), "relative_path": "",
                         "source": "data:image/png;base64," + base64.b64encode(data).decode()})
    outliner = [{"name": name, "origin": [0, 0, 0], "color": 0, "uuid": new_uuid(), "export": True,
                 "mirror_uv": False, "isOpen": False, "locked": False, "visibility": True, "autouv": 0,
                 "children": children} for name, children in groups]
    model = {"meta": {"format_version": "4.10", "model_format": "free", "box_uv": False},
             "name": "office", "model_identifier": "", "visible_box": [1, 1, 0], "variable_placeholders": "",
             "variable_placeholder_buttons": [], "timeline_setups": [], "unhandled_root_fields": {},
             "resolution": {"width": 16, "height": 16}, "elements": elements, "outliner": outliner,
             "textures": textures}
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        json.dump(model, f, separators=(",", ":"))
    with open(MIS, "w", encoding="utf-8", newline="\n") as f:
        f.write(mission())
    print("%s: %d elements, %d textures, %d bytes; %d lights" % (os.path.normpath(OUT), len(elements), len(textures),
                                                                os.path.getsize(OUT), len(fixtures)))


if __name__ == "__main__":
    sys.exit(main())
