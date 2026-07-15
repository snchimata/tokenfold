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

Requires Node.js 22 or newer. The matching native CLI is installed through an
optional platform package; set `TOKENFOLD_BINARY_PATH` to use a custom binary.
