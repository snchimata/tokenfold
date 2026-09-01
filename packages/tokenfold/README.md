# tokenfold for Node.js

Zero-runtime-dependency TypeScript bindings for the tokenfold Rust CLI.

```sh
npm install tokenfold
```

```ts
import { compress } from "tokenfold";

const { payload, report } = await compress(input, {
  format: "json",
  mode: "balanced",
});
```

Opt-in lossy array pruning drops whole array items to hit a token budget instead
of only restructuring them, replacing each with a `$tf_ref` marker that resolves
through the local retrieval store. Generic JSON only:

```ts
import { compress, retrieve } from "tokenfold";

const { payload, report } = await compress(feed, {
  format: "json",
  lossy: "heuristic",
  lossyRatio: 0.35,        // selection hint, not an enforced budget
  lossyPreserve: ["meta"], // arrays that are never pruned
});
const original = await retrieve(JSON.stringify(marker)); // raw hashes also work
```

Pass the same options to `inspect` to preview the projected savings without
writing anything to the store.

Requires Node.js 22 or newer. The matching native CLI is installed through an
optional platform package; set `TOKENFOLD_BINARY_PATH` to use a custom binary.
