"use strict";

const { format } = require("./wasm/zhfmt_wasm.js");
const { parsers: markdownParsers } = require("prettier/plugins/markdown");
// `prettier/doc` exposes `{ builders, printer, utils }`; older shapes export
// the builders directly.
const {
  builders: { hardline, join },
} = require("prettier/doc");

/**
 * Prettier plugin exposing the `zhfmt` WASM engine: adds a single space
 * between CJK and Latin characters (盘古之白), inserting nothing else.
 *
 * Two modes:
 *
 * - The built-in `markdown` / `mdx` parsers are wrapped (same trick as
 *   `prettier-plugin-organize-imports`): the raw text is spaced first, then
 *   handed to Prettier's own parser and printer. Markdown files therefore
 *   keep all core Prettier formatting and only gain CJK spacing — the
 *   behavior Prettier 2 had and Prettier 3 removed.
 * - The standalone `zhfmt` parser runs the engine on plain text and re-emits
 *   it verbatim (line by line, so `endOfLine` still applies). Opt in with
 *   `parser: "zhfmt"`; also inferred for extensions Prettier does not know
 *   (`.txt`, `.rst`).
 */

const astFormat = "zhfmt-text";

const languages = [
  {
    // Only extensions without a built-in parser; `.md`/`.mdx` are covered by
    // the wrapped parsers above and must keep using Prettier's own inference.
    name: "zhfmt",
    parsers: ["zhfmt"],
    extensions: [".txt", ".rst"],
  },
];

/**
 * Pre-format with zhfmt, then delegate to the wrapped parser.
 *
 * The transform must happen in `preprocess`: Prettier assigns
 * `options.originalText` from the text `preprocess` returns, keeping AST
 * positions (which printers slice against) in sync. Transforming in `parse`
 * instead would desync every node position and mangle the output.
 */
function wrap(parser) {
  return {
    ...parser,
    preprocess: async (text, options) => {
      const spaced = format(text);
      const preprocessed = await parser.preprocess?.(spaced, options);
      return preprocessed ?? spaced;
    },
  };
}

const parsers = {
  zhfmt: {
    parse: (text) => format(text),
    astFormat,
    locStart: () => 0,
    locEnd: (node) => node.length,
  },
  markdown: wrap(markdownParsers.markdown),
  mdx: wrap(markdownParsers.mdx),
};

const printers = {
  [astFormat]: {
    print: (path) => join(hardline, path.node.split("\n")),
    getVisitorKeys: () => [],
  },
};

module.exports = { languages, parsers, printers };
