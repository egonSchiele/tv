# Table format

`tv` writes tables as JSON, in the format below: it's what `tv save` and `tv agent get` print. It reads that format, and the [input formats](#input-formats) further down.

## Full format

```json
{
  "version": 1,
  "title": "Open pull requests",
  "key": "number",
  "columns": [
    { "name": "number", "type": "number" },
    { "name": "title", "width": 50 },
    { "name": "author" },
    { "name": "additions", "type": "number" }
  ],
  "rows": [
    { "number": 412, "title": "Fix login redirect", "author": "sam", "additions": 12 }
  ],
  "comments": []
}
```

| Field | Required | Notes |
|---|---|---|
| `version` | no | Defaults to `1`. |
| `title` | no | Shown in the viewer's title bar. |
| `key` | no, strongly encouraged | Name of the column whose values identify rows. See [Validation](#validation). Comments and the cursor follow rows by key when the table is updated. Without a key, rows are identified by position (`#0`, `#1`, …), so comments can land on the wrong row after an update. |
| `columns` | no | Column order and settings. If omitted, inferred from the rows. |
| `columns[].name` | yes | Matches the key in each row object. |
| `columns[].type` | no | `string`, `number`, or `bool`. Inferred if omitted. Only affects alignment: numbers are right-aligned. |
| `columns[].width` | no | Maximum display width. Defaults to 40. |
| `rows` | yes | Array of objects, one per row. |
| `comments` | no | Only present in saved files. A list of comment threads; see [Comments](#comments). Imported when the file is opened. |

Unknown top-level fields are ignored.

## Shorthand

A bare JSON array of objects is also a table. It's the same as `{ "rows": [...] }`, so you can pipe API output straight in:

```sh
gh pr list --json number,title,author | tv agent new --key number --title "Open PRs"
```

## Input formats

`tv open`, `tv -`, `tv agent new` and `tv agent push` read more than this format. Each input becomes a table in this format, which is what a session stores and what `tv agent get` and `tv save` print. The first non-blank line decides how the input is read:

| First character | Read as |
|---|---|
| `{` or `[` | JSON, below |
| `\|` | A Markdown table |
| anything else | CSV, TSV or the like. The delimiter is whichever of `,`, tab, `;` and `\|` the first line has most of. |

A UTF-8 byte order mark is skipped. Empty input is an error.

### JSON

- **One object with `rows`:** the full format.
- **One array:** the shorthand.
- **Several values in a row** (JSON Lines, `jq '.[]'` output, `gh api --paginate`): each object is a row, and each array's items are rows.
- **One object without `rows`:** if its only array is a non-empty array of objects, as in `{"kind": "List", "items": [...]}`, those are the rows. Otherwise the object is one row, and the record view (`r`) shows it a field per line.

### CSV and TSV

- The first line is the header.
- Quoting follows RFC 4180: a field that starts with `"` can hold the delimiter and line breaks, and `""` inside it is one `"`. Lines end in `\n` or `\r\n`.
- Spaces around an unquoted cell are dropped, so `name, age` has the columns `name` and `age`. A quoted cell keeps its spaces.
- Blank lines are skipped, except in a one-column table. There a blank line is a row with an empty cell, since that's how `sqlite3 -csv` and `psql --csv` write NULL.
- Empty and missing cells are null.
- A column holds numbers if every non-empty cell is a JSON number that writes back exactly as written (`412`, `-2.5`), and booleans likewise (`true`, `false`). Any other column holds text, so `02134` and `1.50` show as written. So do integers past 2^53 (9007199254740992), such as 19-digit IDs, which a JSON reader that uses floats, like JavaScript's, would round.
- An empty header cell is named `column_<n>`, counting from 1. A repeated name gets `_2`, `_3`, ….
- A row with more cells than the header is an error, unless the extra cells are empty (a trailing delimiter).
- One line with no delimiter in it is an error, not a one-column table with no rows. That catches a one-word message piped in by mistake. A line with a delimiter is a header with no rows, which is valid: `psql --csv` prints one for an empty result.

### Markdown tables

- A header row, a `|---|` row under it (`:` for alignment is allowed and ignored), then the rows.
- Every non-blank line starts with `|`. The closing `|` is optional. `\|` is a `|` inside a cell.
- Cells are trimmed and typed like CSV's. Formatting such as `**bold**` stays as written.

### Titles

A file that names no title is titled with its file name, such as `sales.csv`. `--title`, the file's own `title`, and on `push` the session's title, all come first.

## Inference

- **Columns:** if `columns` is missing or empty, the columns are every key that appears in any row, in the order they're first seen. For example, `[{"b": 1, "a": 2}, {"c": 3}]` gives columns `b`, `a`, `c`.
- **Types:** for each column without a `type`, look at its non-null values. If they're all numbers, the type is `number`. If they're all booleans, it's `bool`. Anything else, including a column with only nulls, is `string`.

## Validation

If `key` is set:

- It must name one of the columns (given or inferred).
- Every row must have a non-null value for it.
- Values must be unique, compared as they display. For example, `412` and `"412"` count as duplicates.

A table that fails validation is rejected with an error naming the row, for example `duplicate key value "412" at rows 3 and 7`.

## Cell values

| Value | Grid display |
|---|---|
| string | As is. Newlines show as `↵`; tabs and other control characters show as spaces. |
| number | As written, e.g. `412`, `2.5`. |
| boolean | `true` / `false`. |
| null or missing | Empty (dimmed). |
| object or array | Compact JSON, e.g. `["bug","ci"]`. The cell detail view pretty-prints it. |

Fields in a row that aren't in `columns` are ignored when `columns` is given.

## Comments

`tv save` and the viewer's `:w` write a session's comment threads into `comments`. Each thread is a comment on one cell, followed by its replies:

```json
{
  "id": "c7h2k1",
  "row_id": "412",
  "column": "title",
  "value": "Fix login redirect",
  "awaiting_reply": false,
  "orphaned": false,
  "messages": [
    { "id": "c7h2k1", "author": "user", "text": "Why is this one still open?", "ts": "2026-10-28T19:03:00Z" },
    { "id": "c9m4p0", "author": "agent", "text": "Waiting on review from infra.", "ts": "2026-10-28T19:04:12Z" }
  ]
}
```

- `row_id` is the row's key value as text, or `#<index>` without a key. `column` is a column name.
- `author` is `user` or `agent`. `ts` is a UTC timestamp.
- `value`, `awaiting_reply` and `orphaned` are worked out when the file is saved: the cell's value then (`null` if it's gone), whether the user wrote last, and whether the row or column no longer exists. Opening the file ignores them, and they may be left out.
- A thread with no messages is skipped when the file is opened.

Opening the file moves the threads into the new session's comment log, keeping their IDs and timestamps.
