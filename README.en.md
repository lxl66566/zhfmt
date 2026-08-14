# zhfmt

[简体中文](./README.md) | English

High-performance formatter for Chinese documentation: follows the pangu spacing rule by inserting spaces between CJK and Latin/digit characters.

- Safe: only inserts single spaces, never deletes anything; atomic file replacement.
- Fast: SIMD (AVX2/SSE2, runtime detection) + SWAR hybrid scanning + multibyte-run skipping + COW, no regex anywhere; already-formatted files are scanned without allocating a buffer or writing back; files are processed in parallel.
- Focused: unlike autocorrect, zhfmt does one thing — add spaces — minimizing unexpected changes.

The tool targets markdown documents and handles markdown links, inline HTML, inline code, code blocks, and footnote syntax to avoid unintended edits as much as possible. It can also be used to add spacing to other plain-text formats; it has not been tested on code.

## Installation

Download a prebuilt binary from [Releases](https://github.com/lxl66566/zhfmt/releases).

## Usage

```sh
zhfmt                       # format all document files under the current directory (recursive)
zhfmt docs/ README.md       # format the given paths (directories are recursive, files are always processed)
zhfmt --check               # CI mode: only report files that would change; exit code 1 on differences
zhfmt --diff                # print a unified diff without writing back; exit code as above
cat a.md | zhfmt            # pipe mode: stdin -> stdout
zhfmt --ext md,rst docs/    # override the list of extensions to process
zhfmt -j 4                  # set the number of worker threads
```

- By default only files with these extensions are formatted: `md, markdown, mdx, txt, rst`; explicitly passed file paths are not restricted.
- File scanning respects `.gitignore`.
- Exit codes: 0 success; 1 differences found by `--check`/`--diff`; 2 error.

Examples:

```text
在这个`myfunc`函数内           ->  在这个 `myfunc` 函数内
这是一个[link](https:Xxx)格式  ->  这是一个 [link](https:Xxx) 格式
我有3个苹果                    ->  我有 3 个苹果
```

For more edge cases (inline code, links, emphasis, quotes, Japanese/Korean, malformed UTF-8, etc.), see [src/engine/tests.rs](src/engine/tests.rs).

### Use as a Prettier plugin

[`prettier-plugin-zhfmt`](./prettier-plugin-zhfmt) compiles the engine to WASM and wraps Prettier's built-in markdown parser, adding CJK–Latin spacing while keeping Prettier's native formatting:

```sh
pnpm add -D prettier-plugin-zhfmt
```

```json
// In your prettier.config.js
{ "plugins": ["prettier-plugin-zhfmt"] }
```

See the [plugin docs](./prettier-plugin-zhfmt/README.md) for details.

`prettier-plugin-zhfmt` does not come with a SIMD implementation; if conditions allow using a binary, it is not recommended to use it.

## Configuration file

Lookup order: walk up from the current directory for `zhfmt.json` or `.zhfmt.json`, taking the nearest one; otherwise fall back to the global config (`$XDG_CONFIG_HOME/zhfmt/zhfmt.json`, or the `%APPDATA%` counterpart on Windows); otherwise use defaults.

```json
{
  "extensions": ["md", "markdown", "mdx", "txt", "rst"],
  "include": ["docs/*.adoc"],
  "exclude": ["node_modules/", "target/"],
  "jobs": 0
}
```

- `extensions`: whitelist of extensions to process (no dot), overrides the defaults entirely
- `include`: extra glob whitelist; matched files are processed even if the extension does not match
- `exclude`: glob blacklist, matched against relative paths
- `jobs`: number of worker threads; 0 or null for automatic

## Dev

The core lives in [src/engine](src/engine); character classification is in [src/classify.rs](src/classify.rs); the full design is in [docs/design.md](docs/design.md).

## Performance

criterion benchmarks (`cargo bench`). Local results: Linux x86_64 / AVX2, AMD 7945HX, 32 threads, representative values over multiple runs.

| Scenario                         | Input size      | Throughput / time |
| -------------------------------- | --------------- | ----------------- |
| Pure ASCII                       | ~0.89 MiB       | ~43.7 GiB/s       |
| Pure Chinese                     | ~1.03 MiB       | ~74.9 GiB/s       |
| Mixed Chinese/English document   | ~0.68 MiB       | ~1.27 GiB/s       |
| Already-formatted text           | ~0.33 MiB       | ~1.10 GiB/s       |
| Mixed text 1 KB–4 MB             | ~1.5 KB–5.97 MB | ~700 MiB/s        |
| 256 files end-to-end (`--check`) | 256 × ~16 KiB   | ~3.3 ms           |

## License

MIT OR Apache-2.0
