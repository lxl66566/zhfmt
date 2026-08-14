"use strict";

const { test } = require("node:test");
const assert = require("node:assert/strict");
const prettier = require("prettier");

const plugin = require("./index.js");
const { format } = require("./wasm/zhfmt_wasm.js");

const withPlugin = (extra) => ({ plugins: [plugin], ...extra });
const fmt = (text, extra) => prettier.format(text, withPlugin(extra));

test("wasm format basics", () => {
  assert.equal(format("我有3个苹果"), "我有 3 个苹果");
  assert.equal(format("no change"), "no change");
  // Fullwidth punctuation blocks boundaries; nothing else is touched.
  assert.equal(format("中文，English。中文"), "中文，English。中文");
});

test("standalone parser keeps text verbatim apart from spacing", async () => {
  // No list-marker rewrites, no trailing newline added.
  assert.equal(await fmt("*  中文abc\n", { parser: "zhfmt" }), "*  中文 abc\n");
  assert.equal(await fmt("中文abc", { parser: "zhfmt" }), "中文 abc");
});

test("standalone parser is idempotent", async () => {
  const once = await fmt("我有3个苹果，`code`在这里\n", { parser: "zhfmt" });
  assert.equal(await fmt(once, { parser: "zhfmt" }), once);
});

test("endOfLine applies to the standalone parser", async () => {
  assert.equal(
    await fmt("中文abc\n第二行x", { parser: "zhfmt", endOfLine: "crlf" }),
    "中文 abc\r\n第二行 x",
  );
});

test("inline code spans and links are spaced on the outside only", async () => {
  assert.equal(
    await fmt("在这个`myfunc`函数内", { parser: "zhfmt" }),
    "在这个 `myfunc` 函数内",
  );
  // The boundary of a link is decided by its inner text: Latin text gets
  // spaced against CJK, Chinese text does not (engine-tested behavior).
  assert.equal(
    await fmt("这是一个[link](https://example.com/a)格式", { parser: "zhfmt" }),
    "这是一个 [link](https://example.com/a) 格式",
  );
  assert.equal(
    await fmt("这是一个[中文链接](https://example.com)格式", { parser: "zhfmt" }),
    "这是一个[中文链接](https://example.com)格式",
  );
});

test("fenced code block content is never spaced", async () => {
  // Spacing applies outside the block, never inside.
  assert.equal(
    await fmt("中文a\n\n```js\nlet x = 中文a;\n```\n\n中文b\n", { parser: "zhfmt" }),
    "中文 a\n\n```js\nlet x = 中文a;\n```\n\n中文 b\n",
  );
});

test("markdown keeps core formatting and gains CJK spacing", async () => {
  const out = await fmt("*  中文abc\n  二级item\n", { filepath: "readme.md" });
  assert.equal(out, "- 中文 abc\n  二级 item\n");
});

test("mdx goes through the wrapped parser too", async () => {
  const out = await fmt("中文abc <B/>", { filepath: "doc.mdx" });
  assert.equal(out, "中文 abc <B/>\n");
});

test("wrapped markdown is idempotent", async () => {
  const once = await fmt("我有3个苹果，`code`在这里\n", { filepath: "a.md" });
  assert.equal(await fmt(once, { filepath: "a.md" }), once);
});

test("unknown extensions infer the zhfmt parser", async () => {
  assert.equal(await fmt("中文abc", { filepath: "notes.txt" }), "中文 abc");
  assert.equal(await fmt("中文abc", { filepath: "notes.rst" }), "中文 abc");
});

test("check reports files needing spacing", async () => {
  assert.equal(await prettier.check("中文abc", withPlugin({ parser: "zhfmt" })), false);
  assert.equal(await prettier.check("中文 abc", withPlugin({ parser: "zhfmt" })), true);
});

test("plugin resolves by package name", async () => {
  const out = await prettier.format("中文abc", {
    parser: "zhfmt",
    plugins: ["prettier-plugin-zhfmt"],
  });
  assert.equal(out, "中文 abc");
});
