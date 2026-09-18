"use strict";

// Differential test: wasm engine (SIMD) vs native zhfmt binary (AVX2 path).
// Generates adversarial UTF-8 corpora (wake bytes at every alignment, long
// runs, multibyte tails, etc.), formats with both, and compares.

const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { format } = require("./wasm/zhfmt_wasm.js");

const WAKE = ["`", "[", "]", "<", "*", "~", "\n"];
const cjk = "中文与english混排测试";
const samples = [];

// 1. every wake byte at every offset 0..47 (crosses 8/16/32-byte chunk lines)
for (const w of WAKE) {
  for (let n = 0; n < 48; n++) {
    samples.push("a".repeat(n) + w + cjk + "x" + w + cjk);
    samples.push(cjk.slice(0, 3).repeat(2) + "b".repeat(n) + w + cjk + w);
  }
}
// 2. wake byte sequences at every alignment inside long CJK
for (const w of WAKE) {
  for (let n = 0; n < 64; n++) {
    samples.push(cjk.repeat(4).slice(0, n) + w + cjk.repeat(4));
  }
}
// 3. long clean runs: ascii and cjk of many lengths (SIMD loop + tails 0..31)
for (let n = 0; n <= 200; n++) {
  samples.push("z".repeat(n) + cjk);
  samples.push(cjk.repeat(Math.ceil(n / 3)).slice(0, n * 3) + "A" + cjk);
}
// 4. mixed multibyte widths: 2/3/4-byte chars (emoji), fullwidth punct
const widths = ["é", "中", "𝄞", "€", "，。；：！？"];
for (const ch of widths) {
  for (let n = 0; n < 40; n++) {
    samples.push(ch.repeat(n) + "latin9" + ch + "a" + ch.repeat(3));
  }
}
// 5. structural constructs
samples.push(
  "",
  "a",
  cjk,
  "中文`code`后文",
  "这是一个[link](https://example.com/a?b=c)格式",
  "![img](x.png)图注",
  "*强调*中文*b*",
  "~~删除~~中文",
  "<div>html标签</div>尾部",
  "中文<!-- comment -->中文",
  "中文a\n```\ncode中文a\n```\n中文b",
  "脚注[^1]引用[^1]",
  "中文\\*转义\\`后x",
  "列表：\n- item1中文a\n- item2",
  "中" + "很".repeat(500) + "文a",
  "`".repeat(64) + "x",
  "**bold**中文**more**",
  "中文[a](b)[c](d)英文",
);

// every combination of first/second char around each wake byte
const edges = ["a", "1", "中", "，", "`", " ", "é", "𝄞"];
for (const l of edges) for (const r of edges) for (const w of ["`", "[", "*", "<", "~", "\n", "x"])
  samples.push("中文" + l + w + r + "英文");

let fails = 0;
for (const [i, s] of samples.entries()) {
  const wasmOut = format(s);
  const nativeOut = execFileSync(path.resolve(__dirname, "../target/release/zhfmt.exe"), {
    input: s,
    encoding: "utf8",
  });
  if (wasmOut !== nativeOut) {
    fails++;
    console.log(`MISMATCH #${i}:`);
    console.log("  input :", JSON.stringify(s.slice(0, 100)));
    console.log("  wasm  :", JSON.stringify(wasmOut.slice(0, 100)));
    console.log("  native:", JSON.stringify(nativeOut.slice(0, 100)));
    if (fails > 5) break;
  }
}
console.log(`${samples.length} samples, ${fails} mismatches`);
process.exit(fails ? 1 : 0);
