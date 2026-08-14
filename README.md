# zhfmt

高性能中文文档批量格式化工具：遵循中文写作原则（盘古之白），在中文与英文/数字之间自动添加空格。

- 安全：只插入单个空格，从不删除任何内容；原子化替换文件。
- 快速：SIMD（AVX2/SSE2，运行时检测）+ SWAR 混合扫描 + 多字节 run 跳跃 + COW，无任何正则；已经格式化好的文件只扫描、不分配 buffer、不写回；多文件并行处理。
- 专注：不同于 autocorrect，zhfmt 只做加空格这一件事，尽量不产生预期外的修改。

该工具主要用于 markdown 文档格式化，适配 markdown 链接、内嵌 html、inline code、代码块、脚注格式，尽力避免非预期的行为。也可以用于其他纯文本格式的空格添加；未在代码上测试。

## 安装

前往 [Release](https://github.com/lxl66566/zhfmt/releases) 下载 prebuilt binary。

## 使用

```sh
zhfmt                       # 格式化当前目录下所有文档类文件（递归）
zhfmt docs/ README.md       # 格式化指定路径（目录递归，文件总是处理）
zhfmt --check               # CI 模式：只报告会变化的文件，存在差异时退出码为 1
zhfmt --diff                # 打印 unified diff，不写回，退出码同上
cat a.md | zhfmt            # 管道模式：stdin -> stdout
zhfmt --ext md,rst docs/    # 覆盖要处理的扩展名列表
zhfmt -j 4                  # 指定线程数
```

- 默认只格式化以下扩展名文件：`md, markdown, mdx, txt, rst`；显式传入的文件路径不受限制。
- 文件扫描遵循 `.gitignore`。
- exit code：0 正常；1 `--check`/`--diff` 发现差异；2 错误。

效果示例：

```text
在这个`myfunc`函数内           ->  在这个 `myfunc` 函数内
这是一个[link](https:Xxx)格式  ->  这是一个 [link](https:Xxx) 格式
我有3个苹果                    ->  我有 3 个苹果
```

更多边界样例（inline code、链接、强调、引号、日/韩文、畸形 UTF-8 等）请直接阅读 [src/engine/tests.rs](src/engine/tests.rs)。

### 作为 prettier 插件使用

[`prettier-plugin-zhfmt`](./prettier-plugin-zhfmt) 将引擎编译为 WASM 并包装 prettier 内置 markdown parser，在保留 prettier 原生格式化的同时补上中英文空格：

```sh
pnpm add -D prettier-plugin-zhfmt
```

```json
// In your prettier.config.js
{ "plugins": ["prettier-plugin-zhfmt"] }
```

详见[插件文档](./prettier-plugin-zhfmt/README.md)。

## 配置文件

查找顺序：从当前目录逐级向上查找 `zhfmt.json` 或 `.zhfmt.json`，取最近；都没有则尝试全局配置（`$XDG_CONFIG_HOME/zhfmt/zhfmt.json`，Windows 下为 `%APPDATA%` 对应目录）；再没有则使用默认配置。

```json
{
  "extensions": ["md", "markdown", "mdx", "txt", "rst"],
  "include": ["docs/*.adoc"],
  "exclude": ["node_modules/", "target/"],
  "jobs": 0
}
```

- `extensions`：要处理的扩展名白名单（不带点），整体覆盖默认值
- `include`：额外的 glob 白名单，命中后即使扩展名不匹配也会处理
- `exclude`：glob 黑名单，相对路径匹配
- `jobs`：处理线程数，0 或 null 为自动

## 算法简述

核心在 [src/engine/mod.rs](src/engine/mod.rs)，字符分类在 [src/classify.rs](src/classify.rs)，完整设计见 [docs/design.md](docs/design.md)。

1. 字符分类：每个字符被归为 `Latin`（a-zA-Z0-9）、`CJK`（汉字/假名/韩文/全角字母数字）、`Neutral`（全角标点）、`Soft`（透明界定符：仅未配对的 `*`、`~`）、`Hard`（结构界定符：`` ` ``、`[`、`]`）、`Space`、`Other`（含引号、括号、裸 `<`/`>` 等，阻断边界）
2. 混合扫描：边界只可能出现在非 ASCII 字节或结构字节处；短区段用内联 SWAR（u64 字内并行）定位这些"唤醒字节"，长干净区段交给 AVX2/SSE2（运行时检测）续扫
3. 多字节 run 跳跃：`Latin` 必为 ASCII，故连续多字节字符（一段中文）内部不可能有边界——整段只分类首尾字符，中间用 SIMD 扫描整体跳过
4. 边界判定：仅当有效边界两侧分别为 `Latin` 与 `CJK` 时插入一个空格；`Soft` 字符会被透视（取内部最近的内容字符参与判定）；`Neutral`、`Other`、空白都会阻断边界
5. 结构处理：
   - backtick code span（含成对的 fenced code block）内部原样跳过，其左右边界分别由内部首/尾内容字符决定；`[text](url)` 的 text 按正文处理，url 整体跳过且不影响边界
   - HTML 标签/注释（`<...>`、`<!-- ... -->`）与脚注引用 `[^id]` 是不透明原子：内部（含 `title="中文"`、`<template #槽位名>` 等）永不扫描，两侧边界一律阻断
   - 强调符号（`*...*`、`**...**`、`~~...~~`）成对时作为整体：边界由内部内容决定，空格只加在标记**外侧**（`CG**鉴赏**` -> `CG **鉴赏**`），内部仍正常格式化；未配对的标记按 `Soft` 透视
6. 惰性输出：扫描到第一个插入点才分配输出缓冲并开始拷贝；全文无插入则返回"无变化"，调用方跳过写回

畸形 UTF-8 字节被归类为 `Other` 并原样透传，不会导致错误或误改。

## 性能

criterion 基准（`cargo bench`）。本机结果：Linux x86_64 / AVX2，AMD Zen 5，
32 线程，多轮代表值。

| 场景                      | 输入大小        | 吞吐 / 耗时 |
| ------------------------- | --------------- | ----------- |
| 纯 ASCII                  | ~0.89 MiB       | ~43.7 GiB/s |
| 纯中文                    | ~1.03 MiB       | ~74.9 GiB/s |
| 混合中英文档              | ~0.68 MiB       | ~1.27 GiB/s |
| 已格式化文本              | ~0.33 MiB       | ~1.10 GiB/s |
| 混合文本 1KB–4MB          | ~1.5 KB–5.97 MB | ~700 MiB/s  |
| 256 文件端到端（--check） | 256 × ~16 KiB   | ~3.3 ms     |

## License

MIT OR Apache-2.0
