"""Render the README GIF. Requires Pillow, a local CLI and the installed SDK.

python scripts/render-demo.py --sdk-python /path/to/sdk-env/python
Select is a labeled illustration; only Core and base SDK checks execute.
"""
import argparse
from functools import lru_cache
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

from PIL import Image, ImageDraw, ImageFont


ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "docs/assets/tokenfold-demo.gif"
W, H, SCALE, FPS = 1280, 720, 2, 10
BG, PANEL, LINE = "#0b1118", "#121e29", "#293b4b"
WHITE, MUTED, MINT, BLUE = "#f3f7fb", "#afc1d0", "#b8f58b", "#79d8eb"
LABELS = ("FIT", "CORE", "TOKENFOLD SELECT")
DURATIONS = (5, 3.5, 4.5)
FIT_CODE = '''from tokenfold import Data, fit

result = fit(
    Data(records, representation="folded"),
    target_tokens=1500,
)
payload = result.require_met()'''
with Image.open(ROOT / "docs/assets/tokenfold-logo.png") as source:
    LOGO = source.convert("RGBA")
LOGO = LOGO.crop(LOGO.getbbox())
LOGO.thumbnail((220 * SCALE, 82 * SCALE), Image.Resampling.LANCZOS)


def evidence(cli, sdk_python):
    """Fail before rendering if the fixture, round trip or shown SDK call drifts."""
    fixture = ROOT / "examples/api_response.json"
    pins = json.loads((ROOT / "tests/fixtures/readme_metrics.json").read_text())
    assert hashlib.sha256(fixture.read_bytes()).hexdigest() == pins["compression"]["api_responses"]["sha256"]
    with tempfile.TemporaryDirectory(prefix="tokenfold-demo-") as directory:
        packed, restored = (Path(directory) / name for name in ("packed.json", "restored.json"))
        run = subprocess.run([cli, "compress", str(fixture), "-o", str(packed),
                              "--receipt-format", "json"], check=True, capture_output=True, text=True)
        report = json.loads(run.stderr)
        subprocess.run([cli, "decode", str(packed), "-o", str(restored)],
                       check=True, capture_output=True)
        assert json.loads(restored.read_bytes()) == json.loads(fixture.read_bytes())
    assert report["estimator"]["is_exact"] and report["estimator"]["model"] == "o200k_base"
    assert report["preset"] == "balanced" and report["pruning"] is None
    assert (report["original_tokens"], report["compressed_tokens"]) == (3812, 1376)
    assert round(report["savings_pct"], 1) == pins["compression"]["api_responses"]["reduction_pct"]
    code = ('import json\nfrom pathlib import Path\nfrom tokenfold.lowlevel import decode\n'
            'records = json.loads(Path("examples/api_response.json").read_bytes())\n'
            + FIT_CODE + '\nassert json.loads(decode(payload.encode("utf-8"))) == records\n'
            'assert len(records["results"]) == 30\n'
            'baseline = fit(Data(records), target_tokens=result.target_tokens)\n'
            'assert baseline.budget_status == "unmet"\n'
            'assert baseline.final_tokens == result.original_tokens\n'
            'print(result.receipt.to_json())')
    receipt = json.loads(subprocess.run([sdk_python, "-c", code], cwd=ROOT, check=True,
                                       capture_output=True, text=True).stdout)
    assert receipt["budget_status"] == "met" and receipt["target_tokens"] == 1500
    assert (receipt["original_tokens"], receipt["final_tokens"]) == (2366, 1376)
    assert receipt["original_tokens"] > receipt["target_tokens"] >= receipt["final_tokens"]
    assert receipt["accounting"]["original_basis"] == receipt["accounting"]["final_basis"] == "exact"
    assert receipt["accounting"]["scope"] == "rendered_text" and not receipt["scorer_attempted"]
    return report, receipt


@lru_cache(maxsize=None)
def font(size, style="regular"):
    names = {
        "regular": ("segoeui.ttf", "DejaVuSans.ttf", "Arial.ttf"),
        "bold": ("segoeuib.ttf", "DejaVuSans-Bold.ttf", "Arial Bold.ttf"),
        "mono": ("consola.ttf", "DejaVuSansMono.ttf", "Andale Mono.ttf"),
    }[style]
    directories = (Path(os.environ.get("WINDIR", "C:/Windows")) / "Fonts",
                   Path("/usr/share/fonts/truetype/dejavu"), Path("/System/Library/Fonts/Supplemental"))
    for directory in directories:
        for name in names:
            if (directory / name).exists():
                return ImageFont.truetype(str(directory / name), size * SCALE)
    raise RuntimeError("Install Segoe UI/Consolas, DejaVu Sans/Mono or Arial/Andale Mono")


