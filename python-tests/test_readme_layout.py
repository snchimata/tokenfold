"""Model-free README layout and saved Select v3 claim contracts."""
import json
from pathlib import Path
import re
import unittest
from urllib.parse import unquote, urlsplit


ROOT = Path(__file__).resolve().parents[1]


class ReadmeLayout(unittest.TestCase):
    def setUp(self):
        self.text = (ROOT / 'README.md').read_text(encoding='utf-8')

    def test_common_path_and_visible_limits(self):
        self.assertTrue(self.text.isascii())
        self.assertEqual(self.text.count('Exact decode proves data recovery'), 1)
        self.assertIn('Up to 96.3% fewer tokens (showcase)', self.text)
        self.assertIn('45.6-67.6% on three fixtures, lossless', self.text)
        self.assertEqual(self.text.count('```mermaid'), 2)
        self.assertIn('flowchart LR\n    accTitle: Lossless first', self.text)
        self.assertLess(self.text.index('![Terminal demo:'), self.text.index('#### Why Tokenfold'))
        quick = self.text.split('### Drop-in proxy for OpenAI-compatible clients\n', 1)[1]
        quick = quick.split('### Interfaces\n', 1)[0]
        self.assertLess(quick.index('```bash'), quick.index('```python'))
        self.assertLess(quick.index('```python'), quick.index('<details>'))
        self.assertGreater(quick.index('usage_disposition'), quick.index('<details>'))
        self.assertIn('Explicit `--observations` enables lossless', self.text)
        self.assertGreater(self.text.index('> `schema_compaction`'), self.text.index('### Presets'))
        depth = 0
        for tag in re.findall(r'</?details>', self.text):
            depth += -1 if tag.startswith('</') else 1
            self.assertGreaterEqual(depth, 0)
        self.assertEqual(depth, 0)

    def test_local_links_resolve(self):
        targets = re.findall(r'\]\(([^)]+)\)|src="([^"]+)"', self.text)
        for markdown, image in targets:
            target = urlsplit(markdown or image)
            if target.scheme or not target.path:
                continue
            path = ROOT / unquote(target.path)
            with self.subTest(target=markdown or image):
                self.assertTrue(path.exists(), str(path))
                if target.fragment and path.suffix == '.md':
                    content = path.read_text(encoding='utf-8')
                    headings = re.findall(r'^#{1,6} +(.+)$', content, re.M)
                    slugs = {re.sub(r'[^\w -]', '', h.lower()).replace(' ', '-') for h in headings}
                    self.assertTrue(target.fragment in slugs or f'id="{target.fragment}"' in content)

    def test_select_v3_claims_match_versioned_provenance(self):
        manifest = json.loads((ROOT / 'tests/fixtures/readme_metrics.json').read_text(encoding='utf-8'))
        metrics = manifest['select_v3']
        section = self.text.split('### Tokenfold Select v3 benchmarks\n', 1)[1]
        section = section.split('<summary><strong>Historical Select v1 benchmarks', 1)[0]
        visible = section.split('<details>', 1)[0]
        self.assertIn(metrics['source'], section)
        self.assertIn(metrics['evidence'], section)
        self.assertIn(metrics['model_revision'], metrics['evidence'])
        self.assertEqual(len(metrics['evidence_sha256']), 64)
        self.assertFalse(metrics['qualification'])
        for row in metrics['token_rows']:
            original, output = row['original_tokens'], row['v3_output_tokens']
            comparator = f"{row['kompress_output_tokens']:,}"
            if not row['kompress_budget_eligible']:
                comparator = f'Refused; {comparator} raw'
            self.assertIn(f"| {row['workload']} | {original:,} | {output:,} | "
                          f'**{100 * (1 - output / original):.1f}%** | {comparator} |', visible)
        for row in metrics['latency_rows']:
            change = 100 * (row['v3_ms'] / row['kompress_ms'] - 1)
            self.assertIn(f"| {row['workload']} | {row['v3_ms']:.3f} | {row['kompress_ms']:.3f} | "
                          f"{change:.1f}% | {row['basis']} |", visible)
        old = metrics['historical_multiturn_ms']
        self.assertIn(f"{old['v3']:.3f} ms vs {old['kompress']:.3f} ms", section)
        self.assertEqual((metrics['historical_refusals'], metrics['historical_cells']), (6, 10))
        self.assertIn('inconclusive, not V3 wins', visible)
        self.assertIn('not controlled-idle', visible)
        self.assertIn('not representative quality, population p50/p95, or superiority qualification', visible)


if __name__ == '__main__':
    unittest.main()
