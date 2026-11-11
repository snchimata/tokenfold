from tokenfold import fit, Data, Request, JsonObject
text: JsonObject = fit("text", target_tokens=10).payload
structured: str = fit(Request.openai({"model": "m", "messages": []}), target_tokens=10).payload
bad = fit[JsonObject](Data([1]), target_tokens=10)
fit("text", target_tokens=10, budget_mode="unknown")
fit(b"bytes", target_tokens=10)
