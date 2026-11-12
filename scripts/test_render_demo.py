"""Offline layout regressions; no SDK, model or network calls."""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

from PIL import Image, ImageChops


SPEC = importlib.util.spec_from_file_location("render_demo", Path(__file__).with_name("render-demo.py"))
demo = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(demo)
REPORT = {"original_tokens": 3812, "compressed_tokens": 1376, "savings_pct": 63.90346274921301}
RECEIPT = {"budget_status": "met", "original_tokens": 2366, "final_tokens": 1376, "target_tokens": 1500}


class DemoLayout(unittest.TestCase):
    def test_every_scene_uses_the_repository_logo(self):
        with Image.open(demo.ROOT / "docs/assets/tokenfold-logo.png") as source:
            logo = source.convert("RGBA")
        logo = logo.crop(logo.getbbox())
        logo.thumbnail((220 * demo.SCALE, 82 * demo.SCALE), Image.Resampling.LANCZOS)
        expected = Image.new("RGB", logo.size, demo.BG)
        expected.paste(logo, (0, 0), logo)
        x, y = 64 * demo.SCALE, 8 * demo.SCALE
        for scene in range(len(demo.LABELS)):
            frame, _ = demo.base(scene)
            with self.subTest(scene=scene):
                self.assertEqual(frame.crop((x, y, x + logo.width, y + logo.height)).tobytes(),
                                 expected.tobytes())

    def test_sdk_scene_demonstrates_savings_not_just_a_cap(self):
        with patch.object(demo, "text", wraps=demo.text) as rendered:
            demo.render(0, 0, REPORT, RECEIPT)
        copy = "\n".join(call.args[3] for call in rendered.call_args_list)
        for value in ('Data(records, representation="folded")', "2,366", "1,376", "1,500",
                      "41.8% fewer tokens", "30 records. None dropped."):
            with self.subTest(value=value):
                self.assertIn(value, copy)
        self.assertNotIn("63.9%", copy)

    def test_short_loop_core_precedes_opt_in_select(self):
        self.assertEqual(demo.LABELS, ("FIT", "CORE", "TOKENFOLD SELECT"))
        self.assertEqual(len(demo.DURATIONS), len(demo.LABELS))
        self.assertEqual(sum(demo.DURATIONS), 13)
        with patch.object(demo, "text", wraps=demo.text) as rendered:
            demo.render(2, 0, REPORT, RECEIPT)
        copy = "\n".join(call.args[3] for call in rendered.call_args_list)
        self.assertIn("allow_selection=True", copy)
        self.assertIn("Over budget?", copy)
        self.assertIn("Approved runtime", copy)
        self.assertIn("Experimental / unqualified", copy)

    def test_select_brand_is_prominent_and_copy_is_short(self):
        with patch.object(demo, "text", wraps=demo.text) as rendered:
            demo.render(2, demo.DURATIONS[2] - 0.5, REPORT, RECEIPT)
        calls = rendered.call_args_list
        headlines = [call for call in calls if call.args[3] == "Tokenfold Select"]
        self.assertTrue(headlines, "Use the product name as the headline")
        self.assertGreaterEqual(headlines[0].args[4], 60)
        copy = [call.args[3] for call in calls]
        self.assertIn("03  TOKENFOLD SELECT", copy)
        self.assertIn("CONTEXT", copy)
        self.assertIn("SELECTED", copy)
        self.assertIn("not live", " ".join(copy))
        self.assertLessEqual(len(" ".join(copy).split()), 65)

    def test_selection_animation_never_covers_source_text(self):
        baseline = demo.render(2, 0, REPORT, RECEIPT)
        for t in (0.6, 0.8, 1, 1.35, 1.6, 2.1, 2.3, 3.1):
            frame = demo.render(2, t, REPORT, RECEIPT)
            for row in range(3):
                y = 310 + row * 68
                box = (78, y + 10, 550, y + 50)
                with self.subTest(t=t, row=row):
                    self.assertIsNone(ImageChops.difference(baseline.crop(box), frame.crop(box)).getbbox())

    def test_release_copy_and_chapter_numbers(self):
        for scene, duration in enumerate(demo.DURATIONS):
            with patch.object(demo, "text", wraps=demo.text) as rendered:
                demo.render(scene, duration - 0.5, REPORT, RECEIPT)
            copy = [call.args[3] for call in rendered.call_args_list]
            with self.subTest(scene=scene):
                self.assertNotRegex("\n".join(copy).lower(), r"\b(preview|unreleased)\b")
                self.assertTrue(any(value.startswith(f"0{scene + 1} / ") for value in copy))


if __name__ == "__main__":
    unittest.main()
