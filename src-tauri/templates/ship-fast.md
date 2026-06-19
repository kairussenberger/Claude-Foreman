---
description: Fast path — a dedicated fast-coder then fast-reviewer, with no planner, tester, or confirmation gate, for small or obvious changes. Handoffs through .pipeline/.
argument-hint: <change request>
---

Fast pipeline for: $ARGUMENTS

This is the **fast path**: two dedicated agents — `fast-coder` then `fast-reviewer` — with no planner, no tester, and no confirmation gate. Run them in order, do the work *through the subagents* (do not implement or review it yourself), and confirm each handoff file exists before continuing.

1. **Code.** Delegate to the `fast-coder` subagent, passing it the change request verbatim: "$ARGUMENTS". Wait for `.pipeline/changes.md`. If it reports the change is too big for the fast path, **STOP** and tell me to use `/ship` instead.

2. **Review.** Delegate to the `fast-reviewer` subagent. Wait for `.pipeline/review.md`, then show me its full contents.

Finally, report the VERDICT from `review.md` clearly (SHIP / NEEDS WORK / BLOCK).

**Do not merge, commit, or push anything.** Leave the changes exactly as they are for my review.
