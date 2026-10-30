---
name: tv-tables
description: Show tabular data to the user in tv, a terminal table viewer they browse and comment on while you read their cursor and comments. Use it for a table with more than about 6 columns or 20 rows, when the user asks to see something in tv, or when they say to check their tv comments or point at "this one" in a table.
---

# tv: show the user a table

tv is a terminal table viewer you share with the user. You make a session
from JSON rows, and the user opens it in their own terminal, where they can
scroll, read whole cells and comment on them. While they look, you can
replace the table, see which cell their cursor is on, and answer their
comments. Every `tv agent` command prints one line of JSON. Errors go to
stderr, with a non-zero exit status.

## When to use it

Use tv instead of printing a table when it has more than about 6 columns or
20 rows, or whenever the user asks for it. A small table is fine inline.

## Quick start

Pipe rows into `tv agent new`, or give it a file. It reads JSON (an array of
objects, or one object per line), CSV, TSV and Markdown tables, so most
command output goes straight in:

```sh
gh pr list --json number,title,headRefName | tv agent new --key number --title "Open PRs"
sqlite3 -csv -header app.db 'select id, email from users' | tv agent new --key id --title "Users"
tv agent new rows.json --key id --title "Failing tests"
```

It prints the session:

```json
{"session":"k3f9x2","open_command":"tv open k3f9x2","rows":38,"columns":3}
```

Give the user the `open_command` exactly as printed, to run in another
terminal. You can't open the viewer yourself: it needs their terminal.

## Always set a key

`--key` names a column whose values are unique and never null, like an ID or
a PR number. Comments and the user's cursor follow rows by key, so they stay
on the right row when you update the table. Without a key, rows are named by
position, from `#0`, so a comment on `#3` stays on the fourth row, whichever
row a push puts there.

## Updating the table

`push` replaces the whole table. An open viewer reloads it at once, keeps
the cursor on its row, and highlights what changed. `push` keeps the
session's key and title unless you pass `--key` or `--title`:

```sh
gh pr list --json number,title,headRefName | tv agent push k3f9x2
```

For a small edit, `get` the table, change it, and push it back. `get` prints
JSON, whatever format the table came in:

```sh
tv agent get k3f9x2 | jq '(.rows[] | select(.number == 404)).title = "Fixed"' | tv agent push k3f9x2
```

`get` prints `columns` too, and only those columns are shown. To add or
rename a field, drop them with `del(.columns)`, and tv works them out again.
That also drops any `width` a column set:

```sh
tv agent get k3f9x2 | jq 'del(.columns) | .rows[].reviewed = false' | tv agent push k3f9x2
```

A push with missing or repeated key values is rejected, and the table stays
as it was. To drop the key on purpose, push the full format with
`"key": null`.

## Comments

When the user says to check their comments, list the threads waiting for
you:

```sh
tv agent comments k3f9x2
```

```json
[{"id":"c7h2kq","row_id":"404","column":"title","value":"Bump serde","awaiting_reply":true,"orphaned":false,"messages":[{"id":"c7h2kq","author":"user","text":"why the rename?","ts":"2026-10-28T19:03:00Z"}]}]
```

Answer each one, and push any change it asked for. The text is one
argument, so quote it. It may start with `-`:

```sh
tv agent reply k3f9x2 c7h2kq "To match the issue title."
```

`orphaned: true` means a push removed the thread's row or column. To point
the user at a cell yourself, start a thread on it. They jump between
threads with `]`:

```sh
tv agent comment k3f9x2 --row 404 --col title "This one fails CI."
```

## "This one"

When the user says "this one", "here" or "this row", ask where their cursor
is:

```sh
tv agent focus k3f9x2
```

```json
{"viewer_open":true,"row_id":"404","column":"title","value":"Bump serde","row":{"number":404,"title":"Bump serde"}}
```

To move their cursor to a cell:

```sh
tv agent goto k3f9x2 --row 404 --col title
```

It moves once they're back in the grid, not while they're reading a popup
or typing. With no viewer open, the next one opens there.

Row IDs, here and in `comment`, are the key's values as text. Without a key
they're `#0`, `#1`, …, so quote them, as in `--row '#3'`: an unquoted `#`
starts a shell comment.

## Finding a session

If you don't have the session ID, list the sessions, most recent first:

```sh
tv agent sessions
```

## Don't poll

Check comments or focus only when the user asks, or refers to the table.
Nothing tells you when they comment, and nothing needs to: they'll say so.

## Reference

- `tv agent new [FILE] [--title T] [--key K]`: make a session from FILE, or
  from stdin.
- `tv agent push <id> [FILE] [--title T] [--key K]`: replace the table.
- `tv agent get <id>`: print the table.
- `tv agent focus <id>`: the cell the user's cursor is on, and its row.
- `tv agent goto <id> --row R [--col C]`: move the user's cursor.
- `tv agent comments <id> [--all]`: threads awaiting your reply, or all of
  them.
- `tv agent reply <id> <thread-id> <text>`: answer a thread.
- `tv agent comment <id> --row R --col C <text>`: start a thread on a cell.
- `tv agent sessions`: list sessions.
- `tv agent help`: this guide.

`tv agent <command> --help` shows one command's options.
