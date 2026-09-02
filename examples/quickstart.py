"""Minimal Python quickstart for Tokenfold's bytes-first v2 interface."""

import json
from pathlib import Path

import tokenfold

HERE = Path(__file__).parent


def show(label: str, report: tokenfold.CompressionReport) -> None:
    print(f"== {label} ==")
    print(
        f"tokens: {report.original_tokens} -> {report.compressed_tokens} "
        f"({report.saved_tokens} saved, {report.savings_pct:.1f}%)"
    )


body = (HERE / "openai_payload.json").read_bytes()
result = tokenfold.compress(
    body,
    format=tokenfold.InputFormat.OPENAI_JSON,
    preset=tokenfold.Preset.BALANCED,
)
show("OpenAI request", result.report)
print(f"payload: {len(result.payload)} bytes\n")

data = (HERE / "api_response.json").read_bytes()
receipt = tokenfold.inspect(data, format=tokenfold.InputFormat.JSON)
show("generic JSON preview", receipt)

toon_input = json.dumps({"users": [{"id": 1}, {"id": 2}]}).encode()
encoded = tokenfold.compress(toon_input, format="json", encoding=tokenfold.Encoding.TOON)
restored = tokenfold.decode(encoded.payload, from_format="toon")
assert json.loads(restored) == json.loads(toon_input)
show("explicit TOON", encoded.report)
