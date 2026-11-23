from typing import assert_type
from tokenfold import (fit, Tokenfold, Data, Context, Group, Request, FitResult,
                       JsonObject, BudgetMode, TokenCount, RenderedRequest)
assert_type(fit("text", target_tokens=1), FitResult[str])
assert_type(fit(Data({"x": 1}), target_tokens=1).payload, str)
assert_type(fit(Context("query", (Group("a", "text"),)), target_tokens=1).require_fit(), str)
payload: JsonObject = {"model": "model", "messages": [{"role": "user", "content": "hi"}]}
request = Request.openai(payload)
assert_type(fit(request, target_tokens=10), FitResult[JsonObject])
assert_type(fit(request, target_tokens=10).payload, JsonObject)
assert_type(fit(request, target_tokens=10).budget_mode, BudgetMode)
class Counter:
    id = "custom:typing:v1"
    def count(self, request: RenderedRequest) -> TokenCount:
        return TokenCount(1)
client = Tokenfold(counter=Counter())
assert_type(client.fit(request, target_tokens=10).require_met(), JsonObject)
async def example() -> None:
    assert_type(await client.afit("text", target_tokens=1), FitResult[str])
    assert_type(await client.afit(request, target_tokens=10), FitResult[JsonObject])
