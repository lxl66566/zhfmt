"use strict";

// Per-process bench: one module per process; the driver alternates process
// order and aggregates medians, avoiding in-process cross-module interference.

const { createRequire } = await import("node:module");
const req = createRequire(import.meta.url);
const { format } = req(`${process.argv[2]}/zhfmt_wasm.js`);

const asciiRun = "lorem ipsum dolor sit amet ".repeat(2000);
const cjkRun = "这是一段中文文本用于测试扫描器吞吐量。".repeat(2000);
const mixed = [];
for (let i = 0; i < 2000; i++)
  mixed.push("第" + i + "行：some english text与中文混排，还有`code`和[link](https://example.com/a/b?c=d)。");
const mixedText = mixed.join("\n");
const formatted = format(mixedText);

function bench(text) {
  const bytes = Buffer.byteLength(text, "utf8");
  for (let i = 0; i < 30; i++) format(text);
  let best = 0;
  for (let r = 0; r < 5; r++) {
    const iters = Math.max(4, Math.ceil((40 * 1024 * 1024) / bytes));
    let sink = 0;
    const t0 = process.hrtime.bigint();
    for (let i = 0; i < iters; i++) sink += format(text).length;
    const ms = Number(process.hrtime.bigint() - t0) / 1e6;
    if (sink < 0) throw new Error("unreachable");
    best = Math.max(best, ((bytes * iters) / 1024 / 1024) / (ms / 1000));
  }
  return best; // MiB/s (best-of-5 = lowest noise floor)
}

const out = {};
for (const [name, text] of Object.entries({ ascii: asciiRun, cjk: cjkRun, mixed: mixedText, formatted }))
  out[name] = bench(text);
console.log(JSON.stringify(out));
