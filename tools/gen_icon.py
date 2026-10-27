"""生成 hetu 应用图标：assets/icon.png（1024，窗口图标）+ assets/icon.ico（多尺寸，exe 图标）。

用法：python tools/gen_icon.py
依赖：pillow
"""

from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "assets"

S = 4096  # 4x 超采样，最后缩到 1024
RADIUS = int(S * 0.225)
TOP = (99, 102, 241)  # indigo-500
BOTTOM = (14, 165, 233)  # sky-500
ACCENT = (251, 191, 36)  # amber-400


def gradient(size: int) -> Image.Image:
    """对角渐变：左上 indigo → 右下 sky。"""
    img = Image.new("RGB", (size, size))
    px = img.load()
    for y in range(size):
        for x in range(0, size, 8):
            t = (x / size * 0.45 + y / size * 0.55)
            r = round(TOP[0] + (BOTTOM[0] - TOP[0]) * t)
            g = round(TOP[1] + (BOTTOM[1] - TOP[1]) * t)
            b = round(TOP[2] + (BOTTOM[2] - TOP[2]) * t)
            for dx in range(8):
                if x + dx < size:
                    px[x + dx, y] = (r, g, b)
    return img


def build() -> Image.Image:
    bg = gradient(S).convert("RGBA")

    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, S - 1, S - 1], RADIUS, fill=255)

    icon = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    icon.paste(bg, (0, 0), mask)

    # 顶部高光，避免大色块发闷
    gloss = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    gd = ImageDraw.Draw(gloss)
    for i in range(140):
        alpha = int(26 * (1 - i / 140) ** 2)
        gd.rounded_rectangle(
            [0, 0, S - 1, S - 1 - i * 8], RADIUS, outline=(255, 255, 255, alpha), width=10
        )
    icon = Image.alpha_composite(icon, gloss)

    # 拼图版面：上方一张横图，下方两张小图 —— 一眼看出「多图拼成一张」
    draw = ImageDraw.Draw(icon)
    pad = int(S * 0.205)
    box = S - pad * 2  # 内容区边长，整体居中
    gap = int(box * 0.06)
    radius = int(box * 0.085)
    top_h = int(box * 0.44)
    bottom_h = box - top_h - gap
    col_w = (box - gap) // 2
    cells = [
        (pad, pad, box, top_h, False),
        (pad, pad + top_h + gap, col_w, bottom_h, False),
        (pad + col_w + gap, pad + top_h + gap, col_w, bottom_h, True),
    ]
    for x, y, w, h, accent in cells:
        fill = ACCENT if accent else (255, 255, 255, 236)
        draw.rounded_rectangle([x, y, x + w, y + h], radius, fill=fill)

    return icon.resize((1024, 1024), Image.LANCZOS)


def main() -> None:
    ASSETS.mkdir(parents=True, exist_ok=True)
    icon = build()
    icon.save(ASSETS / "icon.png")
    icon.save(
        ASSETS / "icon.ico",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )
    print(f"wrote {ASSETS / 'icon.png'} and icon.ico")


if __name__ == "__main__":
    main()
