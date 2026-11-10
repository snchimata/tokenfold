"""Offline 0.6 fitting; install the current wheel before running."""
from tokenfold import Data, fit

result = fit(Data({"answer": 42, "source": "example"}), target_tokens=32)
payload: str = result.require_met()
assert payload == '{"answer":42,"source":"example"}'
print(payload)
print(result.receipt.to_json())
# Application code decides whether and where to send the checked payload.
