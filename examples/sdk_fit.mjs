// Offline 0.6 fitting; install the current root and platform tarballs first.
import { fit } from 'tokenfold';

const result = await fit({kind: 'json_data', value: {answer: 42, source: 'example'}}, {targetTokens: 32});
const payload = result.requireMet();
if (payload !== '{"answer":42,"source":"example"}') throw Error('Unexpected payload');
console.log(payload);
console.log(result.receipt.toJSON());
// Application code decides whether and where to send the checked payload.