def text(draw, x, y, value, size=24, color=WHITE, style="regular"):
    face = font(size, style)
    box = draw.textbbox((x * SCALE, y * SCALE), value, font=face)
    assert box[2] <= W * SCALE and box[3] <= H * SCALE, value
    draw.text((x * SCALE, y * SCALE), value, fill=color, font=face)


def rect(draw, box, fill=PANEL, outline=LINE, radius=16):
    draw.rounded_rectangle(tuple(round(v * SCALE) for v in box), radius=radius * SCALE,
                           fill=fill, outline=outline, width=SCALE)


def line(draw, points, color=LINE, width=2):
    draw.line([(round(x * SCALE), round(y * SCALE)) for x, y in points], fill=color, width=width * SCALE)


def ease(t):
    t = max(0, min(t, 1))
    return t * t * (3 - 2 * t)


def base(scene):
    im = Image.new("RGB", (W * SCALE, H * SCALE), BG)
    d = ImageDraw.Draw(im)
    # A quiet drafting grid, rather than a stock terminal window.
    for x in range(640, W, 48):
        line(d, [(x, 96), (x, 620)], "#101a24", 1)
    for y in range(96, 620, 48):
        line(d, [(640, y), (W - 64, y)], "#101a24", 1)
    im.paste(LOGO, (64 * SCALE, 8 * SCALE), LOGO)
    label = "PYTHON + TYPESCRIPT"
    text(d, 1216 - font(20).getlength(label) / SCALE, 36, label, 20, MUTED)
    line(d, [(64, 91), (1216, 91)])
    for i, label in enumerate(LABELS):
        x = 64 + i * 392
        line(d, [(x, 655), (x + 368, 655)], MINT if i == scene else LINE, 3)
        text(d, x, 672, f"0{i + 1}  {label}", 18, MINT if i == scene else MUTED, "bold")
    return im, d


