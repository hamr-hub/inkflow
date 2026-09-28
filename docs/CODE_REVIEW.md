# 代码审查 (Code review)

Automated top-N observations. Not a substitute for human review.

**Status**: `active`

## Findings

1. **Large module**: `src/glyph_table.rs` is 474024 LOC — split candidates above 500 LOC.

## Recommended human review checklist

- [ ] Read README/AGENTS.md to grasp intent
- [ ] Skim top 3 largest files for responsibility leaks
- [ ] Check error-handling strategy (typed errors vs exceptions vs Result)
- [ ] Verify config/secrets loading
- [ ] Confirm test fixtures cover the happy + error paths
