---
description: Autonomous (unattended) feature pipeline — planner→coder→tester→reviewer with no questions. Used by Foreman's overnight/parallel mode.
argument-hint: <feature request>
---

Run the full feature pipeline AUTONOMOUSLY for: $ARGUMENTS

This is an **unattended** run. There is no human available to answer anything. These rules override any instinct (yours or a subagent's) to pause:

- **Never ask a question, never wait for approval, never stop for input.** If you are tempted to ask the user something, instead make the decision a thoughtful senior engineer would, write down the assumption, and continue.
- **The feature request above is the single source of truth.** Never infer or change requirements based on the branch name, worktree directory name, or file paths — they are arbitrary. (E.g. if the request says "cap of 99", it is 99, regardless of what any branch/folder is called.)
- **You have full permission** to read, write, edit, and run shell commands (build, tests, etc.). Run them directly.

Execute all four stages in order; confirm each handoff file exists before the next:

1. **Plan.** Delegate to the `planner` subagent. Instruct it explicitly: this is autonomous mode — do **not** raise OPEN QUESTIONS. For anything ambiguous, choose the most reasonable interpretation, record it under an "Assumptions" heading, and produce a complete spec. Wait for `.pipeline/spec.md`. If the spec contains open questions anyway, resolve them yourself with the most sensible answer and continue — do not stop.
2. **Code.** Delegate to the `coder` subagent, honoring the spec and its Assumptions. Wait for `.pipeline/changes.md`.
3. **Test.** Delegate to the `tester` subagent. It writes and **runs** the tests directly — never ask to approve running them. Wait for `.pipeline/test-results.md`. If tests fail, do **not** stop: continue to the Reviewer so the failure is captured in the verdict.
4. **Review.** Delegate to the `reviewer` subagent. Wait for `.pipeline/review.md`.

Finally, report the verdict from `review.md` (SHIP / NEEDS WORK / BLOCK).

Do **not** commit, merge, or push — leave the worktree exactly as the pipeline left it for morning review. Run start-to-finish in one go.
