---
description: Run the full four-agent feature pipeline (planner → coder → tester → reviewer) for a feature request, with handoffs through .pipeline/.
argument-hint: <feature request>
---

Run the full feature pipeline for: $ARGUMENTS

Execute these stages **in order**. Do not skip ahead, do not do the work yourself, and after each stage confirm the handoff file exists before starting the next. Each stage is a subagent that reads the previous stage's handoff file.

0. **Confirm understanding (always do this first).** Delegate to the `planner` subagent: have it read the feature request and write `.pipeline/confirm.md` with a short **Understanding** (what it takes the task to be, in its own words), the key **Assumptions** it will make, and a final line "Did I understand this correctly?". It must **not** write the spec or any code yet. Wait for `.pipeline/confirm.md`, then **STOP and show it to me.**
   - I will reply with a confirmation or corrections. Treat my reply as authoritative and continue to step 1, folding in any corrections.

1. **Plan.** Delegate to the `planner` subagent to write the full `.pipeline/spec.md`, honoring my confirmed understanding and corrections. Wait for `.pipeline/spec.md`.
   - If the spec still contains genuine OPEN QUESTIONS, **STOP** and show them to me. Otherwise continue.

2. **Code.** Delegate to the `coder` subagent. Wait for `.pipeline/changes.md`.
   - If `changes.md` reports the spec was blocked, **STOP** and show me why.

3. **Test.** Delegate to the `tester` subagent. Wait for `.pipeline/test-results.md`.
   - If the status is FAIL, **STOP** and show me the failures. Do not attempt to fix them.

4. **Review.** Delegate to the `reviewer` subagent. Then show me the full contents of `.pipeline/review.md`.

Finally, report the verdict from `review.md` clearly (SHIP / NEEDS WORK / BLOCK).

**Do not merge, commit, or push anything.** Leave the branch exactly as the pipeline left it for my review.
