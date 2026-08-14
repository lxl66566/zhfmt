# prettier-plugin-zhfmt

[简体中文](./README.md) | English

A [Prettier](https://prettier.io) plugin that automatically inserts spaces between CJK and Latin/digit characters (the pangu rule), filling the gap left when Prettier 3 removed this feature. The formatting engine is WebAssembly compiled from [zhfmt](https://github.com/lxl66566/zhfmt), behaving identically to the CLI.

## Installation

```sh
pnpm add -D prettier-plugin-zhfmt
```

Requires Prettier >= 3.6 (peer dependency). Earlier versions do not await the parser's `preprocess`, which would blank the output.

## Usage

In your `.prettierrc`:

```json
{
  "plugins": ["prettier-plugin-zhfmt"]
}
```

No further configuration needed. The plugin **wraps** Prettier's built-in `markdown` / `mdx` parsers: it first adds spacing with zhfmt, then hands the text to Prettier's own parser and printer. Markdown files therefore keep all of Prettier's native formatting (lists, tables, `proseWrap`, embedded code formatting, etc.) and only gain CJK spacing:

### Plain-text mode

For files you don't want Prettier to re-layout — only add spacing — use the standalone `zhfmt` parser. Apart from inserting spaces, output is byte-for-byte identical to the input (no newline changes, no list-marker rewrites). It is inferred automatically for extensions without a built-in parser (`.txt`, `.rst`), and can be selected explicitly for other files:

```json
{
  "plugins": ["prettier-plugin-zhfmt"],
  "overrides": [{ "files": ["*.txt"], "options": { "parser": "zhfmt" } }]
}
```

## Behavior

- Only inserts single spaces, never deletes anything; idempotent.
- No spacing inside code blocks, inline code, link URLs, or HTML tags.
- See the [zhfmt engine boundary rules](https://github.com/lxl66566/zhfmt#algorithm-overview) for details.

## License

MIT OR Apache-2.0
