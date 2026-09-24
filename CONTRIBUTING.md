# Contributing

The implementation has not started. Choose an existing
[issue](https://github.com/fluzo-labs/fluzo/issues) and review its acceptance
criteria, dependencies and referenced specification before coding.

Use English for issues, PRs and project documentation. Keep each change focused
and link the owning issue. The four crates belong in one Cargo workspace;
runtime and TUI depend on the shared core contract, not each other. Do not add
empty crates for future capabilities.

Required tests must be deterministic and independent of private inference
services, credentials, GPU access and paid APIs. Live-model checks are explicit
and optional. Record actual validation commands and results; mark unavailable
checks as unverified rather than passed. Never commit local conversation data,
secrets, private endpoints or model weights.

Changes to scope, security guarantees, defaults or acceptance require review in
[fluzo-docs](https://github.com/fluzo-labs/fluzo-docs). Implemented behavior,
commands, tests and API documentation stay with the code; guides link to matching
revisions rather than maintain a second schema.

Jose Corral (`jmanuelcorral`) is the initial maintainer. PRs, applicable CI and a
recorded self-review are required; peer approval becomes required when another
reviewer is available. No application release is implied by a merged bootstrap
change. Contributions use the MIT license; preserve third-party notices.