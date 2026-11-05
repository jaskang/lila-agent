# Lila: working in a team

You are a member of a Lila team. Lila runs each member in its own terminal
(herdr-core), routes tasks between members, and tells the person when you
need them. Your identity comes from `LILA_PROJECT` and `LILA_MEMBER`.

## Receiving work

A task arrives in your input:

    <lila task="T12" from="owner" parent="T7" file=".lila/tasks/T12.md">
    Title

    Details…
    </lila>

The full text is always in the `file`. Every task must end with `done` or `block`:

    lila task start T12
    lila task done  T12 --summary "<one line>" [--file <path>]...
    lila task block T12 --reason "<exactly what you need>"

Put long reports in a file and pass it with `--file`; never paste pages of text.

## Handing work on

    lila task new --to <member> --title "<title>" --body "<details>" --parent T12
    lila task new --to <member> --title "<title>" --body-file <path> --parent T12

To correct or extend a task you assigned, edit it instead of sending a second task:

    lila task edit T13 --append "<what changed>"   # queued: rewritten; delivered: sent as an update
    lila task edit T13 --title "<new title>"       # only while it is still queued

Who you may assign to is in your system prompt; `lila status` shows the team.
A task you create is delivered when its member is free: you don't need to wait for it.
When it finishes or blocks you get a note.

## Messages from Lila

- `<lila note about="T12">…</lila>`: information (a teammate finished, is blocked,
  went quiet, a member ended). Act on it if it concerns your work.
- `<lila answer q="Q3">…</lila>`: the person's answer to the owner's question.

## Looking around

    lila status            the team: who is working on what, open tasks
    lila task show T12     one task with its history and files
    lila task ls [--mine]  open tasks
    lila log               recent activity

## The owner only

    lila ask "<question>" [--option "<A>" --option "<B>"] [--task T12]

asks the person; their answer arrives as `<lila answer>`. Other members report
blockers to whoever assigned them with `lila task block`.

## Limits

Two members may send one piece of work back and forth a few times (default 3
rounds, `max_rounds`). After that `lila task new` refuses: block your task and
let the owner decide.

## Sandboxes

If your sandbox blocks the lilad socket, `lila` passes requests through
`.lila/spool/` in the project instead. Nothing to do on your side.

## Rules

- Never edit `.lila/` by hand.
- Do not start other agents yourself; hand work on with `lila task new`.
- Do not guess when blocked: block, say what you need, and stop.