def render(scene, t, report, receipt):
    im, d = base(scene)
    if scene == 2:
        text(d, 64, 118, "03 / QUERY-AWARE SELECTION", 20, BLUE, "bold")
        text(d, 60, 152, "Tokenfold Select", 64, WHITE, "bold")
        text(d, 64, 236, "Why did checkout fail?", 26, MINT)
        text(d, 64, 272, "CONTEXT", 18, MUTED, "bold")
        text(d, 735, 272, "SELECTED", 18, MINT, "bold")
        groups = ("Required rules", "Checkout: HTTP 504",
                  "Inventory timeout", "Unrelated changelog")
        for i, title in enumerate(groups):
            y = 310 + i * 68
            chosen = i < 3 and t >= 0.6 + i * 0.75
            dim = i == 3 and t >= 3
            rect(d, (64, y, 615, y + 56), fill=BG if dim else PANEL,
                 outline=MINT if chosen else LINE, radius=10)
            text(d, 80, y + 15, title, 23, MUTED if dim else WHITE)
            if chosen:
                text(d, 570, y + 15, "+", 24, MINT, "bold")
                progress = ease((t - 0.6 - i * 0.75) / 0.6)
                line(d, [(631, y + 28), (631 + 88 * progress, y + 28)], MINT)
                rect(d, (735, y, 1216, y + 56), outline=MINT, radius=10)
                text(d, 751, y + 15, title, 23, MINT)
            if dim:
                text(d, 735, y + 15, "Omitted", 22, MUTED)
        text(d, 64, 579, "Core first · Over budget? · allow_selection=True", 20, BLUE)
        text(d, 64, 612, "Approved runtime · Experimental / unqualified · Illustration (not live)", 18, MUTED)

    elif scene == 1:
        text(d, 64, 118, "02 / CORE FIRST / STRUCTURAL EFFICIENCY", 20, BLUE, "bold")
        text(d, 60, 154, "Less repetition. Same JSON value.", 51, WHITE, "bold")
        text(d, 64, 227, "Formatted JSON fixture / balanced preset / exact o200k_base counts", 23, MUTED)
        before, after = report["original_tokens"], report["compressed_tokens"]
        rect(d, (64, 292, 813, 553))
        text(d, 90, 314, "FORMATTED JSON", 19, MUTED, "bold")
        text(d, 615, 303, f"{before:,}", 38, WHITE, "bold")
        rect(d, (90, 366, 779, 390), BLUE, BLUE, 7)
        text(d, 90, 422, "CORE LOSSLESS", 19, MINT, "bold")
        text(d, 615, 410, f"{after:,}", 38, MINT, "bold")
        rect(d, (90, 475, 90 + 689 * after / before, 499), MINT, MINT, 7)
        text(d, 859, 299, f"{report['savings_pct']:.1f}%", 76, MINT, "bold")
        text(d, 863, 397, "fewer tokens", 29, WHITE, "bold")
        text(d, 863, 450, "Fixture result,", 22, MUTED)
        text(d, 863, 482, "not a universal promise.", 22, MUTED)
        text(d, 64, 573, "VERIFIED / Exact JSON-value recovery after decode", 23, MINT, "bold")
        text(d, 64, 608, "examples/api_response.json · Recovery is not proof of model-answer equivalence.", 20, MUTED)

    elif scene == 0:
        before, after, target = receipt["original_tokens"], receipt["final_tokens"], receipt["target_tokens"]
        text(d, 64, 118, "01 / FIT MORE WITHOUT DROPPING RECORDS", 20, BLUE, "bold")
        text(d, 60, 154, "Fit the budget. Keep every record.", 53, WHITE, "bold")
        text(d, 64, 229, f"Compact JSON: {before:,} tokens / Input budget: {target:,} tokens", 24, MUTED)
        rect(d, (64, 294, 791, 574))
        text(d, 88, 311, "PYTHON / records = bundled API fixture", 18, MUTED, "mono")
        line(d, [(88, 347), (767, 347)])
        for i, row in enumerate(FIT_CODE.splitlines()):
            color = MINT if "require_met" in row else BLUE if "from " in row else WHITE
            text(d, 88, 364 + i * 27, row, 23, color, "mono")
        rect(d, (819, 294, 1216, 574), outline=MINT)
        text(d, 845, 311, "FITS THE INPUT BUDGET", 18, MUTED, "bold")
        text(d, 841, 345, f"{after:,}", 58, MINT, "bold")
        text(d, 845, 431, f"of {target:,} tokens", 29, WHITE, "bold")
        text(d, 845, 481, f"{100 * (1 - after / before):.1f}% fewer tokens", 24, MINT)
        text(d, 845, 530, "30 records. None dropped.", 22, MINT)
        text(d, 64, 608, "Explicit folded representation / Verified JSON-value recovery / Local o200k_base counts", 20, MUTED)
    return im.resize((W, H), Image.Resampling.LANCZOS)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk-python", default=sys.executable, help="Python with the current SDK installed")
    local_cli = ROOT / "target/release" / ("tokenfold.exe" if os.name == "nt" else "tokenfold")
    parser.add_argument("--cli", default=str(local_cli) if local_cli.exists() else shutil.which("tokenfold"))
    args = parser.parse_args()
    if not args.cli:
        parser.error("Build the CLI first: cargo build --release -p tokenfold-cli --locked")
    report, receipt = evidence(args.cli, args.sdk_python)
    previews = [render(i, duration - 0.5, report, receipt) for i, duration in enumerate(DURATIONS)]
    sheet = Image.new("RGB", (W, H * len(previews)), BG)
    for i, preview in enumerate(previews):
        sheet.paste(preview, (0, i * H))
    # Reserve brand colors so the small gradient logo survives GIF quantization.
    brand = Image.new("RGB", LOGO.size, BG)
    brand.paste(LOGO, (0, 0), LOGO)
    palette = sheet.quantize(colors=128, method=Image.Quantize.MEDIANCUT)
    palette.putpalette(palette.getpalette()[:384] + brand.quantize(colors=128).getpalette()[:384])
    frames = []
    for scene, duration in enumerate(DURATIONS):
        for tick in range(round(duration * FPS)):
            frame = render(scene, tick / FPS, report, receipt)
            frames.append(frame.quantize(palette=palette, dither=Image.Dither.NONE))
    OUT.parent.mkdir(exist_ok=True)
    frames[0].save(OUT, save_all=True, append_images=frames[1:], duration=1000 // FPS,
                   loop=0, optimize=True, disposal=1)
    with Image.open(OUT) as gif:
        assert gif.size == (W, H) and gif.info["loop"] == 0
        elapsed = 0
        for i in range(gif.n_frames):
            gif.seek(i)
            expected = frames[elapsed // (1000 // FPS)].convert("RGB")
            assert gif.convert("RGB").tobytes() == expected.tobytes(), f"GIF artifacts in frame {i}"
            elapsed += gif.info["duration"]
        assert elapsed == len(frames) * (1000 // FPS)
        assert OUT.stat().st_size < 4_000_000, "Keep the README GIF below 4 MB"
    scratch = ROOT / "tmp/tokenfold-demo"
    scratch.mkdir(parents=True, exist_ok=True)
    sheet.resize((960, 540 * len(previews)), Image.Resampling.LANCZOS).save(scratch / "storyboard.png")
    for i, preview in enumerate(previews):
        preview.save(scratch / f"scene-{i + 1}.png")
    print(f"Core verified: {report['original_tokens']} -> {report['compressed_tokens']}; exact JSON recovery")
    print(f"SDK verified: {receipt['original_tokens']} -> {receipt['final_tokens']} / {receipt['target_tokens']}; exact rendered text; met")
    print(f"{OUT}: {OUT.stat().st_size:,} bytes; {len(frames) / FPS:.1f}s; Select illustration only")


if __name__ == "__main__":
    main()
