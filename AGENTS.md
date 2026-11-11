# Agent contribution rules

These rules apply to coding agents working in this repository. They do not override CI, repository policies, or explicit human approvals.

- Read relevant code before modifying it, and understand the dependency boundaries before adding dependencies.
- Keep commits small and atomic. Use a single-line Conventional Commit subject and do not bundle unrelated changes.
- Every bug fix must include a regression test. New public APIs need documentation and tests.
- Do not silently change public protocol contracts. Do not suppress compiler warnings without a written technical reason.
- Do not use unsafe Rust without a documented technical requirement and review.
- Avoid unnecessary abstraction layers. Never alter tests to hide implementation defects.
- Verify claimed behavior by executing the relevant checks; do not claim success for checks that were not run.
- Preserve unrelated user changes and review the final diff for unintended changes.
- Give security-sensitive code additional review.
