# Table format

`tv` reads and writes tables as JSON. The same format is the input to `tv agent new` and `tv agent push`, and the output of `tv save` and `tv agent get`.

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
| `comments` | no | Only present in saved files. Imported when the file is opened. |

Unknown top-level fields are ignored.

## Shorthand

A bare JSON array of objects is also a table. It's the same as `{ "rows": [...] }`, so you can pipe API output straight in:

```sh
gh pr list --json number,title,author | tv agent new --key number --title "Open PRs"
```

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
