# Delegating work that lands

A `delegate` call gives a subagent its own conversation and the same tools as
the main session. Subagents optimize for an impressive final report unless the
task forces artifacts: the common failure is a run that spends every round on
reconnaissance (greps, file reads, address tables) and answers with "here is
everything a successor needs" — zero files written. Nothing in the runtime
punishes a beautiful recon report, so the task text has to. This page is the
task-phrasing playbook that makes runs land committed work instead.

## The rules

1. **One deliverable chain per task, verbs explicit.** Spell the pipeline
   out: "disassemble → port → rewire → suite case → commit". A task that
   names a pipeline gets executed; a task that describes an area gets
   studied.
2. **Commit cadence is a checkpoint, not a goal.** "Commit after EVERY
   routine", never "commit when done". A run told to commit at the end
   commits nothing; a run told to commit per unit commits each one.
3. **Floor and cap.** "Land 1 and you have succeeded" plus "max 3" beats
   "up to 8". The floor makes partial success real; the cap keeps the run
   inside one coherent effort.
4. **Bake the reconnaissance in.** Paste the predecessor's findings (exact
   addresses, block line numbers, port shapes, constants, the harness
   skeleton) into the task text so the run starts at the write step. Runs
   that must LEARN the harness from the repo die in reads before their
   first write, even with otherwise perfect phrasing.
5. **Say "do NOT explore the module first".** Explicitly forbidding the
   exploration pass works; runs given the permission explore.
6. **Done means artifacts.** "Your final answer is the list of files
   changed and commit hashes" — never "a report a successor could use".
7. **Mark read-only tasks as read-only.** "Research only, change no files",
   so the write watchdog does not fight a run that is doing its job.

## Worked vs failed phrasings

Failed (11 of 12 parallel runs spent 100% of their tool budget on
reconnaissance and wrote zero files; the death pattern was setup → read
module → read harness → read more files → budget gone):

> Recover up to 8 routines from src/client/client_X.c (list of addresses)...
> PER ROUTINE: 1. disassembly is the spec (objdump cmd)... 2. Port to clean C
> in src/engine/X.c... 3. Rewire... 4. Comparison suite... 5. Syntax check...
> COMMIT when done.

Worked (re-fired runs started writing on the first round):

> CRITICAL DISCIPLINE: work ONE routine at a time — disassemble → port →
> rewire → suite case → syntax-check → git commit — COMMIT AFTER EVERY
> ROUTINE. Do NOT explore the module first. Max 3 routines; land 1 and you
> have succeeded.
>
> (prior run's recon baked in: exact addresses, block line numbers, port
> shapes, constants)

## Runtime helpers

- **Write watchdog.** A run that spends `ZERO_WRITE_NUDGE_ROUNDS` (8) tool
  rounds without a successful `write_file` / `edit_file` / `multi_edit` is
  nudged once, mid-run, to stop exploring and land its first artifact. A
  task that is read-only by nature may ignore the nudge and continue.
- **No tool-round cap.** Subagent runs have no round limit; they end when
  the model stops calling tools, the stream failure ladder is exhausted, or
  the main agent (or user) stops the run. Budget the task in the task text,
  not in the runtime.
- **Mid-run corrections.** `message_subagent` lands a message in a running
  subagent's conversation; use it to steer a run that has gone sideways.
