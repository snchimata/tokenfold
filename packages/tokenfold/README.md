# tokenfold for Node.js

Zero-runtime-dependency TypeScript bindings for the Tokenfold Rust CLI.

```sh
npm install tokenfold
```

```ts
import { compress, decode, inspect } from "tokenfold";

const receipt = await inspect(input, { format: "json", preset: "balanced" });
const { payload, report, text } = await compress(input, {
  format: "json",
  preset: "balanced",
});
```

Payloads are `Uint8Array`; the `text` convenience getter decodes UTF-8 strictly.
`inspect` returns only the side-effect-free receipt.

Recoverable pruning is explicit and generic-JSON-only:

```ts
import { compress, retrieve } from "tokenfold";

const result = await compress(feed, {
  format: "json",
  targetTokens: 2_000,
  pruning: {
    keepRatio: 0.35,
    preservePaths: ["meta"],
    retrievalStore: ".tokenfold/retrieve",
  },
});
const marker = JSON.parse(result.text).items.find((item) => item.$tf_ref);
const original = await retrieve(marker, { retrievalStore: ".tokenfold/retrieve" });
```

Explicit TOON output is verified before emission and restored with `decode`:

```ts
const encoded = await compress(input, { format: "json", encoding: "toon" });
const jsonBytes = await decode(encoded.payload, { from: "toon" });
```

Requires Node.js 22 or newer. The matching native CLI is installed through an
optional platform package; set `TOKENFOLD_BINARY_PATH` to use a custom binary.
